// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tether: edit files on other machines, in containers, or as another user,
//! as if they were local.
//!
//! A remote name looks like `/ssh:user@host#2222:/etc/hosts`. The part between
//! the first `/` and the last `:` is a chain of hops joined by `|`, so
//! `/ssh:jump|ssh:inner|sudo::/etc/shadow` hops through two machines and then
//! becomes root. Methods: `ssh` (also `scp`), `sudo`, `doas`, `docker`,
//! `podman`, `kubectl` and `local` (a plain `sh`, for trying things out).
//!
//! How it works: opening a remote file copies it into a private mirror under
//! the cache directory and visits the mirror, so every editing feature works
//! unchanged. Saving writes the mirror and then pushes it to the remote with an
//! atomic rename that keeps the file's permissions. Every remote operation is a
//! short shell script run through the hop chain; ssh reuses one connection per
//! host (ControlMaster), so only the first operation pays for the handshake.
//!
//! What is better than TRAMP's defaults:
//! - Nothing ever blocks forever: each operation has a timeout and failures
//!   say what went wrong ("a password is required" and the like).
//! - Saving checks that the remote file has not changed since it was fetched
//!   and asks before overwriting it.
//! - Remote saves are atomic (temp file then rename) and follow symlinks.
//! - Mirrors are recognised from their location alone, so a restored session
//!   still saves to the right machine.
//! - Host completion comes from `~/.ssh/config`, `known_hosts` and running
//!   containers; directory listings are cached for a few seconds.
//! - `tether-sudo-edit` reopens the current file as root, on whichever
//!   machine it lives.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use forge_docs::CommandDoc;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the feature is called in messages and buffer names.
pub const NAME: &str = "Tether";

pub const DIR_MODE: &str = "tether-dir-mode";
const METHODS: &[&str] = &[
    "ssh", "scp", "sudo", "doas", "docker", "podman", "kubectl", "local",
];

pub const COMMANDS: &[CommandDoc] = &[
    CommandDoc::line(
        "tether-browse",
        "Find a file on another machine, container or as another user.",
    ),
    CommandDoc::line(
        "tether-connections",
        "List the remote connections used in this session.",
    ),
    CommandDoc::line(
        "tether-dir-open",
        "In a Tether listing: open the entry on this line.",
    ),
    CommandDoc::line(
        "tether-dir-refresh",
        "In a Tether listing: list the directory again.",
    ),
    CommandDoc::line(
        "tether-dir-up",
        "In a Tether listing: go to the parent directory.",
    ),
    CommandDoc::line(
        "tether-disconnect-all",
        "Close Tether's ssh connections and forget typed passwords.",
    ),
    CommandDoc::line(
        "tether-revert",
        "Fetch the remote file again and replace this buffer with it.",
    ),
    CommandDoc::line(
        "tether-sudo-edit",
        "Reopen the current file as root, on the same machine.",
    ),
];

// ------------------------------------------------------------------ names

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hop {
    pub method: String,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub hops: Vec<Hop>,
    /// Absolute once resolved; may start with `~` as typed.
    pub path: String,
}

impl Hop {
    fn parse(method: &str, target: &str) -> Option<Hop> {
        if !METHODS.contains(&method) {
            return None;
        }
        let (user, rest) = match target.split_once('@') {
            Some((u, r)) => (Some(u.to_string()), r),
            None => (None, target),
        };
        let (host, port) = match rest.split_once('#') {
            Some((h, p)) => (h, Some(p.parse().ok()?)),
            None => (rest, None),
        };
        let mut hop = Hop {
            method: method.to_string(),
            user,
            host: host.to_string(),
            port,
        };
        // `/sudo:root:` and `/sudo::` name the user, not a host.
        if matches!(method, "sudo" | "doas") && hop.user.is_none() && !hop.host.is_empty() {
            hop.user = Some(std::mem::take(&mut hop.host));
        }
        if matches!(method, "ssh" | "scp" | "docker" | "podman" | "kubectl") && hop.host.is_empty()
        {
            return None;
        }
        Some(hop)
    }

    fn spec(&self) -> String {
        let mut s = String::new();
        if matches!(self.method.as_str(), "sudo" | "doas") {
            s.push_str(&self.method);
            s.push(':');
            if let Some(u) = &self.user {
                s.push_str(u);
            }
            return s;
        }
        s.push_str(&self.method);
        s.push(':');
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.host);
        if let Some(p) = self.port {
            s.push_str(&format!("#{p}"));
        }
        s
    }
}

impl Remote {
    /// The hops written as in a file name: `ssh:me@box|sudo:`.
    pub fn prefix(&self) -> String {
        self.hops
            .iter()
            .map(Hop::spec)
            .collect::<Vec<_>>()
            .join("|")
    }

    /// `/ssh:me@box:/etc/hosts`.
    pub fn display(&self) -> String {
        format!("/{}:{}", self.prefix(), self.path)
    }

    pub fn with_path(&self, path: impl Into<String>) -> Remote {
        Remote {
            hops: self.hops.clone(),
            path: path.into(),
        }
    }

    pub fn file_name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
    }
}

/// Read a remote file name. Plain local names, and names whose first word is
/// not a method (`/foo:bar`), are not remote.
pub fn parse(s: &str) -> Option<Remote> {
    let rest = s.strip_prefix('/')?;
    let mut hops = Vec::new();
    let (method, after) = rest.split_once(':')?;
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let end = after.find(':')?;
    let target = &after[..end];
    let remainder = &after[end + 1..];
    match target.split_once('|') {
        Some((t, next)) => {
            hops.push(Hop::parse(method, t)?);
            // `next` is the method of the following hop; its target comes
            // after the colon we just consumed.
            let (t2, rem2) = split_target(remainder)?;
            let mut method2 = next.to_string();
            let mut t2 = t2;
            let mut rem2 = rem2;
            // A chain of three or more hops repeats the pattern.
            while let Some((a, b)) = t2.split_once('|') {
                hops.push(Hop::parse(&method2, a)?);
                method2 = b.to_string();
                let (t3, r3) = split_target(rem2)?;
                t2 = t3;
                rem2 = r3;
            }
            hops.push(Hop::parse(&method2, t2)?);
            Some(Remote {
                hops,
                path: rem2.to_string(),
            })
        }
        None => {
            hops.push(Hop::parse(method, target)?);
            Some(Remote {
                hops,
                path: remainder.to_string(),
            })
        }
    }
}

fn split_target(s: &str) -> Option<(&str, &str)> {
    let end = s.find(':')?;
    Some((&s[..end], &s[end + 1..]))
}

pub fn parse_path(p: &Path) -> Option<Remote> {
    parse(p.to_str()?)
}

/// The local path a Tether name stands for, when every hop is `local`.
pub(crate) fn local_path(r: &Remote) -> Option<PathBuf> {
    if !r.hops.iter().all(|h| h.method == "local") {
        return None;
    }
    // A verbatim Windows path only understands `\`; names grown from a
    // listing can carry `/`.
    #[cfg(windows)]
    return Some(PathBuf::from(r.path.replace('/', "\\")));
    #[cfg(not(windows))]
    Some(PathBuf::from(&r.path))
}

