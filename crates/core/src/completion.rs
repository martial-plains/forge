// SPDX-License-Identifier: MIT OR Apache-2.0
//! Completion: how candidates match what's typed, and the settings
//! plugins choose (vertical minibuffer lists, a popup at point, matching
//! styles, annotators, sources).
//!
//! The frontend does the per-keystroke work with [`filter`]; plugins are
//! only asked for candidates or annotations once per list (see the
//! `completion` interface in the WIT).

use std::collections::BTreeMap;

/// How typed text matches a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The candidate starts with the input.
    Prefix,
    /// The input appears anywhere (ignoring case unless it has capitals).
    Substring,
    /// The input's characters appear in order ("ffl" matches find-file-literally).
    Flex,
    /// Space-separated parts, each a substring, in any order ("buf kill"
    /// matches kill-buffer); a part starting with ! must not appear.
    Orderless,
    /// partial-completion: `f-b` matches `foo-bar`, each piece a
    /// prefix of a word, and `*` stands for anything.
    Partial,
    /// The input is a regular expression.
    Regexp,
    /// The input's letters begin the words, in order ("ffl" for
    /// find-file-literally).
    Initials,
}

impl Style {
    pub fn from_name(s: &str) -> Result<Self, String> {
        Ok(match s {
            "prefix" | "basic" => Self::Prefix,
            "substring" => Self::Substring,
            "flex" => Self::Flex,
            "orderless" => Self::Orderless,
            "partial-completion" | "partial" => Self::Partial,
            "regexp" => Self::Regexp,
            "initials" | "initialism" => Self::Initials,
            _ => {
                return Err(format!(
                    "unknown matching style {s:?}: use orderless, prefix, substring, flex, partial-completion, regexp or initials"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Substring => "substring",
            Self::Flex => "flex",
            Self::Orderless => "orderless",
            Self::Partial => "partial-completion",
            Self::Regexp => "regexp",
            Self::Initials => "initials",
        }
    }

    /// Does `candidate` match `input`?
    pub fn matches(self, input: &str, candidate: &str) -> bool {
        match self {
            Self::Orderless => return OrderlessQuery::new(input).is_match(candidate),
            Self::Partial => return partial_regex(input).is_none_or(|re| re.is_match(candidate)),
            Self::Regexp => {
                let fold = !input.chars().any(char::is_uppercase);
                return regex::RegexBuilder::new(input)
                    .case_insensitive(fold)
                    .build()
                    .is_ok_and(|re| re.is_match(candidate));
            }
            Self::Initials => {
                return initialism(
                    &input.chars().collect::<Vec<_>>(),
                    candidate,
                    !input.chars().any(char::is_uppercase),
                );
            }
            _ => {}
        }
        let fold = !input.chars().any(char::is_uppercase);
        if !fold {
            // Case-sensitive: no allocation needed.
            return match self {
                Self::Prefix => candidate.starts_with(input),
                Self::Substring => candidate.contains(input),
                Self::Flex => {
                    let mut it = candidate.chars();
                    input.chars().all(|c| it.any(|d| d == c))
                }
                _ => unreachable!(),
            };
        }
        // Case-insensitive. Unicode names (and most candidates) are ASCII,
        // where `eq_ignore_ascii_case` avoids allocating a lowercased copy
        // of every candidate per keystroke.
        if input.is_ascii() && candidate.is_ascii() {
            let ib = input.as_bytes();
            let cb = candidate.as_bytes();
            return match self {
                Self::Prefix => cb.len() >= ib.len() && cb[..ib.len()].eq_ignore_ascii_case(ib),
                Self::Substring => {
                    ib.is_empty()
                        || (ib.len() <= cb.len()
                            && cb.windows(ib.len()).any(|w| w.eq_ignore_ascii_case(ib)))
                }
                Self::Flex => {
                    let mut it = cb.iter();
                    ib.iter().all(|c| it.any(|d| d.eq_ignore_ascii_case(c)))
                }
                _ => unreachable!(),
            };
        }
        let norm = |s: &str| s.to_lowercase();
        let (input, cand) = (norm(input), norm(candidate));
        match self {
            Self::Prefix => cand.starts_with(&input),
            Self::Substring => cand.contains(&input),
            Self::Flex => {
                let mut it = cand.chars();
                input.chars().all(|c| it.any(|d| d == c))
            }
            _ => unreachable!(),
        }
    }
}

/// The regexp for partial-completion: pieces split at `-_./:| ` each must
/// start a word in order, and `*` is anything. None for an empty input.
fn partial_regex(input: &str) -> Option<regex::Regex> {
    if input.is_empty() {
        return None;
    }
    let fold = !input.chars().any(char::is_uppercase);
    let mut pat = String::from("^");
    for c in input.chars() {
        match c {
            '*' => pat.push_str(".*"),
            '-' | '_' | '.' | '/' | ':' | '|' | ' ' => {
                pat.push_str(".*");
                pat.push_str(&regex::escape(&c.to_string()));
            }
            c => pat.push_str(&regex::escape(&c.to_string())),
        }
    }
    regex::RegexBuilder::new(&pat)
        .case_insensitive(fold)
        .build()
        .ok()
}

/// Do `letters` begin words of `candidate`, in order? (orderless-initialism)
fn initialism(letters: &[char], candidate: &str, fold: bool) -> bool {
    let eq = |a: char, b: char| {
        if fold {
            a.to_lowercase().eq(b.to_lowercase())
        } else {
            a == b
        }
    };
    let mut want = letters.iter().peekable();
    let mut prev: Option<char> = None;
    for c in candidate.chars() {
        let starts = match prev {
            None => true,
            Some(p) => !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()),
        };
        if starts
            && c.is_alphanumeric()
            && let Some(&&w) = want.peek()
            && eq(w, c)
        {
            want.next();
        }
        prev = Some(c);
    }
    want.peek().is_none()
}

/// A parsed orderless input: each part is a literal or a regexp, `!` in
/// front excludes, `=` forces a literal, `,` an initialism and `~` flex
/// matching. Parts are separated by spaces; a backslash keeps one in.
pub struct OrderlessQuery {
    parts: Vec<(bool, Part)>,
    fold: bool,
}

enum Part {
    /// A literal, or a regexp the same text also forms.
    Either(String, Option<regex::Regex>),
    Literal(String),
    Initials(Vec<char>),
    Flex(Vec<char>),
}

impl OrderlessQuery {
    pub fn new(input: &str) -> Self {
        let fold = !input.chars().any(char::is_uppercase);
        let mut parts = Vec::new();
        // Split on spaces that aren't escaped.
        let mut words: Vec<String> = vec![String::new()];
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => {
                    chars.next();
                    words.last_mut().unwrap().push(' ');
                }
                c if c.is_whitespace() => {
                    if !words.last().unwrap().is_empty() {
                        words.push(String::new());
                    }
                }
                c => words.last_mut().unwrap().push(c),
            }
        }
        for word in words.into_iter().filter(|w| !w.is_empty()) {
            let (negate, rest) = match word.strip_prefix('!') {
                Some(r) => (true, r.to_string()),
                None => (false, word),
            };
            if rest.is_empty() {
                continue;
            }
            let part = if let Some(r) = rest.strip_prefix('=') {
                Part::Literal(r.to_string())
            } else if let Some(r) = rest.strip_prefix(',') {
                Part::Initials(r.chars().collect())
            } else if let Some(r) = rest.strip_prefix('~') {
                Part::Flex(r.chars().collect())
            } else {
                let re = regex::RegexBuilder::new(&rest)
                    .case_insensitive(fold)
                    .build()
                    .ok();
                Part::Either(rest, re)
            };
            parts.push((negate, part));
        }
        Self { parts, fold }
    }

    pub fn is_match(&self, candidate: &str) -> bool {
        let lower = self.fold.then(|| candidate.to_lowercase());
        let hay = lower.as_deref().unwrap_or(candidate);
        self.parts.iter().all(|(negate, part)| {
            let hit = match part {
                Part::Either(text, re) => {
                    let needle = if self.fold {
                        text.to_lowercase()
                    } else {
                        text.clone()
                    };
                    hay.contains(&needle) || re.as_ref().is_some_and(|r| r.is_match(candidate))
                }
                Part::Literal(text) => {
                    let needle = if self.fold {
                        text.to_lowercase()
                    } else {
                        text.clone()
                    };
                    hay.contains(&needle)
                }
                Part::Initials(l) => initialism(l, candidate, self.fold),
                Part::Flex(l) => {
                    let mut it = hay.chars();
                    l.iter().all(|c| {
                        let c = if self.fold {
                            c.to_lowercase().next().unwrap_or(*c)
                        } else {
                            *c
                        };
                        it.any(|d| d == c)
                    })
                }
            };
            hit != *negate
        })
    }
}

/// How matching candidates are ordered once they match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// Shortest first, then alphabetical (corfu's default).
    Length,
    /// Alphabetical.
    Alpha,
    /// Most recently used first, then length. `rank` is a position in
    /// the caller's history list, most recent first.
    History,
    /// As `History`, but breaking ties alphabetically before length.
    HistoryLength,
    /// The order the source produced: no reordering at all.
    Position,
}

