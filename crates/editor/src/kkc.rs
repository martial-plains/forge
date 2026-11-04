// SPDX-License-Identifier: MIT OR Apache-2.0
//! Kana-kanji conversion for the `japanese` input method.
//!
//! - `data/ja-dict.txt.gz` is the dictionary: the readings and written
//!   forms of Mozc's open-source dictionary (BSD-3-Clause, Copyright
//!   Google Inc.; see THIRD_PARTY_NOTICES.md), the cheapest candidates
//!   first, built by `scripts/build-kana-kanji-dict.py`. Words are
//!   looked up as they are written in the dictionary: conjugated forms
//!   that it doesn't list convert in pieces.
//! - Hiragana / katakana / half-width conversions follow the layout of
//!   Unicode (hiragana and katakana are 0x60 apart; the half-width forms
//!   come from the compatibility mappings).
//! - [`lookup`] is the dictionary lookup, [`Kkc`] a conversion in progress:
//!   the current phrase, its candidates and the keys that change them.
//!   Picks are learned for the session ([`Learned`]).

use std::collections::HashMap;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

static DIC: &[u8] = include_bytes!("../data/ja-dict.txt.gz");

/// The dictionary: candidates by reading.
struct Dict {
    words: HashMap<String, Vec<String>>,
}

/// The bytes of a gzip file, inflated.
fn gunzip(data: &[u8]) -> Vec<u8> {
    let flags = data[3];
    let mut at = 10;
    if flags & 4 != 0 {
        let n = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
        at += 2 + n;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            while data[at] != 0 {
                at += 1;
            }
            at += 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(&data[at..data.len() - 8])
        .expect("ja-dict.txt.gz inflates")
}

fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut cs = s.chars();
    while let Some(c) = cs.next() {
        if c == '\\' {
            match cs.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn dict() -> &'static Dict {
    static D: OnceLock<Dict> = OnceLock::new();
    D.get_or_init(|| {
        let text = String::from_utf8(gunzip(DIC)).expect("ja-dict is UTF-8");
        let mut words = HashMap::new();
        for line in text.lines() {
            let mut f = line.split('\t');
            let Some(key) = f.next() else { continue };
            words.insert(key.to_string(), f.map(unescape).collect());
        }
        Dict { words }
    })
}

/// japanese-katakana: hiragana made katakana.
pub fn katakana(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{3041}'..='\u{3096}' | '\u{309d}' | '\u{309e}' => {
                char::from_u32(c as u32 + 0x60).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

/// japanese-hiragana: katakana made hiragana.
pub fn hiragana(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{30a1}'..='\u{30f6}' | '\u{30fd}' | '\u{30fe}' => {
                char::from_u32(c as u32 - 0x60).unwrap_or(c)
            }
            _ => c,
        })
        .collect()
}

/// The half-width form of each full-width katakana and punctuation mark,
/// found by asking Unicode what each half-width character is compatible
/// with.
fn halfwidth_map() -> &'static HashMap<char, char> {
    static M: OnceLock<HashMap<char, char>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        for h in '\u{ff61}'..='\u{ff9f}' {
            let mut full = std::iter::once(h).nfkc();
            if let (Some(f), None) = (full.next(), full.next()) {
                m.insert(f, h);
            }
        }
        m.insert('\u{3099}', '\u{ff9e}');
        m.insert('\u{309a}', '\u{ff9f}');
        m
    })
}

/// japanese-hankaku: full-width made half-width.
pub fn hankaku(s: &str) -> String {
    let m = halfwidth_map();
    let mut out = String::new();
    for c in s.chars() {
        let c = if matches!(c, '\u{3041}'..='\u{3096}') {
            katakana(&c.to_string()).chars().next().unwrap_or(c)
        } else {
            c
        };
        match c {
            '\u{ff01}'..='\u{ff5e}' => out.push(char::from_u32(c as u32 - 0xfee0).unwrap_or(c)),
            '\u{3000}' => out.push(' '),
            _ => {
                if let Some(h) = m.get(&c) {
                    out.push(*h);
                } else {
                    // A voiced kana is its plain kana and a mark.
                    let parts: Vec<char> = std::iter::once(c).nfd().collect();
                    if parts.len() > 1 && parts.iter().all(|p| m.contains_key(p)) {
                        out.extend(parts.iter().map(|p| m[p]));
                    } else {
                        out.push(c);
                    }
                }
            }
        }
    }
    out
}

