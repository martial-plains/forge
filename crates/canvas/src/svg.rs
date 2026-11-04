// SPDX-License-Identifier: MIT OR Apache-2.0
//! A scene as SVG.

use std::fmt::Write;

use crate::color::Color;
use crate::model::{Align, Op, Paint, Scene, Stroke};
use crate::text::{measure, wrap};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn to_svg(scene: &Scene) -> String {
    // Roles nobody resolved for a theme come out in the dark palette.
    let scene = &*scene.resolved(&crate::Palette::dark());
    let mut out = String::new();
    let mut defs = String::new();
    let mut body = String::new();
    let mut n = 0;
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n",
        w = scene.width,
        h = scene.height
    );
    if let Some(bg) = &scene.background {
        let _ = writeln!(body, "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>", esc(bg));
    }
    ops(&scene.ops, &mut body, &mut defs, &mut n);
    if !defs.is_empty() {
        let _ = writeln!(out, "<defs>\n{defs}</defs>");
    }
    out.push_str(&body);
    out.push_str("</svg>\n");
    out
}

fn paint(p: &Option<Paint>, defs: &mut String, n: &mut usize) -> String {
    match p {
        None => "fill=\"none\"".into(),
        Some(Paint::Color(c)) => color_attr("fill", c),
        Some(Paint::Linear(g)) => {
            *n += 1;
            let id = format!("g{n}");
            let _ = write!(
                defs,
                "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
                g.x1, g.y1, g.x2, g.y2
            );
            for (o, c) in &g.stops {
                let col = Color::parse(c).unwrap_or(Color::BLACK);
                let _ = write!(
                    defs,
                    "<stop offset=\"{o}\" stop-color=\"{}\" stop-opacity=\"{}\"/>",
                    col.hex_rgb(),
                    col.a
                );
            }
            defs.push_str("</linearGradient>\n");
            format!("fill=\"url(#{id})\"")
        }
    }
}

fn color_attr(name: &str, c: &str) -> String {
    match Color::parse(c) {
        Some(col) if col.a < 1.0 => {
            format!("{name}=\"{}\" {name}-opacity=\"{}\"", col.hex_rgb(), col.a)
        }
        Some(col) => format!("{name}=\"{}\"", col.hex_rgb()),
        None => format!("{name}=\"{}\"", esc(c)),
    }
}

fn stroke_attr(s: &Option<Stroke>, marker: &mut bool) -> String {
    let Some(s) = s else { return "stroke=\"none\"".into() };
    let mut a = format!("{} stroke-width=\"{}\"", color_attr("stroke", &s.color), s.width);
    if !s.dash.is_empty() {
        let d: Vec<String> = s.dash.iter().map(f32::to_string).collect();
        let _ = write!(a, " stroke-dasharray=\"{}\"", d.join(" "));
    }
    if s.arrow_end {
        a.push_str(" marker-end=\"url(#arrow)\"");
        *marker = true;
    }
    if s.arrow_start {
        a.push_str(" marker-start=\"url(#arrow-start)\"");
        *marker = true;
    }
    a
}

