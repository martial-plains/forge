// SPDX-License-Identifier: MIT OR Apache-2.0
//! Settings for the *Messages* log, shared between the editor (which writes
//! it) and Lua (which sets `message-log-max`).

/// How many lines the *Messages* buffer keeps. This is Forge's
/// `message-log-max`: `t` never drops a line, `nil` turns logging off, and a
/// number (the default, 100) keeps that many lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLogMax {
    /// t: keep every line.
    Unlimited,
    /// nil: don't record messages at all.
    Off,
    /// A number: keep only the most recent this-many lines.
    Lines(usize),
}

impl Default for MessageLogMax {
    fn default() -> Self {
        MessageLogMax::Lines(100)
    }
}
