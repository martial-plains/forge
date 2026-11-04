// SPDX-License-Identifier: MIT OR Apache-2.0
//! Korean Hangul input (the 2-Bulsik keyboard): consonant and vowel keys
//! put jamo together into syllables as you type.
//!
//! Syllables are built with the Unicode formula: a syllable is
//! `0xAC00 + (initial * 21 + medial) * 28 + final`, with the indices of
//! the jamo in the Hangul Jamo tables below. A key either extends the
//! syllable being typed or finishes it and starts the next one; a final
//! consonant that is followed by a vowel moves to the new syllable.
//!
//! Hangul -> Hanja conversion (F9) needs a table of Hanja, which isn't
//! shipped: put one in `hanja.txt` in the Forge configuration directory,
//! one Hangul syllable per line followed by tab-separated `hanja` or
//! `hanja:meaning` entries.

/// Which keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// 2-Bulsik (KS X 5002, the usual Korean keyboard).
    Two,
}

/// Initial consonants, in Unicode's order.
const INITIAL: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ',
    'ㅌ', 'ㅍ', 'ㅎ',
];
/// Vowels, in Unicode's order.
const MEDIAL: [char; 21] = [
    'ㅏ', 'ㅐ', 'ㅑ', 'ㅒ', 'ㅓ', 'ㅔ', 'ㅕ', 'ㅖ', 'ㅗ', 'ㅘ', 'ㅙ', 'ㅚ', 'ㅛ', 'ㅜ', 'ㅝ', 'ㅞ',
    'ㅟ', 'ㅠ', 'ㅡ', 'ㅢ', 'ㅣ',
];
/// Final consonants, in Unicode's order (index 0 is "none").
const FINAL: [char; 28] = [
    '\0', 'ㄱ', 'ㄲ', 'ㄳ', 'ㄴ', 'ㄵ', 'ㄶ', 'ㄷ', 'ㄹ', 'ㄺ', 'ㄻ', 'ㄼ', 'ㄽ', 'ㄾ', 'ㄿ', 'ㅀ',
    'ㅁ', 'ㅂ', 'ㅄ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅊ', 'ㅋ', 'ㅌ', 'ㅍ', 'ㅎ',
];
/// Vowels made of two: (first, second, both).
const VOWEL_PAIRS: [(char, char, char); 7] = [
    ('ㅗ', 'ㅏ', 'ㅘ'),
    ('ㅗ', 'ㅐ', 'ㅙ'),
    ('ㅗ', 'ㅣ', 'ㅚ'),
    ('ㅜ', 'ㅓ', 'ㅝ'),
    ('ㅜ', 'ㅔ', 'ㅞ'),
    ('ㅜ', 'ㅣ', 'ㅟ'),
    ('ㅡ', 'ㅣ', 'ㅢ'),
];
/// Finals made of two consonants: (first, second, both).
const FINAL_PAIRS: [(char, char, char); 11] = [
    ('ㄱ', 'ㅅ', 'ㄳ'),
    ('ㄴ', 'ㅈ', 'ㄵ'),
    ('ㄴ', 'ㅎ', 'ㄶ'),
    ('ㄹ', 'ㄱ', 'ㄺ'),
    ('ㄹ', 'ㅁ', 'ㄻ'),
    ('ㄹ', 'ㅂ', 'ㄼ'),
    ('ㄹ', 'ㅅ', 'ㄽ'),
    ('ㄹ', 'ㅌ', 'ㄾ'),
    ('ㄹ', 'ㅍ', 'ㄿ'),
    ('ㄹ', 'ㅎ', 'ㅀ'),
    ('ㅂ', 'ㅅ', 'ㅄ'),
];

