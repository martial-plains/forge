// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hit testing and bounds.

use crate::geom::{Matrix, Point, flatten, parse_path};
use crate::model::{Op, Scene};
use crate::text::{measure, wrap};

pub fn hit(scene: &Scene, x: f32, y: f32) -> Option<String> {
    hit_ops(&scene.ops, &Matrix::IDENTITY, Point::new(x, y), None)
}

fn hit_ops(ops: &[Op], m: &Matrix, p: Point, inherited: Option<&str>) -> Option<String> {
    for op in ops.iter().rev() {
        if let Some(found) = hit_op(op, m, p, inherited) {
            return Some(found);
        }
    }
    None
}

fn hit_op(op: &Op, m: &Matrix, p: Point, inherited: Option<&str>) -> Option<String> {
    let id = op.id().or(inherited);
    // Only a shape that has (or inherits) an id can be reported.
    let local = |m: &Matrix| m.invert().map(|i| i.apply(p));
    match op {
        Op::Group {
            ops,
            transform,
            clip,
            id: gid,
            ..
        } => {
            let gm = match transform {
                Some(t) => m.then(&t.matrix()),
                None => *m,
            };
            if let Some([cx, cy, cw, ch]) = clip {
                let q = local(&gm)?;
                if q.x < *cx || q.y < *cy || q.x > cx + cw || q.y > cy + ch {
                    return None;
                }
            }
            hit_ops(ops, &gm, p, gid.as_deref().or(inherited))
        }
        _ => {
            let id = id?;
            let q = local(m)?;
            let scale = m.mean_scale().max(1e-6);
            let inside = match op {
                Op::Rect { x, y, w, h, .. } | Op::Image { x, y, w, h, .. } => {
                    q.x >= *x && q.x <= x + w && q.y >= *y && q.y <= y + h
                }
                Op::Ellipse { cx, cy, rx, ry, .. } => {
                    let (dx, dy) = ((q.x - cx) / rx.max(1e-6), (q.y - cy) / ry.max(1e-6));
                    dx * dx + dy * dy <= 1.0
                }
                Op::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    stroke,
                    ..
                } => {
                    dist_to_segment(q, Point::new(*x1, *y1), Point::new(*x2, *y2))
                        <= (stroke.width / 2.0).max(3.0 / scale)
                }
                Op::Path {
                    d, fill, stroke, ..
                } => {
                    let cmds = parse_path(d)?;
                    let polys = flatten(&cmds);
                    let tol = stroke
                        .as_ref()
                        .map_or(0.0, |s| (s.width / 2.0).max(3.0 / scale));
                    let near = tol > 0.0
                        && polys.iter().any(|(pts, closed)| {
                            pts.windows(2)
                                .any(|w| dist_to_segment(q, w[0], w[1]) <= tol)
                                || (*closed
                                    && pts.len() > 1
                                    && dist_to_segment(q, pts[pts.len() - 1], pts[0]) <= tol)
                        });
                    near || (fill.is_some() && polys.iter().any(|(pts, _)| in_polygon(q, pts)))
                }
                Op::Text { .. } => text_bounds(op).is_some_and(|[x, y, w, h]| {
                    q.x >= x && q.x <= x + w && q.y >= y && q.y <= y + h
                }),
                Op::Group { .. } => false,
            };
            inside.then(|| id.to_string())
        }
    }
}

fn dist_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.x + t * dx, a.y + t * dy);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

fn in_polygon(p: Point, pts: &[Point]) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A text op's box in its own coordinates.
pub(crate) fn text_bounds(op: &Op) -> Option<[f32; 4]> {
    let Op::Text {
        x,
        y,
        text,
        size,
        bold,
        align,
        max_width,
        max_lines,
        ..
    } = op
    else {
        return None;
    };
    let mut lines = match max_width {
        Some(w) => wrap(text, *size, *bold, *w),
        None => text.split('\n').map(str::to_string).collect(),
    };
    if let Some(n) = max_lines {
        lines.truncate(*n as usize);
    }
    let w = lines
        .iter()
        .map(|l| measure(l, *size, *bold).0)
        .fold(0.0, f32::max);
    let h = lines.len() as f32 * size * 1.3;
    let left = match align {
        crate::model::Align::Left => *x,
        crate::model::Align::Center => x - w / 2.0,
        crate::model::Align::Right => x - w,
    };
    Some([left, *y, w, h])
}

fn union(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> Option<[f32; 4]> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
            let (x1, y1) = (
                (a[0] + a[2]).max(b[0] + b[2]),
                (a[1] + a[3]).max(b[1] + b[3]),
            );
            Some([x0, y0, x1 - x0, y1 - y0])
        }
    }
}

fn boxed(m: &Matrix, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let cs = [
        m.apply(Point::new(x, y)),
        m.apply(Point::new(x + w, y)),
        m.apply(Point::new(x, y + h)),
        m.apply(Point::new(x + w, y + h)),
    ];
    let x0 = cs.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    let y0 = cs.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let x1 = cs.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    let y1 = cs.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    [x0, y0, x1 - x0, y1 - y0]
}

pub fn bounds(ops: &[Op], m: &Matrix) -> Option<[f32; 4]> {
    ops.iter()
        .fold(None, |acc, op| union(acc, op_bounds(op, m)))
}

