// SPDX-License-Identifier: MIT OR Apache-2.0
//! Text navigation on the text itself: sentences (M-a, M-e, M-k),
//! paragraphs (M-{, M-}), pages (C-x [, C-x ]) and word transposition (M-t).
//!
//! Everything here works on a `&[char]` with char offsets, so multibyte
//! text is handled without any byte indexing.

use crate::buffer::is_word_char;
use regex::Regex;

/// The normal `page-delimiter`: a formfeed at the start of a line.
pub const DEFAULT_PAGE_DELIMITER: &str = "^\\f";
/// The normal `paragraph-start`: a formfeed, or a line of blanks.
pub const DEFAULT_PARAGRAPH_START: &str = "\\f\\|[ \t]*$";
/// The normal `paragraph-separate`: a line of blanks or formfeeds.
pub const DEFAULT_PARAGRAPH_SEPARATE: &str = "[ \t\\f]*$";

/// Compile a `page-delimiter` regexp. `^` matches the start of a
/// line, so the multiline flag is on.
pub fn page_delimiter(pattern: &str) -> Result<Regex, String> {
    let pattern = if pattern.is_empty() {
        DEFAULT_PAGE_DELIMITER
    } else {
        pattern
    };
    Regex::new(&format!("(?m){pattern}")).map_err(|e| format!("page-delimiter: {e}"))
}

/// Compile a `paragraph-start` or `paragraph-separate` regexp. Forge matches
/// these from the left margin after optional indentation, and strips a
/// leading `^`, so the pattern is anchored after leading blanks here too.
pub fn paragraph_regexp(pattern: &str, what: &str) -> Result<Regex, String> {
    let pattern = pattern.strip_prefix('^').unwrap_or(pattern);
    Regex::new(&format!("^[ \\t]*(?:{pattern})")).map_err(|e| format!("{what}: {e}"))
}

/// Compile a `sentence-end` regexp, or `None` for the built-in rules.
pub fn sentence_end_regexp(pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|e| format!("sentence-end: {e}"))
}

fn char_to_byte(text: &[char], off: usize) -> usize {
    text[..off.min(text.len())]
        .iter()
        .map(|c| c.len_utf8())
        .sum()
}

fn byte_to_char(text: &[char], byte: usize) -> usize {
    let mut b = 0;
    for (i, c) in text.iter().enumerate() {
        if b >= byte {
            return i;
        }
        b += c.len_utf8();
    }
    text.len()
}

/// The start of every line in `text` (offsets 0 and after each newline).
fn line_starts(text: &[char]) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in text.iter().enumerate() {
        if *c == '\n' && i + 1 < text.len() {
            starts.push(i + 1);
        }
    }
    starts
}

/// The char offsets of the `page-delimiter` matches, in order.
fn delimiter_positions(text: &[char], re: &Regex) -> Vec<usize> {
    let s: String = text.iter().collect();
    line_starts(text)
        .into_iter()
        .filter(|&st| {
            let b = char_to_byte(text, st);
            re.find_at(&s, b).is_some_and(|m| m.start() == b)
        })
        .collect()
}

/// How a sentence ends: the `sentence-end-double-space` and
/// `sentence-end-without-period` options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SentenceEnd {
    /// A sentence ends only where its punctuation is followed by two
    /// spaces or the end of a line, the American typist's convention.
    /// When false (Forge's default), a single space ends one too.
    pub double_space: bool,
    /// A sentence can end without a period, at a line break, for
    /// languages such as Thai that do not use periods.
    pub without_period: bool,
}

/// A character that ends a sentence (ignoring any closing quotes/brackets
/// that follow it). A line break ends one too when `without_period` is set.
fn is_sentence_ender(c: char, end: SentenceEnd) -> bool {
    (end.without_period && c == '\n') || matches!(c, '.' | '!' | '?')
}

/// Closing punctuation that may sit between the ender and the whitespace.
fn is_sentence_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’')
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\u{b}' || c == '\u{c}'
}