/// The jamo a letter key types on a 2-Bulsik keyboard.
fn jamo_of(key: char) -> Option<char> {
    Some(match key {
        'q' => 'ㅂ',
        'w' => 'ㅈ',
        'e' => 'ㄷ',
        'r' => 'ㄱ',
        't' => 'ㅅ',
        'y' => 'ㅛ',
        'u' => 'ㅕ',
        'i' => 'ㅑ',
        'o' => 'ㅐ',
        'p' => 'ㅔ',
        'a' => 'ㅁ',
        's' => 'ㄴ',
        'd' => 'ㅇ',
        'f' => 'ㄹ',
        'g' => 'ㅎ',
        'h' => 'ㅗ',
        'j' => 'ㅓ',
        'k' => 'ㅏ',
        'l' => 'ㅣ',
        'z' => 'ㅋ',
        'x' => 'ㅌ',
        'c' => 'ㅊ',
        'v' => 'ㅍ',
        'b' => 'ㅠ',
        'n' => 'ㅜ',
        'm' => 'ㅡ',
        'Q' => 'ㅃ',
        'W' => 'ㅉ',
        'E' => 'ㄸ',
        'R' => 'ㄲ',
        'T' => 'ㅆ',
        'O' => 'ㅒ',
        'P' => 'ㅖ',
        c if c.is_ascii_uppercase() => return jamo_of(c.to_ascii_lowercase()),
        _ => return None,
    })
}

fn is_vowel(j: char) -> bool {
    MEDIAL.contains(&j)
}

/// The syllable for these jamo (initial, vowel, final or none), or the
/// lone jamo.
fn compose(l: Option<char>, v: Option<char>, t: Option<char>) -> String {
    match (l, v) {
        (Some(l), Some(v)) => {
            let li = INITIAL.iter().position(|&c| c == l);
            let vi = MEDIAL.iter().position(|&c| c == v);
            let ti = t
                .and_then(|t| FINAL.iter().position(|&c| c == t))
                .unwrap_or(0);
            match (li, vi) {
                (Some(li), Some(vi)) => {
                    char::from_u32(0xAC00 + ((li * 21 + vi) * 28 + ti) as u32)
                        .map(String::from)
                        .unwrap_or_default()
                }
                _ => String::new(),
            }
        }
        (Some(c), None) | (None, Some(c)) => c.to_string(),
        (None, None) => String::new(),
    }
}

/// What a key did: text that is done (typed for good), and the syllable
/// still being put together (shown, not yet typed).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Out {
    pub done: String,
    pub composing: String,
}

/// The syllable being typed.
#[derive(Debug, Clone)]
pub struct Hangul {
    layout: Layout,
    initial: Option<char>,
    vowel: Option<char>,
    fin: Option<char>,
    /// What shows as the syllable now.
    shown: String,
}