/// Make a path absolute and tidy `.` and `..`.
fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    let trailing = path.ends_with('/') && !out.is_empty();
    format!("/{}{}", out.join("/"), if trailing { "/" } else { "" })
}

fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A remote path as shell text, letting the remote shell expand `~`.
pub(crate) fn rp(path: &str) -> String {
    if path.is_empty() || path == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", sq(rest))
    } else {
        sq(path)
    }
}

// -------------------------------------------------------------- directories

pub fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("FORGE_TETHER_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("forge").join("tether")
}

fn control_dir() -> PathBuf {
    let dir = cache_root().join("ctl");
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

fn mirror_root() -> PathBuf {
    cache_root().join("mirror")
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Where the local copy of a remote file lives.
pub fn mirror_path(r: &Remote) -> PathBuf {
    let root = mirror_root().join(encode(&r.prefix()));
    #[cfg(windows)]
    if local_path(r).is_some() {
        let path = r.path.replace('\\', "/");
        if let Some((drive, rest)) = path.split_once(":/") {
            return root.join(format!("{drive}%3A")).join(rest);
        }
    }
    root.join(r.path.trim_start_matches('/'))
}

/// The remote file a mirror stands for, from its location alone.
pub fn from_mirror(p: &Path) -> Option<Remote> {
    let root = mirror_root();
    #[cfg(windows)]
    let (root, p) = match (std::fs::canonicalize(&root), std::fs::canonicalize(p)) {
        (Ok(root), Ok(p)) => (root, p),
        _ => (root, p.to_path_buf()),
    };
    #[cfg(not(windows))]
    let (root, p) = (root, p.to_path_buf());
    let rel = p.strip_prefix(root).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?.as_os_str().to_str()?;
    let prefix = decode(first)?;
    let rest: PathBuf = comps.collect();
    let rest = rest.to_string_lossy();
    #[cfg(windows)]
    let rest = {
        let mut rest = rest.replace('\\', "/");
        if rest.len() >= 5
            && rest.as_bytes()[0].is_ascii_alphabetic()
            && rest.as_bytes()[1..5].eq_ignore_ascii_case(b"%3A/")
        {
            rest.replace_range(1..5, ":\\");
            rest = rest.replace('/', "\\");
        }
        rest
    };
    #[cfg(windows)]
    if rest.len() >= 2 && rest.as_bytes()[0].is_ascii_alphabetic() && rest.as_bytes()[1] == b':' {
        return parse(&format!("/{prefix}:{rest}"));
    }
    parse(&format!("/{prefix}:/{rest}"))
}

// -------------------------------------------------------------- transports

fn hop_argv(hop: &Hop, script: &str, pw: bool) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut push = |s: &str| v.push(s.to_string());
    match hop.method.as_str() {
        "ssh" | "scp" => {
            push("ssh");
            if !pw {
                push("-o");
                push("BatchMode=yes");
            }
            for o in [
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=10m",
                "-o",
            ] {
                push(o);
            }
            push(&format!("ControlPath={}/%C", control_dir().display()));
            if let Some(p) = hop.port {
                push("-p");
                push(&p.to_string());
            }
            if let Some(u) = &hop.user {
                push("-l");
                push(u);
            }
            push(&hop.host);
            push("--");
            push(&format!("sh -c {}", sq(script)));
        }
        "sudo" | "doas" => {
            push(&hop.method);
            if pw && hop.method == "sudo" {
                // The password arrives as the first line of stdin.
                push("-S");
                push("-p");
                push("");
            } else {
                push("-n");
            }
            if let Some(u) = &hop.user {
                push("-u");
                push(u);
            }
            push("sh");
            push("-c");
            push(script);
        }
        "docker" | "podman" => {
            push(&hop.method);
            push("exec");
            push("-i");
            if let Some(u) = &hop.user {
                push("-u");
                push(u);
            }
            push(&hop.host);
            push("sh");
            push("-c");
            push(script);
        }
        "kubectl" => {
            for s in ["kubectl", "exec", "-i", &hop.host, "--", "sh", "-c"] {
                push(s);
            }
            push(script);
        }
        _ => {
            push("sh");
            push("-c");
            push(script);
        }
    }
    v
}

/// The local command line that runs `script` at the end of the chain.
pub fn argv(hops: &[Hop], script: &str) -> Vec<String> {
    let mut script = script.to_string();
    let mut out = Vec::new();
    for (i, hop) in hops.iter().enumerate().rev() {
        out = hop_argv(hop, &script, password_for(hops, i).is_some());
        if i > 0 {
            script = out.iter().map(|a| sq(a)).collect::<Vec<_>>().join(" ");
        }
    }
    out
}

fn explain(stderr: &str, code: Option<i32>, argv0: &str) -> String {
    let e = stderr.trim();
    let first = e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    if e.contains("Permission denied") {
        return "login refused (wrong password or no accepted key)".into();
    }
    if e.contains("a password is required") || e.contains("incorrect password") {
        return "sudo needs a password".into();
    }
    if e.contains("Host key verification failed") {
        return "the host key changed; check ~/.ssh/known_hosts".into();
    }
    if e.contains("Could not resolve hostname") || e.contains("Connection timed out") {
        return first.to_string();
    }
    if first.is_empty() {
        format!("{argv0} failed (exit {})", code.unwrap_or(-1))
    } else {
        first.to_string()
    }
}

// -------------------------------------------------------------- passwords

/// Passwords typed this session, by the hops they unlock. Memory only.
static PASSWORDS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
/// Set when an operation failed for want of a password: (key, prompt).
static NEED: Mutex<Option<(String, String)>> = Mutex::new(None);

/// The text identifying hops `0..=i`: what a password belongs to.
fn key_of(hops: &[Hop], i: usize) -> String {
    hops[..=i]
        .iter()
        .map(Hop::spec)
        .collect::<Vec<_>>()
        .join("|")
}

fn password_for(hops: &[Hop], i: usize) -> Option<String> {
    // Only the first ssh hop (answered by ssh itself) and sudo hops (answered
    // through stdin) can be given a password.
    let usable = match hops[i].method.as_str() {
        "ssh" | "scp" => i == 0,
        "sudo" => true,
        _ => false,
    };
    if !usable {
        return None;
    }
    // A password typed this session wins; else auth-source (~/.authinfo).
    let typed = PASSWORDS
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(&key_of(hops, i))
        .cloned();
    typed.or_else(|| authinfo_password(hops, i))
}

/// What ~/.authinfo (or .netrc) has for hop `i`. A sudo hop is looked up
/// by the host it runs on.
fn authinfo_password(hops: &[Hop], i: usize) -> Option<String> {
    let h = &hops[i];
    if h.method == "sudo" {
        let host = hops[..i].iter().rev().find(|p| !p.host.is_empty());
        let host = host.map_or("localhost", |p| p.host.as_str());
        crate::authinfo::lookup(&crate::authinfo::Query {
            host,
            user: Some(h.user.as_deref().unwrap_or("root")),
            port: None,
            service: "sudo",
        })
    } else {
        crate::authinfo::lookup(&crate::authinfo::Query {
            host: &h.host,
            user: h.user.as_deref(),
            port: h.port,
            service: "ssh",
        })
    }
}

pub fn remember_password(key: &str, pw: &str) {
    if let Ok(mut m) = PASSWORDS.get_or_init(Default::default).lock() {
        m.insert(key.to_string(), pw.to_string());
    }
}

fn forget_password(key: &str) {
    if let Ok(mut m) = PASSWORDS.get_or_init(Default::default).lock() {
        m.remove(key);
    }
}

/// Forget every typed password.
pub fn forget_passwords() {
    if let Ok(mut m) = PASSWORDS.get_or_init(Default::default).lock() {
        m.clear();
    }
}

/// Which hop's password does this failure ask for? Records it in `NEED`.
fn note_auth_failure(hops: &[Hop], stderr: &str) -> bool {
    let sudo_wanted = stderr.contains("a password is required")
        || stderr.contains("no tty present")
        || stderr.contains("incorrect password")
        || stderr.contains("Sorry, try again");
    let found = if sudo_wanted {
        hops.iter().position(|h| h.method == "sudo")
    } else if stderr.contains("Permission denied")
        && hops
            .first()
            .is_some_and(|h| matches!(h.method.as_str(), "ssh" | "scp"))
    {
        Some(0)
    } else {
        None
    };
    let Some(i) = found else {
        return false;
    };
    let key = key_of(hops, i);
    // A password that was tried and refused is wrong: forget it.
    forget_password(&key);
    let h = &hops[i];
    let label = if h.method == "sudo" {
        let on = hops[..i]
            .last()
            .map(|p| format!(" on {}", p.host))
            .unwrap_or_default();
        format!(
            "[sudo] password for {}{on}: ",
            h.user.as_deref().unwrap_or("root")
        )
    } else {
        let who = match &h.user {
            Some(u) => format!("{u}@{}", h.host),
            None => h.host.clone(),
        };
        format!("Password for {who}: ")
    };
    if let Ok(mut n) = NEED.lock() {
        *n = Some((key, label));
    }
    true
}

/// The password prompt an operation is waiting for, if any.
pub fn take_need() -> Option<(String, String)> {
    NEED.lock().ok()?.take()
}

/// A tiny program ssh runs to ask for a password: it prints the one in the
/// environment. (ssh refuses to read passwords from a pipe.)
fn askpass_script() -> Option<PathBuf> {
    let path = cache_root().join("askpass.sh");
    let body = "#!/bin/sh\nprintf '%s\\n' \"$TETHER_PASSWORD\"\n";
    if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
        std::fs::create_dir_all(cache_root()).ok()?;
        std::fs::write(&path, body).ok()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(path)
}

/// Environment for the local process, and the stdin lines (sudo passwords)
/// to send before anything else.
pub fn auth_for(hops: &[Hop]) -> (Vec<(String, String)>, Vec<u8>) {
    let mut env = Vec::new();
    let mut lines = Vec::new();
    for i in 0..hops.len() {
        let Some(pw) = password_for(hops, i) else {
            continue;
        };
        if i == 0 && matches!(hops[0].method.as_str(), "ssh" | "scp") {
            if let Some(script) = askpass_script() {
                env.push(("SSH_ASKPASS".into(), script.display().to_string()));
                env.push(("SSH_ASKPASS_REQUIRE".into(), "force".into()));
                env.push(("TETHER_PASSWORD".into(), pw));
                if std::env::var_os("DISPLAY").is_none() {
                    env.push(("DISPLAY".into(), "tether:0".into()));
                }
            }
        } else if hops[i].method == "sudo" {
            lines.extend_from_slice(pw.as_bytes());
            lines.push(b'\n');
        }
    }
    (env, lines)
}

static CONNECTIONS: OnceLock<Mutex<HashMap<String, (Vec<Hop>, usize)>>> = OnceLock::new();

fn note_use(hops: &[Hop]) {
    let key = hops.iter().map(Hop::spec).collect::<Vec<_>>().join("|");
    let map = CONNECTIONS.get_or_init(Default::default);
    if let Ok(mut m) = map.lock() {
        m.entry(key).or_insert_with(|| (hops.to_vec(), 0)).1 += 1;
    }
}

/// Run `script` on the far end. Output on success; a readable reason on failure.
pub fn run(
    hops: &[Hop],
    script: &str,
    input: Option<Vec<u8>>,
    secs: u64,
) -> Result<Vec<u8>, String> {
    let argv = argv(hops, script);
    note_use(hops);
    let (env, mut lines) = auth_for(hops);
    let input = if lines.is_empty() {
        input
    } else {
        lines.extend(input.unwrap_or_default());
        Some(lines)
    };
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", argv[0]))?;
    let stdin = child.stdin.take();
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let writer = std::thread::spawn(move || {
        if let Some(mut s) = stdin
            && let Some(d) = input
        {
            let _ = s.write_all(&d);
        }
    });
    let ot = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let et = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {secs}s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(3)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = writer.join();
    let stdout = ot.join().unwrap_or_default();
    let stderr = et.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&stderr);
        let message = explain(&stderr, status.code(), &argv[0]);
        if note_auth_failure(hops, &stderr) {
            return Err(AUTH_NEEDED.into());
        }
        Err(message)
    }
}