/// Whether the whitespace run starting at `i` makes the sentence end at the
/// punctuation just before it. With `double_space`, the run must be two
/// spaces or contain a line break; without it, any run does.
fn whitespace_ends_sentence(text: &[char], i: usize, end: SentenceEnd) -> bool {
    if !end.double_space {
        return true;
    }
    if i >= text.len() {
        return true;
    }
    if text[i] == '\n' {
        return true;
    }
    let mut j = i;
    while j < text.len() && (text[j] == ' ' || text[j] == '\t') {
        j += 1;
    }
    j - i >= 2
}

/// Offset just past the next sentence end at or after `from` (M-e). When
/// `sentence_end` is set its match decides the boundary; otherwise the
/// built-in rules in `end` are used. Stops at the end of the text.
pub fn forward_sentence(
    text: &[char],
    from: usize,
    end: SentenceEnd,
    sentence_end: Option<&Regex>,
) -> usize {
    if let Some(re) = sentence_end {
        return forward_sentence_regexp(text, from, re);
    }
    let n = text.len();
    let mut i = from;
    while i < n {
        if is_sentence_ender(text[i], end) {
            // Skip a run of enders ("?!" or "...").
            while i < n && is_sentence_ender(text[i], end) {
                i += 1;
            }
            // Skip closing quotes/brackets right after the enders.
            while i < n && is_sentence_closer(text[i]) {
                i += 1;
            }
            // The sentence ends where the following whitespace begins, if
            // that whitespace makes it one.
            if whitespace_ends_sentence(text, i, end) {
                return i;
            }
            continue;
        }
        i += 1;
    }
    n
}

/// M-e with a `sentence-end` regexp: the next match's end, trimmed of the
/// trailing whitespace, so point lands just after the punctuation.
fn forward_sentence_regexp(text: &[char], from: usize, re: &Regex) -> usize {
    let s: String = text.iter().collect();
    let start = char_to_byte(text, from);
    let Some(m) = re.find_at(&s, start) else {
        return text.len();
    };
    let mut e = m.end();
    while e > m.start() {
        let prev = s[..e]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(m.start());
        if s[prev..e].chars().next().is_some_and(char::is_whitespace) {
            e = prev;
        } else {
            break;
        }
    }
    byte_to_char(text, e)
}

/// Offset of the start of the sentence at or before `from` (M-a). Skips
/// whitespace and the enders of earlier sentences.
pub fn backward_sentence(
    text: &[char],
    from: usize,
    end: SentenceEnd,
    sentence_end: Option<&Regex>,
) -> usize {
    if let Some(re) = sentence_end {
        return backward_sentence_regexp(text, from, re);
    }
    let mut i = from.min(text.len());
    // Move off any whitespace (and enders) just before point.
    while i > 0 && is_space(text[i - 1]) {
        i -= 1;
    }
    // If we just stepped over a sentence end, walk back through its
    // enders and closers so we land before them.
    while i > 0 && (is_sentence_ender(text[i - 1], end) || is_sentence_closer(text[i - 1])) {
        i -= 1;
    }
    // Walk back to the start of this sentence.
    while i > 0 {
        let c = text[i - 1];
        if is_sentence_ender(c, end) {
            break;
        }
        if is_space(c) {
            // Whitespace that follows an ender separates sentences; plain
            // runs of spaces do not.
            let mut j = i - 1;
            while j > 0 && is_space(text[j - 1]) {
                j -= 1;
            }
            if j > 0
                && is_sentence_ender(text[j - 1], end)
                && whitespace_ends_sentence(text, j, end)
            {
                break;
            }
            let mut k = j;
            while k > 0 && is_sentence_closer(text[k - 1]) {
                k -= 1;
            }
            if k > 0
                && is_sentence_ender(text[k - 1], end)
                && whitespace_ends_sentence(text, k, end)
            {
                break;
            }
            i = j;
            continue;
        }
        i -= 1;
    }
    i
}

/// M-a with a `sentence-end` regexp: the start of the sentence after the
/// previous match, skipping the whitespace that follows it.
fn backward_sentence_regexp(text: &[char], from: usize, re: &Regex) -> usize {
    let s: String = text.iter().collect();
    let limit = char_to_byte(text, from);
    let Some(m) = re.find_iter(&s).take_while(|m| m.start() < limit).last() else {
        return 0;
    };
    let mut i = byte_to_char(text, m.end());
    while i < text.len() && is_space(text[i]) {
        i += 1;
    }
    i
}

