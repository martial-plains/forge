// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paragraph filling (M-q) and line joining (M-^).
//!
//! Filling keeps a paragraph's prefix: indentation, a comment marker
//! repeated on every line ("// ", "# ", "> ", ";; "), or a list bullet
//! whose continuation lines are indented to line up under the text.

use crate::buffer::Buffer;
use crate::width::line_width;
use regex::Regex;
use std::sync::OnceLock;

fn comment_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[ \t]*(?:(?://+!?|#+|;+|>+|--|%+)[ \t]*)?").expect("valid regex")
    })
}

fn bullet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[ \t]*(?:[-*+•]|\d+[.)])[ \t]+").expect("valid regex"))
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The comment marker in a prefix, without whitespace ("//", "#", "").
fn marker(prefix: &str) -> &str {
    prefix.trim()
}

/// The paragraph at `line`, or the next one if `line` is blank: an
/// inclusive range of non-blank lines.
pub fn paragraph_at(buf: &Buffer, line: usize) -> Option<(usize, usize)> {
    let n = buf.line_count();
    let mut start = line.min(n - 1);
    while is_blank(&buf.line(start)) {
        start += 1;
        if start >= n {
            return None;
        }
    }
    while start > 0 && !is_blank(&buf.line(start - 1)) {
        start -= 1;
    }
    let mut end = start;
    while end + 1 < n && !is_blank(&buf.line(end + 1)) {
        end += 1;
    }
    Some((start, end))
}

/// Refill paragraph lines so none is wider than `fill_column` (a single
/// word longer than that gets a line to itself).
pub fn fill_lines(lines: &[String], fill_column: usize) -> Vec<String> {
    fill_lines_inner(lines, fill_column, false)
}

/// Like `fill_lines`, but justify: pad the gaps so every line but the last
/// reaches exactly `fill_column` (unless a line holds a single word).
pub fn fill_lines_justified(lines: &[String], fill_column: usize) -> Vec<String> {
    fill_lines_inner(lines, fill_column, true)
}

fn fill_lines_inner(lines: &[String], fill_column: usize, justify: bool) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let re = comment_prefix_re();
    let p0 = re.find(&lines[0]).map_or("", |m| m.as_str()).to_string();
    let shared_marker = !marker(&p0).is_empty()
        && lines
            .iter()
            .all(|l| marker(re.find(l).map_or("", |m| m.as_str())) == marker(&p0));

    let (first, rest) = if shared_marker {
        let p1 = lines
            .get(1)
            .and_then(|l| re.find(l))
            .map_or(p0.clone(), |m| m.as_str().to_string());
        (p0.clone(), p1)
    } else if let Some(m) = bullet_re().find(&lines[0]) {
        let b = m.as_str().to_string();
        let hang = " ".repeat(line_width(&b));
        (b, hang)
    } else {
        let indent = |l: &str| {
            l.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        };
        let first = indent(&lines[0]);
        let rest = lines.get(1).map_or(first.clone(), |l| indent(l));
        (first, rest)
    };

    let mut words: Vec<&str> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let body = if i == 0 {
            &l[first.len().min(l.len())..]
        } else if shared_marker {
            &l[re.find(l).map_or(0, |m| m.end())..]
        } else {
            l.as_str()
        };
        words.extend(body.split_whitespace());
    }

    // Group words into lines no wider than fill_column.
    let mut rows: Vec<Vec<&str>> = Vec::new();
    let mut row: Vec<&str> = Vec::new();
    let mut cur = first.clone();
    for w in words {
        let candidate = line_width(&cur) + usize::from(!row.is_empty()) + line_width(w);
        if !row.is_empty() && candidate > fill_column {
            rows.push(std::mem::take(&mut row));
            cur = rest.clone();
        }
        if !row.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
        row.push(w);
    }
    rows.push(row);

    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let prefix = if i == 0 { &first } else { &rest };
        let last = i + 1 == rows.len();
        if justify && !last && row.len() > 1 {
            out.push(justify_row(prefix, row, fill_column));
        } else {
            let mut line = prefix.clone();
            for (j, w) in row.iter().enumerate() {
                if j > 0 {
                    line.push(' ');
                }
                line.push_str(w);
            }
            out.push(line);
        }
    }
    out
}