impl Sort {
    pub fn from_name(s: &str) -> Result<Self, String> {
        Ok(match s {
            "length" | "length-alpha" => Self::Length,
            "alpha" | "alphabetic" => Self::Alpha,
            "history" | "history-alpha" => Self::History,
            "history-length" | "history-length-alpha" => Self::HistoryLength,
            "position" | "none" => Self::Position,
            _ => {
                return Err(format!(
                    "unknown sorting style {s:?}: use length, alpha, history, history-length or position"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Alpha => "alpha",
            Self::History => "history",
            Self::HistoryLength => "history-length",
            Self::Position => "position",
        }
    }

    /// Order `hits` (indices into `candidates`) by this sort. `rank` maps
    /// a candidate to its history position, most recent first.
    ///
    /// `hits` is sorted stably, so candidates a sort considers equal keep
    /// the order they were filtered in.
    pub fn apply(
        self,
        hits: &mut [usize],
        candidates: &[String],
        rank: &dyn Fn(&str) -> Option<usize>,
    ) {
        match self {
            Self::Position => {}
            Self::Alpha => hits.sort_by_key(|&i| candidates[i].clone()),
            Self::Length => {
                hits.sort_by_key(|&i| (candidates[i].chars().count(), candidates[i].clone()));
            }
            // Unknown to the history sorts sort last, then fall back to
            // length so the order is still stable across runs.
            Self::History | Self::HistoryLength => {
                hits.sort_by_key(|&i| {
                    (
                        rank(&candidates[i]).unwrap_or(usize::MAX),
                        candidates[i].chars().count(),
                        candidates[i].clone(),
                    )
                });
            }
        }
    }
}

/// How the popup preselects a candidate (corfu-preselect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preselect {
    /// Always select nothing until you move.
    Prompt,
    /// Always select the first candidate.
    First,
    /// Select nothing only when the input is itself a valid completion
    /// that is not the first candidate (corfu's default).
    Valid,
    /// Like `First`, but select nothing when the input names a directory.
    Directory,
}

impl Preselect {
    pub fn from_name(s: &str) -> Result<Self, String> {
        Ok(match s {
            "prompt" => Self::Prompt,
            "first" => Self::First,
            "valid" => Self::Valid,
            "directory" => Self::Directory,
            _ => {
                return Err(format!(
                    "unknown preselect {s:?}: use prompt, first, valid or directory"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::First => "first",
            Self::Valid => "valid",
            Self::Directory => "directory",
        }
    }
}

/// What to do when the input exactly matches a candidate
/// (corfu-on-exact-match).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactMatch {
    /// Do nothing special; the exact match is dropped from the list.
    Ignore,
    /// Insert it and close.
    Insert,
    /// Keep the popup open and show it.
    Show,
    /// Close the popup.
    Quit,
}

impl ExactMatch {
    pub fn from_name(s: &str) -> Result<Self, String> {
        Ok(match s {
            "" | "nil" | "off" | "false" => Self::Ignore,
            "insert" | "t" => Self::Insert,
            "show" => Self::Show,
            "quit" => Self::Quit,
            _ => {
                return Err(format!(
                    "unknown on-exact-match {s:?}: use nil, insert, show or quit"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ignore => "nil",
            Self::Insert => "insert",
            Self::Show => "show",
            Self::Quit => "quit",
        }
    }
}

/// What to do with the selected candidate when you type something that is
/// not a continue command (corfu-preview-current). `insert` commits the
/// selected candidate before the typed input is applied; corfu's `t`
/// (preview the candidate in the buffer without committing) is not drawn,
/// so it means off here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewCurrent {
    /// Leave the selection to the popup's refiltering (corfu's nil).
    Off,
    /// Commit the selected candidate before applying the input.
    Insert,
}

impl PreviewCurrent {
    pub fn from_name(s: &str) -> Result<Self, String> {
        Ok(match s {
            "" | "nil" | "off" | "false" | "t" => Self::Off,
            "insert" => Self::Insert,
            _ => {
                return Err(format!(
                    "unknown preview-current {s:?}: use nil, t or insert"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "nil",
            Self::Insert => "insert",
        }
    }
}

/// Candidates matching `input`, by the first style that matches any,
/// best first: exact, then prefix matches, then the rest, each in their
/// original order (so a caller's order, like recent first, is kept).
///
/// `sort` (empty: the default exact/prefix/rest order) reorders those
/// hits. `rank` gives a candidate's history position for the sorts that
/// use it.
pub fn filter_with<'a>(
    styles: &[Style],
    sort: &[Sort],
    input: &str,
    candidates: &'a [String],
    rank: &dyn Fn(&str) -> Option<usize>,
) -> Vec<&'a String> {
    let hits = filter(styles, input, candidates);
    if sort.is_empty() {
        return hits;
    }
    // Only the first sorting style applies, like matching.
    let order = sort[0];
    // Hits borrow from `candidates`: their index is their pointer offset,
    // not a linear search per hit (quadratic over every Unicode name).
    let base = candidates.as_ptr() as usize;
    let size = std::mem::size_of::<String>();
    let mut idx: Vec<usize> = hits
        .iter()
        .map(|c| {
            let i = (*c as *const String as usize - base) / size;
            debug_assert!(std::ptr::eq(&candidates[i], *c));
            i
        })
        .collect();
    order.apply(&mut idx, candidates, rank);
    idx.into_iter().map(|i| &candidates[i]).collect()
}

pub fn filter<'a>(styles: &[Style], input: &str, candidates: &'a [String]) -> Vec<&'a String> {
    if input.is_empty() {
        return candidates.iter().collect();
    }
    let default = [Style::Prefix];
    let styles = if styles.is_empty() {
        &default[..]
    } else {
        styles
    };
    for style in styles {
        let mut hits: Vec<&String> = match style {
            // Compiled once for the whole list, not once per candidate.
            Style::Orderless => {
                let q = OrderlessQuery::new(input);
                candidates.iter().filter(|c| q.is_match(c)).collect()
            }
            Style::Partial => match partial_regex(input) {
                Some(re) => candidates.iter().filter(|c| re.is_match(c)).collect(),
                None => candidates.iter().collect(),
            },
            Style::Regexp => {
                let fold = !input.chars().any(char::is_uppercase);
                match regex::RegexBuilder::new(input)
                    .case_insensitive(fold)
                    .build()
                {
                    Ok(re) => candidates.iter().filter(|c| re.is_match(c)).collect(),
                    Err(_) => Vec::new(),
                }
            }
            _ => candidates
                .iter()
                .filter(|c| style.matches(input, c))
                .collect(),
        };
        if !hits.is_empty() {
            let lower = input.to_lowercase();
            let lower_ascii = lower.is_ascii();
            let lb = lower.as_bytes();
            hits.sort_by_key(|c| {
                if lower_ascii && c.is_ascii() {
                    let cb = c.as_bytes();
                    if cb.len() == lb.len() && cb.eq_ignore_ascii_case(lb) {
                        0
                    } else if cb.len() >= lb.len() && cb[..lb.len()].eq_ignore_ascii_case(lb) {
                        1
                    } else {
                        2
                    }
                } else {
                    let l = c.to_lowercase();
                    if l == lower {
                        0
                    } else if l.starts_with(&lower) {
                        1
                    } else {
                        2
                    }
                }
            });
            return hits;
        }
    }
    Vec::new()
}

/// The characters default `crm-separator` splits on: a comma or a
/// tab. A multiple-value prompt (`completing-read-multiple`) completes and
/// validates one field at a time, separated by these.
pub const CRM_SEPARATORS: [char; 2] = [',', '\t'];

/// The input of a multiple-value prompt, split around the field being
/// typed. Everything after the last separator is the field completion
/// matches against; everything before it (with the separator and any
/// spaces) is kept so completing the field leaves it in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields<'a> {
    /// The fields before the current one, trimmed and without empties.
    pub done: Vec<&'a str>,
    /// The input up to and including the last separator and the spaces
    /// after it. Empty when there is no separator yet.
    pub prefix: &'a str,
    /// The field being typed (leading spaces after the separator removed).
    pub field: &'a str,
}

impl<'a> Fields<'a> {
    /// Every entry, including the current field: what a submit validates
    /// and hands back. The current field is dropped when it is blank, so
    /// "a, b, " is the two entries "a" and "b".
    pub fn all(&self) -> Vec<&'a str> {
        let mut all = self.done.clone();
        let field = self.field.trim();
        if !field.is_empty() {
            all.push(field);
        }
        all
    }
}

/// Split `input` around the last of `separators` (see [`Fields`]).
pub fn fields<'a>(input: &'a str, separators: &[char]) -> Fields<'a> {
    let Some(i) = input.rfind(|c| separators.contains(&c)) else {
        return Fields {
            done: Vec::new(),
            prefix: "",
            field: input,
        };
    };
    let sep_len = input[i..].chars().next().map_or(0, char::len_utf8);
    let after = &input[i + sep_len..];
    let spaces = after.len() - after.trim_start().len();
    Fields {
        done: input[..i]
            .split(|c| separators.contains(&c))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect(),
        prefix: &input[..i + sep_len + spaces],
        field: after.trim_start(),
    }
}

/// How completion at point looks and when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtPoint {
    pub popup: bool,
    pub auto: bool,
    pub delay_ms: u32,
    pub min_prefix: u32,
    pub lines: u32,
}

impl Default for AtPoint {
    fn default() -> Self {
        Self {
            popup: false,
            auto: false,
            delay_ms: 200,
            min_prefix: 3,
            lines: 10,
        }
    }
}

/// A plugin's completion source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub plugin: String,
    /// Major modes it serves (and those derived from them); empty: all.
    pub modes: Vec<String>,
}

/// How wide the popup at point may grow to fit its candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Frame {
    /// Columns; 0 leaves that end to Forge.
    pub min_width: u32,
    pub max_width: u32,
    /// Accepted for API parity; a terminal draws no border.
    pub border: u32,
}

impl Frame {
    /// Clamp `wanted` (the widest row) into this frame's range. The
    /// window's own width is the hard limit and is passed in.
    pub fn width(&self, wanted: u32, available: u32) -> u32 {
        let min = self.min_width.min(available);
        let max = match self.max_width {
            0 => available,
            m => m.max(min),
        };
        wanted.clamp(min, max).min(available).max(1)
    }
}

/// The auxiliary popup beside the completion popup (corfu-popupinfo):
/// what to show and how big. Set by a plugin (completion.set-popup-info);
/// Forge draws the selection's documentation and/or location in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub show_documentation: bool,
    pub show_location: bool,
    /// Wait after the selection settles before showing (0: at once).
    pub delay_ms: u32,
    /// Columns; 0 leaves it to Forge.
    pub max_width: u32,
    /// Rows; 0 leaves it to Forge.
    pub max_height: u32,
}

impl Default for Info {
    fn default() -> Self {
        Self {
            show_documentation: true,
            show_location: false,
            delay_ms: 1000,
            max_width: 0,
            max_height: 0,
        }
    }
}

/// Where a candidate is defined: the file, when it is not the buffer the
/// list is in, and a place in it (corfu's M-g).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The file it is defined in. None: the current buffer.
    pub path: Option<std::path::PathBuf>,
    pub line: u32,
    pub column: u32,
}

