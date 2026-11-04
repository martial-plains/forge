// SPDX-License-Identifier: MIT OR Apache-2.0
//! Points, matrices and SVG-style path data.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An affine matrix `[a b c d e f]`, as SVG writes it:
/// `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(x: f32, y: f32) -> Self {
        Self { e: x, f: y, ..Self::IDENTITY }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Self { a: x, d: y, ..Self::IDENTITY }
    }

    pub fn rotate(deg: f32) -> Self {
        let (s, c) = deg.to_radians().sin_cos();
        Self { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// `self` applied after `other` (the child's matrix is `other`).
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    pub fn invert(&self) -> Option<Matrix> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-9 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Matrix { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) })
    }

    /// The average scale, for line widths and font sizes.
    pub fn mean_scale(&self) -> f32 {
        ((self.a * self.d - self.b * self.c).abs()).sqrt()
    }
}

/// A path command, all absolute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}

/// Parse SVG path data: `M L H V C S Q T Z`, absolute or relative.
/// (Arcs are not supported.) None on a malformed path.
pub fn parse_path(d: &str) -> Option<Vec<PathCmd>> {
    let mut out = Vec::new();
    let mut tokens = Tokens { s: d.as_bytes(), i: 0 };
    let mut cur = Point::default();
    let mut start = Point::default();
    let mut last_ctrl: Option<Point> = None;
    let mut cmd = b'M';
    loop {
        tokens.skip();
        if tokens.i >= tokens.s.len() {
            break;
        }
        let c = tokens.s[tokens.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            tokens.i += 1;
        } else if cmd == b'z' || cmd == b'Z' {
            return None;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { Point::default() };
        let pt = |t: &mut Tokens| -> Option<Point> {
            let x = t.number()?;
            let y = t.number()?;
            Some(Point::new(base.x + x, base.y + y))
        };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = pt(&mut tokens)?;
                out.push(PathCmd::Move(p));
                cur = p;
                start = p;
                last_ctrl = None;
                // Extra pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let p = pt(&mut tokens)?;
                out.push(PathCmd::Line(p));
                cur = p;
                last_ctrl = None;
            }
            b'H' => {
                let x = tokens.number()?;
                cur = Point::new(if rel { cur.x + x } else { x }, cur.y);
                out.push(PathCmd::Line(cur));
                last_ctrl = None;
            }
            b'V' => {
                let y = tokens.number()?;
                cur = Point::new(cur.x, if rel { cur.y + y } else { y });
                out.push(PathCmd::Line(cur));
                last_ctrl = None;
            }
            b'C' => {
                let (p1, p2, p) = (pt(&mut tokens)?, pt(&mut tokens)?, pt(&mut tokens)?);
                out.push(PathCmd::Cubic(p1, p2, p));
                last_ctrl = Some(p2);
                cur = p;
            }
            b'S' => {
                let p1 = last_ctrl.map_or(cur, |c| Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y));
                let (p2, p) = (pt(&mut tokens)?, pt(&mut tokens)?);
                out.push(PathCmd::Cubic(p1, p2, p));
                last_ctrl = Some(p2);
                cur = p;
            }
            b'Q' => {
                let (p1, p) = (pt(&mut tokens)?, pt(&mut tokens)?);
                out.push(PathCmd::Quad(p1, p));
                last_ctrl = Some(p1);
                cur = p;
            }
            b'T' => {
                let p1 = last_ctrl.map_or(cur, |c| Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y));
                let p = pt(&mut tokens)?;
                out.push(PathCmd::Quad(p1, p));
                last_ctrl = Some(p1);
                cur = p;
            }
            b'Z' => {
                out.push(PathCmd::Close);
                cur = start;
                last_ctrl = None;
            }
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let mut seen_dot = false;
        let mut seen_exp = false;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            let ok = c.is_ascii_digit()
                || (c == b'.' && !seen_dot && !seen_exp)
                || ((c == b'-' || c == b'+') && (self.i == start || matches!(self.s[self.i - 1], b'e' | b'E')))
                || ((c == b'e' || c == b'E') && !seen_exp && self.i > start);
            if !ok {
                break;
            }
            if c == b'.' {
                seen_dot = true;
            }
            if c == b'e' || c == b'E' {
                seen_exp = true;
            }
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
}