// -------------------------------------------------------------- operations

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub mtime: i64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Missing,
    File(Stamp),
    Dir,
}

/// The error text of an operation that needs a password first.
pub const AUTH_NEEDED: &str = "authentication needed";
const QUICK: u64 = 15;
const TRANSFER: u64 = 300;

/// Turn `~`, a relative name or `..` into an absolute path on the remote.
pub fn resolve(r: &Remote) -> Result<Remote, String> {
    #[cfg(windows)]
    if local_path(r).is_some() {
        let p = PathBuf::from(&r.path);
        let abs = if p.is_absolute() {
            p
        } else {
            std::env::current_dir().map_err(|e| e.to_string())?.join(p)
        };
        return Ok(r.with_path(abs.to_string_lossy().into_owned()));
    }
    if r.path.starts_with('/') {
        return Ok(r.with_path(normalize(&r.path)));
    }
    let out = run(&r.hops, "printf %s \"$HOME\"", None, QUICK)?;
    let home = String::from_utf8_lossy(&out).into_owned();
    let home = if home.is_empty() {
        "/".to_string()
    } else {
        home
    };
    let path = match r.path.as_str() {
        "" | "~" => home,
        p => match p.strip_prefix("~/") {
            Some(rest) => format!("{home}/{rest}"),
            None => format!("{home}/{p}"),
        },
    };
    Ok(r.with_path(normalize(&path)))
}

