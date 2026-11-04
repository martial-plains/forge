// SPDX-License-Identifier: MIT OR Apache-2.0
//! A pseudo-terminal and the shell on its far side.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

pub enum Event {
    Data(Vec<u8>),
    /// The program closed its side.
    Closed,
}

pub struct Pty {
    child: Child,
    writer: File,
    rx: Receiver<Event>,
    pub shell: String,
}

/// The shell to run: `choice` if the user named one (`term_shell`), else
/// `$SHELL`, else the login shell in the password database, else `sh`.
pub fn default_shell(choice: &str) -> String {
    if !choice.trim().is_empty() {
        return choice.trim().to_string();
    }
    if let Ok(s) = std::env::var("SHELL")
        && !s.is_empty()
    {
        return s;
    }
    // SAFETY: getpwuid returns a pointer into static storage (or null),
    // read at once and not kept.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_shell)
                .to_string_lossy()
                .into_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "/bin/sh".to_string()
}

/// The shells installed (from /etc/shells), for the prompt that picks one.
pub fn installed_shells() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string("/etc/shells")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/') && Path::new(l).exists())
        .map(str::to_string)
        .collect();
    let def = default_shell("");
    if !v.contains(&def) {
        v.insert(0, def);
    }
    v.dedup();
    v
}

impl Pty {
    pub fn spawn(
        shell: &str,
        dir: &Path,
        rows: u16,
        cols: u16,
        env: &[(String, String)],
    ) -> std::io::Result<Pty> {
        let mut words = shell.split_whitespace();
        let prog = words.next().unwrap_or("/bin/sh");
        let args: Vec<&str> = words.collect();
        let (mut master, mut slave) = (0, 0);
        let mut ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty fills in two descriptors we own from here on.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut ws,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both descriptors are valid and owned only here.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .current_dir(dir)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "forge")
            .env("INSIDE_FORGE", "1")
            .env_remove("NO_COLOR");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop(cmd);
        let writer = File::from(master.try_clone()?);
        let mut reader = File::from(master);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Event::Closed);
                        break;
                    }
                    Ok(n) => {
                        if tx.send(Event::Data(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Pty {
            child,
            writer,
            rx,
            shell: shell.to_string(),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        use std::os::fd::AsRawFd;
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: a valid descriptor and a winsize that outlives the call.
        unsafe {
            libc::ioctl(self.writer.as_raw_fd(), libc::TIOCSWINSZ as _, &ws);
        }
    }

    /// What has arrived since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// The exit status once the shell is gone.
    pub fn exited(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    pub fn kill(&mut self) {
        // The shell leads its own session and process group.
        // SAFETY: signalling a process group we started.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGHUP);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    #[cfg(unix)]
    fn a_shell_runs_behind_a_pty() {
        let mut p = Pty::spawn("/bin/sh", Path::new("/"), 24, 80, &[]).unwrap();
        p.write(b"echo forge-$((6*7))\n");
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !String::from_utf8_lossy(&got).contains("forge-42\r\n") {
            for e in p.poll() {
                if let Event::Data(d) = e {
                    got.extend(d);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let s = String::from_utf8_lossy(&got).into_owned();
        assert!(s.contains("forge-42\r\n"), "{s:?}");
        p.write(b"exit\n");
    }

    #[test]
    fn the_default_shell_is_never_empty() {
        assert!(!default_shell("").is_empty());
        assert_eq!(default_shell("/bin/zsh"), "/bin/zsh");
    }
}
