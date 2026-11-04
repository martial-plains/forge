// SPDX-License-Identifier: MIT OR Apache-2.0
//! Draws a canvas (Bases cards and boards, the graph, drawings) with Cairo
//! and Pango. The editor hands canvases over as `.scene.json` files in the
//! display-list format described in crates/canvas/src/native.rs: colours
//! are `#rrggbbaa`, paths are M/L/C/Z and text is already broken into
//! lines, so there is no layout here, only drawing calls.
//!
//! UNVERIFIED: written without GTK 4's development files, so it has never
//! been compiled.

use std::collections::HashMap;

use gtk4::cairo::{Context, LinearGradient};
use gtk4::gdk::prelude::GdkCairoContextExt;
use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::pango;
use serde_json::Value;

pub const SUFFIX: &str = ".scene.json";

pub fn is_scene(path: &str) -> bool {
    path.ends_with(SUFFIX)
}

pub struct Scene {
    width: f64,
    height: f64,
    root: Value,
}

/// Scenes and pictures already read, by path (the editor writes a new file
/// for every change, so the path is the key).
#[derive(Default)]
pub struct Cache {
    scenes: HashMap<String, Option<Scene>>,
    pictures: HashMap<String, Option<Pixbuf>>,
}

impl Cache {
    fn scene(&mut self, path: &str) -> Option<&Scene> {
        if self.scenes.len() > 24 && !self.scenes.contains_key(path) {
            self.scenes.clear();
        }
        self.scenes
            .entry(path.to_string())
            .or_insert_with(|| {
                let root: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
                Some(Scene { width: root["w"].as_f64()?, height: root["h"].as_f64()?, root })
            })
            .as_ref()
    }

    fn picture(&mut self, path: &str) -> Option<Pixbuf> {
        if self.pictures.len() > 96 && !self.pictures.contains_key(path) {
            self.pictures.clear();
        }
        self.pictures.entry(path.to_string()).or_insert_with(|| Pixbuf::from_file(path).ok()).clone()
    }
}

/// Draw the scene in `path` into the box, scaled to fit and centred vertically.
pub fn draw(cache: &mut Cache, cr: &Context, path: &str, x: f64, y: f64, w: f64, h: f64) {
    // Taken out for the walk: the walk needs the cache for pictures.
    let Some(scene) = cache.scene(path).map(|s| (s.width, s.height, s.root.clone())) else { return };
    let (sw, sh, root) = scene;
    if sw <= 0.0 || sh <= 0.0 {
        return;
    }
    let s = (w / sw).min(h / sh);
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.translate(x, y + (h - sh * s) / 2.0);
    cr.scale(s, s);
    if let Some(bg) = root["bg"].as_str() {
        rgba(cr, bg);
        cr.rectangle(0.0, 0.0, sw, sh);
        let _ = cr.fill();
    }
    ops(cache, cr, &root["ops"]);
    let _ = cr.restore();
}

fn parse(hex: &str) -> (f64, f64, f64, f64) {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x0000_00ff);
    (
        f64::from(v >> 24 & 255) / 255.0,
        f64::from(v >> 16 & 255) / 255.0,
        f64::from(v >> 8 & 255) / 255.0,
        f64::from(v & 255) / 255.0,
    )
}

fn rgba(cr: &Context, hex: &str) {
    let (r, g, b, a) = parse(hex);
    cr.set_source_rgba(r, g, b, a);
}