/// Is `c` hiragana?
pub fn is_hiragana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309f}')
}

/// The candidates for the first `len` characters of `seq`, if the
/// dictionary has any. (`postfix` and `prefer_noun` are kept for the
/// callers; this dictionary lists whole words.)
pub fn lookup(seq: &[char], len: usize, _postfix: bool, _prefer_noun: bool) -> Option<Vec<String>> {
    let key: String = seq[..len].iter().collect();
    dict().words.get(&key).cloned()
}

// ------------------------------------------------------------ kkc

/// A list of conversions and the one selected: 0 is hiragana, -1 katakana, n the nth.
#[derive(Debug, Clone)]
pub struct Convs {
    pub idx: i64,
    pub list: Vec<String>,
}

impl Convs {
    /// The length of list, the index cell included.
    fn len(&self) -> i64 {
        self.list.len() as i64 + 1
    }
}

/// kkc-lookup-cache: the conversion lists picked from, by reading, for
/// the session. A list in it is shared: picking in it again changes it.
#[derive(Debug, Default)]
pub struct Learned {
    lists: Vec<Convs>,
    by_key: HashMap<String, usize>,
}

impl Learned {
    fn push(&mut self, c: Convs) -> usize {
        self.lists.push(c);
        self.lists.len() - 1
    }
}

/// The candidates shown in the echo area (kkc-current-conversions-width):
/// each one's width, and the first and past-the-last index shown.
#[derive(Debug, Clone)]
struct Shown {
    widths: Vec<usize>,
    this: i64,
    next: i64,
    msg: Option<String>,
}