/// The sentence around point as a (start, end) char offset pair. Used by
/// M-k and C-x DEL.
pub fn sentence_at(
    text: &[char],
    from: usize,
    end: SentenceEnd,
    sentence_end: Option<&Regex>,
) -> (usize, usize) {
    let start = backward_sentence(text, from, end, sentence_end);
    let end = forward_sentence(text, start, end, sentence_end);
    (start, end)
}

/// The `paragraph-start` and `paragraph-separate` regexps.
#[derive(Debug, Clone, Copy)]
pub struct ParagraphRules<'a> {
    /// Matches lines that start or separate paragraphs.
    pub start: &'a Regex,
    /// Matches lines that separate paragraphs (but are not part of one).
    pub separate: &'a Regex,
}

fn is_start(line: &str, rules: ParagraphRules<'_>) -> bool {
    rules.start.is_match(line)
}

fn is_separate(line: &str, rules: ParagraphRules<'_>) -> bool {
    rules.separate.is_match(line)
}

/// Inclusive line range of the paragraph containing `line`; `None` if the
/// line is a paragraph separator. A paragraph is a run of lines that
/// matches none of `paragraph-separate`.
pub fn paragraph_bounds(
    lines: &[String],
    line: usize,
    rules: ParagraphRules<'_>,
) -> Option<(usize, usize)> {
    let n = lines.len();
    if n == 0 {
        return None;
    }
    let mut start = line.min(n - 1);
    if is_separate(&lines[start], rules) {
        return None;
    }
    while start > 0 && !is_separate(&lines[start - 1], rules) {
        start -= 1;
    }
    let mut end = start;
    while end + 1 < n && !is_separate(&lines[end + 1], rules) {
        end += 1;
    }
    Some((start, end))
}

/// Line at or after `line` that ends the current or next paragraph (M-}).
/// A paragraph ends at the first line that matches `paragraph-start` or
/// `paragraph-separate`; returns the last line if there is none.
pub fn forward_paragraph(lines: &[String], line: usize, rules: ParagraphRules<'_>) -> usize {
    let n = lines.len();
    let mut i = line;
    // Move forward over the lines that separate paragraphs.
    while i < n && is_separate(&lines[i], rules) {
        i += 1;
    }
    if i >= n {
        return n.saturating_sub(1);
    }
    i += 1;
    while i < n && !(is_start(&lines[i], rules) || is_separate(&lines[i], rules)) {
        i += 1;
    }
    if i >= n { n.saturating_sub(1) } else { i }
}

/// Line that starts the previous paragraph (M-{). If point is not on the
/// first line of its paragraph, go to that first line; otherwise go back
/// over the separating lines to the previous paragraph.
pub fn backward_paragraph(lines: &[String], line: usize, rules: ParagraphRules<'_>) -> usize {
    let n = lines.len();
    if n == 0 {
        return 0;
    }
    let mut i = line.min(n - 1);
    if is_separate(&lines[i], rules) {
        while i > 0 && is_separate(&lines[i - 1], rules) {
            i -= 1;
        }
        return i;
    }
    // On the first line of a paragraph: go back to the previous one's start.
    if i > 0 && is_separate(&lines[i - 1], rules) {
        while i > 0 && is_separate(&lines[i - 1], rules) {
            i -= 1;
        }
        while i > 0 && !is_separate(&lines[i - 1], rules) {
            i -= 1;
        }
        return i;
    }
    // Otherwise move back over this paragraph's lines to its start.
    while i > 0 && !is_separate(&lines[i - 1], rules) {
        i -= 1;
    }
    i
}

/// Char offset of the first char of `line` in `text`, where lines are
/// separated by a single `\n`.
pub fn line_start_offset(text: &[char], line: usize) -> usize {
    let mut o = 0;
    let mut l = 0;
    while l < line && o < text.len() {
        if text[o] == '\n' {
            l += 1;
        }
        o += 1;
    }
    o
}

