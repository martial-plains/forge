// SPDX-License-Identifier: MIT OR Apache-2.0
//! Platform fallback for targets without Unix pseudo-terminals.

use std::io;
use std::path::Path;

pub enum Event {
    Data(Vec<u8>),
    Closed,
}

pub struct Pty;

pub fn default_shell(choice: &str) -> String {
    if !choice.trim().is_empty() {
        return choice.trim().to_string();
    }
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

pub fn installed_shells() -> Vec<String> {
    vec![default_shell("")]
}

impl Pty {
    pub fn spawn(
        _shell: &str,
        _dir: &Path,
        _rows: u16,
        _cols: u16,
        _env: &[(String, String)],
    ) -> io::Result<Pty> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pseudo-terminals are only supported on Unix",
        ))
    }

    pub fn write(&mut self, _bytes: &[u8]) {}

    pub fn resize(&self, _rows: u16, _cols: u16) {}

    pub fn poll(&mut self) -> Vec<Event> {
        Vec::new()
    }

    pub fn exited(&mut self) -> Option<Option<i32>> {
        Some(Some(0))
    }

    pub fn kill(&mut self) {}
}
