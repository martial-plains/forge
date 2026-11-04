// SPDX-License-Identifier: MIT OR Apache-2.0
//! The scene as a display list for a frontend that draws it itself
//! (CoreGraphics, Compose, cairo, Win2D).
//!
//! The scene model is meant to be written by hand, so it has shortcuts a
//! renderer would each have to implement: SVG path data with relative and
//! smooth commands, lines, arrowheads, text that wraps, colour names. The
//! display list has none of them. Everything is one of:
//!
//! ```text
//! {"w":W,"h":H,"bg":"#rrggbbaa"|null,"ops":[Op...]}
//!
//! {"t":"rect","x","y","w","h","r":radius,"f":Fill?,"s":Stroke?}
//! {"t":"ellipse","cx","cy","rx","ry","f","s"}
//! {"t":"path","c":[["M",x,y],["L",x,y],["C",x1,y1,x2,y2,x,y],["Z"]],"f","s"}
//! {"t":"text","x","y","size","color","bold","italic","align":"left|center|right","lines":["..."]}
//! {"t":"image","path","x","y","w","h","fit":"stretch|cover|contain","o":opacity}
//! {"t":"group","m":[a,b,c,d,e,f],"clip":[x,y,w,h]?,"o":opacity,"ops":[Op...]}
//!
//! Fill   {"c":"#rrggbbaa"} or {"g":[x1,y1,x2,y2],"stops":[[offset,"#rrggbbaa"]...]}
//! Stroke {"c":"#rrggbbaa","w":width,"dash":[on,off,...]}
//! ```
//!
//! Colours are all `#rrggbbaa`, roles already resolved. A matrix maps a
//! group's own coordinates to its parent's (SVG's `matrix(a b c d e f)`).
//! A text line's top is `y + index * size * 1.3`; `x` is its left, centre
//! or right by `align`. Quadratic curves are cubic; arrowheads are filled
//! paths of their own.

use crate::geom::{PathCmd, Point, flatten, parse_path};
use crate::model::{Align, ImageFit, Op, Paint, Scene, Stroke};
use crate::{Color, wrap};
use serde_json::{Value, json};

fn color(c: &str) -> String {
    Color::parse(c).unwrap_or(Color::BLACK).hex_rgba()
}

fn fill(p: &Option<Paint>) -> Value {
    match p {
        None => Value::Null,
        Some(Paint::Color(c)) => json!({"c": color(c)}),
        Some(Paint::Linear(g)) => json!({
            "g": [g.x1, g.y1, g.x2, g.y2],
            "stops": g.stops.iter().map(|(o, c)| json!([o, color(c)])).collect::<Vec<_>>(),
        }),
    }
}

fn stroke(s: &Option<Stroke>) -> Value {
    match s {
        None => Value::Null,
        Some(s) => json!({"c": color(&s.color), "w": s.width, "dash": s.dash}),
    }
}

fn cmds(cmds: &[PathCmd]) -> Value {
    let mut out = Vec::new();
    let mut cur = Point::default();
    let mut start = Point::default();
    for c in cmds {
        match *c {
            PathCmd::Move(p) => {
                out.push(json!(["M", p.x, p.y]));
                cur = p;
                start = p;
            }
            PathCmd::Line(p) => {
                out.push(json!(["L", p.x, p.y]));
                cur = p;
            }
            PathCmd::Quad(q, p) => {
                // The cubic with the same shape.
                let c1 = Point::new(cur.x + 2.0 / 3.0 * (q.x - cur.x), cur.y + 2.0 / 3.0 * (q.y - cur.y));
                let c2 = Point::new(p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y));
                out.push(json!(["C", c1.x, c1.y, c2.x, c2.y, p.x, p.y]));
                cur = p;
            }
            PathCmd::Cubic(a, b, p) => {
                out.push(json!(["C", a.x, a.y, b.x, b.y, p.x, p.y]));
                cur = p;
            }
            PathCmd::Close => {
                out.push(json!(["Z"]));
                cur = start;
            }
        }
    }
    Value::Array(out)
}

fn path_op(c: Value, f: Value, s: Value) -> Value {
    json!({"t": "path", "c": c, "f": f, "s": s})
}

/// The triangle at `tip`, pointing away from `from` (the same shape as the
/// raster renderer draws).
fn arrowhead(tip: Point, from: Point, s: &Stroke) -> Option<Value> {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let size = (s.width * 4.0).max(9.0);
    let (bx, by) = (tip.x - ux * size, tip.y - uy * size);
    let half = size * 0.45;
    Some(path_op(
        json!([["M", tip.x, tip.y], ["L", bx - uy * half, by + ux * half], ["L", bx + uy * half, by - ux * half], ["Z"]]),
        json!({"c": color(&s.color)}),
        Value::Null,
    ))
}

