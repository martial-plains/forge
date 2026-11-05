// SPDX-License-Identifier: MIT OR Apache-2.0
//! The display engine's reading of the `display` text property.
//!
//! A `display` value is a list of specifications ([`DisplaySpec`]). Some of
//! them change what a stretch of text *is* on screen: `string` shows other
//! text, `space` shows a stretch of blank, `min-width` pads, `space-width`
//! widens the spaces. Those are resolved here to plain text, so every
//! frontend (the terminal too) shows them and the cursor and wrapping work
//! on what is shown. Others change how text is *drawn*: `height` and
//! `raise` become a [`TextStyle`] a pixel-capable frontend applies to the
//! cells it paints.
//!
//! Widths are in character cells, as numbers are in canonical
//! character widths. A pixel width (`(N)`) is not supported yet.

use crate::buffer::{DisplaySpec, DisplayValue};

/// How a run of text is drawn, for frontends that can scale and shift it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    /// Font height as a percentage of the default (`(height 1.5)` is 150).
    pub height_pct: u16,
    /// How far the text is raised, as a percentage of its own height
    /// (negative lowers it): `(raise 0.3)` is 30.
    pub raise_pct: i16,
}

impl TextStyle {
    pub const PLAIN: TextStyle = TextStyle {
        height_pct: 100,
        raise_pct: 0,
    };

    pub fn is_plain(&self) -> bool {
        *self == Self::PLAIN
    }
}

/// How much bigger each `(+ 1)` step makes the font.
pub const STEP: f64 = 1.2;

fn number(v: &DisplayValue) -> Option<f64> {
    match v {
        DisplayValue::Int(i) => Some(*i as f64),
        DisplayValue::Real(r) => Some(*r),
        _ => None,
    }
}

/// A width in cells: a plain number is already in cells, a pixel width is
/// divided by the width of a cell (8 pixels when the frontend has not said).
fn cells(v: &DisplayValue, cell_px: f64) -> Option<f64> {
    match v {
        DisplayValue::Pixels(p) => Some(p / cell_px),
        other => number(other),
    }
}

fn first_number(spec: &DisplaySpec) -> Option<f64> {
    spec.args.first().and_then(number)
}

/// How a run's specifications are drawn: `height` and `raise`.
pub fn style_of(props: &[DisplaySpec]) -> TextStyle {
    let mut style = TextStyle::PLAIN;
    for spec in props {
        match spec.name.as_str() {
            "height" => {
                let factor = match spec.args.first() {
                    Some(DisplayValue::Step(n)) => Some(STEP.powi(*n as i32)),
                    other => other.and_then(number).filter(|f| *f > 0.0),
                };
                if let Some(f) = factor {
                    style.height_pct = (f * 100.0).round().clamp(10.0, 1000.0) as u16;
                }
            }
            "raise" => {
                if let Some(f) = first_number(spec) {
                    style.raise_pct = (f * 100.0).round().clamp(-1000.0, 1000.0) as i16;
                }
            }
            _ => {}
        }
    }
    style
}

/// Does any specification here change the text itself?
pub fn changes_text(props: &[DisplaySpec]) -> bool {
    props.iter().any(|s| {
        matches!(
            s.name.as_str(),
            "string" | "space" | "min-width" | "space-width"
        )
    })
}

/// Does any specification here change how the text is drawn?
pub fn changes_style(props: &[DisplaySpec]) -> bool {
    !style_of(props).is_plain()
}

/// What a run of text shows in place of itself: `own` is the run's text and
/// `col` the display column it starts at (for `:align-to`). None when the
/// specifications leave the text as it is.
pub fn replacement(props: &[DisplaySpec], own: &str, col: usize) -> Option<String> {
    let cell_px = crate::org_display::cell_size().map_or(8.0, |c| c.0);
    replacement_with(props, own, col, cell_px)
}

