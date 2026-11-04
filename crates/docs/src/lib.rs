// SPDX-License-Identifier: MIT OR Apache-2.0
//! Structured documentation for Forge.
//!
//! This crate is the single source of truth for every documented thing in
//! the editor: its commands, options, modes, faces, hooks, events, plugin
//! API functions and manual nodes. Documentation is data (a [`Doc`]) that
//! is rendered at the last moment by each consumer (help buffers, the
//! manual browser, the HTML/Texinfo/man exporters and the CI lints).
//!
//! It depends on nothing but `serde`, so it can be used by the editor, by
//! `xtask` and by tests without pulling in `App` or Wasmtime.

#![warn(missing_docs)]

pub mod ast;
pub mod command;
pub mod lint;
pub mod manual;
pub mod news;
pub mod registry;
pub mod render;

pub use ast::*;
pub use command::*;
pub use lint::*;
pub use manual::*;
pub use news::*;
pub use registry::*;
pub use render::*;