/// One justified line: the words joined and the gaps widened so the line is
/// `fill_column` wide (only when that many spaces can be added).
fn justify_row(prefix: &str, words: &[&str], fill_column: usize) -> String {
    let gap = words.len() - 1;
    let text: usize = words.iter().map(|w| line_width(w)).sum();
    let base = line_width(prefix) + text;
    if base + gap > fill_column {
        // Cannot fit even single spaces: leave it as it is.
        let mut line = prefix.to_string();
        for (j, w) in words.iter().enumerate() {
            if j > 0 {
                line.push(' ');
            }
            line.push_str(w);
        }
        return line;
    }
    let extra = fill_column - base - gap;
    let mut line = prefix.to_string();
    for (j, w) in words.iter().enumerate() {
        if j > 0 {
            // Spread the extra spaces left to right.
            let add = extra / gap + usize::from(j <= extra % gap);
            line.push_str(&" ".repeat(1 + add));
        }
        line.push_str(w);
    }
    line
}

/// Join paragraph lines into one, as `M-^` ing them all together would.
/// Returns the joined text (still carrying the first line's prefix).
pub fn unfill_lines(lines: &[String]) -> Option<String> {
    let first = lines.first()?;
    let mut out = first.clone();
    for l in &lines[1..] {
        let prev = out.trim_end();
        let rest = l.trim_start();
        let space =
            !(prev.is_empty() || rest.is_empty() || rest.starts_with(')') || prev.ends_with('('));
        out = format!("{prev}{}{rest}", if space { " " } else { "" });
    }
    Some(out)
}

/// Center `line` in `fill_column`: as much left padding as right. Blank
/// lines are left alone.
pub fn center_line(line: &str, fill_column: usize) -> String {
    let text = line.trim();
    if text.is_empty() {
        return line.to_string();
    }
    let width = line_width(text);
    if width >= fill_column {
        return text.to_string();
    }
    let left = (fill_column - width) / 2;
    format!("{}{}", " ".repeat(left), text)
}

/// Fill the paragraph at `line` in place. Point stays next to the same
/// word. Returns false if there is no paragraph there.
pub fn fill_paragraph(buf: &mut Buffer, line: usize) -> bool {
    let Some((start, end)) = paragraph_at(buf, line) else {
        return false;
    };
    fill_line_range(buf, start, end);
    true
}

/// Fill lines `start..=end` as one paragraph.
pub fn fill_line_range(buf: &mut Buffer, start: usize, end: usize) {
    fill_line_range_inner(buf, start, end, false);
}

/// Fill lines `start..=end` as one paragraph, justifying them too.
pub fn fill_line_range_justified(buf: &mut Buffer, start: usize, end: usize) {
    fill_line_range_inner(buf, start, end, true);
}

/// Fill lines `start..=end`, justifying when `justify` is true.
pub fn fill_line_range_arg(buf: &mut Buffer, start: usize, end: usize, justify: bool) {
    fill_line_range_inner(buf, start, end, justify);
}

fn fill_line_range_inner(buf: &mut Buffer, start: usize, end: usize, justify: bool) {
    let lines: Vec<String> = (start..=end).map(|l| buf.line(l)).collect();
    let filled = if justify {
        fill_lines_justified(&lines, buf.fill_column)
    } else {
        fill_lines(&lines, buf.fill_column)
    };
    if filled == lines {
        return;
    }
    // Remember point as "n non-space characters into the paragraph".
    let a = crate::Pos::new(start, 0);
    let b = crate::Pos::new(end, buf.line_len(end));
    let p = buf.point();
    let anchor = (p >= a && p <= b).then(|| {
        let before = buf
            .text_range(a, p)
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let on_word = buf.char_at(p).is_some_and(|c| !c.is_whitespace());
        (before, on_word)
    });
    let text = filled.join("\n");
    buf.replace_range(a, b, &text);
    let base = buf.pos_to_offset(a);
    match anchor {
        Some((n, on_word)) => {
            let chars: Vec<char> = text.chars().collect();
            let mut seen = 0;
            let mut i = 0;
            while i < chars.len() && seen < n {
                if !chars[i].is_whitespace() {
                    seen += 1;
                }
                i += 1;
            }
            // Point was on a word: skip the (new) whitespace before it.
            while on_word && i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            buf.set_offset(base + i);
        }
        None => buf.set_offset(base + text.chars().count()),
    }
}

