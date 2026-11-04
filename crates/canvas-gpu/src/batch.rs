// SPDX-License-Identifier: MIT OR Apache-2.0
//! Turning a scene into triangles: the part of the GPU renderer that needs
//! no GPU, so it is tested on its own.
//!
//! Shapes are tessellated by lyon in their own coordinates and then mapped
//! through the group transforms (strokes stay exact under any affine map).
//! Text becomes textured quads from the glyph atlas, pictures become one
//! textured quad each. Clips are scissor rectangles, so a clip under a
//! rotated group is ignored. A group's opacity multiplies into what it
//! holds, which differs from a true layer only where children overlap.

use bytemuck::{Pod, Zeroable};
use forge_canvas::{Align, Color, Matrix, Op, Paint, PathCmd, Point, Scene, Stroke, flatten, parse_path, raster, wrap_with};
use lyon_tessellation::geom::Angle;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::math::{Vector, point};
use lyon_tessellation::path::iterator::PathIterator;
use lyon_tessellation::path::{Path, Winding};
use lyon_tessellation::{FillOptions, FillRule, FillTessellator, LineCap, LineJoin, StrokeOptions, StrokeTessellator};

use crate::atlas::{Atlas, Lookup};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    /// Target pixels, y down.
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// Premultiplied.
    pub color: [f32; 4],
    /// 0 flat colour, 1 glyph coverage, 2 picture.
    pub mode: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Flat colours and glyphs (the atlas is always bound).
    Plain,
    Picture(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub source: Source,
    /// x0, y0, x1, y1 in target pixels.
    pub scissor: [u32; 4],
    pub first: u32,
    pub count: u32,
}

/// What the builder knows about pictures.
pub trait Pictures {
    /// The pixel size of a picture, if it can be drawn.
    fn size(&mut self, path: &str) -> Option<(u32, u32)>;
}

#[derive(Default)]
pub struct Batch {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<Draw>,
    pub background: Option<[f32; 4]>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy)]
struct State {
    m: Matrix,
    scissor: [i32; 4],
    opacity: f32,
}

/// Why a batch could not be finished.
#[derive(Debug, PartialEq)]
pub enum Stop {
    /// The glyph atlas filled up; empty it and build again.
    AtlasFull,
}

const MAX_TRIANGLES: usize = 400_000;

pub fn size_of(scene: &Scene, scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    ((scene.width * scale).ceil().clamp(1.0, 16384.0) as u32, (scene.height * scale).ceil().clamp(1.0, 16384.0) as u32)
}

/// Build the triangles for `scene` at `scale` device pixels per unit.
pub fn build(scene: &Scene, scale: f32, atlas: &mut Atlas, pictures: &mut dyn Pictures) -> Result<Batch, Stop> {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (w, h) = size_of(scene, scale);
    let mut b = Batch { width: w, height: h, ..Batch::default() };
    b.background = scene.background.as_deref().and_then(Color::parse).map(|c| premultiplied(c, 1.0));
    let st = State { m: Matrix::scale(scale, scale), scissor: [0, 0, w as i32, h as i32], opacity: 1.0 };
    let mut cx = Cx { b: &mut b, atlas, pictures, fill: FillTessellator::new(), stroke: StrokeTessellator::new() };
    for op in &scene.ops {
        cx.op(op, &st)?;
    }
    Ok(b)
}

fn premultiplied(c: Color, opacity: f32) -> [f32; 4] {
    let a = (c.a * opacity).clamp(0.0, 1.0);
    [f32::from(c.r) / 255.0 * a, f32::from(c.g) / 255.0 * a, f32::from(c.b) / 255.0 * a, a]
}

struct Cx<'a> {
    b: &'a mut Batch,
    atlas: &'a mut Atlas,
    pictures: &'a mut dyn Pictures,
    fill: FillTessellator,
    stroke: StrokeTessellator,
}

/// Local-space triangles.
#[derive(Default)]
struct Mesh {
    pts: Vec<Point>,
    idx: Vec<u32>,
}

impl Mesh {
    fn from_buffers(v: VertexBuffers<lyon_tessellation::math::Point, u32>) -> Mesh {
        Mesh { pts: v.vertices.iter().map(|p| Point::new(p.x, p.y)).collect(), idx: v.indices }
    }
}

fn pt(p: Point) -> lyon_tessellation::math::Point {
    point(p.x, p.y)
}