impl Hangul {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            initial: None,
            vowel: None,
            fin: None,
            shown: String::new(),
        }
    }

    /// Does this key go to the input method at all?
    pub fn takes(layout: Layout, key: char) -> bool {
        match layout {
            Layout::Two => key.is_ascii_alphabetic(),
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The syllable shown now.
    pub fn composing(&self) -> &str {
        &self.shown
    }

    fn text(&self) -> String {
        compose(self.initial, self.vowel, self.fin)
    }

    fn clear(&mut self) {
        self.initial = None;
        self.vowel = None;
        self.fin = None;
    }

    /// The syllable so far is finished; `next` starts the following one.
    fn finish(&mut self, next: (Option<char>, Option<char>)) -> String {
        let done = self.text();
        self.clear();
        self.initial = next.0;
        self.vowel = next.1;
        done
    }

    /// A key the input method takes (see `takes`).
    pub fn key(&mut self, key: char) -> Out {
        let Some(j) = jamo_of(key) else {
            return Out {
                done: String::new(),
                composing: self.shown.clone(),
            };
        };
        let mut done = String::new();
        if is_vowel(j) {
            match (self.initial, self.vowel, self.fin) {
                (None, None, _) | (Some(_), None, _) => self.vowel = Some(j),
                (None, Some(v), _) | (Some(_), Some(v), None) => {
                    if let Some(&(_, _, both)) =
                        VOWEL_PAIRS.iter().find(|&&(a, b, _)| a == v && b == j)
                    {
                        self.vowel = Some(both);
                    } else {
                        done = self.finish((None, Some(j)));
                    }
                }
                (Some(_), Some(_), Some(t)) => {
                    // The final consonant (or the second of two) moves to
                    // the next syllable.
                    if let Some(&(first, second, _)) = FINAL_PAIRS.iter().find(|p| p.2 == t) {
                        self.fin = Some(first);
                        done = self.finish((Some(second), Some(j)));
                    } else {
                        self.fin = None;
                        done = self.finish((Some(t), Some(j)));
                    }
                }
            }
        } else {
            match (self.initial, self.vowel, self.fin) {
                (None, None, _) => self.initial = Some(j),
                (Some(_), None, _) | (None, Some(_), _) => {
                    done = self.finish((Some(j), None));
                }
                (Some(_), Some(_), None) => {
                    if FINAL.contains(&j) {
                        self.fin = Some(j);
                    } else {
                        done = self.finish((Some(j), None));
                    }
                }
                (Some(_), Some(_), Some(t)) => {
                    if let Some(&(_, _, both)) =
                        FINAL_PAIRS.iter().find(|&&(a, b, _)| a == t && b == j)
                    {
                        self.fin = Some(both);
                    } else {
                        done = self.finish((Some(j), None));
                    }
                }
            }
        }
        self.shown = self.text();
        Out {
            done,
            composing: self.shown.clone(),
        }
    }

    /// Take the last jamo off. None when nothing was left, which deletes
    /// the character before point.
    pub fn delete(&mut self) -> Option<Out> {
        if let Some(t) = self.fin {
            self.fin = FINAL_PAIRS.iter().find(|p| p.2 == t).map(|p| p.0);
        } else if let Some(v) = self.vowel {
            self.vowel = VOWEL_PAIRS.iter().find(|p| p.2 == v).map(|p| p.0);
        } else if self.initial.is_some() {
            self.initial = None;
        } else {
            return None;
        }
        self.shown = self.text();
        Some(Out {
            done: String::new(),
            composing: self.shown.clone(),
        })
    }

    /// The syllable becomes something else (a hanja): start over.
    pub fn reset(&mut self) {
        self.clear();
        self.shown.clear();
    }
}

// --------------------------------------------------------------- hanja

/// The user's table of Hanja, if they have one (see the module docs).
fn hanja_table() -> &'static str {
    static TABLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        forge_core::config::config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("hanja.txt")).ok())
            .unwrap_or_default()
    })
}