/// A completion candidate with its display decoration: the `text` to
/// insert, an `annotation` after it, an optional `margin` (an icon or a
/// kind label) drawn before it, and whether it is `deprecated` (shown
/// struck through / dimmed). `text` is what filter, history and insert
/// use; the rest is presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub annotation: Option<String>,
    pub margin: Option<String>,
    pub deprecated: bool,
    /// Longer text shown for the selection (corfu's M-h).
    pub documentation: Option<String>,
    /// Where the candidate is defined (corfu's M-g).
    pub location: Option<Location>,
}

impl Candidate {
    /// A plain candidate: the text to insert, with an optional annotation.
    pub fn new(text: impl Into<String>, annotation: Option<String>) -> Self {
        Self {
            text: text.into(),
            annotation,
            margin: None,
            deprecated: false,
            documentation: None,
            location: None,
        }
    }
}

impl From<String> for Candidate {
    fn from(text: String) -> Self {
        Self::new(text, None)
    }
}

impl From<&str> for Candidate {
    fn from(text: &str) -> Self {
        Self::new(text, None)
    }
}

/// The candidate under the cursor in an open list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub text: String,
    pub annotation: Option<String>,
    pub category: String,
    /// 1-based, as shown to the user.
    pub index: u32,
    pub total: u32,
    /// The selection's documentation, if the source gave one.
    pub documentation: Option<String>,
    /// Where the selection is defined, if the source said.
    pub location: Option<Location>,
}

