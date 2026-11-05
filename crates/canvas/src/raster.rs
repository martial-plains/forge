// SPDX-License-Identifier: MIT OR Apache-2.0
//! The CPU renderer: a [`Scene`] to pixels, with tiny-skia for shapes and
//! fontdue for text. Nothing here is needed to build or hit-test a scene.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use tiny_skia as sk;

use crate::color::Color;
use crate::geom::{PathCmd, Point, flatten, parse_path};
use crate::model::{Align, Gradient, Op, Paint, Scene, Stroke};
use crate::text::wrap_with;

/// A drawn scene.
pub struct Raster {
    pixmap: sk::Pixmap,
}

impl Raster {
    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// Straight-alpha RGBA bytes, row by row.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixmap.pixels().len() * 4);
        for p in self.pixmap.pixels() {
            let c = p.demultiply();
            out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        out
    }

    /// One `0xAARRGGBB` word per pixel (straight alpha): the pixel layout
    /// canvas takes.
    pub fn argb32(&self) -> Vec<u32> {
        self.pixmap
            .pixels()
            .iter()
            .map(|p| {
                let c = p.demultiply();
                u32::from(c.alpha()) << 24
                    | u32::from(c.red()) << 16
                    | u32::from(c.green()) << 8
                    | u32::from(c.blue())
            })
            .collect()
    }

    /// The picture as PNG. Compression is the fast kind: a canvas is drawn
    /// again on every scroll and zoom, and the file goes straight to the
    /// screen, so encoding time matters far more than its size.
    pub fn png(&self) -> Result<Vec<u8>, String> {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let data = self.pixmap.data();
        // The pixmap is premultiplied; a picture with a background has no
        // transparent pixels, so it is already straight.
        let opaque = data.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
        let straight: Vec<u8>;
        let bytes: &[u8] = if opaque {
            data
        } else {
            straight = self
                .pixmap
                .pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect();
            &straight
        };
        let mut out = Vec::with_capacity(bytes.len() / 8);
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub)
            .write_image(
                bytes,
                self.pixmap.width(),
                self.pixmap.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// The pixel at (x, y) as straight RGBA, for tests and probes.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width() || y >= self.height() {
            return None;
        }
        let c = self.pixmap.pixel(x, y)?.demultiply();
        Some([c.red(), c.green(), c.blue(), c.alpha()])
    }
}

/// Draw `scene` at `scale` device pixels per unit (at least 1 px square).
pub fn render(scene: &Scene, scale: f32) -> Option<Raster> {
    // A scene still naming palette roles (nobody resolved them for a theme)
    // is drawn in the dark palette.
    let scene = &*scene.resolved(&crate::Palette::dark());
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let w = (scene.width * scale).ceil().clamp(1.0, 16384.0) as u32;
    let h = (scene.height * scale).ceil().clamp(1.0, 16384.0) as u32;
    let mut pixmap = sk::Pixmap::new(w, h)?;
    if let Some(c) = scene.background.as_deref().and_then(Color::parse) {
        pixmap.fill(sk_color(c, 1.0));
    }
    draw_ops(
        &mut pixmap,
        &scene.ops,
        sk::Transform::from_scale(scale, scale),
        None,
        1.0,
    );
    Some(Raster { pixmap })
}