/// The path as polylines (one per sub-path, with whether it closed),
/// curves flattened into short segments.
pub fn flatten(cmds: &[PathCmd]) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur = Point::default();
    for c in cmds {
        match *c {
            PathCmd::Move(p) => {
                out.push((vec![p], false));
                cur = p;
            }
            PathCmd::Line(p) => {
                if let Some(l) = out.last_mut() {
                    l.0.push(p);
                }
                cur = p;
            }
            PathCmd::Quad(c1, p) => {
                if let Some(l) = out.last_mut() {
                    for i in 1..=16 {
                        let t = i as f32 / 16.0;
                        let u = 1.0 - t;
                        l.0.push(Point::new(
                            u * u * cur.x + 2.0 * u * t * c1.x + t * t * p.x,
                            u * u * cur.y + 2.0 * u * t * c1.y + t * t * p.y,
                        ));
                    }
                }
                cur = p;
            }
            PathCmd::Cubic(c1, c2, p) => {
                if let Some(l) = out.last_mut() {
                    for i in 1..=24 {
                        let t = i as f32 / 24.0;
                        let u = 1.0 - t;
                        l.0.push(Point::new(
                            u * u * u * cur.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * p.x,
                            u * u * u * cur.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * p.y,
                        ));
                    }
                }
                cur = p;
            }
            PathCmd::Close => {
                if let Some(l) = out.last_mut() {
                    l.1 = true;
                    if let Some(first) = l.0.first().copied() {
                        cur = first;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_data_parses_absolute_and_relative() {
        let p = parse_path("M10 10 L 20,10 h5 v5 z").unwrap();
        assert_eq!(p[0], PathCmd::Move(Point::new(10.0, 10.0)));
        assert_eq!(p[2], PathCmd::Line(Point::new(25.0, 10.0)));
        assert_eq!(p[3], PathCmd::Line(Point::new(25.0, 15.0)));
        assert_eq!(p[4], PathCmd::Close);
        let c = parse_path("m0 0 c10 0 10 10 0 10").unwrap();
        assert_eq!(c[1], PathCmd::Cubic(Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)));
        assert!(parse_path("M 1").is_none());
        assert!(parse_path("").is_none());
        let n = parse_path("M-1.5-2.5L.5.5").unwrap();
        assert_eq!(n[1], PathCmd::Line(Point::new(0.5, 0.5)));
    }

    #[test]
    fn matrices_compose_and_invert() {
        let m = Matrix::translate(10.0, 5.0).then(&Matrix::scale(2.0, 2.0));
        assert_eq!(m.apply(Point::new(1.0, 1.0)), Point::new(12.0, 7.0));
        let inv = m.invert().unwrap();
        let p = inv.apply(m.apply(Point::new(3.0, 4.0)));
        assert!((p.x - 3.0).abs() < 1e-5 && (p.y - 4.0).abs() < 1e-5);
        assert!(Matrix::scale(0.0, 1.0).invert().is_none());
        let r = Matrix::rotate(90.0).apply(Point::new(1.0, 0.0));
        assert!((r.x).abs() < 1e-5 && (r.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn curves_flatten_to_their_end_points() {
        let f = flatten(&parse_path("M0 0 C0 10 10 10 10 0 Z").unwrap());
        assert_eq!(f.len(), 1);
        assert!(f[0].1);
        assert_eq!(*f[0].0.last().unwrap(), Point::new(10.0, 0.0));
        assert!(f[0].0.len() > 10);
    }
}