/// kkc-show-conversion-list-index-chars.
const INDEX_CHARS: &[char] = &['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
/// kkc-show-conversion-list-count.
const SHOW_LIST_COUNT: usize = 4;

/// A kana-kanji conversion in progress (kkc-region).
///
/// The text it shows is `fixed` (phrases already accepted with C-f),
/// then the current phrase: its conversion and the rest of it as kana
/// (`mid`), then the kana still to convert. The current phrase is
/// highlighted; from its conversion's end on, it's underlined.
#[derive(Debug, Clone)]
pub struct Kkc {
    /// kkc-current-key, and kkc-original-kana.
    key: Vec<char>,
    original: String,
    /// kkc-length-head (the current phrase) and kkc-length-converted
    /// (the part of it the conversion is for).
    head: usize,
    converted: usize,
    convs: usize,
    shown: Option<Shown>,
    next_count: usize,
    prev_count: usize,
    fixed: String,
    conv: String,
    mid: String,
    /// The echo area's width (the candidate list fits in it).
    pub cols: usize,
    /// For the echo area after a key: the candidate list, or an error.
    pub message: Option<String>,
    pub bell: bool,
}

/// What a key did.
#[derive(Debug, PartialEq)]
pub enum Out {
    More,
    /// The conversion is over: the text is what it shows.
    Done,
    /// Back to the kana (kkc-cancel): the text shows them.
    Cancelled,
    /// Over, and the key isn't one of kkc's: read it again.
    Unread,
}

impl Kkc {
    /// kkc-region on `kana`: converted to the first candidate.
    pub fn new(kana: &str, learned: &mut Learned, cols: usize) -> Kkc {
        let key: Vec<char> = kana.chars().collect();
        let mut k = Kkc {
            head: key.len(),
            key,
            original: kana.to_string(),
            converted: 0,
            convs: learned.push(Convs {
                idx: 0,
                list: Vec::new(),
            }),
            shown: None,
            next_count: 1,
            prev_count: 0,
            fixed: String::new(),
            conv: String::new(),
            mid: String::new(),
            cols,
            message: None,
            bell: false,
        };
        if k.key.is_empty() {
            return k;
        }
        let mut first = true;
        while !k.lookup(learned, k.head, false, first) {
            k.head -= 1;
            first = false;
        }
        k.update(learned, true);
        k
    }

    /// The text the conversion shows.
    pub fn text(&self) -> String {
        let tail: String = self.key[self.head.min(self.key.len())..].iter().collect();
        format!("{}{}{}{tail}", self.fixed, self.conv, self.mid)
    }

    /// Where the current phrase is (highlighted), and where the part
    /// after its conversion starts (underlined from there to the end),
    /// in chars.
    pub fn spans(&self) -> (usize, usize, usize) {
        let a = self.fixed.chars().count();
        let b = a + self.conv.chars().count();
        (a, b + self.mid.chars().count(), b)
    }

    /// kkc-lookup-key.
    fn lookup(
        &mut self,
        learned: &mut Learned,
        len: usize,
        postfix: bool,
        prefer_noun: bool,
    ) -> bool {
        let k: String = self.key[..len].iter().collect();
        if let Some(&i) = learned.by_key.get(&k) {
            self.converted = len;
            self.shown = None;
            self.convs = i;
            return true;
        }
        if let Some(e) = lookup(&self.key, len, postfix, prefer_noun) {
            self.converted = len;
            self.shown = None;
            self.convs = learned.push(Convs { idx: 1, list: e });
            if postfix {
                learned.by_key.insert(k, self.convs);
            }
            return true;
        }
        if len == 1 {
            self.converted = 1;
            self.shown = None;
            self.convs = learned.push(Convs {
                idx: 0,
                list: Vec::new(),
            });
            return true;
        }
        false
    }

    /// kkc-update-conversion.
    fn update(&mut self, learned: &Learned, all: bool) {
        let c = &learned.lists[self.convs];
        let kana: String = self.key[..self.converted].iter().collect();
        self.conv = match c.idx {
            0 => kana,
            -1 => katakana(&kana),
            i => c.list.get(i as usize - 1).cloned().unwrap_or_default(),
        };
        if all {
            let mid: String = self.key[self.converted.min(self.head)..self.head]
                .iter()
                .collect();
            self.mid = if c.idx == -1 { katakana(&mid) } else { mid };
        }
    }

    /// Remember the current list for the current reading.
    fn learn(&self, learned: &mut Learned) {
        let k: String = self.key[..self.converted].iter().collect();
        learned.by_key.insert(k, self.convs);
    }

    fn error(&mut self, msg: &str) {
        self.message = Some(msg.to_string());
        self.bell = true;
    }

    /// One key (kkc-keymap).
    pub fn key(&mut self, learned: &mut Learned, key: Key) -> Out {
        self.message = None;
        self.bell = false;
        match key {
            Key::Next => {
                self.next_count += 1;
                self.prev_count = 0;
            }
            Key::Prev => {
                self.prev_count += 1;
                self.next_count = 0;
            }
            Key::Other => return Out::Unread,
            _ => {
                self.next_count = 0;
                self.prev_count = 0;
            }
        }
        let len = self.key.len();
        match key {
            Key::Next | Key::Prev => {
                let c = &mut learned.lists[self.convs];
                let mut idx = c.idx + if key == Key::Next { 1 } else { -1 };
                if key == Key::Next {
                    if idx < 0 {
                        idx = 1;
                    }
                    if idx >= c.len() {
                        idx = 0;
                    }
                } else if idx < 0 {
                    idx = c.len() - 1;
                }
                c.idx = idx;
                if idx > 1 {
                    self.learn(learned);
                }
                let count = if key == Key::Next {
                    self.next_count
                } else {
                    self.prev_count
                };
                if self.shown.is_some() || count >= SHOW_LIST_COUNT {
                    self.show_list(learned);
                }
                self.update(learned, false);
            }
            Key::Digit(d) => {
                let mut idx = None;
                if let Some(s) = &self.shown {
                    let n = (INDEX_CHARS.len() as i64).min(s.next - s.this).max(0) as usize;
                    if let Some(i) = INDEX_CHARS[..n].iter().position(|&x| x == d) {
                        idx = Some(s.this + i as i64);
                    }
                }
                let Some(idx) = idx else { return Out::Unread };
                learned.lists[self.convs].idx = idx;
                self.show_list(learned);
                self.update(learned, false);
            }
            Key::Terminate => return Out::Done,
            Key::Cancel => {
                self.conv = String::new();
                self.mid = String::new();
                self.key = self.original.chars().collect();
                self.head = 0;
                self.converted = 0;
                return Out::Cancelled;
            }
            Key::FirstChar => {
                let rest = format!(
                    "{}{}{}",
                    self.conv,
                    self.mid,
                    self.key[self.head..].iter().collect::<String>()
                );
                self.conv = rest.chars().next().map(String::from).unwrap_or_default();
                self.mid = String::new();
                self.head = self.key.len();
                return Out::Done;
            }
            Key::Katakana => {
                learned.lists[self.convs].idx = -1;
                self.update(learned, true);
            }
            Key::Hiragana => {
                learned.lists[self.convs].idx = 0;
                self.update(learned, false);
            }
            Key::Shorter => {
                if self.head <= 1 {
                    self.error("Can't be shorter");
                    return Out::More;
                }
                self.head -= 1;
                if self.converted > self.head {
                    let mut l = self.head;
                    self.converted = 0;
                    while !self.lookup(learned, l, false, false) {
                        l -= 1;
                    }
                }
                self.update(learned, true);
            }
            Key::Longer => {
                if self.head >= len {
                    self.error("Can't be longer");
                    return Out::More;
                }
                self.head += 1;
                self.lookup(learned, self.head, true, false);
                self.update(learned, true);
            }
            Key::ShorterConversion => {
                if self.converted <= 1 {
                    self.error("Can't be shorter");
                    return Out::More;
                }
                let mut l = self.converted - 1;
                self.converted = 0;
                while !self.lookup(learned, l, false, false) {
                    l -= 1;
                }
                self.update(learned, true);
            }
            Key::LongerPhrase => {
                if self.head >= len {
                    self.error("Can't be longer");
                    return Out::More;
                }
                self.head += 1;
                self.update(learned, true);
            }
            Key::NextPhrase => {
                if self.head >= len {
                    return Out::Done;
                }
                let mut rest = len - self.head;
                let skipped = self.key[self.head..]
                    .iter()
                    .take_while(|&&c| !is_hiragana(c))
                    .count();
                rest -= skipped;
                if rest == 0 {
                    return Out::Done;
                }
                let at = len - rest;
                self.fixed = format!(
                    "{}{}{}{}",
                    self.fixed,
                    self.conv,
                    self.mid,
                    self.key[self.head..at].iter().collect::<String>()
                );
                self.key = self.key[at..].to_vec();
                self.original = self.key.iter().collect();
                self.converted = 0;
                self.head = rest;
                while !self.lookup(learned, self.head, false, self.head < rest) && self.head > 1 {
                    self.head -= 1;
                }
                self.update(learned, true);
            }
            Key::NextGroup | Key::PrevGroup => {
                if learned.lists[self.convs].len() < 3 {
                    self.error("No alternative");
                    return Out::More;
                }
                match self.shown.clone() {
                    Some(s) => {
                        let n = learned.lists[self.convs].len();
                        let c = &mut learned.lists[self.convs];
                        c.idx = if key == Key::NextGroup {
                            if s.next < n { s.next } else { 1 }
                        } else if s.this > 1 {
                            s.this - 1
                        } else {
                            n - 1
                        };
                        self.show_list(learned);
                        self.update(learned, false);
                    }
                    None => {
                        self.setup_shown(learned);
                        self.show_list(learned);
                    }
                }
            }
            Key::Help => {
                self.message = Some(HELP.to_string());
            }
            Key::Other => unreachable!(),
        }
        Out::More
    }

    /// kkc-setup-current-conversions-width.
    fn setup_shown(&mut self, learned: &Learned) {
        let c = &learned.lists[self.convs];
        let mut widths = vec![0];
        widths.extend(
            c.list
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.as_str()) + 4),
        );
        self.shown = Some(Shown {
            this: c.len(),
            next: -2,
            msg: None,
            widths,
        });
    }

    /// kkc-show-conversion-list-update: the candidates around the
    /// selected one, in the echo area.
    fn show_list(&mut self, learned: &Learned) {
        if self.shown.is_none() {
            self.setup_shown(learned);
        }
        let c = &learned.lists[self.convs];
        let cols = self.cols as i64;
        let s = self.shown.as_mut().expect("set up");
        let current = c.idx;
        let (mut this, mut msg) = (s.this, s.msg.clone());
        if current < this {
            this = 1;
            msg = None;
        } else if current >= s.next {
            this = s.next;
            msg = None;
        }
        if msg.is_none() {
            let len = c.len();
            let w = |i: i64| s.widths.get(i as usize).copied().unwrap_or(0) as i64;
            let mut width = 0;
            let mut idx = this;
            while idx <= current {
                if width + w(idx) <= cols && idx - this < INDEX_CHARS.len() as i64 {
                    width += w(idx);
                } else {
                    this = idx;
                    width = w(idx);
                }
                idx += 1;
            }
            s.this = this;
            while idx < len && width + w(idx) <= cols && idx - this < INDEX_CHARS.len() as i64 {
                width += w(idx);
                idx += 1;
            }
            s.next = idx;
            let cand = |i: i64| {
                c.list
                    .get((i - 1).max(0) as usize)
                    .cloned()
                    .unwrap_or_else(|| "nil".into())
            };
            let mut m = format!(" {} {}", INDEX_CHARS[0], cand(this));
            for i in this + 1..s.next {
                m.push_str(&format!(
                    "  {} {}",
                    INDEX_CHARS[((i - this) as usize).min(9)],
                    cand(i)
                ));
            }
            msg = Some(m);
        }
        s.msg = msg.clone();
        self.message = msg;
    }
}

