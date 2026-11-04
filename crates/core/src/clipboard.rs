// SPDX-License-Identifier: MIT OR Apache-2.0
//! System clipboard and selection targets.

/// `save-interprogram-paste-before-kill`: whether to save
/// external clipboard content to the kill ring before a kill replaces it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SaveInterprogramPasteBeforeKill {
    /// Non-nil / true: always save.
    Always,
    /// nil / false: don't save.
    #[default]
    Off,
    /// A number: only save if smaller (in characters) than this limit.
    Limit(usize),
}

/// The target buffer in the OS selection architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionTarget {
    /// The standard clipboard (macOS pasteboard, X11/Wayland CLIPBOARD).
    Clipboard,
    /// The primary selection (X11/Wayland PRIMARY, set by mouse dragging).
    Primary,
}