/// Unfill lines `start..=end`: join them into one line, keeping the first
/// line's prefix. Returns the paragraph end line, or None if `start > end`.
pub fn unfill_line_range(buf: &mut Buffer, start: usize, end: usize) -> Option<usize> {
    if start >= end || end >= buf.line_count() {
        return None;
    }
    let lines: Vec<String> = (start..=end).map(|l| buf.line(l)).collect();
    let joined = unfill_lines(&lines)?;
    let a = crate::Pos::new(start, 0);
    let b = crate::Pos::new(end, buf.line_len(end));
    let in_para = buf.point() >= a && buf.point() <= b;
    let rel = buf.pos_to_offset(buf.point()) - buf.pos_to_offset(a);
    buf.replace_range(a, b, &joined);
    if in_para {
        let base = buf.pos_to_offset(a);
        buf.set_offset(base + rel.min(joined.chars().count()));
    }
    Some(start)
}

/// Unfill the paragraph at `line` (join its lines). Point stays in the
/// paragraph. Returns false if there is no paragraph there.
pub fn unfill_paragraph(buf: &mut Buffer, line: usize) -> bool {
    let Some((start, end)) = paragraph_at(buf, line) else {
        return false;
    };
    unfill_line_range(buf, start, end).is_some()
}

/// Center `line` in the fill column. Point on that line shifts with the
/// text (by the added leading spaces). Returns false for a blank line.
pub fn center_line_at(buf: &mut Buffer, line: usize) -> bool {
    if line >= buf.line_count() {
        return false;
    }
    let old = buf.line(line);
    let new = center_line(&old, buf.fill_column);
    if old.trim().is_empty() {
        return false;
    }
    if new == old {
        return true;
    }
    let lead = new.chars().take_while(|c| *c == ' ').count() as isize
        - old.chars().take_while(|c| *c == ' ').count() as isize;
    let on_line = buf.point().line == line;
    let col = buf.point().col;
    let a = crate::Pos::new(line, 0);
    let b = crate::Pos::new(line, buf.line_len(line));
    buf.replace_range(a, b, &new);
    if on_line {
        let base = buf.pos_to_offset(a);
        let col = (col as isize + lead)
            .max(0)
            .min(new.chars().count() as isize) as usize;
        buf.set_offset(base + col);
    }
    true
}

