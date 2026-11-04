// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rectangles: the columns between mark and point, on every line between
//! them (C-x r k, C-x r y, C-x r t, ...).
//!
//! Columns are display columns, so tabs and wide characters line up the
//! way they look. A tab or wide character that straddles the left edge
//! counts as inside the rectangle; one straddling the right edge doesn't.

use crate::buffer::{Buffer, Pos};
use crate::width::{col_at_display, display_col, line_width};

/// Lines and display columns covered by a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub first_line: usize,
    pub last_line: usize,
    pub left: usize,
    pub right: usize,
}

impl Bounds {
    pub fn new(buf: &Buffer, a: Pos, b: Pos) -> Self {
        let xa = display_col(&buf.line(a.line), a.col);
        let xb = display_col(&buf.line(b.line), b.col);
        Self {
            first_line: a.line.min(b.line),
            last_line: a.line.max(b.line),
            left: xa.min(xb),
            right: xa.max(xb),
        }
    }

    pub fn width(&self) -> usize {
        self.right - self.left
    }

    fn lines(&self) -> std::ops::RangeInclusive<usize> {
        self.first_line..=self.last_line
    }
}

/// Char columns of the rectangle's part of `line`.
fn segment(line: &str, b: &Bounds) -> (usize, usize) {
    (col_at_display(line, b.left), col_at_display(line, b.right))
}

fn pad(s: &mut String, width: usize) {
    let w = line_width(s);
    if w < width {
        s.push_str(&" ".repeat(width - w));
    }
}

/// Extend `line` with spaces so it reaches display column `x`.
fn pad_line_to(buf: &mut Buffer, line: usize, x: usize) {
    let text = buf.line(line);
    let w = line_width(&text);
    if w < x {
        let end = Pos::new(line, buf.line_len(line));
        buf.replace_range(end, end, &" ".repeat(x - w));
    }
}

/// The rectangle's text, one string per line, padded to its width.
pub fn extract(buf: &Buffer, a: Pos, b: Pos) -> Vec<String> {
    let r = Bounds::new(buf, a, b);
    r.lines()
        .map(|l| {
            let text = buf.line(l);
            let (s, e) = segment(&text, &r);
            let mut part: String = text.chars().skip(s).take(e - s).collect();
            pad(&mut part, r.width());
            part
        })
        .collect()
}

/// Delete the rectangle; returns its text (as [`extract`]). Point goes to
/// the top-left corner.
pub fn delete(buf: &mut Buffer, a: Pos, b: Pos) -> Vec<String> {
    let text = extract(buf, a, b);
    let r = Bounds::new(buf, a, b);
    for l in r.lines() {
        let (s, e) = segment(&buf.line(l), &r);
        buf.delete_range(Pos::new(l, s), Pos::new(l, e));
    }
    goto_corner(buf, r.first_line, r.left);
    text
}

/// Replace the rectangle's text with spaces (C-x r c).
pub fn clear(buf: &mut Buffer, a: Pos, b: Pos) {
    let r = Bounds::new(buf, a, b);
    for l in r.lines() {
        let line = buf.line(l);
        let (s, e) = segment(&line, &r);
        if s == e {
            continue;
        }
        let w = display_col(&line, e) - display_col(&line, s);
        buf.replace_range(Pos::new(l, s), Pos::new(l, e), &" ".repeat(w));
    }
    goto_corner(buf, r.first_line, r.left);
}

/// Shift text right, filling the rectangle with spaces (C-x r o).
pub fn open(buf: &mut Buffer, a: Pos, b: Pos) {
    let r = Bounds::new(buf, a, b);
    for l in r.lines() {
        pad_line_to(buf, l, r.left);
        let s = col_at_display(&buf.line(l), r.left);
        let p = Pos::new(l, s);
        buf.replace_range(p, p, &" ".repeat(r.width()));
    }
    goto_corner(buf, r.first_line, r.left);
}

/// Replace each line of the rectangle with `s` (C-x r t). Point ends
/// after `s` on the last line.
pub fn replace_each(buf: &mut Buffer, a: Pos, b: Pos, s: &str) {
    let r = Bounds::new(buf, a, b);
    for l in r.lines() {
        pad_line_to(buf, l, r.left);
        let (from, to) = segment(&buf.line(l), &r);
        buf.replace_range(Pos::new(l, from), Pos::new(l, to), s);
    }
}