pub fn stat(r: &Remote) -> Result<Kind, String> {
    #[cfg(windows)]
    if let Some(p) = local_path(r) {
        return match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() => Ok(Kind::Dir),
            Ok(m) => {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                Ok(Kind::File(Stamp {
                    mtime,
                    size: m.len(),
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Kind::Missing),
            Err(e) => Err(e.to_string()),
        };
    }
    let p = rp(&r.path);
    let script = format!(
        "p={p}; if [ -d \"$p\" ]; then echo d; elif [ -e \"$p\" ]; then echo f; \
         stat -c '%Y %s' \"$p\" 2>/dev/null || stat -f '%m %z' \"$p\"; else echo n; fi"
    );
    let out = String::from_utf8_lossy(&run(&r.hops, &script, None, QUICK)?).into_owned();
    let mut lines = out.lines();
    match lines.next() {
        Some("d") => Ok(Kind::Dir),
        Some("n") => Ok(Kind::Missing),
        Some("f") => {
            let nums = lines.next().unwrap_or("");
            let mut it = nums.split_whitespace();
            let mtime = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let size = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            Ok(Kind::File(Stamp { mtime, size }))
        }
        _ => Err("unexpected reply from the remote shell".into()),
    }
}

pub fn read(r: &Remote) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    if let Some(p) = local_path(r) {
        return std::fs::read(p).map_err(|e| e.to_string());
    }
    run(&r.hops, &format!("cat -- {}", rp(&r.path)), None, TRANSFER)
}

/// Replace the remote file atomically, keeping its permissions. Symlinks are
/// followed, so the link itself survives.
pub fn write(r: &Remote, data: Vec<u8>) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(p) = local_path(r) {
        let tmp = p.with_file_name(format!(".tether.{}.tmp", std::process::id()));
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
        return std::fs::rename(&tmp, &p).map_err(|e| e.to_string());
    }
    let p = rp(&r.path);
    let script = format!(
        "p={p}; q=$(readlink -f \"$p\" 2>/dev/null) && [ -n \"$q\" ] && p=$q; \
         t=\"$(dirname \"$p\")/.tether.$$.tmp\"; \
         cat > \"$t\" || {{ rm -f \"$t\"; exit 1; }}; \
         if [ -e \"$p\" ]; then chmod --reference=\"$p\" \"$t\" 2>/dev/null || \
         chmod \"$(stat -f %Lp \"$p\" 2>/dev/null)\" \"$t\" 2>/dev/null; fi; \
         mv -f \"$t\" \"$p\" || {{ rm -f \"$t\"; exit 1; }}"
    );
    run(&r.hops, &script, Some(data), TRANSFER).map(|_| ())
}

type ListCache = Mutex<HashMap<String, (Instant, Vec<String>)>>;
static LISTINGS: OnceLock<ListCache> = OnceLock::new();
const LIST_TTL: Duration = Duration::from_secs(5);

/// Names in a directory; directories end with `/`. Cached for a few seconds
/// so completing as you type does not hit the network on every key.
pub fn list(r: &Remote) -> Result<Vec<String>, String> {
    let key = r.display();
    let cache = LISTINGS.get_or_init(Default::default);
    if let Ok(c) = cache.lock()
        && let Some((at, v)) = c.get(&key)
        && at.elapsed() < LIST_TTL
    {
        return Ok(v.clone());
    }
    #[cfg(windows)]
    let names: Vec<String> = if let Some(p) = local_path(r) {
        let mut names: Vec<String> = std::fs::read_dir(p)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| {
                let dir = e.path().is_dir();
                let mut name = e.file_name().to_string_lossy().into_owned();
                if dir {
                    name.push('/');
                }
                name
            })
            .collect();
        names.sort();
        names
    } else {
        let d = rp(&r.path);
        let script = format!("LC_ALL=C ls -1ApL -- {d} 2>/dev/null || LC_ALL=C ls -1Ap -- {d}");
        let out = run(&r.hops, &script, None, 20)?;
        let mut names: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        names.sort();
        names
    };
    #[cfg(not(windows))]
    let names: Vec<String> = {
        let d = rp(&r.path);
        let script = format!("LC_ALL=C ls -1ApL -- {d} 2>/dev/null || LC_ALL=C ls -1Ap -- {d}");
        let out = run(&r.hops, &script, None, 20)?;
        let mut names: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        names.sort();
        names
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(key, (Instant::now(), names.clone()));
    }
    Ok(names)
}

fn forget_listings() {
    if let Some(c) = LISTINGS.get()
        && let Ok(mut c) = c.lock()
    {
        c.clear();
    }
}

// ------------------------------------------------------------ fetch / push

static STAMPS: OnceLock<Mutex<HashMap<PathBuf, Option<Stamp>>>> = OnceLock::new();

fn stamp_key(mirror: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        std::fs::canonicalize(mirror).unwrap_or_else(|_| mirror.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        mirror.to_path_buf()
    }
}

fn set_stamp(mirror: &Path, stamp: Option<Stamp>) {
    if let Ok(mut m) = STAMPS.get_or_init(Default::default).lock() {
        m.insert(stamp_key(mirror), stamp);
    }
}

/// `Some(None)`: fetched while the file did not exist. `None`: never fetched
/// in this run (a restored session).
fn get_stamp(mirror: &Path) -> Option<Option<Stamp>> {
    STAMPS.get()?.lock().ok()?.get(&stamp_key(mirror)).copied()
}

/// Copy the remote file into its mirror; returns the mirror's path.
pub fn fetch(r: &Remote) -> Result<PathBuf, String> {
    let kind = stat(r)?;
    let mirror = mirror_path(r);
    if let Some(dir) = mirror.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    match kind {
        Kind::Dir => return Err("is a directory".into()),
        Kind::Missing => {
            let _ = std::fs::remove_file(&mirror);
            set_stamp(&mirror, None);
        }
        Kind::File(st) => {
            let data = read(r)?;
            std::fs::write(&mirror, data).map_err(|e| e.to_string())?;
            set_stamp(&mirror, Some(st));
        }
    }
    Ok(mirror)
}

/// Has the remote file changed since this mirror was fetched?
/// `Ok(Some(why))` means "ask before overwriting".
fn conflict(r: &Remote, mirror: &Path) -> Result<Option<&'static str>, String> {
    let now = stat(r)?;
    Ok(match (get_stamp(mirror), now) {
        (_, Kind::Dir) => Some("the remote name is now a directory"),
        (None, Kind::Missing) => None,
        (None, Kind::File(_)) => Some("this buffer was restored; the remote file may have changed"),
        (Some(None), Kind::Missing) => None,
        (Some(None), Kind::File(_)) => Some("the remote file was created since you opened it"),
        (Some(Some(_)), Kind::Missing) => Some("the remote file was deleted since you opened it"),
        (Some(Some(old)), Kind::File(new)) if old != new => {
            Some("the remote file changed since you opened it")
        }
        _ => None,
    })
}

/// The remote a buffer's file stands for, if it is a mirror.
pub fn remote_of_buffer(app: &App) -> Option<Remote> {
    from_mirror(app.buf_ref().path()?)
}

