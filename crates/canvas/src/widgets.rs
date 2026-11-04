// SPDX-License-Identifier: MIT OR Apache-2.0
//! Widgets: a plugin describes an interface (cards, lists, boards, buttons)
//! as a tree, and Forge lays it out and draws it, in the terminal, in a
//! window and on a phone alike. The tree is plain data (JSON):
//!
//! ```json
//! {"w":"column","gap":12,"children":[
//!   {"w":"text","style":"title","text":"Reading list"},
//!   {"w":"board","lanes":[
//!     {"id":"todo","title":"To read","cards":[
//!       {"w":"card","id":"n1","title":"Dune","subtitle":"Frank Herbert",
//!        "badges":["sci-fi"],"image":"/path/cover.jpg"}]}]}]}
//! ```
//!
//! Widgets with an `id` (cards, list items, buttons, lanes) are what pointer
//! events report, exactly like shapes in a scene. [`layout`] turns the tree
//! into a [`Scene`], which every frontend already draws natively (see
//! docs/native-drawing.md); colours are palette roles, so a theme change
//! needs no new layout.
//!
//! | `w` | fields |
//! | --- | --- |
//! | `column`, `row` | `children`, `gap` (a row shares its width equally) |
//! | `text` | `text`, `style`: `title`, `heading`, `body`, `caption`, `code` |
//! | `card` | `id`, `title`, `subtitle`, `body`, `badges`, `image`, `children` |
//! | `list` | `items`: `{id, title, subtitle, trailing}` |
//! | `board` | `lanes`: `{id, title, cards}`; lanes are 260 wide and scroll sideways |
//! | `button` | `id`, `label` |
//! | `image` | `path`, `height` |
//! | `separator` | |

use crate::model::{Align, ImageFit, Op, Scene, Stroke};
use crate::text::wrap;
use serde::{Deserialize, Serialize};

fn default_gap() -> f32 {
    8.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "w", rename_all = "snake_case")]
pub enum Widget {
    Column {
        children: Vec<Widget>,
        #[serde(default = "default_gap")]
        gap: f32,
    },
    Row {
        children: Vec<Widget>,
        #[serde(default = "default_gap")]
        gap: f32,
    },
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
    },
    Card(Card),
    List {
        items: Vec<Item>,
    },
    Board {
        lanes: Vec<Lane>,
    },
    Button {
        id: String,
        label: String,
    },
    Image {
        path: String,
        #[serde(default = "default_image_height")]
        height: f32,
    },
    Separator,
}

