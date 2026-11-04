// SPDX-License-Identifier: MIT OR Apache-2.0
//! Highlighting for `*Help*` buffers (help-mode): the headings, key
//! sequences and command names in what C-h f, C-h k, C-h b and C-h m
//! write, a line at a time, since the text has no grammar.
//!
//! The help commands write in a small fixed layout, and this knows it:
//!
//! - a heading is a line at the left margin that ends in a colon
//!   (`Key bindings:`), and a rule (`=====`, `-----`) is dim;
//! - a key sequence (`C-x C-s`, `M-f`, `<f10>`, `RET`) is a constant;
//! - a command or option name (`save-buffer`, `forge.opt.hl_line`) is a
//!   function;
//! - a note in parentheses (`(global-map)`) is a comment.

use crate::syntax::SyntaxFace;

pub const MODE: &str = "help-mode";

/// Words with a hyphen that are English, not command names.
const PLAIN: &[&str] = &[
    "built-in",
    "non-incremental",
    "buffer-local",
    "major-mode",
    "minor-mode",
    "so-called",
    "re-indent",
    "read-only",
    "x-ray",
    "e-mail",
    "one-line",
    "multi-line",
    "per-buffer",
];

const NAMED_KEYS: &[&str] = &["RET", "TAB", "SPC", "ESC", "DEL", "LFD", "NUL", "C-g"];

fn is_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
}

/// Is this word a key sequence element: `C-x`, `M-<left>`, `<f10>`, `RET`?
fn is_key(word: &str) -> bool {
    let w = word.trim_matches(|c: char| ",.;:)(".contains(c));
    if w.is_empty() {
        return false;
    }
    if NAMED_KEYS.contains(&w) {
        return true;
    }
    if w.starts_with('<') && w.ends_with('>') && w.len() > 2 {
        return true;
    }
    // Modifier prefixes: C- M- S- s- H- A- one or more, then a key.
    let mut rest = w;
    let mut mods = 0;
    while rest.len() > 2
        && rest.as_bytes()[1] == b'-'
        && "CMSsHA".contains(rest.as_bytes()[0] as char)
    {
        rest = &rest[2..];
        mods += 1;
    }
    mods > 0 && !rest.is_empty() && rest.chars().count() <= 8 && !rest.contains(' ')
}

/// Is this a command-like name: lowercase words joined by hyphens?
fn is_command(word: &str) -> bool {
    let w = word.trim_matches(|c: char| ",.;:)(\"'".contains(c));
    if PLAIN.contains(&w) || !w.contains('-') || w.len() < 3 {
        return false;
    }
    let mut chars = w.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && w.chars().all(|c| is_name_char(c) || c == '.')
        && !w.ends_with('-')
        && !w.contains("--")
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 3
        && t.chars()
            .all(|c| c == '=' || c == '-' || c == '\u{2500}' || c == ' ')
}

/// A face per character of `line` (without its newline).
pub fn line_faces(line: &str) -> Vec<Option<SyntaxFace>> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = vec![None; chars.len()];
    if is_rule(line) {
        out.fill(Some(SyntaxFace::Comment));
        return out;
    }
    let heading = chars.first().is_some_and(|c| c.is_uppercase())
        && line.ends_with(':')
        && !line.contains("  ");
    if heading {
        out.fill(Some(SyntaxFace::Keyword));
        return out;
    }
    let mut paren_depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        // Trim punctuation for classification but colour only the core.
        let lead = word.chars().take_while(|c| "(\"'[".contains(*c)).count();
        let core: String = word
            .trim_matches(|c: char| ",.;:)(\"'[]".contains(c))
            .to_string();
        if word.starts_with('(') {
            paren_depth += 1;
        }
        let face = if paren_depth > 0 && !is_key(&core) && !is_command(&core) {
            Some(SyntaxFace::Comment)
        } else if word.starts_with("[remap") || word == "[remap" {
            Some(SyntaxFace::Attribute)
        } else if is_key(&core) {
            Some(SyntaxFace::Constant)
        } else if is_command(&core) || core.starts_with("forge.") {
            Some(SyntaxFace::Function)
        } else {
            None
        };
        if let Some(f) = face {
            let from = start + lead.min(word.chars().count());
            let to = (from + core.chars().count()).min(i);
            for slot in &mut out[from..to] {
                *slot = Some(f);
            }
        }
        if word.ends_with(')') {
            paren_depth = paren_depth.saturating_sub(word.matches(')').count().min(paren_depth));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxFace::*;

    fn at(line: &str, word: &str) -> Option<SyntaxFace> {
        let col = line.find(word).unwrap();
        line_faces(line)[line[..col].chars().count()]
    }

    #[test]
    fn headings_and_rules() {
        assert!(
            line_faces("Key bindings:")
                .iter()
                .all(|f| *f == Some(Keyword))
        );
        assert!(line_faces("=======").iter().all(|f| *f == Some(Comment)));
        assert!(
            line_faces("  indented: not a heading")
                .iter()
                .all(|f| *f != Some(Keyword))
        );
    }

    #[test]
    fn keys_and_commands() {
        let l = "  C-x C-s   save-buffer";
        assert_eq!(at(l, "C-x"), Some(Constant));
        assert_eq!(at(l, "save-buffer"), Some(Function));
        assert_eq!(at("press RET to run", "RET"), Some(Constant));
        assert_eq!(at("a <f10> key", "<f10>"), Some(Constant));
        assert_eq!(at("a built-in thing", "built-in"), None);
        assert_eq!(
            at("see forge.opt.hl_line here", "forge.opt"),
            Some(Function)
        );
    }

    #[test]
    fn notes_in_parentheses_are_dim_but_keys_inside_stay() {
        let l = "kill-line (found in global-map)";
        assert_eq!(at(l, "kill-line"), Some(Function));
        assert_eq!(at(l, "found"), Some(Comment));
    }

    #[test]
    fn trailing_punctuation_is_left_plain() {
        let l = "It is bound to C-x C-s.";
        let f = line_faces(l);
        assert_eq!(f[l.find("s.").unwrap()], Some(Constant));
        assert_eq!(f[l.len() - 1], None);
    }
}
