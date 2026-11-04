// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge's canvas: a scene of shapes, paths, text and pictures.
//!
//! canvas (an image type whose ARGB32 pixels a module writes and
//! `canvas-refresh` repaints) leaves the drawing to the program that owns
//! the pixels. Forge's canvas is the drawing itself: a [`Scene`] is plain
//! data (JSON on the wire), so a plugin in any language builds one, every
//! frontend can draw it, and Forge can answer questions about it that
//! pixels can't: which shape is under the pointer ([`Scene::hit`]), how big
//! is everything ([`Scene::bounds`]), what does it look like as a vector
//! file ([`Scene::to_svg`]).
//!
//! * [`Scene`] and [`Op`] — the model: rectangles, ellipses, lines, SVG-style
//!   paths, wrapped text, pictures, and groups with a transform, clip and
//!   opacity. Fills are colours or linear gradients; strokes can dash and
//!   end in arrowheads. Shapes carry an `id`, which is what hit testing and
//!   mouse events report.
//! * [`raster`] (feature `raster`, on by default) — a CPU renderer that
//!   produces RGBA pixels, ARGB32 or a PNG. A GPU
//!   backend can consume the same scene: nothing in the model depends on
//!   how it is drawn.

mod color;
mod geom;
mod hit;
mod model;
mod native;
mod palette;
mod svg;
mod text;
mod widgets;

#[cfg(feature = "raster")]
pub mod raster;

pub use color::Color;
pub use geom::{Matrix, PathCmd, Point, flatten, parse_path};
pub use model::*;
pub use native::display_list;
pub use palette::{Palette, luminance, mix};
pub use text::{measure, wrap, wrap_with};
pub use widgets::{Card, Item, LANE_WIDTH, Lane, TextStyle, Widget, layout as layout_widgets};
