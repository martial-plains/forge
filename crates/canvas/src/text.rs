// SPDX-License-Identifier: MIT OR Apache-2.0
//! Text measuring and wrapping that plugins and renderers agree on.
//!
//! The renderer measures with the real font; this is the estimate a
//! plugin uses to lay boxes out before anything is drawn. It errs on the
//! wide side, so text a plugin sized a box for fits when drawn.

/// The width and height of one line of `text` at `size`.
pub fn measure(text: &str, size: f32, bold: bool) -> (f32, f32) {
    let w: f32 = text.chars().map(|c| advance(c, bold)).sum::<f32>() * size;
    (w, size * 1.3)
}

fn advance(c: char, bold: bool) -> f32 {
    let base = match c {
        ' ' | 'i' | 'l' | 'j' | 't' | 'f' | 'I' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' => 0.30,
        'm' | 'w' | 'M' | 'W' | '@' => 0.85,
        c if c.is_ascii_uppercase() => 0.68,
        c if c.is_ascii() => 0.56,
        // CJK and emoji are about a full em.
        _ => 1.0,
    };
    if bold { base * 1.06 } else { base }
}

/// Break `text` into lines no wider than `max_width` (a word longer than
/// that is split). Newlines are kept. `measure_fn` gives a string's width.
pub fn wrap_with(text: &str, max_width: f32, measure_fn: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if measure_fn(&candidate) <= max_width
                || line.is_empty() && measure_fn(word) <= max_width
            {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word too long for a line is split by characters.
            let mut piece = String::new();
            for c in word.chars() {
                piece.push(c);
                if measure_fn(&piece) > max_width && piece.chars().count() > 1 {
                    piece.pop();
                    lines.push(std::mem::take(&mut piece));
                    piece.push(c);
                }
            }
            line = piece;
        }
        lines.push(line);
    }
    lines
}

/// [`wrap_with`], using [`measure`].
pub fn wrap(text: &str, size: f32, bold: bool, max_width: f32) -> Vec<String> {
    wrap_with(text, max_width, &|s| measure(s, size, bold).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_keeps_lines_inside_the_width() {
        let lines = wrap(
            "the quick brown fox jumps over the lazy dog",
            10.0,
            false,
            80.0,
        );
        assert!(lines.len() > 1);
        assert!(
            lines.iter().all(|l| measure(l, 10.0, false).0 <= 80.0),
            "{lines:?}"
        );
        assert_eq!(
            lines.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );
    }

    #[test]
    fn newlines_and_long_words() {
        assert_eq!(wrap("a\nb", 10.0, false, 100.0), ["a", "b"]);
        let l = wrap("supercalifragilistic", 10.0, false, 40.0);
        assert!(l.len() > 2 && l.iter().all(|s| measure(s, 10.0, false).0 <= 40.0 + 12.0));
        assert_eq!(wrap("", 10.0, false, 50.0), [""]);
    }
}