fn arrows(out: &mut Vec<Value>, polys: &[(Vec<Point>, bool)], s: &Stroke) {
    let Some((pts, _)) = polys.iter().find(|(p, _)| p.len() >= 2) else { return };
    if s.arrow_end && let Some(a) = arrowhead(pts[pts.len() - 1], pts[pts.len() - 2], s) {
        out.push(a);
    }
    if s.arrow_start && let Some(a) = arrowhead(pts[0], pts[1], s) {
        out.push(a);
    }
}

fn convert(ops: &[Op], out: &mut Vec<Value>) {
    for op in ops {
        match op {
            Op::Rect { x, y, w, h, radius, fill: f, stroke: s, .. } => {
                out.push(json!({"t": "rect", "x": x, "y": y, "w": w, "h": h, "r": radius, "f": fill(f), "s": stroke(s)}));
            }
            Op::Ellipse { cx, cy, rx, ry, fill: f, stroke: s, .. } => {
                out.push(json!({"t": "ellipse", "cx": cx, "cy": cy, "rx": rx, "ry": ry, "f": fill(f), "s": stroke(s)}));
            }
            Op::Line { x1, y1, x2, y2, stroke: s, .. } => {
                out.push(path_op(
                    json!([["M", x1, y1], ["L", x2, y2]]),
                    Value::Null,
                    stroke(&Some(s.clone())),
                ));
                let pts = vec![(vec![Point::new(*x1, *y1), Point::new(*x2, *y2)], false)];
                arrows(out, &pts, s);
            }
            Op::Path { d, fill: f, stroke: s, .. } => {
                let Some(parsed) = parse_path(d) else { continue };
                out.push(path_op(cmds(&parsed), fill(f), stroke(s)));
                if let Some(s) = s {
                    arrows(out, &flatten(&parsed), s);
                }
            }
            Op::Text { x, y, text, size, color: c, bold, italic, align, max_width, max_lines, .. } => {
                let mut lines = match max_width {
                    Some(w) => wrap(text, *size, *bold, *w),
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
                let align = match align {
                    Align::Left => "left",
                    Align::Center => "center",
                    Align::Right => "right",
                };
                out.push(json!({"t": "text", "x": x, "y": y, "size": size, "color": color(c),
                    "bold": bold, "italic": italic, "align": align, "lines": lines}));
            }
            Op::Image { path, x, y, w, h, fit, opacity, .. } => {
                let fit = match fit {
                    ImageFit::Stretch => "stretch",
                    ImageFit::Cover => "cover",
                    ImageFit::Contain => "contain",
                };
                out.push(json!({"t": "image", "path": path, "x": x, "y": y, "w": w, "h": h, "fit": fit, "o": opacity}));
            }
            Op::Group { ops, transform, clip, opacity, .. } => {
                let m = transform.as_ref().map_or(crate::Matrix::IDENTITY, |t| t.matrix());
                let mut inner = Vec::new();
                convert(ops, &mut inner);
                out.push(json!({"t": "group", "m": [m.a, m.b, m.c, m.d, m.e, m.f], "clip": clip,
                    "o": opacity, "ops": inner}));
            }
        }
    }
}

/// `scene` (roles already resolved) as the display list, as JSON.
pub fn display_list(scene: &Scene) -> Value {
    let mut ops = Vec::new();
    convert(&scene.ops, &mut ops);
    json!({
        "w": scene.width,
        "h": scene.height,
        "bg": scene.background.as_deref().map(color),
        "ops": ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Stroke;

    #[test]
    fn shortcuts_are_gone_from_the_display_list() {
        let mut s = Scene::new(100.0, 50.0);
        s.background = Some("white".into());
        s.push(Op::path("m 0 0 q 10 0 10 10 t 10 0 z").stroked(Stroke::new("red", 2.0).arrow()));
        s.push(Op::line(0.0, 0.0, 20.0, 0.0, Stroke::new("#00f", 1.0)));
        s.push(Op::text(5.0, 5.0, "one two three four", 10.0, "blue").wrapped(40.0));
        let v = display_list(&s);
        assert_eq!(v["bg"], "#ffffffff");
        let ops = v["ops"].as_array().unwrap();
        // The path, its arrowhead, the line, the text.
        assert_eq!(ops.len(), 4, "{v}");
        assert!(ops[0]["c"].as_array().unwrap().iter().all(|c| matches!(c[0].as_str(), Some("M" | "L" | "C" | "Z"))));
        assert_eq!(ops[1]["f"]["c"], ops[0]["s"]["c"], "the arrowhead has the stroke colour");
        assert_eq!(ops[2]["t"], "path");
        assert!(ops[3]["lines"].as_array().unwrap().len() > 1, "wrapped: {v}");
    }
}
