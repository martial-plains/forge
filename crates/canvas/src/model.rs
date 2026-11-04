// SPDX-License-Identifier: MIT OR Apache-2.0
//! The scene: plain data that serialises to JSON.

use serde::{Deserialize, Serialize};

use crate::geom::Matrix;

fn one() -> f32 {
    1.0
}
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

/// A whole drawing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    /// The colour behind everything; none leaves it transparent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default)]
    pub ops: Vec<Op>,
}

impl Scene {
    /// Does any colour in the scene name a palette role (`@card`)?
    pub fn has_roles(&self) -> bool {
        fn is_role(c: &str) -> bool {
            c.starts_with('@')
        }
        fn paint(p: &Option<Paint>) -> bool {
            match p {
                Some(Paint::Color(c)) => is_role(c),
                Some(Paint::Linear(g)) => g.stops.iter().any(|(_, c)| is_role(c)),
                None => false,
            }
        }
        fn stroke(s: &Option<Stroke>) -> bool {
            s.as_ref().is_some_and(|s| is_role(&s.color))
        }
        fn any(ops: &[Op]) -> bool {
            ops.iter().any(|op| match op {
                Op::Rect { fill, stroke: s, .. } | Op::Ellipse { fill, stroke: s, .. } | Op::Path { fill, stroke: s, .. } => paint(fill) || stroke(s),
                Op::Line { stroke: s, .. } => is_role(&s.color),
                Op::Text { color, .. } => is_role(color),
                Op::Image { .. } => false,
                Op::Group { ops, .. } => any(ops),
            })
        }
        self.background.as_deref().is_some_and(is_role) || any(&self.ops)
    }

    /// Replace each palette role (`@card`) by the colour `pal` gives it.
    pub fn resolve(&mut self, pal: &crate::Palette) {
        fn fix(c: &mut String, pal: &crate::Palette) {
            if let Some(v) = pal.lookup(c) {
                *c = v.to_string();
            }
        }
        fn paint(p: &mut Option<Paint>, pal: &crate::Palette) {
            match p {
                Some(Paint::Color(c)) => fix(c, pal),
                Some(Paint::Linear(g)) => g.stops.iter_mut().for_each(|(_, c)| fix(c, pal)),
                None => {}
            }
        }
        fn walk(ops: &mut [Op], pal: &crate::Palette) {
            for op in ops {
                match op {
                    Op::Rect { fill, stroke, .. } | Op::Ellipse { fill, stroke, .. } | Op::Path { fill, stroke, .. } => {
                        paint(fill, pal);
                        if let Some(s) = stroke {
                            fix(&mut s.color, pal);
                        }
                    }
                    Op::Line { stroke, .. } => fix(&mut stroke.color, pal),
                    Op::Text { color, .. } => fix(color, pal),
                    Op::Image { .. } => {}
                    Op::Group { ops, .. } => walk(ops, pal),
                }
            }
        }
        if let Some(b) = &mut self.background {
            fix(b, pal);
        }
        walk(&mut self.ops, pal);
    }

    /// This scene with its roles resolved against `pal` (a copy only when it
    /// has any).
    pub fn resolved(&self, pal: &crate::Palette) -> std::borrow::Cow<'_, Scene> {
        if !self.has_roles() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut s = self.clone();
        s.resolve(pal);
        std::borrow::Cow::Owned(s)
    }
}

/// How a shape is filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Paint {
    /// `#rrggbb`, `rgba(...)` or a name.
    Color(String),
    Linear(Gradient),
}

/// How a picture fills its box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFit {
    /// Scaled to the box, whatever its shape.
    #[default]
    Stretch,
    /// Scaled to fill the box, keeping its shape; what overflows is cut.
    Cover,
    /// Scaled to fit inside the box, keeping its shape.
    Contain,
}

impl ImageFit {
    pub fn is_stretch(&self) -> bool {
        *self == ImageFit::Stretch
    }

    /// Where a `iw` x `ih` picture lands in the box (`x`, `y`, `w`, `h`): its
    /// own rectangle (which may stick out of the box for `Cover`).
    pub fn place(self, iw: f32, ih: f32, (x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let (iw, ih) = (iw.max(1.0), ih.max(1.0));
        let s = match self {
            ImageFit::Stretch => return (x, y, w, h),
            ImageFit::Cover => (w / iw).max(h / ih),
            ImageFit::Contain => (w / iw).min(h / ih),
        };
        let (dw, dh) = (iw * s, ih * s);
        (x + (w - dw) / 2.0, y + (h - dh) / 2.0, dw, dh)
    }
}

impl From<&str> for Paint {
    fn from(c: &str) -> Self {
        Paint::Color(c.into())
    }
}

impl From<&String> for Paint {
    fn from(c: &String) -> Self {
        Paint::Color(c.as_str().into())
    }
}

/// A linear gradient from (`x1`, `y1`) to (`x2`, `y2`) through `stops`
/// (offset 0..1, colour).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub stops: Vec<(f32, String)>,
}