fn n(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn ops(cache: &mut Cache, cr: &Context, list: &Value) {
    let Some(list) = list.as_array() else { return };
    for op in list {
        match op["t"].as_str() {
            Some("rect") => {
                let (x, y, w, h) = (n(&op["x"]), n(&op["y"]), n(&op["w"]), n(&op["h"]));
                let r = n(&op["r"]).min(w.min(h) / 2.0);
                if r > 0.0 {
                    use std::f64::consts::{FRAC_PI_2, PI};
                    cr.new_sub_path();
                    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
                    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
                    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
                    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
                    cr.close_path();
                } else {
                    cr.rectangle(x, y, w, h);
                }
                paint(cr, op);
            }
            Some("ellipse") => {
                let (cx, cy, rx, ry) = (n(&op["cx"]), n(&op["cy"]), n(&op["rx"]), n(&op["ry"]));
                if rx > 0.0 && ry > 0.0 {
                    let _ = cr.save();
                    cr.translate(cx, cy);
                    cr.scale(rx, ry);
                    cr.new_sub_path();
                    cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
                    let _ = cr.restore();
                    paint(cr, op);
                }
            }
            Some("path") => {
                path(cr, &op["c"]);
                paint(cr, op);
            }
            Some("text") => text(cr, op),
            Some("image") => image(cache, cr, op),
            Some("group") => group(cache, cr, op),
            _ => {}
        }
    }
}

fn path(cr: &Context, cmds: &Value) {
    cr.new_path();
    for c in cmds.as_array().into_iter().flatten() {
        let v: Vec<f64> = c.as_array().map(|a| a.iter().skip(1).map(n).collect()).unwrap_or_default();
        match c[0].as_str() {
            Some("M") if v.len() >= 2 => cr.move_to(v[0], v[1]),
            Some("L") if v.len() >= 2 => cr.line_to(v[0], v[1]),
            Some("C") if v.len() >= 6 => cr.curve_to(v[0], v[1], v[2], v[3], v[4], v[5]),
            Some("Z") => cr.close_path(),
            _ => {}
        }
    }
}

/// Fill and stroke the path that is current (it is consumed).
fn paint(cr: &Context, op: &Value) {
    let fill = &op["f"];
    let stroke = &op["s"];
    let has_fill = fill.is_object();
    let has_stroke = stroke.is_object();
    if has_fill {
        if let Some(c) = fill["c"].as_str() {
            rgba(cr, c);
        } else if let Some(g) = fill["g"].as_array().filter(|g| g.len() == 4) {
            let grad = LinearGradient::new(n(&g[0]), n(&g[1]), n(&g[2]), n(&g[3]));
            for st in fill["stops"].as_array().into_iter().flatten() {
                let (r, gr, b, a) = parse(st[1].as_str().unwrap_or("#000000ff"));
                grad.add_color_stop_rgba(n(&st[0]), r, gr, b, a);
            }
            let _ = cr.set_source(&grad);
        }
        let _ = if has_stroke { cr.fill_preserve() } else { cr.fill() };
    }
    if has_stroke {
        rgba(cr, stroke["c"].as_str().unwrap_or("#000000ff"));
        cr.set_line_width(n(&stroke["w"]).max(0.01));
        cr.set_line_join(gtk4::cairo::LineJoin::Round);
        let dash: Vec<f64> = stroke["dash"].as_array().map(|d| d.iter().map(n).collect()).unwrap_or_default();
        if dash.len() >= 2 {
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.set_dash(&dash, 0.0);
        } else {
            cr.set_line_cap(gtk4::cairo::LineCap::Butt);
            cr.set_dash(&[], 0.0);
        }
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
    } else if !has_fill {
        cr.new_path();
    }
}

fn text(cr: &Context, op: &Value) {
    let size = n(&op["size"]);
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::from_string("Sans");
    font.set_absolute_size(size * f64::from(pango::SCALE));
    if op["bold"].as_bool() == Some(true) {
        font.set_weight(pango::Weight::Bold);
    }
    if op["italic"].as_bool() == Some(true) {
        font.set_style(pango::Style::Italic);
    }
    layout.set_font_description(Some(&font));
    rgba(cr, op["color"].as_str().unwrap_or("#000000ff"));
    let align = op["align"].as_str().unwrap_or("left");
    for (i, line) in op["lines"].as_array().into_iter().flatten().enumerate() {
        layout.set_text(line.as_str().unwrap_or(""));
        let (width, _) = layout.pixel_size();
        let width = f64::from(width);
        let x = match align {
            "center" => n(&op["x"]) - width / 2.0,
            "right" => n(&op["x"]) - width,
            _ => n(&op["x"]),
        };
        cr.move_to(x, n(&op["y"]) + i as f64 * size * 1.3);
        pangocairo::functions::show_layout(cr, &layout);
    }
}

fn image(cache: &mut Cache, cr: &Context, op: &Value) {
    let Some(pic) = cache.picture(op["path"].as_str().unwrap_or("")) else { return };
    let (x, y, w, h) = (n(&op["x"]), n(&op["y"]), n(&op["w"]), n(&op["h"]));
    let (pw, ph) = (f64::from(pic.width()), f64::from(pic.height()));
    if pw <= 0.0 || ph <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let (sx, sy) = match op["fit"].as_str() {
        Some("cover") => {
            let k = (w / pw).max(h / ph);
            (k, k)
        }
        Some("contain") => {
            let k = (w / pw).min(h / ph);
            (k, k)
        }
        _ => (w / pw, h / ph),
    };
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.translate(x + w / 2.0 - pw * sx / 2.0, y + h / 2.0 - ph * sy / 2.0);
    cr.scale(sx, sy);
    cr.set_source_pixbuf(&pic, 0.0, 0.0);
    let _ = cr.paint_with_alpha(op["o"].as_f64().unwrap_or(1.0));
    let _ = cr.restore();
}

fn group(cache: &mut Cache, cr: &Context, op: &Value) {
    let opacity = op["o"].as_f64().unwrap_or(1.0);
    let _ = cr.save();
    if let Some(m) = op["m"].as_array().filter(|m| m.len() == 6) {
        let m: Vec<f64> = m.iter().map(n).collect();
        cr.transform(gtk4::cairo::Matrix::new(m[0], m[1], m[2], m[3], m[4], m[5]));
    }
    if let Some(c) = op["clip"].as_array().filter(|c| c.len() == 4) {
        cr.rectangle(n(&c[0]), n(&c[1]), n(&c[2]), n(&c[3]));
        cr.clip();
    }
    if opacity < 1.0 {
        cr.push_group();
        ops(cache, cr, &op["ops"]);
        let _ = cr.pop_group_to_source();
        let _ = cr.paint_with_alpha(opacity);
    } else {
        ops(cache, cr, &op["ops"]);
    }
    let _ = cr.restore();
}