/// Line number containing char offset `off`.
pub fn offset_line(text: &[char], off: usize) -> usize {
    let off = off.min(text.len());
    text[..off].iter().filter(|c| **c == '\n').count()
}

/// Char offset of the `page-delimiter` match that starts the page after
/// `off` (C-x ]). Returns the text length if there is none.
pub fn forward_page(text: &[char], off: usize, re: &Regex) -> usize {
    let off = off.min(text.len());
    delimiter_positions(text, re)
        .into_iter()
        .find(|&st| st >= off)
        .unwrap_or(text.len())
}

/// Char offset of the `page-delimiter` match that starts the page before
/// `off` (C-x [). Returns 0 for the first page.
pub fn backward_page(text: &[char], off: usize, re: &Regex) -> usize {
    let off = off.min(text.len());
    delimiter_positions(text, re)
        .into_iter()
        .rev()
        .find(|&st| st < off)
        .unwrap_or(0)
}

/// 1-based page number containing `off` (M-x what-page), and the char
/// offset where that page's text begins (just after its delimiter).
pub fn page_number(text: &[char], off: usize, re: &Regex) -> (usize, usize) {
    let off = off.min(text.len());
    let breaks = delimiter_positions(text, re)
        .into_iter()
        .filter(|&st| st < off)
        .count();
    let start = if breaks == 0 {
        0
    } else {
        backward_page(text, off, re) + 1
    };
    (breaks + 1, start)
}

/// Number of lines on the page containing `off` (M-x count-lines-page).
pub fn page_line_count(text: &[char], off: usize, re: &Regex) -> usize {
    let (_, start) = page_number(text, off, re);
    let end = forward_page(text, start, re);
    let slice = &text[start..end];
    // Count newlines plus one for a final line that has content.
    let mut lines = slice.iter().filter(|c| **c == '\n').count() + 1;
    if slice.last().is_some_and(|c| *c == '\n') {
        lines -= 1;
    }
    lines.max(1)
}