/// How the popup treats the completion boundary (`quit-at-boundary`) or a
/// no-match (`quit-no-match`): never quit, always quit, or quit only once
/// a `separator` is in the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quit {
    Never,
    Always,
    #[default]
    Separator,
}

impl Quit {
    pub fn from_name(s: &str) -> Self {
        match s {
            "never" | "nil" | "false" => Self::Never,
            "always" | "t" | "true" => Self::Always,
            _ => Self::Separator,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Always => "always",
            Self::Separator => "separator",
        }
    }
}

/// What a quickselect key does with the candidate it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    Insert,
    Complete,
    Jump,
}

impl QuickAction {
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "insert" => Self::Insert,
            "complete" => Self::Complete,
            "jump" => Self::Jump,
            _ => return None,
        })
    }
}

/// The popup's tunable policy: what a completion plugin (corfu) chooses.
/// Each field mirrors one of corfu's options; the frontend reads them as it
/// draws and updates the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// C-n on the last candidate wraps to the first.
    pub cycle: bool,
    /// Nothing is selected until you move (corfu-preselect 'prompt).
    pub preselect_prompt: bool,
    /// The component separator (corfu-separator); '\0' for none.
    pub separator: char,
    pub quit_at_boundary: Quit,
    pub quit_no_match: Quit,
    /// Names (or `name*` prefixes) that keep completion open at the
    /// boundary (corfu-continue-commands).
    pub continue_commands: Vec<String>,
    /// Number the popup's rows (corfu-indexed-mode).
    pub indexed: bool,
    /// Quickselect keys: the first key labels the first candidates, the
    /// second groups the rest (corfu-quick1 / corfu-quick2).
    pub quick1: String,
    pub quick2: String,
    /// Show the selected candidate's annotation after a pause
    /// (corfu-echo-mode), after `echo_delay_ms`.
    pub echo: bool,
    pub echo_delay_ms: u32,
    /// Click a row to take it (corfu-mouse-mode).
    pub mouse: bool,
    /// Rank candidates you took before first (corfu-history-mode).
    pub history: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            cycle: true,
            preselect_prompt: false,
            separator: ' ',
            quit_at_boundary: Quit::Separator,
            quit_no_match: Quit::Separator,
            continue_commands: vec![
                "ignore".into(),
                "universal-argument".into(),
                "universal-argument-more".into(),
                "digit-argument".into(),
                "mwheel-scroll".into(),
                "corfu-*".into(),
                "completion-popup-*".into(),
                "scroll-other-window*".into(),
            ],
            indexed: false,
            quick1: "asdfgh".into(),
            quick2: "jkluionm".into(),
            echo: false,
            echo_delay_ms: 1000,
            mouse: false,
            history: false,
        }
    }
}

