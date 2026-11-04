// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the note plugins share, as plain Rust that is tested on its own.
//!
//! The plugins (`canvas`, `bases`, `graph`) are thin: they read files,
//! ask the user things and show what comes back. The thinking is here.
//!
//! * [`vault`] — a folder of notes as an index: properties from front
//!   matter, tags, links, backlinks.
//! * [`value`], [`expr`] — the values and expression language Bases files use
//!   for filters, formulas and sorting.
//! * [`base`] — an Obsidian `.base` file and running one of its views.
//! * [`geo`] — coordinates, the map projection and the Map layout's scene.
//! * [`graph`] — the link graph, local or global, its layout and its scene.
//! * [`jsoncanvas`] — the JSON Canvas format (`.canvas`): the model, edits
//!   and the scene that draws it.

pub mod base;
pub mod cache;
pub mod expr;
pub mod geo;
pub mod graph;
pub mod jsoncanvas;
pub mod value;
pub mod vault;
