// SPDX-License-Identifier: MIT OR Apache-2.0
//! The terminal frontend: reads keys and mouse events, and paints what
//! the editor application (`forge-editor`) says to show.
//!
//! `forge-editor` holds the editor itself — keys to commands, the
//! minibuffer, plugins, init.rhai — and the macOS app drives the same
//! thing. What stays here is the terminal's own: ratatui painting
//! (`ui`), terminal capabilities (`caps`), the watchdog thread
//! (`watchdog`) and the `forge` binary's harnesses (`check`, `headless`).
//! The editor's modules are re-exported so `forge_terminal::app` and
//! friends still name them.

pub use forge_editor::*;

pub mod caps;
pub mod check;
pub mod cli;
pub mod gfx;
pub mod headless;
pub mod package_cli;
pub mod ui;
pub mod watchdog;

#[cfg(test)]
mod tests;