/// The popup's session, kept up to date by the frontend so a plugin can
/// drive it (`completion.current`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Session {
    pub open: bool,
    pub popup: bool,
    /// 0-based position of the selected candidate among the matches.
    pub index: u32,
    pub total: u32,
    /// Nothing is selected (corfu-preselect 'prompt, or after a reset).
    pub deselected: bool,
    /// How many rows the popup shows.
    pub lines: u32,
    /// The input typed into the popup.
    pub input: String,
}

/// A request from a plugin to drive the popup; the frontend drains these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Next(i32),
    Previous(i32),
    First,
    Last,
    Scroll {
        down: bool,
    },
    /// Take the selected candidate (corfu-complete / corfu-insert).
    Accept,
    /// Grow the input to what every candidate shares (corfu-expand).
    Expand,
    Quit,
    Reset,
    /// Take the candidate on popup row `row` (corfu-indexed's C-u N TAB).
    AcceptRow(u32),
    /// Wait for the next key to name a candidate (corfu-quick-*).
    ArmQuick(QuickAction),
    /// Insert the component separator and keep completing
    /// (corfu-insert-separator).
    InsertSeparator,
    /// Turn the auxiliary popup off, or back on (corfu-popupinfo-toggle).
    TogglePopupInfo,
    /// Scroll the auxiliary popup a page down (or up)
    /// (corfu-popupinfo-scroll-down / -scroll-up).
    PopupInfoScroll {
        down: bool,
    },
    /// Scroll the auxiliary popup to its first line (corfu-popupinfo-beginning).
    PopupInfoBeginning,
    /// Scroll the auxiliary popup to its last line (corfu-popupinfo-end).
    PopupInfoEnd,
}

/// What plugins chose; each setting remembers who set it, so unloading
/// that plugin puts the default back.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Vertical minibuffer completion: how many lines (None: off).
    pub vertical: Option<(u32, String)>,
    pub styles: Option<(Vec<Style>, String)>,
    /// Category -> the plugin that annotates it.
    pub annotators: BTreeMap<String, String>,
    pub at_point: Option<(AtPoint, String)>,
    pub sources: Vec<Source>,
    /// How matches are ordered (empty: exact, then prefix, then the rest).
    pub sorting: Option<(Vec<Sort>, String)>,
    /// Inputs whose matches keep the source order instead of `sorting`
    /// (corfu-sort-override-function): regexp patterns; when the typed
    /// input matches any, sorting is skipped.
    pub sort_override: Option<(Vec<String>, String)>,
    pub frame: Option<(Frame, String)>,
    /// The auxiliary popup beside the popup (corfu-popupinfo).
    pub info: Option<(Info, String)>,
    /// Whether the minibuffer list wraps around at the ends
    /// (vertico-cycle; off means C-n / C-p stop).
    pub list_cycle: Option<(bool, String)>,
    /// The popup's policy (cycle, separator, quit rules, modes).
    pub policy: Option<(Policy, String)>,
    /// How the popup preselects (corfu-preselect).
    pub preselect: Option<(Preselect, String)>,
    /// What to do on an exact match (corfu-on-exact-match).
    pub on_exact_match: Option<(ExactMatch, String)>,
    /// What to do with the selection when typing (corfu-preview-current).
    pub preview_current: Option<(PreviewCurrent, String)>,
    /// Auto mode's trigger characters: typing one of these (with a
    /// self-inserting command) arms the popup (corfu-auto-trigger). Empty
    /// means every character.
    pub auto_trigger: Option<(Vec<char>, String)>,
    /// Commands that may arm auto mode (corfu-auto-commands); a trailing
    /// `*` matches by prefix. Empty: `self-insert-command` only.
    pub auto_commands: Option<(Vec<String>, String)>,
    /// Commands to run when the list's selection changes (consult live
    /// preview): (plugin, command) pairs.
    pub selection_commands: Vec<(String, String)>,
}

