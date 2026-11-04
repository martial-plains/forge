// SPDX-License-Identifier: MIT OR Apache-2.0
//! File locks makes them (filelock.c).
//!
//! When a buffer visiting `dir/name` is first modified, Forge makes
//! `dir/.#name`, a symbolic link whose target is `USER@HOST.PID:BOOT`
//! (`:BOOT`, the boot time in seconds since the epoch, is left out when it
//! isn't known). The lock goes away when the buffer becomes unmodified
//! again (saved, reverted, or undone back to the saved text), when the
//! buffer is killed, and when Forge exits. Visiting a file without
//! changing it takes no lock.
//!
//! The format is own, so Forge and Forge see each other's locks:
//! a lock written by a live Forge makes Forge ask before editing, and the
//! other way round.
//!
//! - A lock naming this process is ours already (two buffers, one lock).
//! - A lock from this host whose process is gone, or which was made before
//!   the last reboot, is stale and is quietly replaced.
//! - Anything else belongs to someone else, and the caller asks the user
//!   what to do (steal it, proceed without it, or give up the change).
//!
//! Where symbolic links can't be made (some network and Windows file
//! systems), the lock is a regular file holding the same text, as Forge
//! does there.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// `create-lockfiles`: whether modifying a buffer makes a lock at all.
static CREATE_LOCKFILES: AtomicBool = AtomicBool::new(true);

/// Turn lock files on or off (`forge.opt.create_lockfiles`).
pub fn set_create_lockfiles(on: bool) {
    CREATE_LOCKFILES.store(on, Ordering::Relaxed);
}

pub fn create_lockfiles() -> bool {
    CREATE_LOCKFILES.load(Ordering::Relaxed)
}

/// The locks this process holds, by lock path, so two buffers visiting
/// the same file share one lock and the last to let go removes it.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Guard>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Weak<Guard>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// A lock this process holds. Cloning shares it; dropping the last clone
/// removes the lock file (if it still names us).
#[derive(Clone, Debug)]
pub struct FileLock {
    guard: Arc<Guard>,
}

impl FileLock {
    /// The `.#name` file.
    pub fn path(&self) -> &Path {
        &self.guard.path
    }
}