/// An outline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub width: f32,
    /// Dash lengths: on, off, on, off, …
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f32>,
    /// An arrowhead at the start / end of a line or path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub arrow_start: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub arrow_end: bool,
}

impl Stroke {
    pub fn new(color: &str, width: f32) -> Self {
        Self { color: color.into(), width, dash: Vec::new(), arrow_start: false, arrow_end: false }
    }

    pub fn dashed(mut self, on: f32, off: f32) -> Self {
        self.dash = vec![on, off];
        self
    }

    pub fn arrow(mut self) -> Self {
        self.arrow_end = true;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Move, turn and scale what a group holds. Applied in this order about
/// (`ox`, `oy`): scale, rotate, then translate by (`x`, `y`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub sx: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub sy: f32,
    /// Degrees, clockwise (y points down).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotate: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ox: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub oy: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, sx: 1.0, sy: 1.0, rotate: 0.0, ox: 0.0, oy: 0.0 }
    }
}

impl Transform {
    pub fn translate(x: f32, y: f32) -> Self {
        Self { x, y, ..Self::default() }
    }

    pub fn scale_about(s: f32, ox: f32, oy: f32) -> Self {
        Self { sx: s, sy: s, ox, oy, ..Self::default() }
    }

    pub fn matrix(&self) -> Matrix {
        Matrix::translate(self.x + self.ox, self.y + self.oy)
            .then(&Matrix::rotate(self.rotate))
            .then(&Matrix::scale(self.sx, self.sy))
            .then(&Matrix::translate(-self.ox, -self.oy))
    }
}