impl Settings {
    pub fn vertical_lines(&self) -> Option<u32> {
        self.vertical.as_ref().map(|v| v.0)
    }

    pub fn styles(&self) -> &[Style] {
        self.styles.as_ref().map_or(&[], |s| &s.0[..])
    }

    pub fn at_point(&self) -> AtPoint {
        self.at_point
            .as_ref()
            .map(|a| a.0.clone())
            .unwrap_or_default()
    }

    pub fn sorting(&self) -> &[Sort] {
        self.sorting.as_ref().map_or(&[], |s| &s.0[..])
    }

    /// How the popup preselects: an explicit `corfu_preselect` wins, else
    /// the plugin's `preselect-prompt` flag, else `valid` (corfu's default).
    pub fn preselect(&self) -> Preselect {
        if let Some((p, _)) = self.preselect {
            return p;
        }
        match self.policy.as_ref() {
            Some((p, _)) if p.preselect_prompt => Preselect::Prompt,
            _ => Preselect::Valid,
        }
    }

    /// What to do on an exact match (corfu-on-exact-match).
    pub fn on_exact_match(&self) -> ExactMatch {
        self.on_exact_match
            .as_ref()
            .map_or(ExactMatch::Ignore, |e| e.0)
    }

    /// What to do with the selection when typing (corfu-preview-current).
    pub fn preview_current(&self) -> PreviewCurrent {
        self.preview_current
            .as_ref()
            .map_or(PreviewCurrent::Off, |p| p.0)
    }

    /// Whether `command` (with `ch` being the character it inserted, if it
    /// is a self-inserting command) may arm auto mode (corfu-auto-commands /
    /// corfu-auto-trigger).
    pub fn auto_arms(&self, command: &str, ch: Option<char>) -> bool {
        let listed = match self.auto_commands.as_ref() {
            Some((cmds, _)) if !cmds.is_empty() => cmds.iter().any(|c| {
                c == command
                    || c.strip_suffix('*')
                        .is_some_and(|pre| command.starts_with(pre))
            }),
            _ => command == "self-insert-command",
        };
        if !listed {
            return false;
        }
        let trig = self.auto_trigger.as_ref().map_or(&[][..], |t| &t.0[..]);
        trig.is_empty() || command != "self-insert-command" || ch.is_some_and(|c| trig.contains(&c))
    }

    /// Whether `input` is covered by a sort override (keep source order).
    pub fn sort_overridden(&self, input: &str) -> bool {
        self.sort_override.as_ref().is_some_and(|(pats, _)| {
            pats.iter().any(|p| {
                regex::Regex::new(p)
                    .map(|re| re.is_match(input))
                    .unwrap_or_else(|_| input.contains(p.as_str()))
            })
        })
    }

    pub fn frame(&self) -> Frame {
        self.frame.as_ref().map_or_else(Frame::default, |f| f.0)
    }

    /// The auxiliary popup beside the popup, if a plugin asked for one.
    pub fn info(&self) -> Option<Info> {
        self.info.as_ref().map(|i| i.0)
    }

    /// Whether the minibuffer list wraps around at the ends.
    pub fn list_cycle(&self) -> bool {
        self.list_cycle.as_ref().is_some_and(|c| c.0)
    }

    pub fn policy(&self) -> Policy {
        self.policy
            .as_ref()
            .map(|p| p.0.clone())
            .unwrap_or_default()
    }