fn sk_color(c: Color, opacity: f32) -> sk::Color {
    sk::Color::from_rgba8(
        c.r,
        c.g,
        c.b,
        ((c.a * opacity).clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn draw_ops(
    pm: &mut sk::Pixmap,
    ops: &[Op],
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    for op in ops {
        draw_op(pm, op, ts, mask, opacity);
    }
}

fn paint_for(p: &Paint, opacity: f32) -> Option<sk::Paint<'static>> {
    let mut paint = sk::Paint {
        anti_alias: true,
        ..sk::Paint::default()
    };
    match p {
        Paint::Color(c) => paint.set_color(sk_color(Color::parse(c)?, opacity)),
        Paint::Linear(g) => paint.shader = gradient(g, opacity)?,
    }
    Some(paint)
}

fn gradient(g: &Gradient, opacity: f32) -> Option<sk::Shader<'static>> {
    let colors: Vec<(f32, sk::Color)> = g
        .stops
        .iter()
        .filter_map(|(o, c)| Some((o.clamp(0.0, 1.0), sk_color(Color::parse(c)?, opacity))))
        .collect();
    if colors.len() < 2 {
        return colors.first().map(|(_, c)| sk::Shader::SolidColor(*c));
    }
    let stops: Vec<sk::GradientStop> = colors
        .into_iter()
        .map(|(o, c)| sk::GradientStop::new(o, c))
        .collect();
    sk::LinearGradient::new(
        sk::Point::from_xy(g.x1, g.y1),
        sk::Point::from_xy(g.x2, g.y2),
        stops,
        sk::SpreadMode::Pad,
        sk::Transform::identity(),
    )
}

fn stroke_for(s: &Stroke) -> sk::Stroke {
    let mut st = sk::Stroke {
        width: s.width.max(0.0),
        line_cap: sk::LineCap::Butt,
        ..sk::Stroke::default()
    };
    let mut dash: Vec<f32> = s.dash.iter().copied().filter(|d| *d > 0.0).collect();
    if dash.len() % 2 == 1 {
        dash.extend(dash.clone());
    }
    if dash.len() >= 2 {
        st.dash = sk::StrokeDash::new(dash, 0.0);
    }
    st
}

fn build_path(cmds: &[PathCmd]) -> Option<sk::Path> {
    let mut pb = sk::PathBuilder::new();
    for c in cmds {
        match *c {
            PathCmd::Move(p) => pb.move_to(p.x, p.y),
            PathCmd::Line(p) => pb.line_to(p.x, p.y),
            PathCmd::Quad(a, p) => pb.quad_to(a.x, a.y, p.x, p.y),
            PathCmd::Cubic(a, b, p) => pb.cubic_to(a.x, a.y, b.x, b.y, p.x, p.y),
            PathCmd::Close => pb.close(),
        }
    }
    pb.finish()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<sk::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return sk::PathBuilder::from_rect(sk::Rect::from_xywh(x, y, w, h)?).into();
    }
    const K: f32 = 0.552_284_8;
    let k = r * K;
    let mut pb = sk::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_stroke(
    pm: &mut sk::Pixmap,
    path: &sk::Path,
    fill: &Option<Paint>,
    stroke: &Option<Stroke>,
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    if let Some(p) = fill.as_ref().and_then(|f| paint_for(f, opacity)) {
        pm.fill_path(path, &p, sk::FillRule::Winding, ts, mask);
    }
    if let Some(s) = stroke
        && let Some(c) = Color::parse(&s.color)
    {
        let mut p = sk::Paint {
            anti_alias: true,
            ..sk::Paint::default()
        };
        p.set_color(sk_color(c, opacity));
        pm.stroke_path(path, &p, &stroke_for(s), ts, mask);
    }
}

fn arrowhead(
    pm: &mut sk::Pixmap,
    tip: Point,
    from: Point,
    s: &Stroke,
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    let Some(c) = Color::parse(&s.color) else {
        return;
    };
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let size = (s.width * 4.0).max(9.0);
    let (bx, by) = (tip.x - ux * size, tip.y - uy * size);
    let half = size * 0.45;
    let mut pb = sk::PathBuilder::new();
    pb.move_to(tip.x, tip.y);
    pb.line_to(bx - uy * half, by + ux * half);
    pb.line_to(bx + uy * half, by - ux * half);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut p = sk::Paint {
            anti_alias: true,
            ..sk::Paint::default()
        };
        p.set_color(sk_color(c, opacity));
        pm.fill_path(&path, &p, sk::FillRule::Winding, ts, mask);
    }
}

fn arrows(
    pm: &mut sk::Pixmap,
    polys: &[(Vec<Point>, bool)],
    s: &Stroke,
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    let Some((pts, _)) = polys.iter().find(|(p, _)| p.len() >= 2) else {
        return;
    };
    if s.arrow_end {
        arrowhead(
            pm,
            pts[pts.len() - 1],
            pts[pts.len() - 2],
            s,
            ts,
            mask,
            opacity,
        );
    }
    if s.arrow_start {
        arrowhead(pm, pts[0], pts[1], s, ts, mask, opacity);
    }
}