/// `C-x C-s` in a mirror. True when the buffer was a mirror (handled here).
pub fn save(app: &mut App, force: bool) -> bool {
    let Some(remote) = remote_of_buffer(app) else {
        return false;
    };
    let id = app.active_id();
    let mirror = app
        .buf_ref()
        .path()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let shown = remote.display();
    if !force {
        match conflict(&remote, &mirror) {
            Err(e) => {
                fail(
                    app,
                    format!("{NAME}: cannot reach {shown}: {e}"),
                    &e,
                    Retry::Save(false),
                );
                return true;
            }
            Ok(Some(why)) => {
                app.prompt(
                    format!("{NAME}: {why}. Save {shown} anyway? (y or n) "),
                    Purpose::Confirm(ConfirmAction::TetherSave(id)),
                );
                return true;
            }
            Ok(None) => {}
        }
    }
    if let Err(e) = app.buf().save() {
        app.error(format!("Error saving {shown}: {e}"));
        return true;
    }
    let data = match std::fs::read(&mirror) {
        Ok(d) => d,
        Err(e) => {
            app.error(format!("Error reading {}: {e}", mirror.display()));
            return true;
        }
    };
    match write(&remote, data).and_then(|()| stat(&remote)) {
        Ok(kind) => {
            forget_listings();
            set_stamp(
                &mirror,
                match kind {
                    Kind::File(s) => Some(s),
                    _ => None,
                },
            );
            app.msg(format!("Wrote {shown}"));
            crate::files::after_save(app);
        }
        Err(e) => {
            // The local copy is saved, the remote is not: keep the buffer
            // marked as needing a save.
            app.buf().set_modified(true);
            fail(
                app,
                format!("{NAME}: could not write {shown}: {e}"),
                &e,
                Retry::Save(true),
            );
        }
    }
    true
}

// ----------------------------------------------------- password prompts

/// What to do again once a password has been typed.
#[derive(Clone, Debug)]
pub enum Retry {
    Visit {
        name: String,
        other_window: bool,
        other_frame: bool,
        other_tab: bool,
    },
    Save(bool),
    Revert,
    /// Run a compile (`true`) or shell command again from its prompt text.
    Replay {
        compile: bool,
        insert: bool,
        input: String,
    },
}

fn visit_retry(name: &str, other_window: bool, other_frame: bool, other_tab: bool) -> Retry {
    Retry::Visit {
        name: name.to_string(),
        other_window,
        other_frame,
        other_tab,
    }
}

/// Report a failed operation: as a hidden password prompt when it failed
/// for want of one, else as an error.
fn fail(app: &mut App, message: String, err: &str, retry: Retry) {
    if err == AUTH_NEEDED
        && let Some((key, label)) = take_need()
    {
        app.prompt(label, Purpose::TetherAuth { key, retry });
        if let Some(mb) = app.minibuffer.as_mut() {
            mb.secret = true;
        }
        return;
    }
    app.error(message);
}

/// The password prompt's answer: remember it and do the thing again.
pub fn answered(app: &mut App, key: &str, retry: Retry, input: &str) {
    if input.is_empty() {
        return app.msg("Quit");
    }
    remember_password(key, input);
    match retry {
        Retry::Visit {
            name,
            other_window,
            other_frame,
            other_tab,
        } => {
            visit(app, &name, other_window, other_frame, other_tab);
        }
        Retry::Save(force) => {
            save(app, force);
        }
        Retry::Revert => {
            execute(app, "tether-revert");
        }
        Retry::Replay {
            compile,
            insert,
            input,
        } => {
            let purpose = if compile {
                Purpose::Compile
            } else {
                Purpose::ShellCommand { insert }
            };
            crate::compile::answered(app, &purpose, &input);
        }
    }
}

/// Before running a command in a remote directory: make sure the machine can
/// be reached, asking for a password if that is what is missing. False means
/// do not run now (an error or a prompt is showing).
pub fn ensure_auth(app: &mut App, dir: &Path, retry: Retry) -> bool {
    let Some(r) = parse_path(dir) else {
        return true;
    };
    if local_path(&r).is_some() {
        return true;
    }
    match run(&r.hops, "true", None, QUICK) {
        Ok(_) => true,
        Err(e) => {
            fail(app, format!("{NAME}: {}: {e}", r.display()), &e, retry);
            false
        }
    }
}

/// The local command that runs `command` in the remote directory `dir`, the
/// environment it needs, and the stdin lines (sudo passwords) to feed it.
pub fn job_command(dir: &Remote, command: &str) -> (Vec<String>, Vec<(String, String)>, Vec<u8>) {
    #[cfg(windows)]
    if let Some(p) = local_path(dir) {
        let script = format!("cd /d \"{}\" && {}", p.display(), command);
        return (
            vec!["cmd".into(), "/C".into(), script],
            Vec::new(),
            Vec::new(),
        );
    }
    let script = format!("cd {} && {{ {}\n}}", rp(&dir.path), command);
    let (env, lines) = auth_for(&dir.hops);
    (argv(&dir.hops, &script), env, lines)
}

/// The remote directory commands should run in for this buffer: a mirror's
/// directory, or the directory a Tether listing shows.
pub fn default_dir(app: &App) -> Option<PathBuf> {
    if let Some(d) = current_dir(app) {
        return Some(PathBuf::from(d.display()));
    }
    let parent = app.buf_ref().path()?.parent()?;
    from_mirror(parent).map(|r| PathBuf::from(r.display()))
}

// ------------------------------------------------------------------ visits

/// Open a remote name from find-file. True when the name was remote.
pub fn visit(
    app: &mut App,
    name: &str,
    other_window: bool,
    other_frame: bool,
    other_tab: bool,
) -> bool {
    let Some(remote) = parse(name) else {
        return false;
    };
    let remote = match resolve(&remote) {
        Ok(r) => r,
        Err(e) => {
            fail(
                app,
                format!("{NAME}: {}: {e}", remote.display()),
                &e,
                visit_retry(name, other_window, other_frame, other_tab),
            );
            return true;
        }
    };
    match stat(&remote) {
        Err(e) => fail(
            app,
            format!("{NAME}: {}: {e}", remote.display()),
            &e,
            visit_retry(name, other_window, other_frame, other_tab),
        ),
        Ok(Kind::Dir) => {
            if other_tab {
                app.editor.make_tab();
            }
            if other_frame {
                app.editor.make_frame();
            } else if other_window {
                crate::windows::pop_to_other_window(app);
            }
            crate::dired::open_remote(app, &remote);
        }
        Ok(_) => {
            // Re-fetching under an open, edited buffer would lose the edits:
            // just go to it.
            let mirror = mirror_path(&remote);
            let open = app.editor.find_buffer_by_path(&mirror).is_some();
            if !open && let Err(e) = fetch(&remote) {
                fail(
                    app,
                    format!("{NAME}: {}: {e}", remote.display()),
                    &e,
                    visit_retry(name, other_window, other_frame, other_tab),
                );
                return true;
            }
            crate::minibuffer::visit_file_where(app, &mirror, other_window, other_frame, other_tab);
        }
    }
    true
}

fn dir_buffer_name(r: &Remote) -> String {
    format!("*{NAME}: {}*", r.display())
}

fn show_dir(app: &mut App, dir: &Remote) {
    let mut dir = dir.clone();
    if !dir.path.ends_with('/') {
        dir.path.push('/');
    }
    forget_listings();
    let names = match list(&dir) {
        Ok(n) => n,
        Err(e) => {
            let retry = visit_retry(&dir.display(), false, false, false);
            return fail(app, format!("{NAME}: {}: {e}", dir.display()), &e, retry);
        }
    };
    let mut text = format!("{}\n", dir.display());
    for n in &names {
        text.push_str(n);
        text.push('\n');
    }
    let name = dir_buffer_name(&dir);
    app.show_special_buffer(&name, DIR_MODE, &text);
    let b = app.buf();
    b.set_read_only(true);
    b.set_point(forge_core::Pos::new(
        if names.is_empty() { 0 } else { 1 },
        0,
    ));
    app.msg(format!("{} entries; RET opens, ^ goes up", names.len()));
}