/// M-t: transpose the word before point with the one after it, ending up
/// after the second word. `from` is the point; returns the new point
/// offset, or `from` unchanged if there are not two words.
pub fn transpose_words(text: &mut Vec<char>, from: usize) -> usize {
    let n = text.len();
    // Find the end of the word before point (skip spaces, then the word).
    let mut e1 = from.min(n);
    while e1 > 0 && !is_word_char(text[e1 - 1]) {
        e1 -= 1;
    }
    if e1 == 0 {
        return from;
    }
    let mut s1 = e1;
    while s1 > 0 && is_word_char(text[s1 - 1]) {
        s1 -= 1;
    }
    // Find the next word after point.
    let mut s2 = e1;
    while s2 < n && !is_word_char(text[s2]) {
        s2 += 1;
    }
    let (w1s, w1e, w2s, w2e) = if s2 >= n {
        // No word after point: transpose the two words before it.
        let w2e = e1;
        let mut w2s = w2e;
        while w2s > 0 && is_word_char(text[w2s - 1]) {
            w2s -= 1;
        }
        let mut w1e = w2s;
        while w1e > 0 && !is_word_char(text[w1e - 1]) {
            w1e -= 1;
        }
        if w1e == 0 {
            return from;
        }
        let mut w1s = w1e;
        while w1s > 0 && is_word_char(text[w1s - 1]) {
            w1s -= 1;
        }
        (w1s, w1e, w2s, w2e)
    } else {
        let mut e2 = s2;
        while e2 < n && is_word_char(text[e2]) {
            e2 += 1;
        }
        (s1, e1, s2, e2)
    };
    // Rebuild: [..w1s] W2 [gap] W1 [..w2e].
    let w1: Vec<char> = text[w1s..w1e].to_vec();
    let w2: Vec<char> = text[w2s..w2e].to_vec();
    let gap: Vec<char> = text[w1e..w2s].to_vec();
    let mut out: Vec<char> = Vec::with_capacity(n);
    out.extend_from_slice(&text[..w1s]);
    out.extend_from_slice(&w2);
    out.extend_from_slice(&gap);
    out.extend_from_slice(&w1);
    out.extend_from_slice(&text[w2e..]);
    // Point ends after the second (originally first) word.
    let new_point = w1s + w2.len() + gap.len() + w1.len();
    *text = out;
    new_point
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn forward_sentence_finds_the_ender() {
        let t = chars("One two. Three four! Five");
        assert_eq!(forward_sentence(&t, 0, SentenceEnd::default(), None), 8);
        assert_eq!(forward_sentence(&t, 8, SentenceEnd::default(), None), 20);
        assert_eq!(forward_sentence(&t, 20, SentenceEnd::default(), None), 25);
    }

    #[test]
    fn backward_sentence_lands_on_the_start() {
        let t = chars("One two. Three four! Five");
        assert_eq!(backward_sentence(&t, 24, SentenceEnd::default(), None), 21);
        assert_eq!(backward_sentence(&t, 20, SentenceEnd::default(), None), 9);
        assert_eq!(backward_sentence(&t, 8, SentenceEnd::default(), None), 0);
    }

    #[test]
    fn sentence_handles_quotes_and_multiple_enders() {
        let t = chars("Hi there.\" Then stop?! Done.");
        let (s, e) = sentence_at(&t, 5, SentenceEnd::default(), None);
        assert_eq!(&t[s..e].iter().collect::<String>(), "Hi there.\"");
        let (s2, e2) = sentence_at(&t, 12, SentenceEnd::default(), None);
        assert_eq!(&t[s2..e2].iter().collect::<String>(), "Then stop?!");
    }

    #[test]
    fn sentence_at_multibyte() {
        let t = chars("héllo wörld. 再见。 Next");
        let (s, e) = sentence_at(&t, 3, SentenceEnd::default(), None);
        assert_eq!(&t[s..e].iter().collect::<String>(), "héllo wörld.");
    }

    #[test]
    fn double_space_needs_two_spaces_or_a_line_break() {
        // A single space does not end a sentence, so the whole line is
        // one sentence until the final period (end of the buffer).
        let end = SentenceEnd {
            double_space: true,
            ..SentenceEnd::default()
        };
        let t = chars("One two. Three four! Five.");
        assert_eq!(forward_sentence(&t, 0, end, None), 26);
        let t2 = chars("One two.  Three four!  Five.");
        assert_eq!(forward_sentence(&t2, 0, end, None), 8);
        assert_eq!(backward_sentence(&t2, 10, end, None), 0);
        let t3 = chars("One two.\nThree");
        assert_eq!(forward_sentence(&t3, 0, end, None), 8);
    }

    #[test]
    fn without_period_ends_a_sentence_at_a_line_break() {
        let end = SentenceEnd {
            without_period: true,
            ..SentenceEnd::default()
        };
        let t = chars("สวัสดี\nครับ");
        assert_eq!(forward_sentence(&t, 0, end, None), 7);
    }

    fn default_paragraph_rules() -> (Regex, Regex) {
        (
            paragraph_regexp(DEFAULT_PARAGRAPH_START, "paragraph-start").unwrap(),
            paragraph_regexp(DEFAULT_PARAGRAPH_SEPARATE, "paragraph-separate").unwrap(),
        )
    }

    fn default_page_re() -> Regex {
        page_delimiter(DEFAULT_PAGE_DELIMITER).unwrap()
    }

    #[test]
    fn paragraph_bounds_walks_blank_lines() {
        let lines: Vec<String> = ["a", "b", "", "c", "d", "e", ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (start, separate) = default_paragraph_rules();
        let rules = ParagraphRules {
            start: &start,
            separate: &separate,
        };
        assert_eq!(paragraph_bounds(&lines, 0, rules), Some((0, 1)));
        assert_eq!(paragraph_bounds(&lines, 4, rules), Some((3, 5)));
        assert_eq!(paragraph_bounds(&lines, 2, rules), None);
    }

    #[test]
    fn forward_paragraph_skips_blanks() {
        let lines: Vec<String> = ["a", "b", "", "c", "d", "e", ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (start, separate) = default_paragraph_rules();
        let rules = ParagraphRules {
            start: &start,
            separate: &separate,
        };
        assert_eq!(forward_paragraph(&lines, 0, rules), 2);
        assert_eq!(forward_paragraph(&lines, 4, rules), 6);
    }

    #[test]
    fn backward_paragraph_moves_up() {
        let lines: Vec<String> = ["a", "b", "", "c", "d", "e", ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (start, separate) = default_paragraph_rules();
        let rules = ParagraphRules {
            start: &start,
            separate: &separate,
        };
        assert_eq!(backward_paragraph(&lines, 4, rules), 3);
        assert_eq!(backward_paragraph(&lines, 3, rules), 0);
        assert_eq!(backward_paragraph(&lines, 5, rules), 3);
    }

    #[test]
    fn paragraph_start_matches_only_some_lines() {
        // Only lines starting with a bullet start a paragraph; the rest of
        // a run are ordinary lines in it.
        let lines: Vec<String> = ["* one", "continued", "", "* two", "more"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let start = paragraph_regexp("^\\*", "paragraph-start").unwrap();
        let separate = paragraph_regexp(DEFAULT_PARAGRAPH_SEPARATE, "paragraph-separate").unwrap();
        let rules = ParagraphRules {
            start: &start,
            separate: &separate,
        };
        assert_eq!(paragraph_bounds(&lines, 1, rules), Some((0, 1)));
        assert_eq!(forward_paragraph(&lines, 1, rules), 2);
        assert_eq!(backward_paragraph(&lines, 1, rules), 0);
        assert_eq!(backward_paragraph(&lines, 4, rules), 3);
    }

    #[test]
    fn line_offsets_and_pages() {
        let t = chars("one\ntwo\n\u{c}three\nfour");
        let re = default_page_re();
        assert_eq!(line_start_offset(&t, 2), 8);
        assert_eq!(offset_line(&t, 9), 2);
        assert_eq!(forward_page(&t, 0, &re), 8);
        assert_eq!(backward_page(&t, 12, &re), 8);
        let (num, start) = page_number(&t, 9, &re);
        assert_eq!((num, start), (2, 9));
        assert_eq!(page_number(&t, 0, &re), (1, 0));
    }

    #[test]
    fn page_delimiter_can_be_another_line() {
        // A line of three dashes separates pages instead of a formfeed.
        let t = chars("a\n---\nb");
        let re = page_delimiter("^---").unwrap();
        assert_eq!(forward_page(&t, 0, &re), 2);
        assert_eq!(backward_page(&t, 5, &re), 2);
        assert_eq!(page_number(&t, 4, &re), (2, 3));
    }

    #[test]
    fn page_line_count_counts_lines() {
        let t = chars("a\nb\n\u{c}c\nd\n");
        let re = default_page_re();
        assert_eq!(page_line_count(&t, 0, &re), 2);
        assert_eq!(page_line_count(&t, 9, &re), 2);
    }

    #[test]
    fn sentence_end_regexp_decides_the_boundary() {
        let t = chars("One two. Three");
        let re = sentence_end_regexp("[.?!]+[\"')]*\r?\n?").unwrap();
        assert_eq!(
            forward_sentence(&t, 0, SentenceEnd::default(), Some(&re)),
            8
        );
        assert_eq!(
            backward_sentence(&t, 14, SentenceEnd::default(), Some(&re)),
            9
        );
        let (s, e) = sentence_at(&t, 14, SentenceEnd::default(), Some(&re));
        assert_eq!(&t[s..e].iter().collect::<String>(), "Three");
    }

    #[test]
    fn transpose_words_swaps_and_moves_point() {
        let mut t = chars("alpha beta gamma");
        let np = transpose_words(&mut t, 10);
        assert_eq!(t.iter().collect::<String>(), "alpha gamma beta");
        assert_eq!(np, 16);
    }

    #[test]
    fn transpose_words_at_end_uses_previous_two() {
        let mut t = chars("one two");
        let np = transpose_words(&mut t, 7);
        assert_eq!(t.iter().collect::<String>(), "two one");
        assert_eq!(np, 7);
    }
}