#[derive(Debug)]
struct Guard {
    path: PathBuf,
    contents: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let Ok(mut r) = registry().lock() else {
            return;
        };
        // A newer guard for the same lock (we lost it and took it back)
        // writes the same text, so it is the one that removes the file.
        if r.get(&self.path).is_some_and(|w| w.strong_count() > 0) {
            return;
        }
        r.remove(&self.path);
        if read_lock(&self.path).ok().as_deref() == Some(self.contents.as_str()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Who holds a lock, as its file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOwner {
    /// The lock's text, as written.
    pub raw: String,
    pub user: Option<String>,
    pub host: Option<String>,
    pub pid: Option<i64>,
    pub boot: Option<u64>,
}

impl LockOwner {
    pub fn parse(raw: &str) -> Self {
        let mut owner = LockOwner {
            raw: raw.to_string(),
            user: None,
            host: None,
            pid: None,
            boot: None,
        };
        // USER@HOST.PID[:BOOT], read from the right, so a host name with
        // dots in it (or a user name with an @) still parses.
        let Some((user, rest)) = raw.rsplit_once('@') else {
            return owner;
        };
        let (host_pid, boot) = match rest.rsplit_once(':') {
            Some((hp, b)) if !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()) => {
                (hp, b.parse().ok())
            }
            _ => (rest, None),
        };
        let Some((host, pid)) = host_pid.rsplit_once('.') else {
            return owner;
        };
        let Ok(pid) = pid.parse::<i64>() else {
            return owner;
        };
        owner.user = Some(user.to_string());
        owner.host = Some(host.to_string());
        owner.pid = Some(pid);
        owner.boot = boot;
        owner
    }

    /// As Forge shows it in its question: `user@host (pid 1234)`.
    pub fn describe(&self) -> String {
        match (&self.user, &self.host, self.pid) {
            (Some(u), Some(h), Some(p)) => format!("{u}@{h} (pid {p})"),
            _ if self.raw.is_empty() => "an unknown owner".into(),
            _ => self.raw.clone(),
        }
    }

    fn is_this_process(&self) -> bool {
        self.host.as_deref() == Some(local_hostname().as_str())
            && self.pid == Some(std::process::id() as i64)
    }

    /// The owner can't be holding it any more: it ran on this host and its
    /// process is gone, or it was started before the last boot. A lock
    /// from another host is taken at its word, and so is one we can't
    /// read. An empty lock is stale.
    pub fn is_stale(&self) -> bool {
        if self.raw.is_empty() {
            return true;
        }
        let (Some(host), Some(pid)) = (&self.host, self.pid) else {
            return false;
        };
        if host != &local_hostname() {
            return false;
        }
        if pid <= 0 {
            return true;
        }
        if !process_alive(pid) {
            return true;
        }
        // Within a second of our own boot time: the two
        // ways of finding it can disagree by a little.
        match (self.boot, boot_time()) {
            (Some(theirs), Some(ours)) => theirs.abs_diff(ours) > 1,
            _ => false,
        }
    }
}

pub enum Attempt {
    Acquired(FileLock),
    /// Someone else holds it.
    Locked(LockOwner),
}

/// `.#name` next to `file`.
pub fn lock_path(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?;
    let mut lock = std::ffi::OsString::from(".#");
    lock.push(name);
    Some(file.with_file_name(lock))
}

/// Lock `file` for this process. `steal` takes the lock even from a live
/// owner (the `s` answer). A lock we already hold is shared.
pub fn acquire(file: &Path, steal: bool) -> io::Result<Attempt> {
    let path = lock_path(file).ok_or_else(|| io::Error::other("file has no name"))?;
    let ours = our_lock_text();
    // A few rounds: the lock may change hands while we look at it.
    for _ in 0..4 {
        if let Some(lock) = held(&path) {
            return Ok(Attempt::Acquired(lock));
        }
        match create_lock(&path, &ours) {
            Ok(()) => return Ok(Attempt::Acquired(register(path, ours))),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let raw = match read_lock(&path) {
            Ok(raw) => raw,
            // Gone between the two calls: try again.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let owner = LockOwner::parse(&raw);
        if owner.is_this_process() {
            // Ours from before (another buffer that has since let go
            // without removing it, say): take it as it is.
            return Ok(Attempt::Acquired(register(path, raw)));
        }
        if !steal && !owner.is_stale() {
            return Ok(Attempt::Locked(owner));
        }
        break_lock(&path, &raw);
    }
    match read_lock(&path) {
        Ok(raw) => Ok(Attempt::Locked(LockOwner::parse(&raw))),
        Err(e) => Err(e),
    }
}

/// Who else holds `file`'s lock: None when it is free, stale, or ours
///.
pub fn other_owner(file: &Path) -> Option<LockOwner> {
    let path = lock_path(file)?;
    let raw = read_lock(&path).ok()?;
    let owner = LockOwner::parse(&raw);
    (!owner.is_this_process() && !owner.is_stale()).then_some(owner)
}

fn held(path: &Path) -> Option<FileLock> {
    let r = registry().lock().ok()?;
    let guard = r.get(path)?.upgrade()?;
    // Someone may have stolen it from under us since.
    (read_lock(path).ok().as_deref() == Some(guard.contents.as_str())).then_some(FileLock { guard })
}

fn register(path: PathBuf, contents: String) -> FileLock {
    let guard = Arc::new(Guard {
        path: path.clone(),
        contents,
    });
    if let Ok(mut r) = registry().lock() {
        r.insert(path, Arc::downgrade(&guard));
    }
    FileLock { guard }
}

/// Remove the lock, but only if it still says `raw`: a lock that changed
/// hands meanwhile belongs to its new owner.
fn break_lock(path: &Path, raw: &str) -> bool {
    if read_lock(path).ok().as_deref() != Some(raw) {
        return false;
    }
    std::fs::remove_file(path).is_ok()
}

/// What a lock says: a symlink's target, or a regular file's text.
pub fn read_lock(path: &Path) -> io::Result<String> {
    #[cfg(unix)]
    {
        match std::fs::read_link(path) {
            Ok(t) => return Ok(t.to_string_lossy().into_owned()),
            // EINVAL: there, but not a symlink.
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {}
            Err(e) => return Err(e),
        }
    }
    let text = std::fs::read_to_string(path)?;
    Ok(text.trim_end_matches(['\n', '\0']).to_string())
}

fn create_lock(path: &Path, text: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        match std::os::unix::fs::symlink(text, path) {
            Ok(()) => return Ok(()),
            // No symlinks here: fall back to a regular file.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::EPERM) | Some(libc::ENOSYS) | Some(libc::ENAMETOOLONG)
                ) => {}
            Err(e) => return Err(e),
        }
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = f.write_all(text.as_bytes()) {
        drop(f);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// `USER@HOST.PID:BOOT` it.
pub fn our_lock_text() -> String {
    let mut s = format!(
        "{}@{}.{}",
        user_login_name(),
        local_hostname(),
        std::process::id()
    );
    if let Some(boot) = boot_time() {
        s.push_str(&format!(":{boot}"));
    }
    s
}

/// `user-login-name`: LOGNAME, then USER, then the password file.
fn user_login_name() -> String {
    for var in ["LOGNAME", "USER"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return v;
        }
    }
    #[cfg(unix)]
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned();
        }
    }
    "unknown".into()
}

#[cfg(unix)]
pub fn local_hostname() -> String {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        let mut hostname = [0u8; 256];
        unsafe {
            if libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) == 0 {
                std::ffi::CStr::from_ptr(hostname.as_ptr().cast())
                    .to_string_lossy()
                    .into_owned()
            } else {
                "unknown-host".into()
            }
        }
    })
    .clone()
}

