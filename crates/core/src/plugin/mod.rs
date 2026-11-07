// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plugins: the WebAssembly components Forge loads, the API they call,
//! and the metadata Forge keeps about them.
//!
//! - [`api`]: the plugin metadata (`plugin.toml`).
//! - [`registry`]: the installed plugins and their versions.
//! - [`runtime`]: the Wasmtime component runtime and the host side of the
//!   WIT API (`wit/forge-editor.wit`).
//! - [`host`]: the loaded plugins and every call into them.
//! - [`summary`]: what a plugin set up when its init ran, cached so a
//!   second start can stand it up without running the plugin again.

pub mod api;
pub mod host;
pub mod profile;
pub mod registry;
pub mod runtime;
pub mod summary;