/// Insert `lines` as a rectangle with its top-left corner at `at`,
/// adding lines at the end of the buffer if needed (C-x r y). Point ends
/// after the last inserted line's text.
pub fn insert(buf: &mut Buffer, at: Pos, lines: &[String]) {
    let x = display_col(&buf.line(at.line), at.col);
    for (i, text) in lines.iter().enumerate() {
        let l = at.line + i;
        if l >= buf.line_count() {
            let end = buf.end();
            buf.replace_range(end, end, "\n");
        }
        pad_line_to(buf, l, x);
        let c = col_at_display(&buf.line(l), x);
        let p = Pos::new(l, c);
        buf.replace_range(p, p, text);
    }
}

/// Number the lines of the rectangle, inserting "1 ", "2 ", ... at its
/// left edge (C-x r N). Numbers are right-aligned.
pub fn number_lines(buf: &mut Buffer, a: Pos, b: Pos, first: i64) {
    let r = Bounds::new(buf, a, b);
    let last = first + (r.last_line - r.first_line) as i64;
    let w = first.to_string().len().max(last.to_string().len());
    let labels: Vec<String> = (first..=last).map(|n| format!("{n:>w$} ")).collect();
    for (l, label) in r.lines().zip(labels) {
        pad_line_to(buf, l, r.left);
        let c = col_at_display(&buf.line(l), r.left);
        let p = Pos::new(l, c);
        buf.replace_range(p, p, &label);
    }
    goto_corner(buf, r.first_line, r.left);
}

fn goto_corner(buf: &mut Buffer, line: usize, x: usize) {
    let c = col_at_display(&buf.line(line), x);
    buf.set_point(Pos::new(line, c));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("t");
        b.set_contents(text);
        b
    }

    const TEXT: &str = "abcdef\nab\nabcdefgh";

    #[test]
    fn extract_pads_short_lines() {
        let b = buf(TEXT);
        assert_eq!(
            extract(&b, Pos::new(0, 1), Pos::new(2, 4)),
            ["bcd", "b  ", "bcd"]
        );
    }

    #[test]
    fn delete_and_reinsert() {
        let mut b = buf(TEXT);
        let killed = delete(&mut b, Pos::new(0, 1), Pos::new(2, 4));
        assert_eq!(b.get_contents(), "aef\na\naefgh");
        assert_eq!(b.point(), Pos::new(0, 1));
        insert(&mut b, Pos::new(0, 1), &killed);
        assert_eq!(b.get_contents(), "abcdef\nab  \nabcdefgh");
    }

    #[test]
    fn open_clear_string_and_number() {
        let mut b = buf(TEXT);
        open(&mut b, Pos::new(0, 1), Pos::new(2, 3));
        assert_eq!(b.get_contents(), "a  bcdef\na  b\na  bcdefgh");

        let mut b = buf(TEXT);
        clear(&mut b, Pos::new(0, 1), Pos::new(2, 3));
        assert_eq!(b.get_contents(), "a  def\na \na  defgh");

        let mut b = buf(TEXT);
        replace_each(&mut b, Pos::new(0, 2), Pos::new(2, 4), ">>");
        assert_eq!(b.get_contents(), "ab>>ef\nab>>\nab>>efgh");
        assert_eq!(b.point(), Pos::new(2, 4));

        let mut b = buf("x\ny\nz");
        number_lines(&mut b, Pos::new(0, 0), Pos::new(2, 0), 9);
        assert_eq!(b.get_contents(), " 9 x\n10 y\n11 z");
    }

    #[test]
    fn insert_adds_lines_and_uses_display_columns() {
        let mut b = buf("\tx");
        insert(&mut b, Pos::new(0, 1), &["A".to_string(), "B".to_string()]);
        assert_eq!(b.get_contents(), "\tAx\n        B");
        // A wide character left of the rectangle is two columns.
        let b = buf("漢abc\nxyzw");
        assert_eq!(extract(&b, Pos::new(0, 1), Pos::new(1, 3)), ["a", "z"]);
    }
}
