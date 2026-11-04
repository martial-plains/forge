// SPDX-License-Identifier: MIT OR Apache-2.0
//! Editing features Forge has built in: commenting, matching brackets,
//! word completion from buffers (dabbrev), counting, trailing
//! whitespace, occur and imenu. Frontends turn these into commands.

use crate::app::{BufferId, Editor};
use crate::buffer::{Buffer, Pos, is_word_char};
use crate::syntax;

const OPENERS: &str = "([{";
const CLOSERS: &str = ")]}";
/// How far a text scan for a matching bracket looks.
const SCAN_LIMIT: usize = 200_000;

/// (lines, words, characters) in a piece of text, like M-=.
pub fn count(text: &str) -> (usize, usize, usize) {
    let lines = if text.is_empty() {
        0
    } else {
        text.matches('\n').count() + usize::from(!text.ends_with('\n'))
    };
    (lines, text.split_whitespace().count(), text.chars().count())
}

impl Buffer {
    // ---------------------------------------------------------- comments

    /// The line comment marker for this buffer's language.
    pub fn comment_marker(&self) -> Option<&'static str> {
        self.language().and_then(|l| l.line_comment)
    }

    /// Is `line` (ignoring indentation) a comment?
    pub fn line_is_comment(&self, line: usize) -> bool {
        self.comment_marker()
            .is_some_and(|m| self.line(line).trim_start().starts_with(m))
    }

    /// Comment or uncomment lines `first..=last`: if every non-blank line
    /// is a comment, uncomment them all; otherwise comment them all, with
    /// the markers lined up at the smallest indentation. Returns whether
    /// they are comments now, or None without comment syntax.
    pub fn toggle_comment_lines(&mut self, first: usize, last: usize) -> Option<bool> {
        let marker = self.comment_marker()?;
        let last = last.min(self.line_count().saturating_sub(1));
        let lines: Vec<usize> = (first..=last)
            .filter(|&l| !self.line(l).trim().is_empty())
            .collect();
        if lines.is_empty() {
            return Some(false);
        }
        let uncomment = lines.iter().all(|&l| self.line_is_comment(l));
        let point = self.point();
        let mut col = point.col as isize;
        let indent = lines
            .iter()
            .map(|&l| {
                self.line(l)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count()
            })
            .min()
            .unwrap_or(0);
        for &l in &lines {
            let text = self.line(l);
            let start = self.pos_to_offset(Pos::new(l, 0));
            let (at, del, ins): (usize, usize, String) = if uncomment {
                let ws = text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                let rest: String = text.chars().skip(ws + marker.chars().count()).collect();
                let space = usize::from(rest.starts_with(' '));
                (ws, marker.chars().count() + space, String::new())
            } else {
                (indent, 0, format!("{marker} "))
            };
            self.replace_offsets(start + at, start + at + del, &ins);
            if l == point.line && point.col >= at {
                col = (col + ins.chars().count() as isize - del as isize).max(at as isize);
            }
        }
        self.set_point(Pos::new(point.line, col.max(0) as usize));
        Some(!uncomment)
    }

    /// Where the comment on `line` starts (char column), if it has one
    /// after some code or on its own.
    pub fn comment_column(&self, line: usize) -> Option<usize> {
        let marker = self.comment_marker()?;
        let text = self.line(line);
        let byte = syntax::comment_start(&text, marker)?;
        Some(text[..byte].chars().count())
    }

    // ---------------------------------------------------------- brackets

    /// Brackets for show-paren-mode: the one at `offset` (an opener) or
    /// just before it (a closer), and its match. The syntax tree decides
    /// when there is one (brackets in strings and comments don't count);
    /// otherwise the text is scanned. `Err(at)`: a bracket with no match.
    pub fn matching_bracket(&mut self, offset: usize) -> Option<Result<(usize, usize), usize>> {
        let at_char = self.char_at(self.offset_to_pos(offset));
        let before = offset
            .checked_sub(1)
            .and_then(|o| self.char_at(self.offset_to_pos(o)).map(|c| (o, c)));
        let (at, c) = match (at_char, before) {
            (Some(c), _) if OPENERS.contains(c) => (offset, c),
            (_, Some((o, c))) if CLOSERS.contains(c) => (o, c),
            _ => return None,
        };
        let byte = self.char_to_byte(at);
        if let Some(s) = self.syntax.0.as_mut() {
            if let Some(m) = s.bracket_match(&self.text, byte) {
                return Some(Ok((at, self.byte_to_char(m))));
            }
            // A bracket token without a match: a syntax error.
            #[cfg(feature = "tree-sitter")]
            if s.tree(&self.text).is_some() {
                let node_is_token = s
                    .tree(&self.text)
                    .and_then(|t| t.root_node().descendant_for_byte_range(byte, byte + 1))
                    .is_some_and(|n| n.child_count() == 0 && n.kind().len() == 1);
                if node_is_token {
                    return Some(Err(at));
                }
                // In a string or comment: fall through to the text scan.
            }
        }
        Some(self.scan_bracket(at, c).map(|m| (at, m)).ok_or(at))
    }

    fn scan_bracket(&self, at: usize, c: char) -> Option<usize> {
        let (open, close, forward) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => return None,
        };
        let mut depth = 0usize;
        if forward {
            for (i, ch) in self.text.chars_at(at).enumerate().take(SCAN_LIMIT) {
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at + i);
                    }
                }
            }
        } else {
            let mut i = at + 1;
            for ch in self.text.chars_at(at + 1).reversed().take(SCAN_LIMIT) {
                i -= 1;
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    // -------------------------------------------------------- whitespace

    /// Remove blanks at the ends of lines `first..=last` (and, for the
    /// whole buffer, blank lines at its end). Returns how many lines
    /// changed.
    pub fn delete_trailing_whitespace(&mut self, first: usize, last: usize) -> usize {
        let whole = first == 0 && last >= self.line_count().saturating_sub(1);
        let point = self.point_offset();
        let mut changed = 0;
        for l in (first..=last.min(self.line_count().saturating_sub(1))).rev() {
            let text = self.line(l);
            let trimmed = text.trim_end_matches([' ', '\t']).chars().count();
            let len = text.chars().count();
            if trimmed < len {
                let start = self.pos_to_offset(Pos::new(l, 0));
                self.replace_offsets(start + trimmed, start + len, "");
                changed += 1;
            }
        }
        if whole {
            let text = self.get_contents();
            let keep =
                text.trim_end_matches('\n').chars().count() + usize::from(text.contains('\n'));
            if keep < self.char_count() && !text.trim().is_empty() {
                let n = self.char_count();
                self.replace_offsets(keep, n, "");
                changed += 1;
            }
        }
        self.set_offset(point.min(self.char_count()));
        changed
    }

    /// Add a newline at the end if the text doesn't end with one.
    pub fn ensure_final_newline(&mut self) -> bool {
        let n = self.char_count();
        if n == 0 || self.char_at(self.offset_to_pos(n - 1)) == Some('\n') {
            return false;
        }
        let point = self.point_offset();
        self.replace_offsets(n, n, "\n");
        self.set_offset(point);
        true
    }

    /// Before saving: what .editorconfig (or the options) asked for.
    pub(crate) fn before_save(&mut self) {
        if self.options.trim_trailing_whitespace == Some(true) {
            let last = self.line_count().saturating_sub(1);
            self.delete_trailing_whitespace(0, last);
        }
        if self.options.insert_final_newline == Some(true) {
            self.ensure_final_newline();
        }
    }

    // ----------------------------------------------------------- dabbrev

    /// The word being typed before `offset`: (start offset, text).
    pub fn word_before(&self, offset: usize) -> Option<(usize, String)> {
        let mut start = offset;
        while start > 0
            && self
                .char_at(self.offset_to_pos(start - 1))
                .is_some_and(is_word_char)
        {
            start -= 1;
        }
        (start < offset).then(|| (start, self.text_offsets(start, offset)))
    }

    // ------------------------------------------------------------- occur

    /// Lines matching a regular expression: (line, text).
    pub fn occur(&self, pattern: &str) -> Result<Vec<(usize, String)>, String> {
        let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
        Ok(self
            .get_contents()
            .split('\n')
            .enumerate()
            .filter(|(_, l)| re.is_match(l))
            .map(|(i, l)| (i, l.to_string()))
            .collect())
    }

    // ------------------------------------------------------------- imenu

    /// Definitions in this buffer, from the grammar's tags query:
    /// (name, kind, position).
    pub fn definitions(&mut self) -> Vec<(String, String, Pos)> {
        let Some(s) = self.syntax.0.as_mut() else {
            return Vec::new();
        };
        let tags = s.tags(&self.text);
        tags.into_iter()
            .map(|(n, k, o)| (n, k, self.offset_to_pos(o)))
            .collect()
    }
}

/// Words to complete the one before point with (dabbrev-expand): from
/// this buffer, nearest before point first, then after it, then from
/// other buffers. Case-sensitive, no duplicates, at most `max`.
/// Returns the word's start and the candidates.
pub fn dabbrev(
    editor: &Editor,
    id: BufferId,
    offset: usize,
    max: usize,
) -> Option<(usize, Vec<String>)> {
    let buf = editor.state.get_buffer(id)?;
    let (start, prefix) = buf.word_before(offset)?;
    let mut out: Vec<String> = Vec::new();
    let add = |w: &str, out: &mut Vec<String>| {
        if w.len() > prefix.len() && w.starts_with(&prefix) && !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    };
    let words = |text: &str| -> Vec<(usize, String)> {
        let mut v = Vec::new();
        let mut cur = String::new();
        let mut cur_start = 0;
        for (i, c) in text.chars().enumerate() {
            if is_word_char(c) {
                if cur.is_empty() {
                    cur_start = i;
                }
                cur.push(c);
            } else if !cur.is_empty() {
                v.push((cur_start, std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            v.push((cur_start, cur));
        }
        v
    };
    let own = words(&buf.get_contents());
    for (s, w) in own.iter().rev().filter(|(s, _)| *s < start) {
        let _ = s;
        add(w, &mut out);
    }
    for (_, w) in own.iter().filter(|(s, _)| *s > start) {
        add(w, &mut out);
    }
    for other in editor.buffer_ids().into_iter().filter(|b| *b != id) {
        if let Some(b) = editor.state.get_buffer(other) {
            for (_, w) in words(&b.get_contents()) {
                add(&w, &mut out);
            }
        }
        if out.len() >= max {
            break;
        }
    }
    out.truncate(max);
    Some((start, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(lang: &str, text: &str) -> Buffer {
        let mut b = Buffer::new("t");
        b.set_contents(text);
        b.set_language(syntax::language(lang));
        b
    }

    #[test]
    fn commenting_lines_and_back() {
        let mut b = buf("rust", "fn a() {\n    x();\n\n    y();\n}\n");
        b.set_point(Pos::new(3, 6));
        assert_eq!(b.toggle_comment_lines(1, 3), Some(true));
        assert_eq!(
            b.get_contents(),
            "fn a() {\n    // x();\n\n    // y();\n}\n"
        );
        assert_eq!(b.point(), Pos::new(3, 9), "point stays on its text");
        assert_eq!(b.toggle_comment_lines(1, 3), Some(false));
        assert_eq!(b.get_contents(), "fn a() {\n    x();\n\n    y();\n}\n");
        // Mixed: comment everything.
        let mut b = buf("python", "# a\nb\n");
        assert_eq!(b.toggle_comment_lines(0, 1), Some(true));
        assert_eq!(b.get_contents(), "# # a\n# b\n");
        assert_eq!(buf("json", "{}").toggle_comment_lines(0, 0), None);
        let b = buf("rust", "let x = 1; // one\n");
        assert_eq!(b.comment_column(0), Some(11));
    }

    #[test]
    fn matching_brackets_by_tree_and_by_text() {
        let mut b = buf("rust", "fn a() { let s = \"(\"; }\n");
        assert_eq!(b.matching_bracket(7), Some(Ok((7, 22))), "{{ at point");
        assert_eq!(b.matching_bracket(23), Some(Ok((22, 7))), "}} before point");
        assert_eq!(
            b.matching_bracket(18),
            Some(Err(18)),
            "( in a string has no match"
        );
        let mut t = Buffer::new("t");
        t.set_contents("(a [b] c)");
        assert_eq!(t.matching_bracket(0), Some(Ok((0, 8))));
        assert_eq!(t.matching_bracket(6), Some(Ok((5, 3))));
        assert_eq!(t.matching_bracket(1), None);
    }

    #[test]
    fn trailing_whitespace_and_final_newline() {
        let mut b = Buffer::new("t");
        b.set_contents("a  \nb\t\n\n\n");
        assert_eq!(b.delete_trailing_whitespace(0, 4), 3);
        assert_eq!(b.get_contents(), "a\nb\n");
        let mut b = Buffer::new("t");
        b.set_contents("x");
        assert!(b.ensure_final_newline());
        assert_eq!(b.get_contents(), "x\n");
        assert!(!b.ensure_final_newline());
    }

    #[test]
    fn counting_occur_and_definitions() {
        assert_eq!(count("one two\nthree\n"), (2, 3, 14));
        assert_eq!(count(""), (0, 0, 0));
        let mut b = buf(
            "rust",
            "struct S;\nimpl S {\n    fn m(&self) {}\n}\nfn main() {}\n",
        );
        assert_eq!(
            b.occur("fn").unwrap(),
            [
                (2, "    fn m(&self) {}".to_string()),
                (4, "fn main() {}".to_string())
            ]
        );
        assert!(b.occur("(").is_err());
        let defs: Vec<(String, String, usize)> = b
            .definitions()
            .into_iter()
            .map(|(n, k, p)| (n, k, p.line))
            .collect();
        assert_eq!(
            defs,
            [
                ("S".into(), "class".into(), 0),
                ("m".into(), "method".into(), 2),
                ("main".into(), "function".into(), 4)
            ]
        );
    }

    #[test]
    fn dabbrev_finds_nearest_words_first() {
        let mut ed = Editor::new();
        let a = ed.create_buffer("a");
        ed.state
            .get_buffer_mut(a)
            .unwrap()
            .set_contents("forge forget fo foremost\n");
        let b = ed.create_buffer("b");
        ed.state
            .get_buffer_mut(b)
            .unwrap()
            .set_contents("format fork");
        let (start, words) = dabbrev(&ed, a, 15, 10).unwrap();
        assert_eq!(start, 13);
        assert_eq!(words, ["forget", "forge", "foremost", "format", "fork"]);
        assert!(dabbrev(&ed, a, 16, 10).is_none(), "no word before point");
    }
}