    /// Forget what `plugin` set (it was unloaded).
    pub fn remove_plugin(&mut self, plugin: &str) {
        if self.vertical.as_ref().is_some_and(|v| v.1 == plugin) {
            self.vertical = None;
        }
        if self.styles.as_ref().is_some_and(|v| v.1 == plugin) {
            self.styles = None;
        }
        if self.at_point.as_ref().is_some_and(|v| v.1 == plugin) {
            self.at_point = None;
        }
        if self.sorting.as_ref().is_some_and(|s| s.1 == plugin) {
            self.sorting = None;
        }
        if self.sort_override.as_ref().is_some_and(|s| s.1 == plugin) {
            self.sort_override = None;
        }
        if self.frame.as_ref().is_some_and(|f| f.1 == plugin) {
            self.frame = None;
        }
        if self.info.as_ref().is_some_and(|i| i.1 == plugin) {
            self.info = None;
        }
        if self.list_cycle.as_ref().is_some_and(|c| c.1 == plugin) {
            self.list_cycle = None;
        }
        if self.policy.as_ref().is_some_and(|p| p.1 == plugin) {
            self.policy = None;
        }
        if self.preselect.as_ref().is_some_and(|p| p.1 == plugin) {
            self.preselect = None;
        }
        if self.on_exact_match.as_ref().is_some_and(|e| e.1 == plugin) {
            self.on_exact_match = None;
        }
        if self.preview_current.as_ref().is_some_and(|p| p.1 == plugin) {
            self.preview_current = None;
        }
        if self.auto_trigger.as_ref().is_some_and(|t| t.1 == plugin) {
            self.auto_trigger = None;
        }
        if self.auto_commands.as_ref().is_some_and(|c| c.1 == plugin) {
            self.auto_commands = None;
        }
        self.annotators.retain(|_, p| p != plugin);
        self.sources.retain(|s| s.plugin != plugin);
        self.selection_commands.retain(|(p, _)| p != plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: Vec<&String>) -> Vec<&str> {
        v.into_iter().map(String::as_str).collect()
    }

    #[test]
    fn styles_match() {
        assert!(Style::Orderless.matches("buf kill", "kill-buffer"));
        assert!(!Style::Orderless.matches("buf !kill", "kill-buffer"));
        assert!(Style::Flex.matches("ffl", "find-file-literally"));
        assert!(!Style::Flex.matches("lff", "find-file"));
        assert!(Style::Substring.matches("File", "find-File"));
        assert!(
            !Style::Substring.matches("File", "find-file"),
            "capitals match case"
        );
        assert!(!Style::Prefix.matches("FIND", "find"));
        assert!(Style::from_name("fuzzy").is_err());
    }

    #[test]
    fn fields_split_a_multiple_value_prompt() {
        // Nothing typed yet: the whole input is the field.
        let f = fields("red", &CRM_SEPARATORS);
        assert_eq!((f.done.as_slice(), f.prefix, f.field), (&[][..], "", "red"));
        assert_eq!(f.all(), ["red"]);

        // A comma (and the spaces after it) is kept in the prefix, so
        // completing the field leaves "red, " in place.
        let f = fields("red, gr", &CRM_SEPARATORS);
        assert_eq!(
            (f.done.as_slice(), f.prefix, f.field),
            (&["red"][..], "red, ", "gr")
        );
        assert_eq!(f.all(), ["red", "gr"]);

        // A tab separates too, like default crm-separator.
        let f = fields("red\tgr", &CRM_SEPARATORS);
        assert_eq!(
            (f.done.as_slice(), f.prefix, f.field),
            (&["red"][..], "red\t", "gr")
        );

        // Empty fields between separators are dropped from the prefix
        // search but the trailing one is what's being typed.
        let f = fields("red, , bl", &CRM_SEPARATORS);
        assert_eq!(f.done.as_slice(), &["red"][..]);
        assert_eq!((f.prefix, f.field), ("red, , ", "bl"));
        assert_eq!(f.all(), ["red", "bl"]);

        // A trailing separator means the next field is empty but still
        // counts as every entry so far.
        let f = fields("red, gr, ", &CRM_SEPARATORS);
        assert_eq!(f.all(), ["red", "gr"]);
        assert_eq!(f.field, "");
    }

    #[test]
    fn filtering_orders_exact_and_prefix_first() {
        let c: Vec<String> = ["kill-buffer", "buffer-menu", "buffer", "list-buffers"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            names(filter(&[Style::Substring], "buffer", &c)),
            ["buffer", "buffer-menu", "kill-buffer", "list-buffers"]
        );
        assert_eq!(names(filter(&[], "buf", &c)), ["buffer-menu", "buffer"]);
        // The first style with matches wins.
        assert_eq!(
            names(filter(&[Style::Prefix, Style::Flex], "lb", &c)),
            ["kill-buffer", "list-buffers"]
        );
        assert_eq!(filter(&[], "", &c).len(), 4);
    }

    #[test]
    fn sorting_orders_matches_after_filtering() {
        let c: Vec<String> = ["alpha", "be", "beta-long", "gamma"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let none = |_: &str| None;
        let sorted = |style: &str| {
            names(filter_with(
                &[Style::Prefix],
                &[Sort::from_name(style).expect("known")],
                "",
                &c,
                &none,
            ))
        };
        // Shortest first, then alphabetically.
        assert_eq!(sorted("length"), ["be", "alpha", "gamma", "beta-long"]);
        assert_eq!(sorted("alpha"), ["alpha", "be", "beta-long", "gamma"]);
        assert_eq!(sorted("position"), ["alpha", "be", "beta-long", "gamma"]);

        // History: most recent first, unknown names last by length.
        let hist: Vec<String> = ["gamma", "alpha", "beta-long"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rank = |c: &str| hist.iter().position(|h| h == c);
        let names_in = |style: &str| {
            names(filter_with(
                &[Style::Prefix],
                &[Sort::from_name(style).expect("known")],
                "",
                &c,
                &rank,
            ))
        };
        assert_eq!(names_in("history"), ["gamma", "alpha", "beta-long", "be"]);
        assert_eq!(
            names_in("history-length"),
            ["gamma", "alpha", "beta-long", "be"]
        );

        // Only the first style applies, like matching.
        assert_eq!(
            names(filter_with(
                &[Style::Prefix],
                &[Sort::Alpha, Sort::Length],
                "",
                &c,
                &none
            )),
            ["alpha", "be", "beta-long", "gamma"]
        );
        // Filtering still happens first: the sort only reorders matches.
        assert_eq!(sorted("length").len(), 4);
        assert_eq!(
            names(filter_with(
                &[Style::Prefix],
                &[Sort::Length],
                "gam",
                &c,
                &none
            )),
            ["gamma"]
        );
    }

    #[test]
    fn popup_frames_clamp_to_the_window() {
        let f = Frame {
            min_width: 20,
            max_width: 60,
            border: 0,
        };
        assert_eq!(f.width(10, 80), 20, "never narrower than min");
        assert_eq!(f.width(100, 80), 60, "never wider than max");
        assert_eq!(f.width(40, 80), 40, "what it asks for");
        // The window is the hard limit on both ends.
        assert_eq!(f.width(100, 30), 30);
        assert_eq!(f.width(1, 30), 20, "min is clipped to the window");
        // Zero on either end leaves it to Forge.
        let open = Frame {
            min_width: 0,
            max_width: 0,
            border: 0,
        };
        assert_eq!(open.width(37, 80), 37);
        let maxed = Frame {
            min_width: 0,
            max_width: 25,
            border: 0,
        };
        assert_eq!(maxed.width(60, 80), 25);
        // A max below the min is the min, not a panic.
        let silly = Frame {
            min_width: 30,
            max_width: 10,
            border: 0,
        };
        assert_eq!(silly.width(1, 80), 30);
        assert!(f.width(0, 0) >= 1, "never zero");
    }

    #[test]
    fn unloading_restores_defaults() {
        let mut s = Settings {
            vertical: Some((10, "v".into())),
            info: Some((Info::default(), "v".into())),
            ..Default::default()
        };
        s.annotators.insert("command".into(), "v".into());
        s.sources.push(Source {
            name: "words".into(),
            plugin: "c".into(),
            modes: vec![],
        });
        s.remove_plugin("v");
        assert_eq!(s.vertical_lines(), None);
        assert_eq!(s.info(), None, "the auxiliary popup setting is dropped");
        assert!(s.annotators.is_empty());
        assert_eq!(s.sources.len(), 1);
    }

    #[test]
    fn preselect_and_exact_match_names_round_trip() {
        for p in [
            Preselect::Prompt,
            Preselect::First,
            Preselect::Valid,
            Preselect::Directory,
        ] {
            assert_eq!(Preselect::from_name(p.name()).unwrap(), p);
        }
        for e in [
            ExactMatch::Ignore,
            ExactMatch::Insert,
            ExactMatch::Show,
            ExactMatch::Quit,
        ] {
            assert_eq!(ExactMatch::from_name(e.name()).unwrap(), e);
        }
        assert!(Preselect::from_name("nope").is_err());
        assert!(ExactMatch::from_name("nope").is_err());
    }

    #[test]
    fn preview_current_defaults_to_off_and_round_trips() {
        assert_eq!(Settings::default().preview_current(), PreviewCurrent::Off);
        for p in [PreviewCurrent::Off, PreviewCurrent::Insert] {
            assert_eq!(PreviewCurrent::from_name(p.name()).unwrap(), p);
        }
        // corfu's `t` previews without committing; with no overlay, off.
        assert_eq!(PreviewCurrent::from_name("t").unwrap(), PreviewCurrent::Off);
        assert!(PreviewCurrent::from_name("nope").is_err());
        let s = Settings {
            preview_current: Some((PreviewCurrent::Insert, "c".into())),
            ..Default::default()
        };
        assert_eq!(s.preview_current(), PreviewCurrent::Insert);
    }

    #[test]
    fn preselect_defaults_to_valid_and_honors_prompt_flag() {
        let s = Settings::default();
        assert_eq!(s.preselect(), Preselect::Valid);
        let s = Settings {
            policy: Some((
                Policy {
                    preselect_prompt: true,
                    ..Default::default()
                },
                "p".into(),
            )),
            ..Default::default()
        };
        assert_eq!(s.preselect(), Preselect::Prompt);
        let s = Settings {
            preselect: Some((Preselect::First, "c".into())),
            policy: Some((
                Policy {
                    preselect_prompt: true,
                    ..Default::default()
                },
                "p".into(),
            )),
            ..Default::default()
        };
        assert_eq!(s.preselect(), Preselect::First, "config wins");
    }

    #[test]
    fn auto_arms_checks_commands_and_trigger_characters() {
        let mut s = Settings::default();
        assert!(s.auto_arms("self-insert-command", Some('a')));
        assert!(!s.auto_arms("corfu-next", None));
        s.auto_trigger = Some((vec!['l', '.'], "c".into()));
        assert!(s.auto_arms("self-insert-command", Some('l')));
        assert!(!s.auto_arms("self-insert-command", Some('a')));
        s.auto_commands = Some((vec!["corfu-*".into()], "c".into()));
        assert!(s.auto_arms("corfu-next", None), "prefix match");
        assert!(!s.auto_arms("self-insert-command", Some('l')));
    }

    #[test]
    fn orderless_parts_are_literals_or_regexps() {
        let m = |i: &str, c: &str| Style::Orderless.matches(i, c);
        // A regexp part, anchors included.
        assert!(m("^kill", "kill-buffer"));
        assert!(!m("^buffer", "kill-buffer"));
        assert!(m("buffer$", "kill-buffer"));
        assert!(m("k.*r", "kill-buffer"));
        // Literal and regexp together, any order, with negation.
        assert!(m("buffer ^kill", "kill-buffer"));
        assert!(!m("buffer !kill", "kill-buffer"));
        // A part that is no valid regexp still matches as text.
        assert!(m("c++", "c++-mode"));
        // Dispatchers: literal, initialism, flex.
        assert!(m("=k.l", "k.ll") && !m("=k.l", "kill"));
        assert!(m(",fb", "find-buffer") && m(",fb", "findBuffer"));
        assert!(!m(",bf", "find-buffer"));
        assert!(m("~fbr", "find-buffer"));
        // An escaped space is part of the text.
        assert!(m("a\\ b", "xa bx") && !m("a\\ b", "ab"));
        // Smart case still holds.
        assert!(m("KILL", "KILL-x") && !m("KILL", "kill-x"));
    }

    #[test]
    fn partial_completion_and_friends() {
        assert!(Style::Partial.matches("f-b", "foo-bar"));
        assert!(Style::Partial.matches("fi-fi", "find-file"));
        assert!(!Style::Partial.matches("b-f", "find-file"));
        assert!(Style::Partial.matches("fo*ar", "foo-bar"));
        assert!(Style::Regexp.matches("^f.+r$", "foo-bar"));
        assert!(Style::Initials.matches("ffl", "find-file-literally"));
        assert_eq!(Style::from_name("partial-completion"), Ok(Style::Partial));
    }
}