fn current_dir(app: &App) -> Option<Remote> {
    if app.buf_ref().major_mode() != DIR_MODE {
        return None;
    }
    parse(app.buf_ref().line(0).trim())
}

// ------------------------------------------------------------- completion

fn ssh_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    if let Ok(cfg) = std::fs::read_to_string(home.join(".ssh/config")) {
        for line in cfg.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("Host ").or_else(|| l.strip_prefix("Host\t")) {
                hosts.extend(
                    rest.split_whitespace()
                        .filter(|h| !h.contains(['*', '?', '!']))
                        .map(str::to_string),
                );
            }
        }
    }
    if let Ok(known) = std::fs::read_to_string(home.join(".ssh/known_hosts")) {
        for line in known.lines() {
            let first = line.split_whitespace().next().unwrap_or("");
            if first.starts_with('|') || first.starts_with('#') || first.starts_with('@') {
                continue;
            }
            for h in first.split(',') {
                let h = h.trim_start_matches('[');
                let h = h.split("]:").next().unwrap_or(h);
                if !h.is_empty() {
                    hosts.push(h.to_string());
                }
            }
        }
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

fn containers(tool: &str) -> Vec<String> {
    let out = Command::new(tool)
        .args(["ps", "--format", "{{.Names}}"])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Candidates for a find-file prompt whose input is (or is turning into) a
/// remote name. None: this is an ordinary local path.
pub fn complete(input: &str) -> Option<Vec<String>> {
    let rest = input.strip_prefix('/')?;
    // `/ssh:` + a partial host: complete hosts.
    let (method, after) = rest.split_once(':')?;
    if !METHODS.contains(&method) {
        return None;
    }
    if !after.contains(':') {
        let (user_part, host_part) = match after.rsplit_once('@') {
            Some((u, h)) => (format!("{u}@"), h),
            None => (String::new(), after),
        };
        let pool = match method {
            "ssh" | "scp" => ssh_hosts(),
            "docker" | "podman" => containers(method),
            "sudo" | "doas" => vec!["root".to_string()],
            _ => Vec::new(),
        };
        return Some(
            pool.into_iter()
                .filter(|h| h.starts_with(host_part))
                .map(|h| format!("/{method}:{user_part}{h}:"))
                .collect(),
        );
    }
    let local = cfg!(windows) && method == "local";
    let split = input.rfind(|c: char| c == '/' || (local && c == '\\'));
    let (dir_part, prefix) = match split {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => return Some(Vec::new()),
    };
    let remote = parse(dir_part)?;
    let names = list(&remote).ok()?;
    Some(
        names
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .map(|n| format!("{dir_part}{n}"))
            .collect(),
    )
}

// ---------------------------------------------------------------- commands

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "tether-browse" => {
            app.prompt(
                "Find file: ",
                Purpose::FindFile {
                    other_window: false,
                    other_frame: false,
                    other_tab: false,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = "/ssh:".to_string();
            }
        }
        "tether-sudo-edit" => {
            let Some(path) = app.buf_ref().path().map(Path::to_path_buf) else {
                app.error("Buffer is not visiting a file");
                return true;
            };
            let target = match from_mirror(&path) {
                Some(mut r) => {
                    r.hops.push(Hop {
                        method: "sudo".into(),
                        user: None,
                        host: String::new(),
                        port: None,
                    });
                    r.display()
                }
                None => format!("/sudo::{}", path.display()),
            };
            visit(app, &target, false, false, false);
        }
        "tether-revert" => {
            let Some(remote) = remote_of_buffer(app) else {
                app.error("This buffer is not a remote file");
                return true;
            };
            match fetch(&remote) {
                Ok(_) => match app.buf().revert() {
                    Ok(()) => app.msg(format!("Reverted {}", remote.display())),
                    Err(e) => app.error(format!("{NAME}: {e}")),
                },
                Err(e) => fail(
                    app,
                    format!("{NAME}: {}: {e}", remote.display()),
                    &e,
                    Retry::Revert,
                ),
            }
        }
        "tether-connections" => {
            let mut text = String::from("Connections used in this session\n\n");
            let map = CONNECTIONS.get_or_init(Default::default);
            let mut rows: Vec<(String, usize)> = map
                .lock()
                .map(|m| m.iter().map(|(k, (_, n))| (k.clone(), *n)).collect())
                .unwrap_or_default();
            rows.sort();
            if rows.is_empty() {
                text.push_str("(none)\n");
            }
            for (k, n) in rows {
                text.push_str(&format!("{k:<40} {n} operations\n"));
            }
            app.show_special_buffer(&format!("*{NAME} connections*"), "special-mode", &text);
        }
        "tether-disconnect-all" => {
            let hops: Vec<Hop> = CONNECTIONS
                .get_or_init(Default::default)
                .lock()
                .map(|m| {
                    m.values()
                        .flat_map(|(h, _)| h.iter().cloned())
                        .filter(|h| matches!(h.method.as_str(), "ssh" | "scp"))
                        .collect()
                })
                .unwrap_or_default();
            let mut closed = 0;
            for hop in hops {
                let mut c = Command::new("ssh");
                c.args(["-O", "exit", "-o"])
                    .arg(format!("ControlPath={}/%C", control_dir().display()));
                if let Some(p) = hop.port {
                    c.args(["-p", &p.to_string()]);
                }
                if let Some(u) = &hop.user {
                    c.args(["-l", u]);
                }
                c.arg(&hop.host).stdout(Stdio::null()).stderr(Stdio::null());
                if c.status().is_ok_and(|s| s.success()) {
                    closed += 1;
                }
            }
            forget_listings();
            forget_passwords();
            app.msg(format!(
                "{NAME}: closed {closed} connection(s) and forgot passwords"
            ));
        }
        "tether-dir-open" => {
            let Some(dir) = current_dir(app) else {
                app.error("Not in a Tether directory listing");
                return true;
            };
            let line = app.buf_ref().point().line;
            let name = app.buf_ref().line(line).trim().to_string();
            if line == 0 || name.is_empty() {
                app.error("No file on this line");
            } else {
                let target = dir.with_path(format!("{}{}", dir.path, name)).display();
                visit(app, &target, false, false, false);
            }
        }
        "tether-dir-up" => {
            let Some(dir) = current_dir(app) else {
                app.error("Not in a Tether directory listing");
                return true;
            };
            let up = normalize(&format!("{}/..", dir.path.trim_end_matches('/')));
            show_dir(app, &dir.with_path(up));
        }
        "tether-dir-refresh" => {
            let Some(dir) = current_dir(app) else {
                app.error("Not in a Tether directory listing");
                return true;
            };
            show_dir(app, &dir);
        }
        _ => return false,
    }
    true
}

pub const DIR_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "tether-dir-open"),
    ("^", "tether-dir-up"),
    ("g", "tether-dir-refresh"),
];

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn local(path: &str) -> Remote {
        parse(&format!("/local::{path}")).unwrap()
    }

    #[test]
    fn parses_names() {
        let r = parse("/ssh:me@box#2222:/etc/hosts").unwrap();
        assert_eq!(r.hops.len(), 1);
        assert_eq!(r.hops[0].user.as_deref(), Some("me"));
        assert_eq!(r.hops[0].host, "box");
        assert_eq!(r.hops[0].port, Some(2222));
        assert_eq!(r.path, "/etc/hosts");
        assert_eq!(r.display(), "/ssh:me@box#2222:/etc/hosts");
        let s = parse("/sudo::/etc/shadow").unwrap();
        assert_eq!(s.hops[0].method, "sudo");
        assert_eq!(s.display(), "/sudo::/etc/shadow");
        let s = parse("/sudo:www:/x").unwrap();
        assert_eq!(s.hops[0].user.as_deref(), Some("www"));
        let d = parse("/docker:web:/app/main.rs").unwrap();
        assert_eq!(d.hops[0].host, "web");
        assert!(parse("/home/me/file").is_none());
        assert!(parse("/foo:bar:/x").is_none());
        assert!(parse("/ssh::/x").is_none());
        assert!(parse("relative").is_none());
    }

    #[test]
    fn parses_chains() {
        let r = parse("/ssh:jump|ssh:inner|sudo::/etc/shadow").unwrap();
        let m: Vec<_> = r.hops.iter().map(|h| h.method.as_str()).collect();
        assert_eq!(m, ["ssh", "ssh", "sudo"]);
        assert_eq!(r.hops[1].host, "inner");
        assert_eq!(r.path, "/etc/shadow");
        assert_eq!(r.display(), "/ssh:jump|ssh:inner|sudo::/etc/shadow");
        let r = parse("/ssh:jump|sudo::/x").unwrap();
        assert_eq!(r.hops.len(), 2);
        assert_eq!(r.path, "/x");
    }

    #[test]
    fn quoting_and_tilde() {
        assert_eq!(sq("it's"), "'it'\\''s'");
        assert_eq!(rp("~"), "\"$HOME\"");
        assert_eq!(rp("~/a b"), "\"$HOME\"/'a b'");
        assert_eq!(normalize("/a/./b/../c/"), "/a/c/");
        assert_eq!(normalize("/.."), "/");
    }

    #[test]
    fn mirrors_map_back() {
        let r = parse("/ssh:me@box#22|sudo::/etc/my file.conf").unwrap();
        let m = mirror_path(&r);
        assert_eq!(from_mirror(&m), Some(r));
        assert!(from_mirror(Path::new("/home/me/x")).is_none());
    }

    #[test]
    fn ssh_command_line_reuses_connections() {
        let r = parse("/ssh:me@box#99:/x").unwrap();
        let a = argv(&r.hops, "true");
        assert_eq!(a[0], "ssh");
        assert!(a.iter().any(|s| s == "BatchMode=yes"));
        assert!(a.iter().any(|s| s.starts_with("ControlPath=")));
        assert!(a.iter().any(|s| s == "99"));
        assert!(a.iter().any(|s| s == "me"));
        let chain = parse("/ssh:a|sudo::/x").unwrap();
        let a = argv(&chain.hops, "true");
        assert_eq!(a[0], "ssh");
        assert!(a.last().unwrap().contains("sudo"));
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-tether-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cleanup_test_mirror(remote: &Remote) {
        let mirror = mirror_path(remote);
        let _ = std::fs::remove_file(&mirror);
        let _ = std::fs::remove_dir_all(mirror);
    }

    #[test]
    fn local_method_reads_writes_and_lists() {
        let dir = scratch("ops");
        let file = dir.join("it's a file.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let r = local(file.to_str().unwrap());
        assert!(matches!(stat(&r).unwrap(), Kind::File(s) if s.size == 6));
        assert_eq!(read(&r).unwrap(), b"hello\n");
        write(&r, b"changed\n".to_vec()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "changed\n");
        // No temp file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tether."))
            .collect();
        assert!(leftovers.is_empty());
        std::fs::create_dir(dir.join("sub")).unwrap();
        let names = list(&local(&format!("{}/", dir.display()))).unwrap();
        assert!(names.contains(&"sub/".to_string()));
        assert!(names.contains(&"it's a file.txt".to_string()));
        assert_eq!(stat(&local(dir.to_str().unwrap())).unwrap(), Kind::Dir);
        assert_eq!(
            stat(&local(dir.join("nope").to_str().unwrap())).unwrap(),
            Kind::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn writing_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("perm");
        let real = dir.join("real.sh");
        std::fs::write(&real, "a").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.join("link.sh");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(&local(link.to_str().unwrap()), b"b".to_vec()).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "b");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chain_of_two_local_hops_works() {
        let dir = scratch("chain");
        let file = dir.join("f");
        std::fs::write(&file, "deep").unwrap();
        let r = parse(&format!("/local:|local::{}", file.display())).unwrap();
        assert_eq!(r.hops.len(), 2);
        assert_eq!(read(&r).unwrap(), b"deep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn failures_are_readable_and_time_out() {
        let r = local("/definitely/not/here");
        let e = run(&r.hops, "cat /definitely/not/here", None, 5).unwrap_err();
        assert!(e.to_lowercase().contains("no such file"), "{e}");
        let e = run(&r.hops, "sleep 5", None, 1).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        assert!(explain("sudo: a password is required", Some(1), "sudo").contains("password"));
    }

    #[test]
    fn completes_remote_directories_and_hosts() {
        let dir = scratch("comp");
        std::fs::write(dir.join("alpha.txt"), "").unwrap();
        std::fs::write(dir.join("beta.txt"), "").unwrap();
        std::fs::create_dir(dir.join("alps")).unwrap();
        let typed = format!("/local::{}/al", dir.display());
        let mut got = complete(&typed).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                format!("/local::{}/alpha.txt", dir.display()),
                format!("/local::{}/alps/", dir.display())
            ]
        );
        assert!(complete("/etc/hosts").is_none());
        assert!(complete("/ssh:").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflict_detection() {
        let dir = scratch("conf");
        let file = dir.join("f");
        std::fs::write(&file, "one").unwrap();
        let r = local(file.to_str().unwrap());
        let mirror = fetch(&r).unwrap();
        assert_eq!(std::fs::read_to_string(&mirror).unwrap(), "one");
        assert_eq!(conflict(&r, &mirror).unwrap(), None);
        std::fs::write(&file, "two and more").unwrap();
        assert!(conflict(&r, &mirror).unwrap().is_some());
        // A name that did not exist is fetched as empty and can be created.
        let new = local(dir.join("new").to_str().unwrap());
        let m = fetch(&new).unwrap();
        assert!(!m.exists());
        assert_eq!(conflict(&new, &m).unwrap(), None);
        std::fs::write(dir.join("new"), "x").unwrap();
        assert!(conflict(&new, &m).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
        cleanup_test_mirror(&r);
    }

    fn app() -> App {
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(forge_core::Buffer::new("scratch"));
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn open_edit_and_save_a_remote_file() {
        let dir = scratch("e2e");
        let file = dir.join("note.txt");
        std::fs::write(&file, "first\n").unwrap();
        let name = format!("/local::{}", file.display());
        let mut a = app();
        crate::minibuffer::visit_file(&mut a, Path::new(&name), false);
        assert_eq!(a.buf_ref().get_contents(), "first\n");
        assert_eq!(remote_of_buffer(&a).unwrap().display(), name);
        a.buf().insert_str("more ");
        crate::commands::save(&mut a);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "more first\n",
            "message={:?}, prompt={:?}, buffer={:?}, modified={}",
            a.message,
            a.minibuffer.as_ref().map(|mb| &mb.prompt),
            a.buf_ref().get_contents(),
            a.buf_ref().is_modified()
        );
        assert!(!a.buf_ref().is_modified());

        // Someone else changes the file: the next save asks first.
        std::fs::write(&file, "theirs and longer\n").unwrap();
        a.buf().insert_str("x");
        crate::commands::save(&mut a);
        assert!(a.minibuffer.is_some(), "should ask before overwriting");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "theirs and longer\n"
        );
        a.minibuffer = None;
        assert!(save(&mut a, true));
        assert!(std::fs::read_to_string(&file).unwrap().contains("more x"));

        // Relative names in a remote buffer mean the remote directory.
        assert_eq!(
            crate::minibuffer::buffer_directory_for_test(&a),
            PathBuf::from(format!("/local::{}", dir.display()))
        );
        let remote = remote_of_buffer(&a).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        cleanup_test_mirror(&remote);
    }

    #[test]
    fn remote_directories_list_and_open() {
        let dir = scratch("dirs");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::create_dir(dir.join("d")).unwrap();
        let mut a = app();
        let remote = local(&dir.display().to_string());
        crate::minibuffer::visit_file(&mut a, Path::new(&remote.display()), false);
        assert_eq!(a.buf_ref().major_mode(), crate::dired::MODE);
        assert!(a.buf_ref().get_contents().contains(" d\n"));
        let line = (0..a.buf_ref().line_count())
            .find(|&l| a.buf_ref().line(l).ends_with(" a.txt"))
            .unwrap();
        let col = a.buf_ref().line(line).chars().count() - 1;
        a.buf().set_point(forge_core::Pos::new(line, col));
        assert!(crate::dired::execute(&mut a, "dired-find-file"));
        assert_eq!(a.buf_ref().get_contents(), "A");
        let _ = std::fs::remove_dir_all(&dir);
        cleanup_test_mirror(&remote);
    }

    #[test]
    fn authinfo_supplies_ssh_and_sudo_passwords() {
        let f = std::env::temp_dir().join(format!("forge-authinfo-{}", std::process::id()));
        std::fs::write(
            &f,
            "machine authhost.invalid login ann port ssh password sshpw\n\
             machine authhost.invalid login root port sudo password sudopw\n",
        )
        .unwrap();
        crate::authinfo::set_files(Some(vec![f.display().to_string()]));
        let r = parse("/ssh:ann@authhost.invalid|sudo::/x").unwrap();
        let (env, lines) = auth_for(&r.hops);
        crate::authinfo::set_files(None);
        let _ = std::fs::remove_file(&f);
        assert!(
            env.iter()
                .any(|(k, v)| k == "TETHER_PASSWORD" && v == "sshpw")
        );
        assert_eq!(lines, b"sudopw\n");
    }

    #[test]
    #[cfg(unix)]
    fn passwords_change_the_command_line() {
        let r = parse("/ssh:pwuser@pwhost:/x").unwrap();
        let before = argv(&r.hops, "true");
        assert!(before.iter().any(|a| a == "BatchMode=yes"));
        remember_password(&key_of(&r.hops, 0), "s3cret");
        let after = argv(&r.hops, "true");
        assert!(!after.iter().any(|a| a == "BatchMode=yes"));
        let (env, lines) = auth_for(&r.hops);
        assert!(
            env.iter()
                .any(|(k, v)| k == "TETHER_PASSWORD" && v == "s3cret")
        );
        assert!(env.iter().any(|(k, _)| k == "SSH_ASKPASS"));
        assert!(lines.is_empty());
        // The askpass helper really prints the password.
        let script = askpass_script().unwrap();
        let out = Command::new(script)
            .env("TETHER_PASSWORD", "s3cret")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "s3cret\n");

        let s = parse("/ssh:pwhost2|sudo::/etc/x").unwrap();
        let plain = |v: Vec<String>| v.last().unwrap().replace(['\'', '\\'], "");
        assert!(plain(argv(&s.hops, "true")).contains("sudo -n"));
        remember_password(&key_of(&s.hops, 1), "rootpw");
        let a = argv(&s.hops, "true");
        assert!(plain(a.clone()).contains("sudo -S -p  "), "{a:?}");
        let (_, lines) = auth_for(&s.hops);
        assert_eq!(lines, b"rootpw\n");
        forget_passwords();
        assert!(auth_for(&r.hops).0.is_empty());
    }

    #[test]
    fn auth_failures_become_prompts() {
        let r = parse("/ssh:nobody@prompthost:/x").unwrap();
        remember_password(&key_of(&r.hops, 0), "wrong");
        assert!(note_auth_failure(
            &r.hops,
            "user@h: Permission denied (publickey,password)."
        ));
        let (key, label) = take_need().unwrap();
        assert_eq!(key, "ssh:nobody@prompthost");
        assert_eq!(label, "Password for nobody@prompthost: ");
        // The refused password was forgotten.
        assert!(password_for(&r.hops, 0).is_none());
        let s = parse("/ssh:box|sudo:root:/x").unwrap();
        assert!(note_auth_failure(&s.hops, "sudo: a password is required"));
        let (_, label) = take_need().unwrap();
        assert_eq!(label, "[sudo] password for root on box: ");
        assert!(!note_auth_failure(&s.hops, "cat: nope: No such file"));
    }

    #[test]
    fn typing_a_password_retries_the_open() {
        let dir = scratch("retry");
        let file = dir.join("f.txt");
        std::fs::write(&file, "hi").unwrap();
        let mut a = app();
        let name = format!("/local::{}", file.display());
        let remote = parse(&name).unwrap();
        answered(
            &mut a,
            "local:",
            visit_retry(&name, false, false, false),
            "pw",
        );
        assert_eq!(a.buf_ref().get_contents(), "hi");
        let _ = std::fs::remove_dir_all(&dir);
        cleanup_test_mirror(&remote);
        forget_passwords();
    }

    #[test]
    #[cfg(unix)]
    fn shell_commands_run_in_the_remote_directory() {
        let dir = scratch("shell");
        std::fs::write(dir.join("marker.txt"), "x").unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "hi").unwrap();
        let mut a = app();
        let remote = local(&file.display().to_string());
        crate::minibuffer::visit_file(&mut a, Path::new(&remote.display()), false);
        assert_eq!(
            crate::compile::default_directory(&a),
            PathBuf::from(format!("/local::{}", dir.display()))
        );
        crate::compile::answered(&mut a, &Purpose::Compile, "ls; pwd");
        let mut out = String::new();
        for _ in 0..200 {
            crate::compile::poll(&mut a);
            out = a
                .editor
                .find_buffer("*compilation*")
                .and_then(|id| a.editor.state.get_buffer(id))
                .map(|b| b.get_contents())
                .unwrap_or_default();
            if out.contains("finished") || out.contains("exited") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(out.contains("marker.txt"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
        cleanup_test_mirror(&remote);
    }
}