/// One drawing step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<Paint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<Paint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke: Stroke,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// SVG path data (`M L H V C S Q T Z`, absolute or relative).
    Path {
        d: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<Paint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Text with its top-left (or top-centre/right, per `align`) at
    /// (`x`, `y`), wrapped to `max_width` when given.
    Text {
        x: f32,
        y: f32,
        text: String,
        #[serde(default = "default_size")]
        size: f32,
        #[serde(default = "default_ink")]
        color: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        bold: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        italic: bool,
        #[serde(default)]
        align: Align,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_width: Option<f32>,
        /// Lines beyond this many are cut, the last ending in `…`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// A picture file scaled into (`x`, `y`, `w`, `h`): stretched to it, or
    /// (`fit`) covering it with the overflow cut, or fitted inside it.
    Image {
        path: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default, skip_serializing_if = "ImageFit::is_stretch")]
        fit: ImageFit,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Ops drawn together under a transform, a clip and an opacity.
    Group {
        ops: Vec<Op>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transform: Option<Transform>,
        /// Clip to (x, y, w, h) in the group's own coordinates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clip: Option<[f32; 4]>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

fn default_size() -> f32 {
    14.0
}
fn default_ink() -> String {
    "#222222".into()
}

impl Op {
    pub fn id(&self) -> Option<&str> {
        match self {
            Op::Rect { id, .. }
            | Op::Ellipse { id, .. }
            | Op::Line { id, .. }
            | Op::Path { id, .. }
            | Op::Text { id, .. }
            | Op::Image { id, .. }
            | Op::Group { id, .. } => id.as_deref(),
        }
    }

    /// The box `[x, y, w, h]` this shape covers in its parent's space, or
    /// None when it can't be told (then it must be drawn).
    pub fn bounds(&self) -> Option<[f32; 4]> {
        crate::hit::bounds(std::slice::from_ref(self), &crate::geom::Matrix::IDENTITY)
    }

    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Op {
        Op::Rect { x, y, w, h, radius: 0.0, fill: None, stroke: None, id: None }
    }

    pub fn fill(mut self, paint: impl Into<Paint>) -> Op {
        match &mut self {
            Op::Rect { fill, .. } | Op::Ellipse { fill, .. } | Op::Path { fill, .. } => {
                *fill = Some(paint.into())
            }
            _ => {}
        }
        self
    }

    pub fn stroked(mut self, s: Stroke) -> Op {
        match &mut self {
            Op::Rect { stroke, .. } | Op::Ellipse { stroke, .. } | Op::Path { stroke, .. } => {
                *stroke = Some(s)
            }
            Op::Line { stroke, .. } => *stroke = s,
            _ => {}
        }
        self
    }

    pub fn with_id(mut self, new: &str) -> Op {
        match &mut self {
            Op::Rect { id, .. }
            | Op::Ellipse { id, .. }
            | Op::Line { id, .. }
            | Op::Path { id, .. }
            | Op::Text { id, .. }
            | Op::Image { id, .. }
            | Op::Group { id, .. } => *id = Some(new.into()),
        }
        self
    }

    pub fn rounded(mut self, r: f32) -> Op {
        if let Op::Rect { radius, .. } = &mut self {
            *radius = r;
        }
        self
    }

    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Op {
        Op::Ellipse { cx, cy, rx, ry, fill: None, stroke: None, id: None }
    }

    pub fn line(x1: f32, y1: f32, x2: f32, y2: f32, stroke: Stroke) -> Op {
        Op::Line { x1, y1, x2, y2, stroke, id: None }
    }

    pub fn path(d: &str) -> Op {
        Op::Path { d: d.into(), fill: None, stroke: None, id: None }
    }

    pub fn text(x: f32, y: f32, text: &str, size: f32, color: &str) -> Op {
        Op::Text {
            x,
            y,
            text: text.into(),
            size,
            color: color.into(),
            bold: false,
            italic: false,
            align: Align::Left,
            max_width: None,
            max_lines: None,
            id: None,
        }
    }

    pub fn wrapped(mut self, width: f32) -> Op {
        if let Op::Text { max_width, .. } = &mut self {
            *max_width = Some(width);
        }
        self
    }

    pub fn group(ops: Vec<Op>) -> Op {
        Op::Group { ops, transform: None, clip: None, opacity: 1.0, id: None }
    }

    pub fn transformed(mut self, t: Transform) -> Op {
        if let Op::Group { transform, .. } = &mut self {
            *transform = Some(t);
        }
        self
    }

    pub fn faded(mut self, o: f32) -> Op {
        if let Op::Group { opacity, .. } = &mut self {
            *opacity = o.clamp(0.0, 1.0);
        }
        self
    }

    pub fn image(path: &str, x: f32, y: f32, w: f32, h: f32) -> Op {
        Op::Image { path: path.into(), x, y, w, h, fit: ImageFit::Stretch, opacity: 1.0, id: None }
    }

    /// An image that fills its box without distortion (the overflow is cut).
    pub fn image_cover(path: &str, x: f32, y: f32, w: f32, h: f32) -> Op {
        Op::Image { path: path.into(), x, y, w, h, fit: ImageFit::Cover, opacity: 1.0, id: None }
    }

    pub fn fill_linear(mut self, x1: f32, y1: f32, x2: f32, y2: f32, stops: &[(f32, &str)]) -> Op {
        let g = Paint::Linear(Gradient {
            x1,
            y1,
            x2,
            y2,
            stops: stops.iter().map(|(o, c)| (*o, (*c).to_string())).collect(),
        });
        match &mut self {
            Op::Rect { fill, .. } | Op::Ellipse { fill, .. } | Op::Path { fill, .. } => *fill = Some(g),
            _ => {}
        }
        self
    }

    pub fn clipped(mut self, x: f32, y: f32, w: f32, h: f32) -> Op {
        if let Op::Group { clip, .. } = &mut self {
            *clip = Some([x, y, w, h]);
        }
        self
    }
}

impl Scene {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height, background: None, ops: Vec::new() }
    }

    pub fn background(mut self, c: &str) -> Self {
        self.background = Some(c.into());
        self
    }

    pub fn push(&mut self, op: Op) -> &mut Self {
        self.ops.push(op);
        self
    }

    pub fn from_json(s: &str) -> Result<Scene, String> {
        serde_json::from_str(s).map_err(|e| format!("bad scene: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// The topmost shape with an id under (`x`, `y`), if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<String> {
        crate::hit::hit(self, x, y)
    }

    /// The bounding box of everything drawn, as (x, y, w, h).
    pub fn bounds(&self) -> Option<[f32; 4]> {
        crate::hit::bounds(&self.ops, &Matrix::IDENTITY)
    }

    /// The scene as an SVG document.
    pub fn to_svg(&self) -> String {
        crate::svg::to_svg(self)
    }

    /// The bounds of the shape (or group) with this id.
    pub fn bounds_of(&self, id: &str) -> Option<[f32; 4]> {
        crate::hit::bounds_of(&self.ops, &Matrix::IDENTITY, id)
    }
}