fn draw_op(pm: &mut sk::Pixmap, op: &Op, ts: sk::Transform, mask: Option<&sk::Mask>, opacity: f32) {
    match op {
        Op::Rect {
            x,
            y,
            w,
            h,
            radius,
            fill,
            stroke,
            ..
        } => {
            if let Some(path) = rounded_rect(*x, *y, *w, *h, *radius) {
                fill_stroke(pm, &path, fill, stroke, ts, mask, opacity);
            }
        }
        Op::Ellipse {
            cx,
            cy,
            rx,
            ry,
            fill,
            stroke,
            ..
        } => {
            if let Some(path) = sk::Rect::from_xywh(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry)
                .and_then(sk::PathBuilder::from_oval)
            {
                fill_stroke(pm, &path, fill, stroke, ts, mask, opacity);
            }
        }
        Op::Line {
            x1,
            y1,
            x2,
            y2,
            stroke,
            ..
        } => {
            let mut pb = sk::PathBuilder::new();
            pb.move_to(*x1, *y1);
            pb.line_to(*x2, *y2);
            if let Some(path) = pb.finish() {
                fill_stroke(pm, &path, &None, &Some(stroke.clone()), ts, mask, opacity);
                let pts = vec![(vec![Point::new(*x1, *y1), Point::new(*x2, *y2)], false)];
                arrows(pm, &pts, stroke, ts, mask, opacity);
            }
        }
        Op::Path {
            d, fill, stroke, ..
        } => {
            let Some(cmds) = parse_path(d) else { return };
            if let Some(path) = build_path(&cmds) {
                fill_stroke(pm, &path, fill, stroke, ts, mask, opacity);
                if let Some(s) = stroke {
                    arrows(pm, &flatten(&cmds), s, ts, mask, opacity);
                }
            }
        }
        Op::Text { .. } => draw_text(pm, op, ts, mask, opacity),
        Op::Image {
            path,
            x,
            y,
            w,
            h,
            fit,
            opacity: o,
            ..
        } => {
            draw_image(pm, path, (*x, *y, *w, *h), *fit, ts, mask, opacity * o);
        }
        Op::Group {
            ops,
            transform,
            clip,
            opacity: o,
            ..
        } => {
            let gts = match transform {
                Some(t) => {
                    let m = t.matrix();
                    ts.pre_concat(sk::Transform::from_row(m.a, m.b, m.c, m.d, m.e, m.f))
                }
                None => ts,
            };
            let clipped = clip.and_then(|[cx, cy, cw, ch]| {
                let rect = sk::Rect::from_xywh(cx, cy, cw, ch)?;
                let path = sk::PathBuilder::from_rect(rect);
                let mut m = match mask {
                    Some(m) => m.clone(),
                    None => {
                        let mut full = sk::Mask::new(pm.width(), pm.height())?;
                        full.fill_path(
                            &sk::PathBuilder::from_rect(sk::Rect::from_xywh(
                                0.0,
                                0.0,
                                pm.width() as f32,
                                pm.height() as f32,
                            )?),
                            sk::FillRule::Winding,
                            false,
                            sk::Transform::identity(),
                        );
                        full
                    }
                };
                m.intersect_path(&path, sk::FillRule::Winding, true, gts);
                Some(m)
            });
            let effective = if clip.is_some() {
                clipped.as_ref()
            } else {
                mask
            };
            if clip.is_some() && effective.is_none() {
                return;
            }
            if *o < 1.0 {
                if let Some(mut layer) = sk::Pixmap::new(pm.width(), pm.height()) {
                    draw_ops(&mut layer, ops, gts, effective, opacity);
                    let paint = sk::PixmapPaint {
                        opacity: o.clamp(0.0, 1.0),
                        ..sk::PixmapPaint::default()
                    };
                    pm.draw_pixmap(
                        0,
                        0,
                        layer.as_ref(),
                        &paint,
                        sk::Transform::identity(),
                        None,
                    );
                }
            } else {
                draw_ops(pm, ops, gts, effective, opacity);
            }
        }
    }
}

/// Decoded pictures, so a board of posters isn't decoded again on every
/// redraw. Big ones are shrunk on the way in (a card is a few hundred pixels
/// wide); the cache keeps the most recent few and notices a changed file.
static PICTURES: std::sync::Mutex<
    Vec<(
        String,
        Option<std::time::SystemTime>,
        std::sync::Arc<sk::Pixmap>,
    )>,
> = std::sync::Mutex::new(Vec::new());
const PICTURE_SIDE: u32 = 1024;
const PICTURE_KEEP: usize = 64;