/// M-^: join line `line` to the previous one, leaving one space between
/// them (none before ")" or after "(" or at an empty line). Point goes to
/// the join. Returns false on the first line.
pub fn join_line(buf: &mut Buffer, line: usize) -> bool {
    if line == 0 || line >= buf.line_count() {
        return false;
    }
    let prev = buf.line(line - 1);
    let cur = buf.line(line);
    let keep = prev.trim_end().chars().count();
    let skip = cur.chars().take_while(|c| c.is_whitespace()).count();
    let rest = &cur.trim_start();
    let space = !(prev.trim_end().is_empty()
        || rest.is_empty()
        || rest.starts_with(')')
        || prev.trim_end().ends_with('('));
    let a = crate::Pos::new(line - 1, keep);
    let b = crate::Pos::new(line, skip);
    buf.replace_range(a, b, if space { " " } else { "" });
    buf.set_point(a);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn fills_plain_text() {
        let out = fill_lines(&v(&["one two three", "four five six seven"]), 14);
        assert_eq!(out, v(&["one two three", "four five six", "seven"]));
    }

    #[test]
    fn keeps_comment_prefixes_and_indentation() {
        let out = fill_lines(&v(&["    // alpha beta", "    // gamma delta epsilon"]), 22);
        assert_eq!(
            out,
            v(&["    // alpha beta", "    // gamma delta", "    // epsilon"])
        );
        let out = fill_lines(&v(&["  a b c d e f"]), 6);
        assert_eq!(out, v(&["  a b", "  c d", "  e f"]));
        let out = fill_lines(&v(&["> quoted text here"]), 10);
        assert_eq!(out, v(&["> quoted", "> text", "> here"]));
    }

    #[test]
    fn hangs_bullets() {
        let out = fill_lines(&v(&["- first item that wraps"]), 12);
        assert_eq!(out, v(&["- first item", "  that wraps"]));
        let out = fill_lines(&v(&["10. numbered list item"]), 12);
        assert_eq!(out, v(&["10. numbered", "    list", "    item"]));
    }

    #[test]
    fn long_words_get_their_own_line() {
        let out = fill_lines(&v(&["a supercalifragilistic b"]), 8);
        assert_eq!(out, v(&["a", "supercalifragilistic", "b"]));
    }

    #[test]
    fn justify_pads_to_the_column_except_the_last_line() {
        let out = fill_lines_justified(&v(&["one two three four"]), 11);
        assert_eq!(out, v(&["one     two", "three four"]));
        assert_eq!(line_width(&out[0]), 11);
    }

    #[test]
    fn unfill_joins_lines_keeping_the_prefix() {
        // Like repeated M-^: no fill prefix, so the text simply meets.
        let out = unfill_lines(&v(&["alpha beta", "gamma delta"])).unwrap();
        assert_eq!(out, "alpha beta gamma delta");
        assert_eq!(
            unfill_lines(&v(&["foo(", "  bar)", "  baz"])).unwrap(),
            "foo(bar) baz"
        );
    }

    #[test]
    fn centers_a_line_in_the_fill_column() {
        assert_eq!(center_line("hello", 11), "   hello");
        assert_eq!(center_line("  hi  ", 6), "  hi");
        assert_eq!(center_line("toolongforcol", 4), "toolongforcol");
    }

    #[test]
    fn unfill_and_center_in_a_buffer() {
        let mut b = Buffer::new("t");
        b.set_contents("one two\nthree four\n\nnext");
        b.fill_column = 20;
        assert!(unfill_paragraph(&mut b, 1));
        assert_eq!(b.get_contents(), "one two three four\n\nnext");
        assert!(center_line_at(&mut b, 0));
        assert_eq!(b.get_contents(), " one two three four\n\nnext");
        assert!(!center_line_at(&mut b, 1));
    }

    #[test]
    fn centering_a_line_that_is_already_wider_than_the_column() {
        let mut b = Buffer::new("t");
        b.set_contents("  abcdefghij");
        b.fill_column = 10;
        b.set_point(Pos::new(0, 4));
        assert!(center_line_at(&mut b, 0));
        assert_eq!(b.get_contents(), "abcdefghij");
        assert_eq!(
            b.point(),
            Pos::new(0, 2),
            "the point keeps its place in the text"
        );
    }

    #[test]
    fn unfilling_keeps_the_point_on_its_word() {
        let mut b = Buffer::new("t");
        b.set_contents("abc\ndef");
        b.set_point(Pos::new(1, 1));
        assert!(unfill_paragraph(&mut b, 0));
        assert_eq!(b.get_contents(), "abc def");
        assert_eq!(b.point(), Pos::new(0, 5), "the 'e' of 'def'");
    }

    #[test]
    fn fill_lines_justified_in_buffer() {
        let mut b = Buffer::new("t");
        b.set_contents("one two three four\nfive six seven");
        b.fill_column = 11;
        b.set_point(Pos::new(0, 0));
        fill_line_range_justified(&mut b, 0, 1);
        assert_eq!(
            b.get_contents(),
            "one     two\nthree  four\nfive    six\nseven"
        );
    }

    #[test]
    fn fill_paragraph_in_buffer_keeps_point_on_its_word() {
        let mut b = Buffer::new("t");
        b.set_contents("intro\n\none two\nthree four five\n\nnext");
        b.fill_column = 9;
        b.set_point(Pos::new(3, 6)); // on "four"
        assert!(fill_paragraph(&mut b, 3));
        assert_eq!(
            b.get_contents(),
            "intro\n\none two\nthree\nfour five\n\nnext"
        );
        assert_eq!(b.point(), Pos::new(4, 0));
        assert_eq!(
            paragraph_at(&b, 1),
            Some((2, 4)),
            "blank line: the next paragraph"
        );
    }

    #[test]
    fn join_lines() {
        let mut b = Buffer::new("t");
        b.set_contents("foo(  \n   bar\n  )");
        assert!(join_line(&mut b, 1));
        assert_eq!(b.get_contents(), "foo(bar\n  )");
        assert!(join_line(&mut b, 1));
        assert_eq!(b.get_contents(), "foo(bar)");
        assert_eq!(b.point(), Pos::new(0, 7));
        b.set_contents("a  \n  b");
        join_line(&mut b, 1);
        assert_eq!(b.get_contents(), "a b");
        assert!(!join_line(&mut b, 0));
    }
}
