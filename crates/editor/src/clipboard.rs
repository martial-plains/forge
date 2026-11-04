// SPDX-License-Identifier: MIT OR Apache-2.0
//! System clipboard and selection buffer integration.
//!
//! Links kill and yank commands to the OS clipboard and primary selection buffers
//!.

use forge_core::clipboard::SelectionTarget;

/// Provider trait for reading and writing OS selection buffers.
pub trait ClipboardProvider: Send + Sync {
    /// Read text from the given OS selection buffer.
    fn get_text(&self, target: SelectionTarget) -> Option<String>;
    /// Write text to the given OS selection buffer.
    fn set_text(&mut self, target: SelectionTarget, text: &str);
}

/// The system clipboard provider using OS tools (pbcopy/pbpaste on macOS, wl-clipboard or xclip/xsel on Linux).
#[derive(Debug, Default)]
pub struct SystemClipboard;

impl SystemClipboard {
    pub fn new() -> Self {
        Self
    }
}

impl ClipboardProvider for SystemClipboard {
    fn get_text(&self, target: SelectionTarget) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            if target == SelectionTarget::Clipboard {
                let Ok(out) = std::process::Command::new("pbpaste").output() else {
                    return None;
                };
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            None
        }

        #[cfg(not(target_os = "macos"))]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                let mut cmd = std::process::Command::new("wl-paste");
                cmd.arg("--no-newline");
                if target == SelectionTarget::Primary {
                    cmd.arg("--primary");
                }
                if let Ok(out) = cmd.output() {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout).to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
            if std::env::var_os("DISPLAY").is_some() {
                let sel = match target {
                    SelectionTarget::Clipboard => "clipboard",
                    SelectionTarget::Primary => "primary",
                };
                if let Ok(out) = std::process::Command::new("xclip")
                    .args(["-selection", sel, "-o"])
                    .output()
                {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout).to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                } else if let Ok(out) = std::process::Command::new("xsel")
                    .args(match target {
                        SelectionTarget::Clipboard => ["-b", "-o"],
                        SelectionTarget::Primary => ["-p", "-o"],
                    })
                    .output()
                {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout).to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
            None
        }
    }

    fn set_text(&mut self, target: SelectionTarget, text: &str) {
        // Most clipboard APIs treat a NUL as the end of the string, so the
        // text would be cut short there. Send the two characters `\0` instead.
        let sanitized = text.replace('\0', "\\0");

        #[cfg(target_os = "macos")]
        {
            if target == SelectionTarget::Clipboard {
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(sanitized.as_bytes());
                    }
                    let _ = child.wait();
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            use std::io::Write;
            use std::process::{Command, Stdio};
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                let mut cmd = Command::new("wl-copy");
                if target == SelectionTarget::Primary {
                    cmd.arg("--primary");
                }
                cmd.stdin(Stdio::piped());
                if let Ok(mut child) = cmd.spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(sanitized.as_bytes());
                    }
                    let _ = child.wait();
                    return;
                }
            }
            if std::env::var_os("DISPLAY").is_some() {
                let sel = match target {
                    SelectionTarget::Clipboard => "clipboard",
                    SelectionTarget::Primary => "primary",
                };
                if let Ok(mut child) = Command::new("xclip")
                    .args(["-selection", sel])
                    .stdin(Stdio::piped())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(sanitized.as_bytes());
                    }
                    let _ = child.wait();
                } else if let Ok(mut child) = Command::new("xsel")
                    .args(match target {
                        SelectionTarget::Clipboard => ["-b", "-i"],
                        SelectionTarget::Primary => ["-p", "-i"],
                    })
                    .stdin(Stdio::piped())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(sanitized.as_bytes());
                    }
                    let _ = child.wait();
                }
            }
        }
    }
}

/// In-memory clipboard provider for unit tests and headless environments.
#[derive(Debug, Default, Clone)]
pub struct InMemoryClipboard {
    pub clipboard: Option<String>,
    pub primary: Option<String>,
}

impl InMemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ClipboardProvider for InMemoryClipboard {
    fn get_text(&self, target: SelectionTarget) -> Option<String> {
        match target {
            SelectionTarget::Clipboard => self.clipboard.clone(),
            SelectionTarget::Primary => self.primary.clone(),
        }
    }

    fn set_text(&mut self, target: SelectionTarget, text: &str) {
        let sanitized = text.replace('\0', "\\0");
        match target {
            SelectionTarget::Clipboard => self.clipboard = Some(sanitized),
            SelectionTarget::Primary => self.primary = Some(sanitized),
        }
    }
}