fn picture(path: &str) -> Option<std::sync::Arc<sk::Pixmap>> {
    let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Ok(mut cache) = PICTURES.lock()
        && let Some(at) = cache.iter().position(|(p, t, _)| p == path && *t == stamp)
    {
        let hit = cache.remove(at);
        let pix = hit.2.clone();
        cache.push(hit);
        return Some(pix);
    }
    let mut img = image::open(path).ok()?;
    if img.width().max(img.height()) > PICTURE_SIDE {
        img = img.thumbnail(PICTURE_SIDE, PICTURE_SIDE);
    }
    let rgba = img.to_rgba8();
    let (iw, ih) = rgba.dimensions();
    let mut src = sk::Pixmap::new(iw, ih)?;
    for (dst, px) in src.pixels_mut().iter_mut().zip(rgba.pixels()) {
        *dst = sk::ColorU8::from_rgba(px[0], px[1], px[2], px[3]).premultiply();
    }
    let pix = std::sync::Arc::new(src);
    if let Ok(mut cache) = PICTURES.lock() {
        cache.retain(|(p, ..)| p != path);
        cache.push((path.to_string(), stamp, pix.clone()));
        if cache.len() > PICTURE_KEEP {
            cache.remove(0);
        }
    }
    Some(pix)
}

fn draw_image(
    pm: &mut sk::Pixmap,
    path: &str,
    (x, y, w, h): (f32, f32, f32, f32),
    fit: crate::ImageFit,
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    let Some(src) = picture(path) else {
        // A missing picture shows as an outlined box, so layout is visible.
        if let Some(rect) = sk::Rect::from_xywh(x, y, w, h) {
            let mut p = sk::Paint::default();
            p.set_color_rgba8(150, 150, 150, 255);
            pm.stroke_path(
                &sk::PathBuilder::from_rect(rect),
                &p,
                &sk::Stroke::default(),
                ts,
                mask,
            );
        }
        return;
    };
    let (iw, ih) = (src.width() as f32, src.height() as f32);
    let paint = sk::PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        quality: sk::FilterQuality::Bilinear,
        ..sk::PixmapPaint::default()
    };
    let (dx, dy, dw, dh) = fit.place(iw, ih, (x, y, w, h));
    if fit != crate::ImageFit::Cover {
        let sc = sk::Transform::from_scale(dw / iw, dh / ih).post_translate(dx, dy);
        pm.draw_pixmap(0, 0, (*src).as_ref(), &paint, ts.pre_concat(sc), mask);
        return;
    }
    // Covering: draw the whole picture into a box-sized layer (the overflow
    // falls off its edge), then lay the layer on the page.
    let (sx, sy) = (ts.sx.abs().max(0.01), ts.sy.abs().max(0.01));
    let (tw, th) = (
        (w * sx).ceil().max(1.0) as u32,
        (h * sy).ceil().max(1.0) as u32,
    );
    let Some(mut layer) = sk::Pixmap::new(tw, th) else {
        return;
    };
    let (lx, ly, lw, lh) = fit.place(iw, ih, (0.0, 0.0, tw as f32, th as f32));
    layer.draw_pixmap(
        0,
        0,
        (*src).as_ref(),
        &sk::PixmapPaint {
            quality: sk::FilterQuality::Bilinear,
            ..sk::PixmapPaint::default()
        },
        sk::Transform::from_scale(lw / iw, lh / ih).post_translate(lx, ly),
        None,
    );
    let back = sk::Transform::from_scale(w / tw as f32, h / th as f32).post_translate(x, y);
    pm.draw_pixmap(0, 0, layer.as_ref(), &paint, ts.pre_concat(back), mask);
}

// --- text -----------------------------------------------------------------

struct Fonts {
    regular: Vec<Font>,
    bold: Vec<Font>,
}

const REGULAR: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNS.ttf",
    "/Library/Fonts/Arial.ttf",
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/system/fonts/Roboto-Regular.ttf",
    // Wide coverage last, for characters the first font lacks.
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\seguisym.ttf",
];

const BOLD: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial Bold.ttf",
    "C:\\Windows\\Fonts\\segoeuib.ttf",
    "C:\\Windows\\Fonts\\arialbd.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/system/fonts/Roboto-Bold.ttf",
];

fn load(paths: &[&str]) -> Vec<Font> {
    paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
        .collect()
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let regular = load(REGULAR);
        let mut bold = load(BOLD);
        // Bold falls back to the regular face where there's no bold one.
        bold.extend(load(REGULAR));
        Fonts { regular, bold }
    })
}

