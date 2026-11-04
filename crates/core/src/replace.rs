// SPDX-License-Identifier: MIT OR Apache-2.0
//! Search and replace (M-%, C-M-%, replace-string, replace-regexp).
//!
//! A [`Replacer`] finds matches one at a time so the caller can ask about
//! each one. It keeps a copy of the buffer text in sync with its own
//! replacements, so regex matching (which needs contiguous text) doesn't
//! re-copy the whole buffer for every match.
//!
//! Patterns use Rust `regex` syntax (`(a|b)`, `\b`, `\d`). Replacements
//! use syntax: `\&` is the whole match, `\1`..`\9` are groups,
//! `\\` is a backslash.

use crate::buffer::Buffer;
use regex::{Captures, Regex, RegexBuilder};

#[derive(Debug, Clone, Default)]
pub struct ReplaceOptions {
    /// Treat the pattern as a regex (else literal text).
    pub regexp: bool,
    /// Only match whole words (C-u M-%).
    pub delimited: bool,
    /// Char offset to start searching at.
    pub start: usize,
    /// Char offset to stop at (the end of the region).
    pub end: Option<usize>,
}

pub struct Replacer {
    re: Regex,
    replacement: String,
    literal: bool,
    preserve_case: bool,
    shadow: String,
    /// Byte offset in `shadow` to search from.
    from: usize,
    /// Byte offset where the searched region ends.
    limit: Option<usize>,
    current: Option<(usize, usize)>,
    pub replaced: usize,
}

/// Uppercase letters, ignoring the letter after a backslash (\W, \S, \D).
fn has_upper(s: &str, is_regex: bool) -> bool {
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && is_regex {
            escaped = true;
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

/// Make `rep` follow the case of `matched`: "FOO" -> "BAR", "Foo" -> "Bar".
fn follow_case(matched: &str, rep: &str) -> String {
    let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        rep.to_uppercase()
    } else if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut cs = rep.chars();
        cs.next()
            .map(|f| f.to_uppercase().chain(cs).collect())
            .unwrap_or_default()
    } else {
        rep.to_string()
    }
}