fn lyon_path(cmds: &[PathCmd]) -> Path {
    let mut b = Path::builder();
    let mut open = false;
    for c in cmds {
        match *c {
            PathCmd::Move(p) => {
                if open {
                    b.end(false);
                }
                b.begin(pt(p));
                open = true;
            }
            PathCmd::Line(p) if open => {
                b.line_to(pt(p));
            }
            PathCmd::Quad(a, p) if open => {
                b.quadratic_bezier_to(pt(a), pt(p));
            }
            PathCmd::Cubic(a, c2, p) if open => {
                b.cubic_bezier_to(pt(a), pt(c2), pt(p));
            }
            PathCmd::Close if open => {
                b.end(true);
                open = false;
            }
            _ => {}
        }
    }
    if open {
        b.end(false);
    }
    b.build()
}

fn rect_cmds(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<PathCmd> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let p = Point::new;
    if r <= 0.0 {
        return vec![PathCmd::Move(p(x, y)), PathCmd::Line(p(x + w, y)), PathCmd::Line(p(x + w, y + h)), PathCmd::Line(p(x, y + h)), PathCmd::Close];
    }
    const K: f32 = 0.552_284_75;
    let k = r * K;
    vec![
        PathCmd::Move(p(x + r, y)),
        PathCmd::Line(p(x + w - r, y)),
        PathCmd::Cubic(p(x + w - r + k, y), p(x + w, y + r - k), p(x + w, y + r)),
        PathCmd::Line(p(x + w, y + h - r)),
        PathCmd::Cubic(p(x + w, y + h - r + k), p(x + w - r + k, y + h), p(x + w - r, y + h)),
        PathCmd::Line(p(x + r, y + h)),
        PathCmd::Cubic(p(x + r - k, y + h), p(x, y + h - r + k), p(x, y + h - r)),
        PathCmd::Line(p(x, y + r)),
        PathCmd::Cubic(p(x, y + r - k), p(x + r - k, y), p(x + r, y)),
        PathCmd::Close,
    ]
}

fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    let mut b = Path::builder();
    b.add_ellipse(point(cx, cy), Vector::new(rx.abs(), ry.abs()), Angle::radians(0.0), Winding::Positive);
    b.build()
}

/// Split the triangles until no edge is longer than `max` (so per-vertex
/// gradient colours follow the gradient).
fn refine(mesh: Mesh, max: f32) -> Mesh {
    let mut out = Mesh::default();
    let mut work: Vec<[Point; 3]> = mesh.idx.chunks_exact(3).map(|t| [mesh.pts[t[0] as usize], mesh.pts[t[1] as usize], mesh.pts[t[2] as usize]]).collect();
    let len = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let mid = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    while let Some(t) = work.pop() {
        if out.idx.len() / 3 + work.len() > MAX_TRIANGLES {
            break;
        }
        let edges = [len(t[0], t[1]), len(t[1], t[2]), len(t[2], t[0])];
        let (longest, e) = edges.iter().copied().enumerate().fold((0, 0.0), |acc, (i, l)| if l > acc.1 { (i, l) } else { acc });
        if e <= max {
            let base = out.pts.len() as u32;
            out.pts.extend_from_slice(&t);
            out.idx.extend([base, base + 1, base + 2]);
            continue;
        }
        // Cut the longest edge in two.
        let (a, b, c) = (t[longest], t[(longest + 1) % 3], t[(longest + 2) % 3]);
        let m = mid(a, b);
        work.push([a, m, c]);
        work.push([m, b, c]);
    }
    out
}

struct Ramp {
    p1: Point,
    d: Point,
    d2: f32,
    stops: Vec<(f32, Color)>,
}

impl Ramp {
    fn new(g: &forge_canvas::Gradient) -> Option<Ramp> {
        let mut stops: Vec<(f32, Color)> = g.stops.iter().filter_map(|(o, c)| Some((o.clamp(0.0, 1.0), Color::parse(c)?))).collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if stops.is_empty() {
            return None;
        }
        let d = Point::new(g.x2 - g.x1, g.y2 - g.y1);
        Some(Ramp { p1: Point::new(g.x1, g.y1), d, d2: (d.x * d.x + d.y * d.y).max(1e-9), stops })
    }