fn ops(list: &[Op], out: &mut String, defs: &mut String, n: &mut usize) {
    for op in list {
        let id = op.id().map(|i| format!(" id=\"{}\"", esc(i))).unwrap_or_default();
        let mut marker = false;
        match op {
            Op::Rect { x, y, w, h, radius, fill, stroke, .. } => {
                let f = paint(fill, defs, n);
                let s = stroke_attr(stroke, &mut marker);
                let _ = writeln!(out, "<rect{id} x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" rx=\"{radius}\" {f} {s}/>");
            }
            Op::Ellipse { cx, cy, rx, ry, fill, stroke, .. } => {
                let f = paint(fill, defs, n);
                let s = stroke_attr(stroke, &mut marker);
                let _ = writeln!(out, "<ellipse{id} cx=\"{cx}\" cy=\"{cy}\" rx=\"{rx}\" ry=\"{ry}\" {f} {s}/>");
            }
            Op::Line { x1, y1, x2, y2, stroke, .. } => {
                let s = stroke_attr(&Some(stroke.clone()), &mut marker);
                let _ = writeln!(out, "<line{id} x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" {s}/>");
            }
            Op::Path { d, fill, stroke, .. } => {
                let f = paint(fill, defs, n);
                let s = stroke_attr(stroke, &mut marker);
                let _ = writeln!(out, "<path{id} d=\"{}\" {f} {s}/>", esc(d));
            }
            Op::Text { x, y, text, size, color, bold, italic, align, max_width, max_lines, .. } => {
                let mut lines = match max_width {
                    Some(w) => wrap(text, *size, *bold, *w),
                    None => text.split('\n').map(str::to_string).collect(),
                };
                if let Some(m) = max_lines {
                    lines.truncate(*m as usize);
                }
                let anchor = match align {
                    Align::Left => "start",
                    Align::Center => "middle",
                    Align::Right => "end",
                };
                let weight = if *bold { " font-weight=\"bold\"" } else { "" };
                let style = if *italic { " font-style=\"italic\"" } else { "" };
                let _ = write!(
                    out,
                    "<text{id} x=\"{x}\" y=\"{y}\" font-family=\"sans-serif\" font-size=\"{size}\" text-anchor=\"{anchor}\"{weight}{style} {}>",
                    color_attr("fill", color)
                );
                for (i, l) in lines.iter().enumerate() {
                    let dy = if i == 0 { size * 1.0 } else { size * 1.3 };
                    let _ = write!(out, "<tspan x=\"{x}\" dy=\"{dy}\">{}</tspan>", esc(l));
                }
                out.push_str("</text>\n");
                let _ = measure("", 0.0, false);
            }
            Op::Image { path, x, y, w, h, fit, opacity, .. } => {
                let par = match fit {
                    crate::ImageFit::Stretch => "none",
                    crate::ImageFit::Cover => "xMidYMid slice",
                    crate::ImageFit::Contain => "xMidYMid meet",
                };
                let _ = writeln!(
                    out,
                    "<image{id} xlink:href=\"file://{}\" x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"{par}\" opacity=\"{opacity}\"/>",
                    esc(path)
                );
            }
            Op::Group { ops: inner, transform, clip, opacity, .. } => {
                let mut attrs = String::new();
                if let Some(t) = transform {
                    let m = t.matrix();
                    let _ = write!(attrs, " transform=\"matrix({} {} {} {} {} {})\"", m.a, m.b, m.c, m.d, m.e, m.f);
                }
                if let Some([cx, cy, cw, ch]) = clip {
                    *n += 1;
                    let cid = format!("c{n}");
                    let _ = writeln!(defs, "<clipPath id=\"{cid}\"><rect x=\"{cx}\" y=\"{cy}\" width=\"{cw}\" height=\"{ch}\"/></clipPath>");
                    let _ = write!(attrs, " clip-path=\"url(#{cid})\"");
                }
                if *opacity < 1.0 {
                    let _ = write!(attrs, " opacity=\"{opacity}\"");
                }
                let _ = writeln!(out, "<g{id}{attrs}>");
                ops(inner, out, defs, n);
                out.push_str("</g>\n");
            }
        }
        if marker && !defs.contains("id=\"arrow\"") {
            defs.push_str("<marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto\"><path d=\"M0 0 L10 5 L0 10 z\" fill=\"context-stroke\"/></marker>\n<marker id=\"arrow-start\" viewBox=\"0 0 10 10\" refX=\"1\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\"><path d=\"M0 0 L10 5 L0 10 z\" fill=\"context-stroke\"/></marker>\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::*;

    #[test]
    fn a_scene_becomes_svg() {
        let mut s = Scene::new(100.0, 50.0).background("#fff");
        s.push(Op::rect(1.0, 2.0, 30.0, 20.0).fill("#ff0000").rounded(3.0).with_id("a&b"));
        s.push(Op::line(0.0, 0.0, 10.0, 10.0, Stroke::new("blue", 2.0).dashed(4.0, 2.0).arrow()));
        s.push(Op::text(5.0, 5.0, "x < y", 12.0, "#000"));
        s.push(Op::group(vec![Op::ellipse(5.0, 5.0, 2.0, 2.0)]).transformed(Transform::translate(3.0, 4.0)));
        let svg = s.to_svg();
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("id=\"a&amp;b\"") && svg.contains("rx=\"3\""));
        assert!(svg.contains("stroke-dasharray=\"4 2\"") && svg.contains("marker-end"));
        assert!(svg.contains("x &lt; y") && svg.contains("matrix(1 0 0 1 3 4)"));
        assert!(svg.contains("<marker id=\"arrow\""));
    }
}