/// kkc-keymap's commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    /// SPC, C-n.
    Next,
    /// C-p.
    Prev,
    /// 1..9, 0.
    Digit(char),
    /// RET.
    Terminate,
    /// C-c, DEL.
    Cancel,
    /// C-SPC, C-@.
    FirstChar,
    /// K, H.
    Katakana,
    Hiragana,
    /// C-i, C-o, I, O.
    Shorter,
    Longer,
    ShorterConversion,
    LongerPhrase,
    /// C-f.
    NextPhrase,
    /// l, L.
    NextGroup,
    PrevGroup,
    /// C-h.
    Help,
    Other,
}

/// kkc-help's list.
pub const HELP: &str = "SPC, C-n next  C-p prev  l/L list  0..9 select  H hiragana  K katakana  C-o longer  C-i shorter  O longer phrase  I shorter conversion  C-f next phrase  DEL, C-c cancel  RET accept  C-SPC first char only";

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(kana: &str) -> Vec<String> {
        let k: Vec<char> = kana.chars().collect();
        lookup(&k, k.len(), false, true).unwrap_or_default()
    }

    #[test]
    fn the_dictionary_converts() {
        assert!(conv("にほん").contains(&"日本".to_string()));
        assert!(conv("かんじ").contains(&"漢字".to_string()));
        // Okurigana: 書く from か + く.
        let k: Vec<char> = "かく".chars().collect();
        assert!(
            lookup(&k, 2, false, false)
                .unwrap()
                .contains(&"書く".to_string())
        );
        assert_eq!(katakana("かな"), "カナ");
        assert_eq!(hiragana("カナ"), "かな");
        assert_eq!(hankaku("が"), "ｶﾞ");
        assert!(is_hiragana('あ') && !is_hiragana('ア'));
    }

    #[test]
    fn a_conversion_and_its_keys() {
        let mut l = Learned::default();
        let mut k = Kkc::new("にほんご", &mut l, 80);
        assert_eq!(k.text(), "日本語");
        assert_eq!(k.key(&mut l, Key::Terminate), Out::Done);
        let mut k = Kkc::new("にほんご", &mut l, 80);
        k.key(&mut l, Key::Katakana);
        assert_eq!(k.text(), "ニホンゴ");
        assert_eq!(k.key(&mut l, Key::Cancel), Out::Cancelled);
        assert_eq!(k.text(), "にほんご");
    }
}