/// The Hanja (with meanings) a Hangul character converts to.
pub fn hanja(c: char) -> Vec<(char, Option<String>)> {
    let mut b = [0; 4];
    let key = c.encode_utf8(&mut b);
    hanja_table()
        .lines()
        .find(|l| l.split('\t').next() == Some(key))
        .map(|l| {
            l.split('\t')
                .skip(1)
                .filter_map(|e| {
                    let (ch, meaning) = match e.split_once(':') {
                        Some((h, m)) => (h, Some(m.to_string())),
                        None => (e, None),
                    };
                    Some((ch.chars().next()?, meaning))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// hanja-list-width.
pub const HANJA_WIDTH: usize = 10;

/// The Hanja list being chosen from (hanja-conversions): which group of
/// ten shows, and which is selected in it.
#[derive(Debug, Clone)]
pub struct HanjaList {
    pub base: usize,
    pub offset: usize,
    pub list: Vec<(char, Option<String>)>,
}

impl HanjaList {
    fn group_len(&self, base: usize) -> usize {
        self.list
            .len()
            .saturating_sub(base * HANJA_WIDTH)
            .min(HANJA_WIDTH)
    }

    pub fn prev_group(&mut self) {
        if self.base > 0 {
            self.base -= 1;
        }
    }

    pub fn next_group(&mut self) {
        let next = self.group_len(self.base + 1);
        if next > 0 {
            self.base += 1;
            if next - 1 < self.offset {
                self.offset = next - 1;
            }
        }
    }

    pub fn prev(&mut self) {
        if self.offset > 0 {
            self.offset -= 1;
        } else if self.base > 0 {
            self.prev_group();
            self.offset = HANJA_WIDTH - 1;
        }
    }

    pub fn next(&mut self) {
        if self.offset < HANJA_WIDTH - 1 && self.offset + 1 < self.group_len(self.base) {
            self.offset += 1;
        } else if self.group_len(self.base + 1) > 0 {
            self.next_group();
            self.offset = 0;
        }
    }

    pub fn selected(&self) -> Option<char> {
        self.list
            .get(self.base * HANJA_WIDTH + self.offset)
            .map(|(c, _)| *c)
    }

    /// hanja-list-show: "0.漢 1.韓 ...   (뜻 : meaning)", the selected one
    /// marked.
    pub fn show(&self) -> String {
        let mut msg = String::new();
        let mut meaning = None;
        for (i, (c, m)) in self
            .list
            .iter()
            .skip(self.base * HANJA_WIDTH)
            .take(HANJA_WIDTH)
            .enumerate()
        {
            if i == self.offset {
                msg.push_str(&format!("{i}.[{c}] "));
                meaning = m.clone();
            } else {
                msg.push_str(&format!("{i}.{c} "));
            }
        }
        if let Some(m) = meaning {
            msg.push_str(&format!("   (뜻 : {m})"));
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(layout: Layout, keys: &str) -> String {
        let mut h = Hangul::new(layout);
        let mut text = String::new();
        for k in keys.chars() {
            if Hangul::takes(layout, k) {
                text.push_str(&h.key(k).done);
            } else {
                text.push_str(h.composing());
                h.reset();
                text.push(k);
            }
        }
        text + h.composing()
    }

    #[test]
    fn two_bulsik_composes_syllables() {
        // 한국어: gks rnr dj
        assert_eq!(typed(Layout::Two, "gksrnrdj"), "한국어");
        // A final consonant moves to the next syllable before a vowel.
        assert_eq!(typed(Layout::Two, "dkssudgktpdy"), "안녕하세요");
        // Doubles: ㄲ (Shift-r), ㅘ (h k).
        assert_eq!(typed(Layout::Two, "Rhk"), "꽈");
        assert_eq!(typed(Layout::Two, "r"), "ㄱ");
        assert_eq!(typed(Layout::Two, "k"), "ㅏ");
    }

    #[test]
    fn deleting_takes_off_one_jamo() {
        let mut h = Hangul::new(Layout::Two);
        h.key('g');
        h.key('k');
        assert_eq!(h.key('s').composing, "한");
        assert_eq!(h.delete().unwrap().composing, "하");
        assert_eq!(h.delete().unwrap().composing, "ㅎ");
        assert_eq!(h.delete(), Some(Out::default()));
        assert_eq!(
            h.delete(),
            None,
            "nothing left: the character before point goes"
        );
    }

    #[test]
    fn compound_vowels_and_finals() {
        assert_eq!(typed(Layout::Two, "dhk"), "와");
        // 닭: ㄹㄱ final; a vowel splits it.
        assert_eq!(typed(Layout::Two, "ekfr"), "닭");
        assert_eq!(typed(Layout::Two, "ekfrk"), "달가");
    }

    #[test]
    fn hanja_list_moves() {
        let mut l = HanjaList {
            base: 0,
            offset: 0,
            list: vec![('韓', None), ('漢', Some("Han".into()))],
        };
        l.next();
        assert_eq!(l.offset, 1);
        assert!(l.show().contains("1.[漢]"));
    }
}