/// [`replacement`] for cells `cell_px` pixels wide.
pub fn replacement_with(
    props: &[DisplaySpec],
    own: &str,
    col: usize,
    cell_px: f64,
) -> Option<String> {
    // `string` and `space` stand in for the text.
    let mut shown: Option<String> = None;
    for spec in props {
        match spec.name.as_str() {
            "string" => {
                if let Some(DisplayValue::Str(s)) = spec.args.first() {
                    shown = Some(s.clone());
                }
            }
            "space" => {
                let cells = match (
                    spec.arg("align-to").and_then(|v| cells(v, cell_px)),
                    spec.arg("width").and_then(|v| cells(v, cell_px)),
                ) {
                    (Some(to), _) => (to - col as f64).max(0.0),
                    (None, Some(w)) => w.max(0.0),
                    (None, None) => 1.0,
                };
                shown = Some(" ".repeat(cells.round() as usize));
            }
            _ => {}
        }
    }
    // `space-width` stretches the spaces that are shown.
    for spec in props.iter().filter(|s| s.name == "space-width") {
        if let Some(f) = first_number(spec).filter(|f| *f >= 0.0) {
            let base = shown.take().unwrap_or_else(|| own.to_string());
            let each = f.round() as usize;
            shown = Some(
                base.chars()
                    .map(|c| {
                        if c == ' ' {
                            " ".repeat(each)
                        } else {
                            c.to_string()
                        }
                    })
                    .collect(),
            );
        }
    }
    // `min-width` pads what is shown at the end.
    for spec in props.iter().filter(|s| s.name == "min-width") {
        let want = spec
            .args
            .first()
            .and_then(|v| cells(v, cell_px))
            .unwrap_or(0.0)
            .max(0.0)
            .round() as usize;
        let base = shown.take().unwrap_or_else(|| own.to_string());
        let have = crate::width::line_width(&base);
        shown = Some(if have < want {
            format!("{base}{}", " ".repeat(want - have))
        } else {
            base
        });
    }
    shown
}

/// The percentage a buffer's text scale stands for: each step is [`STEP`]
/// times the last. 100 when `pixel_text` is off (a terminal cannot scale).
pub fn scale_pct(steps: i32, pixel_text: bool) -> u16 {
    if !pixel_text || steps == 0 {
        return 100;
    }
    ((STEP.powi(steps) * 100.0).round() as i64).clamp(25, 800) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, args: Vec<DisplayValue>) -> DisplaySpec {
        DisplaySpec {
            name: name.into(),
            args,
        }
    }
    fn sym(s: &str) -> DisplayValue {
        DisplayValue::Symbol(s.into())
    }

    #[test]
    fn a_string_stands_in_for_the_text() {
        let p = [spec("string", vec![DisplayValue::Str("→".into())])];
        assert_eq!(replacement(&p, "->", 0).as_deref(), Some("→"));
    }

    #[test]
    fn a_space_has_a_width_or_reaches_a_column() {
        let w = [spec("space", vec![sym("width"), DisplayValue::Int(4)])];
        assert_eq!(replacement(&w, "x", 0).as_deref(), Some("    "));
        let a = [spec("space", vec![sym("align-to"), DisplayValue::Int(10)])];
        assert_eq!(replacement(&a, "x", 3).as_deref(), Some("       "));
        assert_eq!(
            replacement(&a, "x", 12).as_deref(),
            Some(""),
            "already past"
        );
    }

    #[test]
    fn min_width_pads_short_text_only() {
        let p = [spec("min-width", vec![DisplayValue::Real(6.0)])];
        assert_eq!(replacement(&p, "foo", 0).as_deref(), Some("foo   "));
        assert_eq!(
            replacement(&p, "foobarbaz", 0).as_deref(),
            Some("foobarbaz")
        );
    }

    #[test]
    fn space_width_widens_spaces_only() {
        let p = [spec("space-width", vec![DisplayValue::Int(2)])];
        assert_eq!(replacement(&p, "a b", 0).as_deref(), Some("a  b"));
        let none = [spec("space-width", vec![DisplayValue::Int(0)])];
        assert_eq!(replacement(&none, "a b", 0).as_deref(), Some("ab"));
    }

    #[test]
    fn specs_combine_in_their_documented_order() {
        let p = [
            spec("string", vec![DisplayValue::Str("ab".into())]),
            spec("min-width", vec![DisplayValue::Int(4)]),
        ];
        assert_eq!(replacement(&p, "x", 0).as_deref(), Some("ab  "));
    }

    #[test]
    fn height_and_raise_make_a_style() {
        let p = [
            spec("height", vec![DisplayValue::Real(0.8)]),
            spec("raise", vec![DisplayValue::Real(0.3)]),
        ];
        assert_eq!(
            style_of(&p),
            TextStyle {
                height_pct: 80,
                raise_pct: 30
            }
        );
        assert!(!changes_text(&p));
        assert!(changes_style(&p));
        assert!(style_of(&[]).is_plain());
    }

    #[test]
    fn pixel_widths_and_font_steps() {
        let px = [spec(
            "space",
            vec![sym("width"), DisplayValue::Pixels(35.0)],
        )];
        assert_eq!(replacement_with(&px, "", 0, 10.0).as_deref(), Some("    "));
        let up = [spec("height", vec![DisplayValue::Step(2)])];
        assert_eq!(style_of(&up).height_pct, 144);
        let down = [spec("height", vec![DisplayValue::Step(-1)])];
        assert_eq!(style_of(&down).height_pct, 83);
    }

    #[test]
    fn other_specs_leave_text_alone() {
        let p = [spec("image", vec![])];
        assert!(replacement(&p, "x", 0).is_none());
        assert!(!changes_text(&p) && !changes_style(&p));
    }
}