    fn at(&self, p: Point, opacity: f32) -> [f32; 4] {
        let t = (((p.x - self.p1.x) * self.d.x + (p.y - self.p1.y) * self.d.y) / self.d2).clamp(0.0, 1.0);
        let i = self.stops.iter().position(|s| s.0 >= t).unwrap_or(self.stops.len() - 1);
        let (b, a) = (self.stops[i], self.stops[i.saturating_sub(1)]);
        let f = if b.0 > a.0 { ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 1.0 };
        let mix = |x: f32, y: f32| x + (y - x) * f;
        let (ca, cb) = (premultiplied(a.1, opacity), premultiplied(b.1, opacity));
        [mix(ca[0], cb[0]), mix(ca[1], cb[1]), mix(ca[2], cb[2]), mix(ca[3], cb[3])]
    }
}

impl Cx<'_> {
    fn op(&mut self, op: &Op, st: &State) -> Result<(), Stop> {
        match op {
            Op::Rect { x, y, w, h, radius, fill, stroke, .. } => {
                let cmds = rect_cmds(*x, *y, *w, *h, *radius);
                self.shape(&lyon_path(&cmds), &cmds, fill.as_ref(), stroke.as_ref(), st);
            }
            Op::Ellipse { cx, cy, rx, ry, fill, stroke, .. } => {
                let path = ellipse_path(*cx, *cy, *rx, *ry);
                self.shape(&path, &[], fill.as_ref(), stroke.as_ref(), st);
            }
            Op::Line { x1, y1, x2, y2, stroke, .. } => {
                let cmds = vec![PathCmd::Move(Point::new(*x1, *y1)), PathCmd::Line(Point::new(*x2, *y2))];
                self.shape(&lyon_path(&cmds), &cmds, None, Some(stroke), st);
            }
            Op::Path { d, fill, stroke, .. } => {
                if let Some(cmds) = parse_path(d) {
                    self.shape(&lyon_path(&cmds), &cmds, fill.as_ref(), stroke.as_ref(), st);
                }
            }
            Op::Text { .. } => self.text(op, st)?,
            Op::Image { path, x, y, w, h, fit, opacity, .. } => self.image(path, (*x, *y, *w, *h), *fit, st.opacity * opacity, st),
            Op::Group { ops, transform, clip, opacity, .. } => {
                let mut inner = *st;
                if let Some(t) = transform {
                    inner.m = st.m.then(&t.matrix());
                }
                inner.opacity *= opacity;
                if let Some([cx, cy, cw, ch]) = clip {
                    // Only an axis-aligned map can be a scissor.
                    if inner.m.b.abs() < 1e-4 && inner.m.c.abs() < 1e-4 {
                        let a = inner.m.apply(Point::new(*cx, *cy));
                        let b = inner.m.apply(Point::new(cx + cw, cy + ch));
                        let (x0, x1) = (a.x.min(b.x).floor() as i32, a.x.max(b.x).ceil() as i32);
                        let (y0, y1) = (a.y.min(b.y).floor() as i32, a.y.max(b.y).ceil() as i32);
                        inner.scissor = [inner.scissor[0].max(x0), inner.scissor[1].max(y0), inner.scissor[2].min(x1), inner.scissor[3].min(y1)];
                    }
                }
                if inner.scissor[2] <= inner.scissor[0] || inner.scissor[3] <= inner.scissor[1] {
                    return Ok(());
                }
                for o in ops {
                    self.op(o, &inner)?;
                }
            }
        }
        Ok(())
    }

    fn tolerance(&self, st: &State) -> f32 {
        (0.15 / st.m.mean_scale().max(1e-3)).clamp(0.001, 2.0)
    }

    fn shape(&mut self, path: &Path, cmds: &[PathCmd], fill: Option<&Paint>, stroke: Option<&Stroke>, st: &State) {
        let tol = self.tolerance(st);
        if let Some(paint) = fill {
            let mut buf: VertexBuffers<lyon_tessellation::math::Point, u32> = VertexBuffers::new();
            let ok = self.fill.tessellate_path(path, &FillOptions::tolerance(tol).with_fill_rule(FillRule::NonZero), &mut BuffersBuilder::new(&mut buf, Positions)).is_ok();
            if ok {
                let mut mesh = Mesh::from_buffers(buf);
                match paint {
                    Paint::Color(c) => {
                        if let Some(c) = Color::parse(c) {
                            self.emit(&mesh, st, |_| premultiplied(c, st.opacity));
                        }
                    }
                    Paint::Linear(g) => {
                        if let Some(ramp) = Ramp::new(g) {
                            mesh = refine(mesh, 6.0 / st.m.mean_scale().max(1e-3));
                            self.emit(&mesh, st, |p| ramp.at(p, st.opacity));
                        }
                    }
                }
            }
        }
        if let Some(s) = stroke {
            let Some(c) = Color::parse(&s.color) else { return };
            let col = premultiplied(c, st.opacity);
            let width = s.width.max(0.0);
            let mut dash: Vec<f32> = s.dash.iter().copied().filter(|d| *d > 0.0).collect();
            if dash.len() % 2 == 1 {
                dash.extend(dash.clone());
            }
            let opts = StrokeOptions::tolerance(tol).with_line_width(width).with_line_cap(LineCap::Butt).with_line_join(LineJoin::Miter);
            let polys_for_arrows;
            if dash.len() >= 2 {
                let polys = dashes(&flatten_path(path, tol), &dash);
                for poly in &polys {
                    if poly.len() < 2 {
                        continue;
                    }
                    let mut b = Path::builder();
                    b.begin(pt(poly[0]));
                    for p in &poly[1..] {
                        b.line_to(pt(*p));
                    }
                    b.end(false);
                    self.stroke_path(&b.build(), &opts, col, st);
                }
                polys_for_arrows = flatten_path(path, tol);
            } else {
                self.stroke_path(path, &opts, col, st);
                polys_for_arrows = if cmds.is_empty() { flatten_path(path, tol) } else { flatten(cmds).into_iter().map(|p| p.0).collect() };
            }
            if s.arrow_start || s.arrow_end {
                self.arrows(&polys_for_arrows, s, col, st);
            }
        }
    }

    fn stroke_path(&mut self, path: &Path, opts: &StrokeOptions, col: [f32; 4], st: &State) {
        let mut buf: VertexBuffers<lyon_tessellation::math::Point, u32> = VertexBuffers::new();
        if self.stroke.tessellate_path(path, opts, &mut BuffersBuilder::new(&mut buf, Positions)).is_ok() {
            self.emit(&Mesh::from_buffers(buf), st, |_| col);
        }
    }

    fn arrows(&mut self, polys: &[Vec<Point>], s: &Stroke, col: [f32; 4], st: &State) {
        let Some(pts) = polys.iter().find(|p| p.len() >= 2) else { return };
        let mut heads = Vec::new();
        if s.arrow_end {
            heads.push((pts[pts.len() - 1], pts[pts.len() - 2]));
        }
        if s.arrow_start {
            heads.push((pts[0], pts[1]));
        }
        for (tip, from) in heads {
            let (dx, dy) = (tip.x - from.x, tip.y - from.y);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-3 {
                continue;
            }
            let (ux, uy) = (dx / len, dy / len);
            let size = (s.width * 4.0).max(9.0);
            let (bx, by) = (tip.x - ux * size, tip.y - uy * size);
            let half = size * 0.45;
            let mesh = Mesh { pts: vec![tip, Point::new(bx - uy * half, by + ux * half), Point::new(bx + uy * half, by - ux * half)], idx: vec![0, 1, 2] };
            self.emit(&mesh, st, |_| col);
        }
    }

    /// Add `mesh` (local coordinates) under `st`, coloured by `color(local point)`.
    fn emit(&mut self, mesh: &Mesh, st: &State, color: impl Fn(Point) -> [f32; 4]) {
        let verts: Vec<Vertex> = mesh
            .pts
            .iter()
            .map(|&p| {
                let d = st.m.apply(p);
                Vertex { pos: [d.x, d.y], uv: [0.0, 0.0], color: color(p), mode: 0.0 }
            })
            .collect();
        self.push(Source::Plain, st, &verts, &mesh.idx, mesh.pts.first().is_some_and(|_| st.m.a * st.m.d - st.m.b * st.m.c < 0.0));
    }

    fn push(&mut self, source: Source, st: &State, verts: &[Vertex], idx: &[u32], flipped: bool) {
        if idx.is_empty() {
            return;
        }
        let base = self.b.vertices.len() as u32;
        self.b.vertices.extend_from_slice(verts);
        let first = self.b.indices.len() as u32;
        if flipped {
            // A mirroring transform reverses winding; the pipeline culls nothing, but keep it consistent.
            for t in idx.chunks_exact(3) {
                self.b.indices.extend([base + t[0], base + t[2], base + t[1]]);
            }
        } else {
            self.b.indices.extend(idx.iter().map(|i| base + i));
        }
        let scissor = [st.scissor[0].max(0) as u32, st.scissor[1].max(0) as u32, st.scissor[2].max(0) as u32, st.scissor[3].max(0) as u32];
        let count = idx.len() as u32;
        match self.b.draws.last_mut() {
            Some(d) if d.source == source && d.scissor == scissor && d.first + d.count == first => d.count += count,
            _ => self.b.draws.push(Draw { source, scissor, first, count }),
        }
    }

    fn image(&mut self, path: &str, (x, y, w, h): (f32, f32, f32, f32), fit: forge_canvas::ImageFit, opacity: f32, st: &State) {
        let Some((iw, ih)) = self.pictures.size(path) else {
            // A picture that can't be read shows as an outlined box, as on the CPU.
            let cmds = rect_cmds(x, y, w, h, 0.0);
            let s = Stroke::new("#969696", 1.0);
            self.shape(&lyon_path(&cmds), &cmds, None, Some(&s), st);
            return;
        };
        let a = opacity.clamp(0.0, 1.0);
        // The part of the picture that shows: all of it, or (covering) the
        // middle that fills the box.
        let (px0, py0, pw, ph) = fit.place(iw as f32, ih as f32, (x, y, w, h));
        let (u0, u1) = if fit == forge_canvas::ImageFit::Cover { ((x - px0) / pw, (x + w - px0) / pw) } else { (0.0, 1.0) };
        let (v0, v1) = if fit == forge_canvas::ImageFit::Cover { ((y - py0) / ph, (y + h - py0) / ph) } else { (0.0, 1.0) };
        let (x, y, w, h) = if fit == forge_canvas::ImageFit::Contain { (px0, py0, pw, ph) } else { (x, y, w, h) };
        let corners = [(x, y, u0, v0), (x + w, y, u1, v0), (x + w, y + h, u1, v1), (x, y + h, u0, v1)];
        let verts: Vec<Vertex> = corners
            .iter()
            .map(|&(px, py, u, v)| {
                let d = st.m.apply(Point::new(px, py));
                Vertex { pos: [d.x, d.y], uv: [u, v], color: [a, a, a, a], mode: 2.0 }
            })
            .collect();
        self.push(Source::Picture(path.to_string()), st, &verts, &[0, 1, 2, 0, 2, 3], false);
    }

    fn text(&mut self, op: &Op, st: &State) -> Result<(), Stop> {
        let Op::Text { x, y, text, size, color, bold, align, max_width, max_lines, .. } = op else { return Ok(()) };
        let Some(col) = Color::parse(color) else { return Ok(()) };
        if !raster::has_fonts() {
            return Ok(());
        }
        let ms = st.m.mean_scale().max(1e-3);
        let px = (size * ms).max(1.0);
        let pxq = px.round().clamp(1.0, 512.0) as u16;
        let pxf = f32::from(pxq);
        let measure = |s: &str| raster::text_width(s, pxf, *bold) / ms;
        let mut lines = match max_width {
            Some(w) => wrap_with(text, *w, &measure),
            None => text.split('\n').map(str::to_string).collect(),
        };
        if let Some(n) = max_lines {
            let n = (*n as usize).max(1);
            if lines.len() > n {
                lines.truncate(n);
                if let Some(last) = lines.last_mut() {
                    last.push('…');
                }
            }
        }
        let line_h = size * 1.3;
        let ascent = raster::ascent(pxf, *bold);
        let baseline = ascent + pxf * 0.1;
        let axis_aligned = st.m.b.abs() < 1e-4 && st.m.c.abs() < 1e-4;
        let color = premultiplied(col, st.opacity);
        for (i, line) in lines.iter().enumerate() {
            let width = raster::text_width(line, pxf, *bold);
            let left = match align {
                Align::Left => *x,
                Align::Center => x - width / ms / 2.0,
                Align::Right => x - width / ms,
            };
            let origin = st.m.apply(Point::new(left, y + i as f32 * line_h));
            let (mut ox, mut oy) = (origin.x, origin.y);
            if axis_aligned {
                ox = ox.round();
                oy = oy.round();
            }
            let mut pen = 0.0f32;
            let mut verts = Vec::new();
            let mut idx = Vec::new();
            for c in line.chars() {
                let slot = match self.atlas.glyph(c, pxq, *bold) {
                    Lookup::Full => return Err(Stop::AtlasFull),
                    Lookup::Blank => {
                        pen += pxf * 0.55;
                        continue;
                    }
                    Lookup::Slot(s) => s,
                };
                if slot.w > 0 {
                    // Glyph box relative to the line's origin, in device pixels.
                    let gx = (pen + slot.xmin as f32).round();
                    let gy = (baseline - slot.h as f32 - slot.ymin as f32).round();
                    let (x0, y0, x1, y1) = (gx, gy, gx + slot.w as f32, gy + slot.h as f32);
                    let s = crate::atlas::SIZE as f32;
                    let (u0, v0, u1, v1) = (slot.x as f32 / s, slot.y as f32 / s, (slot.x + slot.w) as f32 / s, (slot.y + slot.h) as f32 / s);
                    let place = |dx: f32, dy: f32| -> [f32; 2] {
                        if axis_aligned {
                            [ox + dx, oy + dy]
                        } else {
                            // Turn the glyph with the group: undo the scale, then use the map.
                            let p = st.m.apply(Point::new(left + dx / ms, y + i as f32 * line_h + dy / ms));
                            [p.x, p.y]
                        }
                    };
                    let base = verts.len() as u32;
                    for (dx, dy, u, v) in [(x0, y0, u0, v0), (x1, y0, u1, v0), (x1, y1, u1, v1), (x0, y1, u0, v1)] {
                        verts.push(Vertex { pos: place(dx, dy), uv: [u, v], color, mode: 1.0 });
                    }
                    idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                }
                pen += slot.advance;
            }
            self.push(Source::Plain, st, &verts, &idx, false);
        }
        Ok(())
    }
}