fn pick(set: &[Font], c: char) -> Option<&Font> {
    set.iter()
        .find(|f| f.lookup_glyph_index(c) != 0)
        .or_else(|| set.first())
}

/// True when some font is installed to draw text with.
pub fn has_fonts() -> bool {
    !fonts().regular.is_empty()
}

fn line_width(line: &str, px: f32, bold: bool) -> f32 {
    let f = fonts();
    let set = if bold { &f.bold } else { &f.regular };
    line.chars()
        .map(|c| pick(set, c).map_or(px * 0.55, |f| f.metrics(c, px).advance_width))
        .sum()
}

/// One rasterised glyph, for renderers that draw text themselves.
pub struct Glyph {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub advance: f32,
    /// Coverage, `width * height` bytes, top row first.
    pub bitmap: Vec<u8>,
}

/// `c` in the installed fonts at `px` pixels tall (None when no font is installed).
pub fn glyph(c: char, px: f32, bold: bool) -> Option<Glyph> {
    let f = fonts();
    let set = if bold { &f.bold } else { &f.regular };
    let font = pick(set, c)?;
    let (m, bitmap) = font.rasterize(c, px);
    Some(Glyph {
        xmin: m.xmin,
        ymin: m.ymin,
        width: m.width,
        height: m.height,
        advance: m.advance_width,
        bitmap,
    })
}

/// The distance from the top of a line to its baseline at `px`.
pub fn ascent(px: f32, bold: bool) -> f32 {
    let f = fonts();
    let set = if bold { &f.bold } else { &f.regular };
    set.first()
        .and_then(|f| f.horizontal_line_metrics(px))
        .map_or(px * 0.8, |m| m.ascent)
}

/// The width of `line` at `px`.
pub fn text_width(line: &str, px: f32, bold: bool) -> f32 {
    line_width(line, px, bold)
}