#[cfg(not(unix))]
pub fn local_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".into())
}

/// Is `pid` running? (Signal 0: EPERM means alive but not ours.)
#[cfg(unix)]
fn process_alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    unsafe {
        libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Without a way to ask, assume the owner is alive: only an explicit
/// steal clears the lock.
#[cfg(not(unix))]
fn process_alive(_pid: i64) -> bool {
    true
}

/// When this machine booted, in seconds since the epoch.
pub fn boot_time() -> Option<u64> {
    static BOOT: OnceLock<Option<u64>> = OnceLock::new();
    *BOOT.get_or_init(read_boot_time)
}

#[cfg(target_os = "linux")]
fn read_boot_time() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn read_boot_time() -> Option<u64> {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut size = std::mem::size_of::<libc::timeval>();
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut tv as *mut libc::timeval).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && tv.tv_sec > 0).then_some(tv.tv_sec as u64)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd"
)))]
fn read_boot_time() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-filelock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write a lock the way another editor would.
    pub(crate) fn write_foreign_lock(lock: &Path, text: &str) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(text, lock).unwrap();
        #[cfg(not(unix))]
        std::fs::write(lock, text).unwrap();
    }

    #[test]
    fn our_lock_text_is_the_standard_format_and_parses_back() {
        let text = our_lock_text();
        let o = LockOwner::parse(&text);
        assert_eq!(o.pid, Some(std::process::id() as i64));
        assert_eq!(o.host.as_deref(), Some(local_hostname().as_str()));
        assert_eq!(o.boot, boot_time(), "{text}");
        assert!(o.is_this_process());
        assert!(!o.is_stale());
        // own forms, with and without the boot time, dotted hosts.
        let e = LockOwner::parse("alice@box.example.org.4242:1700000000");
        assert_eq!(e.user.as_deref(), Some("alice"));
        assert_eq!(e.host.as_deref(), Some("box.example.org"));
        assert_eq!(e.pid, Some(4242));
        assert_eq!(e.boot, Some(1_700_000_000));
        assert_eq!(e.describe(), "alice@box.example.org (pid 4242)");
        let e = LockOwner::parse("bob@host.17");
        assert_eq!((e.pid, e.boot), (Some(17), None));
        for bad in ["", "garbage", "no-at.12", "u@host.notapid"] {
            assert_eq!(LockOwner::parse(bad).pid, None, "{bad:?}");
        }
        assert!(
            !LockOwner::parse("garbage").is_stale(),
            "unknown is not stale"
        );
        assert!(LockOwner::parse("").is_stale(), "empty is stale");
    }

    #[test]
    fn stale_means_dead_here_or_before_this_boot() {
        let host = local_hostname();
        // A live process that isn't us: pid 1.
        let live = LockOwner::parse(&format!("someone@{host}.1"));
        assert!(!live.is_stale());
        if let Some(boot) = boot_time() {
            let rebooted = LockOwner::parse(&format!("someone@{host}.1:{}", boot - 1000));
            assert!(rebooted.is_stale(), "from before the last boot");
            let same = LockOwner::parse(&format!("someone@{host}.1:{}", boot + 1));
            assert!(!same.is_stale(), "a second's slack allows");
        }
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        assert!(LockOwner::parse(&format!("ghost@{host}.{dead}")).is_stale());
        let remote = LockOwner::parse(&format!("ghost@not-{host}.{dead}"));
        assert!(
            !remote.is_stale(),
            "another machine's lock is taken at its word"
        );
    }

    #[test]
    fn locks_are_shared_in_process_and_removed_by_the_last_holder() {
        let d = dir("share");
        let f = d.join("a.txt");
        let lock = d.join(".#a.txt");
        let Attempt::Acquired(one) = acquire(&f, false).unwrap() else {
            panic!("free lock");
        };
        assert_eq!(read_lock(&lock).unwrap(), our_lock_text());
        let Attempt::Acquired(two) = acquire(&f, false).unwrap() else {
            panic!("our own lock is ours");
        };
        assert!(
            other_owner(&f).is_none(),
            "ours doesn't count as someone else's"
        );
        drop(one);
        assert!(
            std::fs::symlink_metadata(&lock).is_ok(),
            "still held by two"
        );
        drop(two);
        assert!(
            std::fs::symlink_metadata(&lock).is_err(),
            "last one out removes it"
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_live_foreign_lock_is_reported_and_can_be_stolen() {
        let d = dir("foreign");
        let f = d.join("b.txt");
        let lock = d.join(".#b.txt");
        let theirs = format!("other-user@{}.1", local_hostname());
        write_foreign_lock(&lock, &theirs);
        match acquire(&f, false).unwrap() {
            Attempt::Locked(o) => assert_eq!(o.raw, theirs),
            Attempt::Acquired(_) => panic!("must not take a live lock"),
        }
        assert_eq!(other_owner(&f).map(|o| o.raw), Some(theirs.clone()));
        assert_eq!(read_lock(&lock).unwrap(), theirs, "left alone");
        let Attempt::Acquired(stolen) = acquire(&f, true).unwrap() else {
            panic!("steal takes it");
        };
        assert_eq!(read_lock(&lock).unwrap(), our_lock_text());
        drop(stolen);
        assert!(std::fs::symlink_metadata(&lock).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn stale_and_malformed_locks() {
        let d = dir("stale");
        let f = d.join("c.txt");
        let lock = d.join(".#c.txt");
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        write_foreign_lock(&lock, &format!("ghost@{}.{dead}", local_hostname()));
        let Attempt::Acquired(l) = acquire(&f, false).unwrap() else {
            panic!("a dead owner's lock is reclaimed");
        };
        drop(l);
        write_foreign_lock(&lock, "someone-elses-format");
        assert!(matches!(acquire(&f, false).unwrap(), Attempt::Locked(_)));
        assert_eq!(read_lock(&lock).unwrap(), "someone-elses-format");
        std::fs::remove_file(&lock).unwrap();
        // A regular-file lock (what Forge writes where symlinks fail).
        std::fs::write(&lock, format!("other@{}.1", local_hostname())).unwrap();
        assert!(matches!(acquire(&f, false).unwrap(), Attempt::Locked(_)));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_stolen_lock_is_not_removed_by_its_old_holder() {
        let d = dir("stolen");
        let f = d.join("e.txt");
        let lock = d.join(".#e.txt");
        let Attempt::Acquired(mine) = acquire(&f, false).unwrap() else {
            panic!()
        };
        // Another editor steals it.
        std::fs::remove_file(&lock).unwrap();
        let theirs = format!("thief@{}.1", local_hostname());
        write_foreign_lock(&lock, &theirs);
        assert!(
            matches!(acquire(&f, false).unwrap(), Attempt::Locked(_)),
            "we know it isn't ours any more"
        );
        drop(mine);
        assert_eq!(read_lock(&lock).unwrap(), theirs, "the thief keeps it");
        std::fs::remove_dir_all(d).unwrap();
    }
}