fn flatten_path(path: &Path, tol: f32) -> Vec<Vec<Point>> {
    use lyon_tessellation::path::PathEvent;
    let mut out: Vec<Vec<Point>> = Vec::new();
    for e in path.iter().flattened(tol) {
        match e {
            PathEvent::Begin { at } => out.push(vec![Point::new(at.x, at.y)]),
            PathEvent::Line { to, .. } => {
                if let Some(l) = out.last_mut() {
                    l.push(Point::new(to.x, to.y));
                }
            }
            PathEvent::End { first, close, .. } => {
                if close && let Some(l) = out.last_mut() {
                    l.push(Point::new(first.x, first.y));
                }
            }
            _ => {}
        }
    }
    out
}

/// Cut polylines into dashes: on, off, on, off, …
pub fn dashes(polys: &[Vec<Point>], pattern: &[f32]) -> Vec<Vec<Point>> {
    let mut out = Vec::new();
    for poly in polys {
        let (mut i, mut left, mut on) = (0usize, pattern[0], true);
        let mut cur: Vec<Point> = Vec::new();
        if let Some(&p) = poly.first() {
            cur.push(p);
        }
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
            let mut done = 0.0;
            while len - done > left {
                done += left;
                let t = done / len;
                let p = Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                if on {
                    cur.push(p);
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur = vec![p];
                }
                on = !on;
                i = (i + 1) % pattern.len();
                left = pattern[i];
            }
            left -= len - done;
            if on {
                cur.push(b);
            }
        }
        if on && cur.len() >= 2 {
            out.push(cur);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_canvas::Transform;

    struct NoPictures;
    impl Pictures for NoPictures {
        fn size(&mut self, _: &str) -> Option<(u32, u32)> {
            None
        }
    }

    fn make(scene: &Scene) -> Batch {
        build(scene, 1.0, &mut Atlas::default(), &mut NoPictures).unwrap()
    }

    #[test]
    fn a_rect_is_two_triangles_at_the_right_place() {
        let mut s = Scene::new(40.0, 20.0).background("#ffffff");
        s.push(Op::rect(5.0, 5.0, 10.0, 10.0).fill("#ff0000"));
        let b = make(&s);
        assert_eq!((b.width, b.height), (40, 20));
        assert_eq!(b.indices.len(), 6);
        let xs: Vec<f32> = b.vertices.iter().map(|v| v.pos[0]).collect();
        assert!(xs.iter().all(|x| (5.0..=15.0).contains(x)), "{xs:?}");
        assert_eq!(b.vertices[0].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(b.background, Some([1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn scale_and_group_transforms_apply_and_clips_become_scissors() {
        let mut s = Scene::new(40.0, 40.0);
        s.push(Op::group(vec![Op::rect(0.0, 0.0, 40.0, 40.0).fill("#000000")]).transformed(Transform::translate(10.0, 10.0)).clipped(0.0, 0.0, 10.0, 10.0));
        let b = build(&s, 2.0, &mut Atlas::default(), &mut NoPictures).unwrap();
        assert_eq!((b.width, b.height), (80, 80));
        assert_eq!(b.draws.len(), 1);
        assert_eq!(b.draws[0].scissor, [20, 20, 40, 40]);
        let min = b.vertices.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!((min - 20.0).abs() < 1e-3, "{min}");
    }

    #[test]
    fn opacity_multiplies_in_and_premultiplies() {
        let mut s = Scene::new(10.0, 10.0);
        s.push(Op::group(vec![Op::rect(0.0, 0.0, 10.0, 10.0).fill("#ffffff")]).faded(0.5));
        let b = make(&s);
        assert_eq!(b.vertices[0].color, [0.5, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn gradients_vary_across_a_rect() {
        let mut s = Scene::new(100.0, 10.0);
        s.push(Op::rect(0.0, 0.0, 100.0, 10.0).fill_linear(0.0, 0.0, 100.0, 0.0, &[(0.0, "#000000"), (1.0, "#ffffff")]));
        let b = make(&s);
        let left = b.vertices.iter().filter(|v| v.pos[0] < 1.0).map(|v| v.color[0]).fold(1.0, f32::min);
        let right = b.vertices.iter().filter(|v| v.pos[0] > 99.0).map(|v| v.color[0]).fold(0.0, f32::max);
        assert!(left < 0.05 && right > 0.95, "{left} {right}");
        assert!(b.vertices.len() > 4, "refined so the middle follows the ramp");
    }

    #[test]
    fn dashes_split_a_line() {
        let d = dashes(&[vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)]], &[5.0, 5.0]);
        assert_eq!(d.len(), 4);
        assert_eq!(d[0], vec![Point::new(0.0, 0.0), Point::new(5.0, 0.0)]);
        assert_eq!(d[1][0], Point::new(10.0, 0.0));
        let mut s = Scene::new(40.0, 10.0);
        s.push(Op::line(0.0, 5.0, 40.0, 5.0, Stroke::new("#000", 4.0).dashed(5.0, 5.0)));
        assert!(make(&s).indices.len() >= 4 * 6);
    }

    #[test]
    fn arrows_add_a_head() {
        let mut plain = Scene::new(60.0, 20.0);
        plain.push(Op::line(0.0, 10.0, 50.0, 10.0, Stroke::new("#000", 4.0)));
        let mut arrow = Scene::new(60.0, 20.0);
        arrow.push(Op::line(0.0, 10.0, 50.0, 10.0, Stroke::new("#000", 4.0).arrow()));
        assert_eq!(make(&arrow).indices.len(), make(&plain).indices.len() + 3);
    }

    #[test]
    fn text_makes_glyph_quads_and_a_missing_picture_an_outline() {
        let mut s = Scene::new(100.0, 40.0);
        s.push(Op::text(2.0, 2.0, "Hi", 20.0, "#000000"));
        s.push(Op::image("/no/such.png", 0.0, 0.0, 10.0, 10.0));
        let b = make(&s);
        if raster::has_fonts() {
            assert!(b.vertices.iter().any(|v| v.mode == 1.0), "glyph quads");
        }
        assert!(b.vertices.iter().any(|v| v.mode == 0.0), "the outline");
        assert!(b.draws.iter().all(|d| d.source == Source::Plain));
    }

    #[test]
    fn ellipses_and_paths_tessellate() {
        let mut s = Scene::new(40.0, 40.0);
        s.push(Op::ellipse(20.0, 20.0, 10.0, 10.0).fill("#00f"));
        s.push(Op::path("M0 0 L10 0 L5 10 Z").fill("#f00"));
        let b = make(&s);
        assert!(b.indices.len() > 6 + 3);
    }
}