fn op_bounds(op: &Op, m: &Matrix) -> Option<[f32; 4]> {
    let raw = match op {
        Op::Rect { x, y, w, h, .. } | Op::Image { x, y, w, h, .. } => Some([*x, *y, *w, *h]),
        Op::Ellipse { cx, cy, rx, ry, .. } => Some([cx - rx, cy - ry, 2.0 * rx, 2.0 * ry]),
        Op::Line { x1, y1, x2, y2, .. } => {
            Some([x1.min(*x2), y1.min(*y2), (x1 - x2).abs(), (y1 - y2).abs()])
        }
        Op::Path { d, .. } => {
            let polys = flatten(&parse_path(d)?);
            let pts: Vec<Point> = polys.into_iter().flat_map(|(p, _)| p).collect();
            let x0 = pts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
            let y0 = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            let x1 = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
            let y1 = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            (x1 >= x0).then_some([x0, y0, x1 - x0, y1 - y0])
        }
        Op::Text { .. } => text_bounds(op),
        Op::Group {
            ops,
            transform,
            clip,
            ..
        } => {
            let gm = match transform {
                Some(t) => m.then(&t.matrix()),
                None => *m,
            };
            let inner = bounds(ops, &gm)?;
            return match clip {
                Some(c) => Some(intersect(inner, boxed(&gm, *c))),
                None => Some(inner),
            };
        }
    };
    raw.map(|b| boxed(m, b))
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x1, y1) = (
        (a[0] + a[2]).min(b[0] + b[2]),
        (a[1] + a[3]).min(b[1] + b[3]),
    );
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

pub fn bounds_of(ops: &[Op], m: &Matrix, id: &str) -> Option<[f32; 4]> {
    for op in ops {
        if op.id() == Some(id) {
            return op_bounds(op, m);
        }
        if let Op::Group { ops, transform, .. } = op {
            let gm = match transform {
                Some(t) => m.then(&t.matrix()),
                None => *m,
            };
            if let Some(b) = bounds_of(ops, &gm, id) {
                return Some(b);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::model::{Op, Scene, Stroke, Transform};

    fn scene() -> Scene {
        let mut s = Scene::new(200.0, 100.0);
        s.push(Op::rect(0.0, 0.0, 100.0, 50.0).fill("#fff").with_id("back"));
        s.push(
            Op::ellipse(50.0, 25.0, 10.0, 10.0)
                .fill("red")
                .with_id("dot"),
        );
        s.push(Op::line(0.0, 80.0, 100.0, 80.0, Stroke::new("#000", 2.0)).with_id("rule"));
        s.push(Op::rect(5.0, 5.0, 4.0, 4.0).fill("blue")); // no id: never reported
        s
    }

    #[test]
    fn the_topmost_shape_with_an_id_wins() {
        let s = scene();
        assert_eq!(s.hit(50.0, 25.0).as_deref(), Some("dot"));
        assert_eq!(s.hit(10.0, 10.0).as_deref(), Some("back"));
        assert_eq!(
            s.hit(7.0, 7.0).as_deref(),
            Some("back"),
            "an id-less shape doesn't block"
        );
        assert_eq!(s.hit(50.0, 81.0).as_deref(), Some("rule"));
        assert_eq!(s.hit(150.0, 90.0), None);
    }

    #[test]
    fn groups_apply_their_transform_and_pass_their_id_down() {
        let mut s = Scene::new(200.0, 200.0);
        let card = Op::group(vec![Op::rect(0.0, 0.0, 10.0, 10.0).fill("#000")])
            .transformed(Transform {
                x: 100.0,
                y: 100.0,
                sx: 2.0,
                sy: 2.0,
                ..Default::default()
            })
            .with_id("card");
        s.push(card);
        assert_eq!(s.hit(110.0, 110.0).as_deref(), Some("card"));
        assert_eq!(s.hit(105.0, 105.0).as_deref(), Some("card"));
        assert_eq!(s.hit(125.0, 125.0), None, "10 units at 2x is 20 wide");
        assert_eq!(s.bounds_of("card"), Some([100.0, 100.0, 20.0, 20.0]));
        assert_eq!(s.bounds(), Some([100.0, 100.0, 20.0, 20.0]));
    }

    #[test]
    fn clips_and_paths_and_text() {
        let mut s = Scene::new(100.0, 100.0);
        s.push(
            Op::group(vec![
                Op::rect(0.0, 0.0, 100.0, 100.0).fill("#000").with_id("big"),
            ])
            .clipped(0.0, 0.0, 10.0, 10.0),
        );
        assert_eq!(s.hit(5.0, 5.0).as_deref(), Some("big"));
        assert_eq!(s.hit(50.0, 50.0), None, "outside the clip");

        let mut t = Scene::new(100.0, 100.0);
        t.push(
            Op::path("M10 10 L60 10 L60 60 Z")
                .fill("#f00")
                .with_id("tri"),
        );
        t.push(Op::text(70.0, 70.0, "hello", 10.0, "#000").with_id("label"));
        assert_eq!(t.hit(50.0, 20.0).as_deref(), Some("tri"));
        assert_eq!(t.hit(15.0, 50.0), None);
        assert_eq!(t.hit(75.0, 75.0).as_deref(), Some("label"));
    }
}