/// Expand `\&`, `\1`..`\9` and `\\` in a replacement.
fn expand(caps: &Captures, rep: &str) -> String {
    let mut out = String::new();
    let mut chars = rep.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('&') => out.push_str(caps.get(0).map_or("", |m| m.as_str())),
            Some(d @ '0'..='9') => {
                let n = d.to_digit(10).expect("digit") as usize;
                out.push_str(caps.get(n).map_or("", |m| m.as_str()));
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

impl Replacer {
    pub fn new(
        buf: &Buffer,
        pattern: &str,
        replacement: &str,
        opts: &ReplaceOptions,
    ) -> Result<Self, String> {
        let mut p = if opts.regexp {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        if opts.delimited {
            p = format!(r"\b(?:{p})\b");
        }
        let fold = !has_upper(pattern, opts.regexp);
        let re = RegexBuilder::new(&p)
            .case_insensitive(fold)
            .multi_line(true)
            .build()
            .map_err(|e| {
                // The first line of regex's error is enough for the echo area.
                format!(
                    "Invalid regexp: {}",
                    e.to_string().lines().last().unwrap_or("").trim()
                )
            })?;
        Ok(Self {
            re,
            replacement: replacement.to_string(),
            literal: !opts.regexp,
            preserve_case: fold && !has_upper(replacement, false),
            shadow: buf.get_contents(),
            from: buf.char_to_byte(opts.start),
            limit: opts.end.map(|e| buf.char_to_byte(e)),
            current: None,
            replaced: 0,
        })
    }

    /// Find the next match; returns its char offsets.
    pub fn find_next(&mut self, buf: &Buffer) -> Option<(usize, usize)> {
        self.current = None;
        if self.from > self.shadow.len() {
            return None;
        }
        let m = self.re.find_at(&self.shadow, self.from)?;
        if self.limit.is_some_and(|l| m.end() > l) {
            return None;
        }
        self.current = Some((m.start(), m.end()));
        Some((buf.byte_to_char(m.start()), buf.byte_to_char(m.end())))
    }

    fn next_char_boundary(&self, b: usize) -> usize {
        self.shadow[b..]
            .chars()
            .next()
            .map_or(b + 1, |c| b + c.len_utf8())
    }

    /// Leave the current match alone and search on after it.
    pub fn skip(&mut self) {
        if let Some((s, e)) = self.current.take() {
            self.from = if e > s { e } else { self.next_char_boundary(s) };
        }
    }

    /// What the current match would be replaced with.
    pub fn replacement_for_current(&self) -> Option<String> {
        let (s, e) = self.current?;
        let text = if self.literal {
            self.replacement.clone()
        } else {
            let caps = self.re.captures_at(&self.shadow, s)?;
            expand(&caps, &self.replacement)
        };
        Some(if self.preserve_case {
            follow_case(&self.shadow[s..e], &text)
        } else {
            text
        })
    }

    /// Replace the current match (one undo step) and search on after it.
    /// Returns the char offsets of the new text.
    pub fn replace_current(&mut self, buf: &mut Buffer) -> Option<(usize, usize)> {
        let text = self.replacement_for_current()?;
        let (s, e) = self.current?;
        let (cs, ce) = (buf.byte_to_char(s), buf.byte_to_char(e));
        buf.break_undo_group();
        let applied = buf.try_replace_offsets(cs, ce, &text);
        buf.break_undo_group();
        if !applied {
            // The buffer refused the edit, so the shadow copy of it must
            // not be changed: the next match has to be found in the text
            // that is really there.
            return None;
        }
        self.current = Some((s, e));
        self.shadow.replace_range(s..e, &text);
        if let Some(l) = self.limit.as_mut() {
            *l = (*l + text.len()).saturating_sub(e - s);
        }
        self.from = s + text.len();
        if e == s {
            // Don't match the same empty spot again.
            if self.from >= self.shadow.len() {
                self.from = self.shadow.len() + 1;
            } else {
                self.from = self.next_char_boundary(self.from);
            }
        }
        self.replaced += 1;
        Some((cs, cs + text.chars().count()))
    }

    /// Replace every remaining match.
    pub fn replace_all(&mut self, buf: &mut Buffer) {
        while self.find_next(buf).is_some() {
            self.replace_current(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("t");
        b.set_contents(text);
        b
    }

    fn all(text: &str, from: &str, to: &str, opts: ReplaceOptions) -> (String, usize) {
        let mut b = buf(text);
        let mut r = Replacer::new(&b, from, to, &opts).unwrap();
        r.replace_all(&mut b);
        (b.get_contents(), r.replaced)
    }

    #[test]
    fn literal_with_case_following() {
        let (t, n) = all("foo Foo FOO food", "foo", "bar", ReplaceOptions::default());
        assert_eq!((t.as_str(), n), ("bar Bar BAR bard", 4));
        // An uppercase letter in the pattern makes it case-sensitive and literal.
        let (t, _) = all("foo Foo", "Foo", "bar", ReplaceOptions::default());
        assert_eq!(t, "foo bar");
        let (t, _) = all("a.b axb", ".", "!", ReplaceOptions::default());
        assert_eq!(t, "a!b axb");
    }

    #[test]
    fn delimited_words() {
        let opts = ReplaceOptions {
            delimited: true,
            ..Default::default()
        };
        assert_eq!(
            all("cat catalog cat", "cat", "dog", opts).0,
            "dog catalog dog"
        );
    }

    #[test]
    fn regexp_groups_and_whole_match() {
        let opts = ReplaceOptions {
            regexp: true,
            ..Default::default()
        };
        assert_eq!(
            all("x=1, y=22", r"(\w)=(\d+)", r"\2:\1", opts.clone()).0,
            "1:x, 22:y"
        );
        assert_eq!(all("ab", r"\w", r"[\&]", opts.clone()).0, "[a][b]");
        assert_eq!(all("a\nb", "^", "> ", opts.clone()).0, "> a\n> b");
        assert_eq!(all("é漢", "", "-", opts.clone()).0, "-é-漢-");
        assert!(Replacer::new(&buf(""), "(", "", &opts).is_err());
    }

    #[test]
    fn region_limits_and_skipping() {
        let opts = ReplaceOptions {
            start: 4,
            end: Some(11),
            ..Default::default()
        };
        assert_eq!(all("aa. aa. aa. aa", "aa", "b", opts).0, "aa. b. b. aa");

        let mut b = buf("one one one");
        let mut r = Replacer::new(&b, "one", "1", &ReplaceOptions::default()).unwrap();
        assert_eq!(r.find_next(&b), Some((0, 3)));
        r.skip();
        assert_eq!(r.find_next(&b), Some((4, 7)));
        assert_eq!(r.replace_current(&mut b), Some((4, 5)));
        assert_eq!(r.find_next(&b), Some((6, 9)));
        assert_eq!(b.get_contents(), "one 1 one");
        // Each replacement undoes on its own.
        b.undo();
        assert_eq!(b.get_contents(), "one one one");
    }
}