fn draw_text(
    pm: &mut sk::Pixmap,
    op: &Op,
    ts: sk::Transform,
    mask: Option<&sk::Mask>,
    opacity: f32,
) {
    let Op::Text {
        x,
        y,
        text,
        size,
        color,
        bold,
        align,
        max_width,
        max_lines,
        ..
    } = op
    else {
        return;
    };
    let Some(col) = Color::parse(color) else {
        return;
    };
    let f = fonts();
    let set = if *bold { &f.bold } else { &f.regular };
    if set.is_empty() {
        return;
    }
    let scale = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt().max(1e-3);
    let px = (size * scale).max(1.0);
    let measure = |s: &str| line_width(s, px, *bold) / scale;
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
    let ascent = set[0]
        .horizontal_line_metrics(px)
        .map_or(px * 0.8, |m| m.ascent);
    let pad = (px * 0.25).ceil();
    for (i, line) in lines.iter().enumerate() {
        let width = line_width(line, px, *bold);
        let left = match align {
            Align::Left => *x,
            Align::Center => x - width / scale / 2.0,
            Align::Right => x - width / scale,
        };
        let mut origin = sk::Point::from_xy(left, y + i as f32 * line_h);
        ts.map_point(&mut origin);
        let (w, h) = (
            (width + 2.0 * pad).ceil() as u32,
            (px * 1.5 + 2.0 * pad).ceil() as u32,
        );
        let Some(mut layer) = sk::Pixmap::new(w.max(1), h.max(1)) else {
            continue;
        };
        let mut pen = pad;
        let baseline = pad + ascent + px * 0.1;
        for c in line.chars() {
            let Some(font) = pick(set, c) else { continue };
            let (m, bitmap) = font.rasterize(c, px);
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
            for row in 0..m.height {
                for colx in 0..m.width {
                    let cov = bitmap[row * m.width + colx];
                    if cov == 0 {
                        continue;
                    }
                    let (tx, ty) = (gx + colx as i32, gy + row as i32);
                    if tx < 0 || ty < 0 || tx >= w as i32 || ty >= h as i32 {
                        continue;
                    }
                    let a = f32::from(cov) / 255.0 * col.a;
                    let idx = (ty as u32 * w + tx as u32) as usize;
                    let px_out = &mut layer.pixels_mut()[idx];
                    let prev = f32::from(px_out.alpha()) / 255.0;
                    let na = (a + prev * (1.0 - a)).min(1.0);
                    let blend = |c: u8, p: u8| {
                        (f32::from(c) * a + f32::from(p) * (1.0 - a)).clamp(0.0, 255.0) as u8
                    };
                    let (r, g, b) = (
                        blend(col.r, px_out.red()),
                        blend(col.g, px_out.green()),
                        blend(col.b, px_out.blue()),
                    );
                    // Premultiplied storage: colour channels scale with alpha.
                    let na8 = (na * 255.0).round() as u8;
                    let pm8 = |v: u8| ((f32::from(v) * na).round() as u8).min(na8);
                    if let Some(c) =
                        sk::PremultipliedColorU8::from_rgba(pm8(r), pm8(g), pm8(b), na8)
                    {
                        *px_out = c;
                    }
                }
            }
            pen += m.advance_width;
        }
        let paint = sk::PixmapPaint {
            opacity: opacity.clamp(0.0, 1.0),
            ..sk::PixmapPaint::default()
        };
        pm.draw_pixmap(
            (origin.x - pad).round() as i32,
            (origin.y - pad).round() as i32,
            layer.as_ref(),
            &paint,
            sk::Transform::identity(),
            mask,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Op, Scene, Stroke, Transform};

    #[test]
    fn shapes_land_where_they_should() {
        let mut s = Scene::new(40.0, 20.0).background("#ffffff");
        s.push(Op::rect(0.0, 0.0, 10.0, 10.0).fill("#ff0000"));
        s.push(Op::ellipse(30.0, 10.0, 8.0, 8.0).fill("#0000ff"));
        let r = render(&s, 1.0).unwrap();
        assert_eq!((r.width(), r.height()), (40, 20));
        assert_eq!(r.pixel(5, 5), Some([255, 0, 0, 255]));
        assert_eq!(r.pixel(30, 10), Some([0, 0, 255, 255]));
        assert_eq!(r.pixel(20, 18), Some([255, 255, 255, 255]));
        assert_eq!(r.pixel(99, 0), None);
    }

    #[test]
    fn scale_background_and_formats() {
        let mut s = Scene::new(10.0, 10.0);
        s.push(Op::rect(0.0, 0.0, 5.0, 5.0).fill("rgba(0,255,0,0.5)"));
        let r = render(&s, 2.0).unwrap();
        assert_eq!((r.width(), r.height()), (20, 20));
        let p = r.pixel(2, 2).unwrap();
        assert!(
            p[1] > 250 && (120..136).contains(&p[3]),
            "half-transparent green: {p:?}"
        );
        assert_eq!(
            r.pixel(15, 15),
            Some([0, 0, 0, 0]),
            "transparent where nothing is drawn"
        );
        assert_eq!(r.rgba().len(), 20 * 20 * 4);
        assert_eq!(r.argb32().len(), 400);
        assert!(r.png().unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn groups_transform_clip_and_fade() {
        let mut s = Scene::new(40.0, 40.0).background("#ffffff");
        s.push(
            Op::group(vec![Op::rect(0.0, 0.0, 40.0, 40.0).fill("#000000")])
                .transformed(Transform::translate(10.0, 10.0))
                .clipped(0.0, 0.0, 10.0, 10.0),
        );
        let r = render(&s, 1.0).unwrap();
        assert_eq!(r.pixel(15, 15), Some([0, 0, 0, 255]));
        assert_eq!(
            r.pixel(25, 25),
            Some([255, 255, 255, 255]),
            "outside the clip"
        );
        assert_eq!(
            r.pixel(5, 5),
            Some([255, 255, 255, 255]),
            "before the translate"
        );

        let mut f = Scene::new(10.0, 10.0).background("#ffffff");
        f.push(Op::group(vec![Op::rect(0.0, 0.0, 10.0, 10.0).fill("#000000")]).faded(0.5));
        let p = render(&f, 1.0).unwrap().pixel(5, 5).unwrap();
        assert!(
            (120..136).contains(&p[0]),
            "half black on white is grey: {p:?}"
        );
    }

    #[test]
    fn strokes_dashes_gradients_and_arrows() {
        let mut s = Scene::new(60.0, 20.0).background("#ffffff");
        s.push(Op::line(
            0.0,
            10.0,
            50.0,
            10.0,
            Stroke::new("#000000", 4.0).arrow(),
        ));
        let r = render(&s, 1.0).unwrap();
        assert_eq!(r.pixel(10, 10).unwrap()[0], 0);
        assert!(
            r.pixel(38, 6).unwrap()[0] < 128,
            "the arrowhead is wider than the shaft"
        );

        let mut d = Scene::new(40.0, 10.0).background("#ffffff");
        d.push(Op::line(
            0.0,
            5.0,
            40.0,
            5.0,
            Stroke::new("#000000", 4.0).dashed(5.0, 5.0),
        ));
        let r = render(&d, 1.0).unwrap();
        assert_eq!(r.pixel(2, 5).unwrap()[0], 0);
        assert_eq!(r.pixel(7, 5).unwrap()[0], 255, "the gap");

        let mut g = Scene::new(100.0, 10.0);
        g.push(Op::rect(0.0, 0.0, 100.0, 10.0).fill_linear(
            0.0,
            0.0,
            100.0,
            0.0,
            &[(0.0, "#000000"), (1.0, "#ffffff")],
        ));
        let r = render(&g, 1.0).unwrap();
        assert!(r.pixel(2, 5).unwrap()[0] < 30 && r.pixel(97, 5).unwrap()[0] > 225);
    }

    #[test]
    fn text_draws_when_a_font_is_installed() {
        if !has_fonts() {
            eprintln!("no system font; skipping");
            return;
        }
        let mut s = Scene::new(120.0, 40.0).background("#ffffff");
        s.push(Op::text(4.0, 4.0, "Hello", 20.0, "#000000"));
        let r = render(&s, 1.0).unwrap();
        let dark = (0..120)
            .flat_map(|x| (0..40).map(move |y| (x, y)))
            .filter(|&(x, y)| r.pixel(x, y).unwrap()[0] < 128)
            .count();
        assert!(dark > 60, "ink was drawn: {dark}");

        let mut wrapped = Scene::new(60.0, 100.0).background("#ffffff");
        wrapped.push(Op::text(0.0, 0.0, "one two three four five six", 14.0, "#000").wrapped(60.0));
        let r = render(&wrapped, 1.0).unwrap();
        let low = (0..60).any(|x| (40..100).any(|y| r.pixel(x, y).unwrap()[0] < 128));
        assert!(low, "wrapped text reaches later lines");
    }

    #[test]
    fn a_missing_picture_is_an_outline_not_a_crash() {
        let mut s = Scene::new(30.0, 30.0).background("#ffffff");
        s.push(Op::image("/no/such/file.png", 5.0, 5.0, 20.0, 20.0));
        assert!(render(&s, 1.0).is_some());
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::{Op, Scene, Stroke};

    /// `cargo test -p forge-canvas --release bench_redraw -- --ignored --nocapture`
    /// How long one redraw of a busy board takes on the CPU rasteriser.
    #[test]
    #[ignore]
    fn bench_redraw() {
        for (cards, w, scale) in [
            (60usize, 1200.0f32, 1.0f32),
            (300, 1200.0, 2.0),
            (1000, 1200.0, 2.0),
        ] {
            let mut ops = Vec::new();
            for i in 0..cards {
                let (x, y) = (16.0 + (i % 5) as f32 * 236.0, 16.0 + (i / 5) as f32 * 186.0);
                ops.push(
                    Op::group(vec![
                        Op::rect(0.0, 0.0, 220.0, 170.0)
                            .fill("#313244")
                            .stroked(Stroke::new("#585b70", 1.0))
                            .rounded(10.0),
                        Op::text(
                            12.0,
                            12.0,
                            &format!("A card with a fairly long title {i}"),
                            16.0,
                            "#ffffff",
                        )
                        .wrapped(196.0),
                        Op::text(12.0, 60.0, "status: active", 12.0, "#a6adc8"),
                        Op::text(12.0, 80.0, "author: somebody", 12.0, "#a6adc8"),
                    ])
                    .transformed(crate::Transform::translate(x, y)),
                );
            }
            let h = 16.0 + (cards / 5 + 1) as f32 * 186.0;
            let mut s = Scene::new(w, h).background("#1e1e2e");
            s.ops = ops;
            let t = std::time::Instant::now();
            let pm = render(&s, scale).unwrap();
            let drawn = t.elapsed();
            let t = std::time::Instant::now();
            let png = pm.png().unwrap();
            println!(
                "{cards} cards, {}x{} px: draw {drawn:?}, png {:?} ({} KB)",
                pm.width(),
                pm.height(),
                t.elapsed(),
                png.len() / 1024
            );
        }
    }
}
