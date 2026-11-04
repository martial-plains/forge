// SPDX-License-Identifier: MIT OR Apache-2.0
//! Grapheme clusters and display columns.
//!
//! Point moves by grapheme cluster (what a reader sees as one character:
//! "e" + combining accent, a flag, a family emoji), and columns on screen
//! are display columns: tabs go to the next tab stop, CJK and emoji take
//! two cells, control characters show as ^X.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const TAB_WIDTH: usize = 8;

/// Screen width of one grapheme cluster that starts at display column `x`.
pub fn grapheme_width(g: &str, x: usize) -> usize {
    match g {
        "\t" => TAB_WIDTH - x % TAB_WIDTH,
        _ if g.chars().next().is_some_and(char::is_control) => 2,
        _ => g.width(),
    }
}

/// (char column, grapheme) for each cluster in `line`.
pub fn graphemes(line: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut col = 0;
    line.graphemes(true).map(move |g| {
        let start = col;
        col += g.chars().count();
        (start, g)
    })
}

/// Char columns where grapheme clusters start, plus the line length.
pub fn boundaries(line: &str) -> Vec<usize> {
    let mut out: Vec<usize> = graphemes(line).map(|(c, _)| c).collect();
    out.push(line.chars().count());
    out
}

/// Snap a char column back to the start of the cluster containing it.
pub fn snap(line: &str, col: usize) -> usize {
    let mut end = 0;
    for (c, g) in graphemes(line) {
        end = c + g.chars().count();
        if col < end {
            return c;
        }
    }
    col.min(end)
}

/// Display column of char column `col`.
pub fn display_col(line: &str, col: usize) -> usize {
    let mut x = 0;
    for (c, g) in graphemes(line) {
        if c >= col {
            break;
        }
        x += grapheme_width(g, x);
    }
    x
}

/// Char column of the cluster that covers display column `x` (or the end
/// of the line if it is shorter).
pub fn col_at_display(line: &str, x: usize) -> usize {
    let mut cur = 0;
    for (c, g) in graphemes(line) {
        let w = grapheme_width(g, cur);
        if cur + w > x {
            return c;
        }
        cur += w;
    }
    line.chars().count()
}

/// Display width of a whole line.
pub fn line_width(line: &str) -> usize {
    display_col(line, usize::MAX)
}

/// The longest start of `s` at most `cells` wide.
pub fn truncate(s: &str, cells: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for (_, g) in graphemes(s) {
        let gw = grapheme_width(g, w);
        if w + gw > cells {
            break;
        }
        w += gw;
        out.push_str(g);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clusters_and_columns() {
        let s = "ae\u{301}🇹🇹x";
        assert_eq!(boundaries(s), vec![0, 1, 3, 5, 6]);
        assert_eq!(snap(s, 2), 1);
        assert_eq!(snap(s, 4), 3);
        assert_eq!(display_col(s, 3), 2);
        assert_eq!(display_col(s, 5), 4);
        assert_eq!(col_at_display(s, 3), 3);
    }

    #[test]
    fn tabs_and_wide_chars() {
        assert_eq!(display_col("\tx", 1), 8);
        assert_eq!(display_col("ab\tx", 3), 8);
        assert_eq!(display_col("漢字x", 2), 4);
        assert_eq!(col_at_display("漢字x", 3), 1);
        assert_eq!(col_at_display("ab", 10), 2);
        assert_eq!(display_col("a\u{1}b", 2), 3);
    }
}
