// SPDX-License-Identifier: MIT OR Apache-2.0
//! Auto Fill mode: break the line as you type.
//!
//! After a self inserting character pushes the line past the fill column,
//! break it at the last space that fits, keeping the line's indentation
//! and comment marker. A character inserted with `C-q` or `C-o` does not
//! break the line, and RET does not break it either. Long runs of CJK
//! characters break between them, since they have no spaces.

use crate::buffer::Buffer;
use crate::width::{grapheme_width, graphemes, line_width};
use std::sync::OnceLock;

/// The comment/indentation prefix of a line, as the fill code sees it.
fn prefix_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^[ \t]*(?:(?://+!?|#+|;+|>+|--|%+)[ \t]*)?").expect("valid regexp")
    })
}

/// The text a continuation line carries over (indentation, comment mark).
fn prefix_of(line: &str) -> String {
    prefix_re()
        .find(line)
        .map_or("", |m| m.as_str())
        .to_string()
}

/// Is `line` one we may break? Not a quoted line, not an Org heading, not
/// a rule made of one repeated punctuation character.
fn is_breakable(line: &str, major_mode: &str) -> bool {
    let m = line.trim_start();
    if m.starts_with('>') {
        return false;
    }
    if major_mode == crate::org::MODE && m.starts_with('*') {
        return false;
    }
    let Some(first) = m.chars().next() else {
        return false;
    };
    !(['-', '=', '*', '_'].contains(&first) && m.chars().all(|c| c == first))
}

/// The char column where `line` should break, so its first part is at most
/// `fill_column` wide and ends after a space: at the last space that
/// fits, or, when there is no such space (long words, CJK text), the
/// first boundary past the column. `after` is the char column the text
/// starts at (past any prefix), so spaces inside the prefix are ignored.
/// The returned column is where the newline goes; the space there is
/// dropped. None when the line fits.
pub fn break_column(line: &str, fill_column: usize, after: usize) -> Option<usize> {
    let clusters: Vec<(usize, &str)> = graphemes(line).collect();
    let mut col = 0usize;
    let mut last_space: Option<usize> = None;
    let mut first_past: Option<usize> = None;
    for (i, (c, g)) in clusters.iter().enumerate() {
        let w = grapheme_width(g, col);
        if col + w > fill_column && first_past.is_none() {
            first_past = Some(*c);
        }
        if *g == " " && i > 0 && col <= fill_column && *c >= after {
            // Break here: everything before this space stays on the line.
            last_space = Some(*c);
        }
        col += w;
    }
    if col <= fill_column {
        return None;
    }
    last_space.or(first_past)
}

/// Auto Fill after a typed character: break the line when it has grown
/// past the fill column. Returns whether it broke. (`space` is true when
/// the typed character was a space, which always counts, even one past
/// the column; any other character is measured without trailing blanks.)
pub fn maybe_break(buf: &mut Buffer, space: bool) -> bool {
    let p = buf.point();
    let line = buf.line(p.line);
    if !is_breakable(&line, buf.major_mode()) {
        return false;
    }
    let fill = buf.fill_column;
    let prefix = prefix_of(&line);
    // With a comment marker, break a bit later so the marker has room.
    let threshold = if prefix.trim().is_empty() {
        fill
    } else {
        fill + line_width(&prefix).saturating_sub(prefix.chars().count())
    };
    let measured = if space {
        line.clone()
    } else {
        line.trim_end().to_string()
    };
    if line_width(&measured) <= threshold {
        return false;
    }
    let Some(at) = break_column(&line, fill, prefix.chars().count()) else {
        return false;
    };
    // Buffer offsets are char offsets, so char columns are offsets here.
    let line_start = buf.pos_to_offset(crate::Pos::new(p.line, 0));
    // The break column is at the space (or just past the column for a long
    // word): drop the run of spaces there and start the new line with the
    // prefix.
    let chars: Vec<char> = line.chars().collect();
    let mut take = at;
    while chars.get(take) == Some(&' ') {
        take += 1;
    }
    let replacement = format!("\n{prefix}");
    let added = replacement.chars().count();
    let removed = take - at;
    // Point before the break is unmoved; at or after it, it shifts left by
    // the removed spaces and right by the inserted newline and prefix.
    let point = if p.col <= at {
        p.col
    } else {
        (p.col - removed.min(p.col - at)) + added
    };
    buf.replace_offsets(line_start + at, line_start + take, &replacement);
    buf.set_offset(line_start + point);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_at_the_last_space_that_fits() {
        // Break at the space after "two" (column 7), dropping it.
        assert_eq!(break_column("one two three four", 12, 0), Some(7));
        assert_eq!(break_column("no break here", 40, 0), None);
        // A long word with no space: break at the column.
        assert_eq!(break_column("supercalifragilistic", 8, 0), Some(8));
    }

    #[test]
    fn breaks_between_wide_characters() {
        // Each CJK character is two cells wide.
        let line = "中文字符测试";
        assert_eq!(break_column(line, 4, 0), Some(2));
        // Latin space wins when there is one.
        assert_eq!(break_column("中文 字符", 5, 0), Some(2));
    }

    #[test]
    fn keeps_the_prefix_and_skips_headings_and_quotes() {
        assert_eq!(prefix_of("    // alpha beta"), "    // ");
        assert!(!is_breakable("> cited text", "text-mode"));
        assert!(!is_breakable("* Heading", crate::org::MODE));
        assert!(!is_breakable("-----", "text-mode"));
        assert!(is_breakable("* list item", "text-mode"));
    }

    #[test]
    fn maybe_break_moves_point_to_the_typed_character() {
        let mut b = Buffer::new("t");
        b.set_contents("one two three");
        b.fill_column = 8;
        b.set_offset(13); // end of line
        assert!(maybe_break(&mut b, true));
        assert_eq!(b.get_contents(), "one two\nthree");
        assert_eq!(b.point_offset(), 13); // after "three"
    }

    #[test]
    fn maybe_break_keeps_a_comment_prefix() {
        let mut b = Buffer::new("t");
        b.set_contents("    // alpha beta");
        b.fill_column = 12;
        b.set_offset(19);
        assert!(maybe_break(&mut b, true));
        assert_eq!(b.get_contents(), "    // alpha\n    // beta");
    }

    #[test]
    fn maybe_break_breaks_a_long_word_at_the_column() {
        let mut b = Buffer::new("t");
        b.set_contents("aaaaaaaaaaaaa");
        b.fill_column = 8;
        b.set_offset(13);
        assert!(maybe_break(&mut b, false));
        assert_eq!(b.get_contents(), "aaaaaaaa\naaaaa");
    }

    #[test]
    fn maybe_break_splits_multibyte_lines_between_characters() {
        let mut b = Buffer::new("t");
        b.set_contents("中文字符测试");
        b.fill_column = 4;
        b.set_offset(6);
        assert!(maybe_break(&mut b, false));
        assert_eq!(b.get_contents(), "中文\n字符测试");
    }
}
