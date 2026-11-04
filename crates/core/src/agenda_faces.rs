// SPDX-License-Identifier: MIT OR Apache-2.0
//! Colours for the Org agenda buffer, a line at a time: the title, the day headers (weekends
//! apart), each entry's category, time, "Scheduled:" or "In 2 d.:"
//! leader, TODO keyword, priority and tags, and the time grid and "now"
//! line.

use crate::syntax::SyntaxFace;

pub const MODE: &str = "org-agenda-mode";

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

fn is_time(tok: &str) -> bool {
    let t = tok.trim_end_matches('.');
    let part = |p: &str| {
        let Some((h, m)) = p.split_once(':') else { return false };
        (1..=2).contains(&h.len())
            && h.chars().all(|c| c.is_ascii_digit())
            && m.len() == 2
            && m.chars().all(|c| c.is_ascii_digit())
    };
    !t.is_empty() && t.split('-').all(part)
}

fn is_tags(tok: &str) -> bool {
    tok.len() > 2
        && tok.starts_with(':')
        && tok.ends_with(':')
        && tok.chars().all(|c| c.is_alphanumeric() || "_@#%:".contains(c))
}

fn is_keyword(tok: &str) -> bool {
    tok.len() >= 3 && tok.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c == '-')
}

/// A face per character of `line` (without its newline).
pub fn line_faces(line: &str) -> Vec<Option<SyntaxFace>> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = vec![None; chars.len()];
    let mut paint = |from: usize, to: usize, face: SyntaxFace| {
        for slot in &mut out[from..to.min(chars.len())] {
            *slot = Some(face);
        }
    };
    // Tokens with their char ranges.
    let mut toks: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let s = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        toks.push((s, i, chars[s..i].iter().collect()));
    }
    let Some((_, _, first)) = toks.first() else { return out };
    let indented = chars.first().is_some_and(|c| c.is_whitespace());

    // The view's title and the list headings.
    if !indented && DAYS.contains(&first.as_str()) {
        let weekend = matches!(first.as_str(), "Saturday" | "Sunday");
        paint(0, chars.len(), if weekend { SyntaxFace::Constant } else { SyntaxFace::Function });
        return out;
    }
    if !indented && line.trim_end().ends_with(':') {
        paint(0, chars.len(), SyntaxFace::Keyword);
        return out;
    }

    // An entry or a grid row: category, time, leader, keyword, priority,
    // title, tags.
    let mut n = 0;
    if indented && toks[0].2.ends_with(':') && !is_time(toks[0].2.trim_end_matches(':')) {
        paint(toks[0].0, toks[0].1, SyntaxFace::Property);
        n = 1;
    }
    let mut seen_keyword = false;
    while n < toks.len() {
        let (s, e, t) = (toks[n].0, toks[n].1, toks[n].2.as_str());
        if is_time(t) {
            paint(s, e, SyntaxFace::Type);
        } else if t.chars().all(|c| c == '-') && t.len() >= 3 {
            // The grid's rule, and what is left of the "now" line.
            paint(s, e, SyntaxFace::Comment);
        } else if t == "now" || t == "\u{2190}" {
            paint(s, chars.len(), SyntaxFace::Type);
            break;
        } else if t.starts_with("Sched.") || t == "Scheduled:" {
            paint(s, e, SyntaxFace::String);
        } else if t == "Deadline:" || t.ends_with("ago:") || t.starts_with("Past") {
            paint(s, e, SyntaxFace::Constant);
        } else if t == "In" && toks.get(n + 2).is_some_and(|x| x.2.ends_with(':')) {
            // "In   2 d.:" is three tokens.
            paint(s, toks[n + 2].1, SyntaxFace::Attribute);
            n += 2;
        } else if !seen_keyword && is_keyword(t) {
            paint(s, e, if t == "DONE" { SyntaxFace::String } else { SyntaxFace::Constant });
            seen_keyword = true;
        } else if t.starts_with("[#") && t.ends_with(']') {
            paint(s, e, SyntaxFace::Attribute);
        } else if is_tags(t) && n + 1 == toks.len() {
            paint(s, e, SyntaxFace::Comment);
        } else if is_tags(t) {
            // A tag-looking word inside the title: leave it.
        } else {
            // The entry's own text: default colour, up to the tags.
            seen_keyword = true;
        }
        n += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxFace::*;

    fn at(line: &str, word: &str) -> Option<SyntaxFace> {
        let i = line.find(word).unwrap();
        line_faces(line)[line[..i].chars().count()]
    }

    #[test]
    fn titles_days_and_weekends_differ() {
        assert_eq!(at("Week-agenda (W45):", "Week"), Some(Keyword));
        assert_eq!(at("Tuesday     3 November 2026", "Tuesday"), Some(Function));
        assert_eq!(at("Sunday      8 November 2026", "8 Nov"), Some(Constant));
    }

    #[test]
    fn an_entry_is_coloured_part_by_part() {
        let l = "  church:     10:00 ------ Scheduled:  TODO Pick the lime :home:";
        assert_eq!(at(l, "church"), Some(Property));
        assert_eq!(at(l, "10:00"), Some(Type));
        assert_eq!(at(l, "Scheduled"), Some(String));
        assert_eq!(at(l, "TODO"), Some(Constant));
        assert_eq!(at(l, "Pick"), None);
        assert_eq!(at(l, ":home:"), Some(Comment));
        let d = "  tasks:      In   2 d.:  TODO [#A] Write report";
        assert_eq!(at(d, "2 d"), Some(Attribute));
        assert_eq!(at(d, "[#A]"), Some(Attribute));
        // A capitalised word in the title is not a keyword.
        assert_eq!(at("  tasks:      TODO Send IBM the PDF", "IBM"), None);
    }

    #[test]
    fn the_grid_and_the_now_line() {
        assert_eq!(at("               8:00...... ----------------", "8:00"), Some(Type));
        assert_eq!(at("               8:00...... ----------------", "----"), Some(Comment));
        assert_eq!(at("               4:06...... now - - - - -", "now"), Some(Type));
    }
}