fn default_image_height() -> f32 {
    160.0
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    Title,
    Heading,
    #[default]
    Body,
    Caption,
    Code,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Card {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub badges: Vec<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub children: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub trailing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub cards: Vec<Card>,
}

/// The width of a board's lane.
pub const LANE_WIDTH: f32 = 260.0;
const PAD: f32 = 12.0;
const LINE: f32 = 1.3;

impl Widget {
    pub fn parse(json: &str) -> Result<Widget, String> {
        serde_json::from_str(json).map_err(|e| format!("bad widget tree: {e}"))
    }
}

/// What a laid-out widget needs: its ops and how tall it is. `min_w` is the
/// width it wants at least (a board is wider than the page).
struct Placed {
    ops: Vec<Op>,
    height: f32,
    width: f32,
}

fn text_op(x: f32, y: f32, text: &str, size: f32, color: &str, bold: bool, width: f32, lines: Option<u32>) -> (Op, f32) {
    let mut op = Op::text(x, y, text, size, color).wrapped(width);
    if let Op::Text { bold: b, max_lines, .. } = &mut op {
        *b = bold;
        *max_lines = lines;
    }
    let n = wrap(text, size, bold, width).len().min(lines.map_or(usize::MAX, |l| l as usize)).max(1);
    (op, n as f32 * size * LINE)
}

fn card(c: &Card, x: f32, y: f32, width: f32) -> Placed {
    let inner = width - 2.0 * PAD;
    let mut ops = Vec::new();
    let mut cy = y + PAD;
    if let Some(img) = &c.image {
        let h = (inner * 0.5).clamp(60.0, 150.0);
        ops.push(Op::Image { path: img.clone(), x: x + PAD, y: cy, w: inner, h, fit: ImageFit::Cover, opacity: 1.0, id: None });
        cy += h + 8.0;
    }
    let (t, h) = text_op(x + PAD, cy, &c.title, 14.0, "@strong", true, inner, Some(2));
    ops.push(t);
    cy += h + 2.0;
    if let Some(s) = &c.subtitle {
        let (t, h) = text_op(x + PAD, cy, s, 11.0, "@dim", false, inner, Some(1));
        ops.push(t);
        cy += h + 4.0;
    }
    if let Some(b) = &c.body {
        let (t, h) = text_op(x + PAD, cy, b, 12.0, "@soft", false, inner, Some(6));
        ops.push(t);
        cy += h + 4.0;
    }
    if !c.badges.is_empty() {
        let mut bx = x + PAD;
        let mut by = cy;
        for badge in &c.badges {
            let w = crate::measure(badge, 10.0, false).0 + 12.0;
            if bx + w > x + width - PAD && bx > x + PAD {
                bx = x + PAD;
                by += 20.0;
            }
            ops.push(Op::rect(bx, by, w, 16.0).rounded(8.0).fill("@chip"));
            ops.push(Op::text(bx + 6.0, by + 1.5, badge, 10.0, "@tag"));
            bx += w + 6.0;
        }
        cy = by + 16.0 + 4.0;
    }
    for child in &c.children {
        let p = place(child, x + PAD, cy, inner);
        cy += p.height + 6.0;
        ops.extend(p.ops);
    }
    let height = (cy - y + PAD - 4.0).max(2.0 * PAD);
    let mut frame = Op::rect(x, y, width, height).rounded(8.0).fill("@card").stroked(Stroke::new("@line", 1.0));
    if let Some(id) = &c.id {
        frame = frame.with_id(id);
    }
    let mut all = vec![frame];
    all.extend(ops);
    Placed { ops: all, height, width }
}

fn place(w: &Widget, x: f32, y: f32, width: f32) -> Placed {
    match w {
        Widget::Column { children, gap } => {
            let mut ops = Vec::new();
            let mut cy = y;
            let mut widest = width;
            for (i, c) in children.iter().enumerate() {
                if i > 0 {
                    cy += gap;
                }
                let p = place(c, x, cy, width);
                cy += p.height;
                widest = widest.max(p.width);
                ops.extend(p.ops);
            }
            Placed { ops, height: cy - y, width: widest }
        }
        Widget::Row { children, gap } => {
            let n = children.len().max(1) as f32;
            let each = (width - gap * (n - 1.0)) / n;
            let mut ops = Vec::new();
            let mut height: f32 = 0.0;
            for (i, c) in children.iter().enumerate() {
                let p = place(c, x + i as f32 * (each + gap), y, each);
                height = height.max(p.height);
                ops.extend(p.ops);
            }
            Placed { ops, height, width }
        }
        Widget::Text { text, style } => {
            let (size, color, bold) = match style {
                TextStyle::Title => (20.0, "@strong", true),
                TextStyle::Heading => (15.0, "@strong", true),
                TextStyle::Body => (13.0, "@fg", false),
                TextStyle::Caption => (11.0, "@dim", false),
                TextStyle::Code => (12.0, "@soft", false),
            };
            let (op, h) = text_op(x, y, text, size, color, bold, width, None);
            Placed { ops: vec![op], height: h, width }
        }
        Widget::Card(c) => card(c, x, y, width),
        Widget::List { items } => {
            let mut ops = Vec::new();
            let mut cy = y;
            for it in items {
                let h = if it.subtitle.is_some() { 48.0 } else { 34.0 };
                let mut row = Op::rect(x, cy, width, h).fill("@card");
                if let Some(id) = &it.id {
                    row = row.with_id(id);
                }
                ops.push(row);
                ops.push(Op::text(x + PAD, cy + if it.subtitle.is_some() { 6.0 } else { 9.0 }, &it.title, 13.0, "@fg"));
                if let Some(s) = &it.subtitle {
                    ops.push(Op::text(x + PAD, cy + 25.0, s, 11.0, "@dim"));
                }
                if let Some(t) = &it.trailing {
                    let mut op = Op::text(x + width - PAD, cy + (h - 11.0 * LINE) / 2.0, t, 11.0, "@dim");
                    if let Op::Text { align, .. } = &mut op {
                        *align = Align::Right;
                    }
                    ops.push(op);
                }
                ops.push(Op::line(x, cy + h, x + width, cy + h, Stroke::new("@line", 1.0)));
                cy += h + 1.0;
            }
            Placed { ops, height: cy - y, width }
        }
        Widget::Board { lanes } => {
            let mut ops = Vec::new();
            let mut tallest: f32 = 0.0;
            for (i, lane) in lanes.iter().enumerate() {
                let lx = x + i as f32 * (LANE_WIDTH + 12.0);
                let mut cy = y + PAD;
                let mut inner = Vec::new();
                let (t, h) = text_op(lx + PAD, cy, &lane.title, 13.0, "@strong", true, LANE_WIDTH - 2.0 * PAD, Some(1));
                inner.push(t);
                cy += h + 8.0;
                for c in &lane.cards {
                    let p = card(c, lx + 8.0, cy, LANE_WIDTH - 16.0);
                    cy += p.height + 8.0;
                    inner.extend(p.ops);
                }
                let height = cy - y + 4.0;
                tallest = tallest.max(height);
                let mut frame = Op::rect(lx, y, LANE_WIDTH, height).rounded(10.0).fill("@sunken");
                if let Some(id) = &lane.id {
                    frame = frame.with_id(id);
                }
                ops.push(frame);
                ops.extend(inner);
            }
            let total = lanes.len() as f32 * (LANE_WIDTH + 12.0) - 12.0;
            // Every lane is as tall as the tallest.
            for op in ops.iter_mut() {
                if let Op::Rect { y: ry, h, w, fill: Some(crate::Paint::Color(c)), .. } = op
                    && *ry == y
                    && *w == LANE_WIDTH
                    && c == "@sunken"
                {
                    *h = tallest;
                }
            }
            Placed { ops, height: tallest, width: total.max(width) }
        }
        Widget::Button { id, label } => {
            let w = (crate::measure(label, 13.0, true).0 + 28.0).min(width);
            let ops = vec![
                Op::rect(x, y, w, 32.0).rounded(8.0).fill("@accent0").with_id(id),
                {
                    let mut t = Op::text(x + w / 2.0, y + 8.0, label, 13.0, "@bg");
                    if let Op::Text { align, bold, .. } = &mut t {
                        *align = Align::Center;
                        *bold = true;
                    }
                    t
                },
            ];
            Placed { ops, height: 32.0, width: w }
        }
        Widget::Image { path, height } => Placed {
            ops: vec![Op::Image { path: path.clone(), x, y, w: width, h: *height, fit: ImageFit::Contain, opacity: 1.0, id: None }],
            height: *height,
            width,
        },
        Widget::Separator => Placed { ops: vec![Op::line(x, y + 4.0, x + width, y + 4.0, Stroke::new("@line", 1.0))], height: 9.0, width },
    }
}

/// Lay `tree` out in a page `width` units wide. The scene is as tall as the
/// content (a board may also be wider than the page).
pub fn layout(tree: &Widget, width: f32) -> Scene {
    let margin = 16.0;
    let p = place(tree, margin, margin, width - 2.0 * margin);
    let mut scene = Scene::new(width.max(p.width + 2.0 * margin), p.height + 2.0 * margin);
    scene.background = Some("@bg".into());
    scene.ops = p.ops;
    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = r#"{"w":"column","gap":10,"children":[
        {"w":"text","style":"title","text":"Reading list"},
        {"w":"row","children":[
            {"w":"card","id":"a","title":"A long title that has to wrap onto a second line for sure","badges":["x","y"]},
            {"w":"card","id":"b","title":"B","body":"short"}]},
        {"w":"board","lanes":[
            {"id":"todo","title":"To read","cards":[{"w":"card","id":"n1","title":"Dune","subtitle":"Herbert"},{"w":"card","id":"n2","title":"Emma"}]},
            {"id":"done","title":"Done","cards":[]}]},
        {"w":"list","items":[{"id":"i1","title":"One","trailing":"2"},{"id":"i2","title":"Two","subtitle":"sub"}]},
        {"w":"button","id":"go","label":"Add"},
        {"w":"separator"}]}"#;

    #[test]
    fn a_tree_lays_out_with_named_shapes_and_only_colour_roles() {
        let tree = Widget::parse(TREE).unwrap();
        let scene = layout(&tree, 600.0);
        assert!(scene.has_roles());
        // Every id the plugin gave is a shape pointer events can report.
        for id in ["a", "b", "todo", "done", "n1", "n2", "i1", "i2", "go"] {
            assert!(scene.bounds_of(id).is_some(), "{id} is in the scene");
        }
        // Two lanes make the page wider than a narrow window.
        assert!(layout(&tree, 300.0).width >= 2.0 * LANE_WIDTH);
    }

    #[test]
    fn cards_in_a_lane_never_overlap_and_lanes_are_equally_tall() {
        let tree = Widget::parse(TREE).unwrap();
        let scene = layout(&tree, 600.0);
        let n1 = scene.bounds_of("n1").unwrap();
        let n2 = scene.bounds_of("n2").unwrap();
        assert!(n2[1] >= n1[1] + n1[3], "n2 starts below n1: {n1:?} {n2:?}");
        let (todo, done) = (scene.bounds_of("todo").unwrap(), scene.bounds_of("done").unwrap());
        assert_eq!(todo[3], done[3]);
    }

    #[test]
    fn a_bad_tree_says_what_is_wrong() {
        let e = Widget::parse(r#"{"w":"card"}"#).unwrap_err();
        assert!(e.contains("bad widget tree"), "{e}");
        assert!(Widget::parse(r#"{"w":"nope"}"#).is_err());
    }
}
