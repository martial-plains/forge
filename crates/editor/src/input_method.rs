// SPDX-License-Identifier: MIT OR Apache-2.0
//! Input methods: type with Roman letters and get other scripts or
//! accented letters (a quail-style engine).
//!
//! - **The methods** are generated for Forge by
//!   `scripts/gen-input-methods.py` into `data/input-methods.txt`: Latin
//!   with accents typed before (`latin-prefix`) or after (`latin-postfix`)
//!   the letter, TeX-like input, Greek and Russian transliteration, and
//!   the Japanese family (romaji to hiragana, katakana, half-width kana and
//!   zenkaku, with kana-kanji conversion, `kkc.rs`). Korean Hangul
//!   (`hangul.rs`) and `ucs` aren't tables and are done natively.
//! - **Commands.**
//!   - `C-x \` (`activate-transient-input-method`) turns an input method on
//!     for the next character only, then puts back whatever was on. It
//!     asks which method the first time, or with `C-u`
//!     (`default_transient_input_method` in init.rhai sets it ahead).
//!   - `C-\` (`toggle-input-method`) turns the buffer's input method on and
//!     off. `C-x RET C-\` (`set-input-method`) picks one.
//!   - `C-h I` (`describe-input-method`) shows a method's keys, and
//!     `M-x list-input-methods` lists them all.
//! - **Translating** works as quail does. Typed keys are translated
//!   longest match first, and the text so far is shown underlined in the
//!   buffer (not yet an edit: undo never sees it). A key that can't
//!   continue the translation finishes it and is read again.
//!   - DEL takes back the last key.
//!   - Several candidates show in the echo area. A digit picks one, and
//!     in the methods that aren't "simple", C-f / C-b (and C-n / C-p by
//!     tens) move through them.
//!   - TAB lists the completions in *Quail Completions*
//!     (quail-completion), in the methods whose translation keymap has
//!     it: the keys that can follow, to five levels deep, with what each
//!     types. As, the "simple" methods (TeX, the Latin ones and
//!     most others) leave TAB alone; `forge.opt.input_method_completion_everywhere`
//!     gives them TAB completion too. While the list is up, the entry for
//!     the keys typed is highlighted. TAB again scrolls it, and a click
//!     on a candidate chooses it.
//!   - Any other key (RET, C-a, ...) finishes the translation and then
//!     does what it does.
//! - **Where it applies:**, only the first key of a key
//!   sequence goes through the input method (after `C-x`, `d` is just
//!   `d`), only in buffers that aren't read-only, and only for keys up to
//!   U+00FF.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::BufferId;
use forge_core::decoration::{DecoFace, DecoSpec};
use forge_docs::CommandDoc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

static DATA: &str = include_str!("../data/input-methods.txt");

/// Who owns the translation's underline.
const OWNER: &str = "input-method";

pub const COMMANDS: &[CommandDoc] = &[
    CommandDoc::new("activate-transient-input-method", "Turn an input method on for the next character only (C-x \\).").body("It asks which the first time, or with C-u; afterwards the input method that was on (if any) is back."),
    CommandDoc::new("describe-input-method", "Show an input method's documentation and every key sequence it translates (C-h").body("I)."),
    CommandDoc::line(
        "list-input-methods",
        "List the input methods, by language, with their mode-line titles.",
    ),
    CommandDoc::line(
        "set-input-method",
        "Pick an input method for this buffer and turn it on (C-x RET C-\\).",
    ),
    CommandDoc::new("toggle-input-method", "Turn this buffer's input method off, or on again (C-\\).").body("With C-u, or the first time, ask which."),
];

pub const BINDINGS: &[(&str, &str)] = &[
    ("C-\\", "toggle-input-method"),
    ("C-x \\", "activate-transient-input-method"),
    ("C-x RET C-\\", "set-input-method"),
    ("C-h I", "describe-input-method"),
];

// ---------------------------------------------------------------- data

/// What a node of a quail map defines.
#[derive(Debug, Clone, PartialEq)]
pub enum Def {
    None,
    /// What the keys type; `list`: a candidate list, where a digit picks even among one.
    Cands {
        list: bool,
        cands: Vec<String>,
    },
    /// A function quail calls when the keys are looked up (Japanese's
    /// package switches, mozhi's `_`): see `functions`.
    Func(String),
}

/// A node of a quail map: what its keys define, and the keys that can
/// follow (in the order quail lists them), each to a node. Maps are
/// graphs: nodes are shared, and some loop (Tibetan's states).
#[derive(Debug, Clone)]
pub struct Node {
    pub def: Def,
    pub kids: Vec<(char, u32)>,
}

/// One input method: a quail package.
#[derive(Debug)]
pub struct Method {
    pub name: String,
    pub language: String,
    /// What the mode line shows while it's on.
    pub title: String,
    /// The first line of its documentation.
    pub doc: String,
    /// On a key that can't continue, commit the longest translated part
    /// (rather than all but the last key).
    pub deterministic: bool,
    pub maximum_shortest: bool,
    /// Only digits pick candidates (no C-f / C-b).
    pub simple: bool,
    /// Translated text gathers in a conversion region (Japanese, where SPC
    /// converts it to kanji).
    pub conversion: bool,
    /// The package's update-translation-function, which has the last word
    /// on what the keys come to (Lao, Tibetan, Japanese, mozhi).
    pub update: Option<String>,
    /// The map: node 0 is the root.
    pub nodes: Vec<Node>,
    /// Start each translation from the first candidate, not from the one
    /// picked last time for the same keys.
    pub forget: bool,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cs = s.chars();
    while let Some(c) = cs.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match cs.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// Where each method is in DATA: (header fields, first node line's
/// offset, end offset).
fn index() -> &'static [(Vec<String>, usize, usize)] {
    static INDEX: OnceLock<Vec<(Vec<String>, usize, usize)>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut out: Vec<(Vec<String>, usize, usize)> = Vec::new();
        let mut at = 0;
        for line in DATA.split_inclusive('\n') {
            if let Some(h) = line.strip_prefix('\u{1}') {
                if let Some(last) = out.last_mut() {
                    last.2 = at;
                }
                let fields: Vec<String> = h
                    .trim_end_matches('\n')
                    .split('\t')
                    .skip(1)
                    .map(unescape)
                    .collect();
                out.push((fields, at + line.len(), DATA.len()));
            }
            at += line.len();
        }
        out
    })
}

/// The input methods that aren't quail tables (Hangul's automaton, ucs's
/// hex digits): name, language, title, documentation.
pub const NATIVE: &[(&str, &str, &str, &str)] = &[
    ("korean-hangul", "UTF-8", "한2", "Hangul 2-Bulsik Input"),
    (
        "ucs",
        "UTF-8",
        "U+",
        "Unicode input as hex in the form Uxxxx.",
    ),
];

/// Every input method's name, in order.
pub fn names() -> Vec<&'static str> {
    let mut v: Vec<&str> = index().iter().map(|(h, _, _)| h[0].as_str()).collect();
    v.extend(NATIVE.iter().map(|n| n.0));
    v.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    v
}

/// (name, language, title, first doc line) of every method.
pub fn summaries() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    let mut v: Vec<_> = index()
        .iter()
        .map(|(h, _, _)| {
            (
                h[0].as_str(),
                h[1].as_str(),
                h[2].as_str(),
                h.get(9).map_or("", |s| s.as_str()),
            )
        })
        .collect();
    v.extend(NATIVE.iter().copied());
    v
}

/// Is `name` an input method?
pub fn exists(name: &str) -> bool {
    NATIVE.iter().any(|n| n.0 == name) || index().iter().any(|(h, _, _)| h[0] == name)
}

/// The quail method called `name`, parsed on first use.
pub fn method(name: &str) -> Option<&'static Method> {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static Method>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(m) = cache.lock().ok()?.get(name) {
        return Some(m);
    }
    let (h, start, end) = index().iter().find(|(h, _, _)| h[0] == name)?;
    let flag = |i: usize| h.get(i).is_some_and(|v| v == "1");
    let mut nodes = Vec::new();
    for line in DATA[*start..*end].lines() {
        let mut fields = line.split('\t');
        let (Some(_id), Some(kids)) = (fields.next(), fields.next()) else {
            continue;
        };
        let kids = kids
            .split('\u{1e}')
            .filter(|k| !k.is_empty())
            .filter_map(|k| {
                let (c, id) = k.split_once('\u{1f}')?;
                Some((unescape(c).chars().next()?, id.parse().ok()?))
            })
            .collect();
        let rest: Vec<String> = fields.map(unescape).collect();
        let def = match rest.first().map(String::as_str) {
            None => Def::None,
            Some("\u{3}") => Def::Func(rest.get(1).cloned().unwrap_or_default()),
            // A function making the children: none of methods
            // that Forge has needs it.
            Some("\u{4}") => Def::None,
            Some("\u{2}") if rest.len() > 1 => Def::Cands {
                list: true,
                cands: rest[1..].to_vec(),
            },
            Some("\u{2}") => Def::None,
            Some(_) => Def::Cands {
                list: false,
                cands: rest,
            },
        };
        nodes.push(Node { def, kids });
    }
    let update = h.get(7).filter(|u| u.as_str() != "-").cloned();
    let m: &'static Method = Box::leak(Box::new(Method {
        name: h[0].clone(),
        language: h[1].clone(),
        title: h[2].clone(),
        doc: h.get(9).cloned().unwrap_or_default(),
        deterministic: flag(3),
        maximum_shortest: flag(4),
        simple: flag(5),
        conversion: flag(6),
        update,
        nodes,
        forget: flag(10),
    }));
    cache.lock().ok()?.insert(name.to_string(), m);
    Some(m)
}

impl Method {
    /// The node `keys` lead to (quail-lookup-key), if they lead anywhere.
    pub fn node(&self, keys: &str) -> Option<&Node> {
        let mut n = self.nodes.first()?;
        for c in keys.chars() {
            let (_, id) = n.kids.iter().find(|(k, _)| *k == c)?;
            n = self.nodes.get(*id as usize)?;
        }
        Some(n)
    }

    /// What `keys` type, if they are a rule.
    fn def(&self, keys: &str) -> Option<&Vec<String>> {
        match &self.node(keys)?.def {
            Def::Cands { cands, .. } if !cands.is_empty() => Some(cands),
            _ => None,
        }
    }

    /// Is the translation of `keys` a candidate list?
    fn listed(&self, keys: &str) -> bool {
        matches!(
            self.node(keys).map(|n| &n.def),
            Some(Def::Cands { list: true, .. })
        )
    }

    /// Is `keys` a rule or the start of one (quail-lookup-key finds a map)?
    fn is_map(&self, keys: &str) -> bool {
        self.node(keys).is_some()
    }

    /// Does a longer rule start with `keys`?
    fn has_longer(&self, keys: &str) -> bool {
        self.node(keys).is_some_and(|n| !n.kids.is_empty())
    }

    /// Can `c` start a translation?
    pub fn starts(&self, c: char) -> bool {
        let mut b = [0; 4];
        self.is_map(c.encode_utf8(&mut b))
    }

    /// Every rule, to `depth` keys (maps can loop): (keys, candidates).
    pub fn rules(&self, depth: usize) -> Vec<(String, Vec<String>)> {
        fn walk(
            m: &Method,
            id: u32,
            keys: &mut String,
            depth: usize,
            out: &mut Vec<(String, Vec<String>)>,
        ) {
            let Some(n) = m.nodes.get(id as usize) else {
                return;
            };
            match &n.def {
                Def::Cands { cands, .. } if !keys.is_empty() => {
                    out.push((keys.clone(), cands.clone()))
                }
                Def::Func(f) => out.push((keys.clone(), vec![format!("({f})")])),
                _ => {}
            }
            if keys.chars().count() < depth {
                for (c, kid) in &n.kids {
                    keys.push(*c);
                    walk(m, *kid, keys, depth, out);
                    keys.pop();
                }
            }
        }
        let mut out = Vec::new();
        walk(self, 0, &mut String::new(), depth, &mut out);
        out
    }
}

// ------------------------------------------------------- translating

/// A translation in progress (quail's current key and string).
#[derive(Debug, Clone)]
pub struct Translation {
    pub buffer: BufferId,
    method: &'static Method,
    /// Where the provisional text starts, and how many chars it is.
    start: usize,
    shown: usize,
    /// The keys typed (quail-current-key).
    keys: String,
    /// What they come to so far (quail-current-str).
    current: String,
    /// The candidates for `keys`, and the one picked.
    candidates: Vec<String>,
    index: usize,
    /// The candidate picked last for each key sequence (quail keeps it in
    /// the map, for the whole session, unless the package forgets).
    remembered: HashMap<String, usize>,
}

/// What a key did to the translation (quail-translate-key's result).
#[derive(Debug, PartialEq)]
enum Control {
    /// Done: commit `current`.
    Done,
    /// More keys may follow.
    More,
    /// Commit `current` for the first n keys; the rest are read again.
    Head(usize),
    /// The keys are a function rule: quail calls it (Japanese's package
    /// switches).
    Func(String),
}

fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn suffix(s: &str, n: usize) -> &str {
    &s[prefix(s, n).len()..]
}

impl Translation {
    /// quail-get-current-str: what the first `len` keys show as.
    fn current_str(&self, len: usize) -> Option<String> {
        let keys = prefix(&self.keys, len);
        let whole = len == self.keys.chars().count();
        if let Some(c) = self.method.def(keys) {
            let i = if whole { self.index } else { self.picked(keys) };
            return Some(c[i.min(c.len() - 1)].clone());
        }
        if len <= 1 {
            return None;
        }
        let head = self.current_str(len - 1)?;
        let last = suffix(keys, len - 1);
        let tail = self
            .method
            .deterministic
            .then(|| self.method.def(last).map(|c| c[0].clone()))
            .flatten()
            .unwrap_or_else(|| last.to_string());
        Some(format!("{head}{tail}"))
    }

    /// The candidate picked last for `keys`.
    fn picked(&self, keys: &str) -> usize {
        if self.method.forget {
            0
        } else {
            self.remembered.get(keys).copied().unwrap_or(0)
        }
    }

    /// Pick candidate `i` for the keys typed (and remember it).
    fn pick(&mut self, i: usize) {
        self.index = i;
        self.remembered.insert(self.keys.clone(), i);
    }

    /// quail-translate-key.
    fn translate(&mut self) -> Control {
        let m = self.method;
        let len = self.keys.chars().count();
        if m.is_map(&self.keys) {
            self.candidates = m.def(&self.keys).cloned().unwrap_or_default();
            self.index = self
                .picked(&self.keys)
                .min(self.candidates.len().saturating_sub(1));
            self.current = self.current_str(len).unwrap_or_else(|| self.keys.clone());
            let single = self.candidates.len() <= 1;
            let no_longer = !self.candidates.is_empty() && !m.has_longer(&self.keys);
            let no_break = !m.maximum_shortest
                || len < 3
                || !m.is_map(prefix(&self.keys, len - 1))
                || !m.is_map(suffix(prefix(&self.keys, len - 1), len - 2));
            return if single && no_longer && no_break {
                Control::Done
            } else {
                Control::More
            };
        }
        // No translation for these keys: commit what can be, and read the
        // rest again.
        let last = self.keys.chars().last().unwrap_or(' ');
        if m.maximum_shortest
            && len >= 3
            && m.def(prefix(&self.keys, len - 2)).is_some()
            && m.is_map(suffix(&self.keys, len - 2))
        {
            self.current = self.current_str(len - 2).unwrap_or_default();
            return Control::Head(len - 2);
        }
        if len > 0
            && !m.deterministic
            && m.listed(prefix(&self.keys, len - 1))
            && !self.candidates.is_empty()
            && last.is_ascii_digit()
        {
            // A digit picks a candidate: 1..9 then 0, in the current ten
            // (past the end of them, the last).
            self.keys = prefix(&self.keys, len - 1).to_string();
            let d = last.to_digit(10).unwrap_or(1) as usize;
            let n = if d == 0 { 9 } else { d - 1 };
            let block = self.index / 10 * 10;
            let end = (block + 10).min(self.candidates.len());
            self.pick((block + n).min(end - 1));
            self.current = self.candidates[self.index].clone();
            return Control::Done;
        }
        if m.deterministic {
            let mut l = len;
            let mut found = false;
            while !found && l > 1 {
                l -= 1;
                found = m.def(prefix(&self.keys, l)).is_some();
            }
            self.current = if found {
                self.current_str(l).unwrap_or_default()
            } else {
                self.keys
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_default()
            };
            return Control::Head(l);
        }
        if m.def(prefix(&self.keys, len - 1)).is_some() {
            self.current = self.current_str(len - 1).unwrap_or_default();
        }
        Control::Head(len - 1)
    }

    /// quail-translate-key, then the package's update-translation-function
    /// (quail-update-translation): what the keys come to, all said.
    fn translate_updated(&mut self) -> Control {
        if let Some(Def::Func(f)) = self.method.node(&self.keys).map(|n| &n.def) {
            return Control::Func(f.clone());
        }
        let control = self.translate();
        let Some(update) = self.method.update.as_deref() else {
            return control;
        };
        let keys: Vec<char> = self.keys.chars().collect();
        let control = match control {
            Control::More => {
                if keys.first() == Some(&'q') {
                    self.current.clear();
                }
                Control::More
            }
            Control::Head(0) => {
                self.current = keys[0].to_string();
                Control::Done
            }
            Control::Head(n) => {
                self.current = if keys[0] == 'n' {
                    "ん".into()
                } else if keys.len() > 1 && keys[0] == keys[1] {
                    "っ".into()
                } else {
                    keys[0].to_string()
                };
                Control::Head(n)
            }
            c => c,
        };
        if !self.current.is_empty() {
            match update {
                "quail-japanese-katakana-update-translation" => {
                    self.current = crate::kkc::katakana(&self.current)
                }
                "quail-japanese-hankaku-update-translation" => {
                    self.current = crate::kkc::hankaku(&self.current)
                }
                _ => {}
            }
        }
        control
    }

    /// The candidates for the echo area: "[2/3] 1.é 2.è 3.ê".
    fn guidance(&self) -> Option<String> {
        if self.candidates.len() < 2 {
            return None;
        }
        let block = self.index / 10 * 10;
        let shown: Vec<String> = self
            .candidates
            .iter()
            .enumerate()
            .skip(block)
            .take(10)
            .map(|(i, c)| {
                let n = (i - block + 1) % 10;
                if i == self.index {
                    format!("{n}.[{c}]")
                } else {
                    format!("{n}.{c}")
                }
            })
            .collect();
        Some(format!(
            "{} [{}/{}] {}",
            self.keys,
            self.index + 1,
            self.candidates.len(),
            shown.join(" ")
        ))
    }
}

// --------------------------------------------------------- the state

/// Each buffer's input method.
#[derive(Debug, Default, Clone)]
struct BufferIm {
    current: Option<String>,
    /// Turned on by C-x \ for one chunk: what was on before.
    transient: bool,
    previous: Option<String>,
    /// What `qq` goes back to from japanese-ascii
    /// (quail-japanese-package-saved).
    japanese_saved: Option<String>,
}

#[derive(Default)]
pub struct State {
    buffers: HashMap<BufferId, BufferIm>,
    /// What C-\ turns on (default-input-method).
    pub default: Option<String>,
    /// What C-x \ turns on (default-transient-input-method).
    pub default_transient: Option<String>,
    /// The methods picked, latest first (input-method-history).
    pub history: Vec<String>,
    pub translation: Option<Translation>,
    /// The candidate picked last, by method and key sequence (see
    /// `Translation::remembered`).
    selections: HashMap<String, HashMap<String, usize>>,
    /// A Hangul or ucs input in progress (input methods that aren't quail
    /// tables).
    native: Option<Native>,
    /// Hangul -> Hanja: the list being chosen from (F9).
    hanja: Option<Hanja>,
    /// The `japanese` method's conversion region, while text gathers in
    /// it.
    conv: Option<Conv>,
    /// Kana-kanji picks, for the session (kkc-lookup-cache).
    learned: crate::kkc::Learned,
    /// TAB lists completions in every method, not only the ones whose
    /// translation keymap has it (forge.opt.input_method_completion_everywhere).
    pub completion_everywhere: bool,
    /// *Quail Completions*: the keys it lists completions of, whether the
    /// last key was TAB (another one scrolls), and where each candidate
    /// is (line, first and end column, text) for a click to choose.
    completion_key: Option<String>,
    completion_again: bool,
    completion_spans: Vec<(usize, usize, usize, String)>,
}

pub const COMPLETIONS: &str = "*Quail Completions*";

/// Text shown in a buffer for now: an input method's syllable or keys,
/// underlined and not yet an edit.
#[derive(Debug, Clone)]
struct Shown {
    buffer: BufferId,
    start: usize,
    len: usize,
}

impl Shown {
    fn at_point(app: &App) -> Self {
        Shown {
            buffer: app.active_id(),
            start: app.buf_ref().point_offset(),
            len: 0,
        }
    }

    /// Show `text` in place of what was shown.
    fn show(&mut self, app: &mut App, text: &str) {
        let Some(b) = app.editor.state.get_buffer_mut(self.buffer) else {
            return;
        };
        b.replace_silently(self.start, self.start + self.len, text);
        self.len = text.chars().count();
        b.set_decorations(
            OWNER,
            vec![DecoSpec::new(
                self.start,
                self.start + self.len,
                DecoFace::Underline,
            )],
        );
    }

    /// Show nothing.
    fn clear(&mut self, app: &mut App) {
        self.show(app, "");
        if let Some(b) = app.editor.state.get_buffer_mut(self.buffer) {
            b.set_decorations(OWNER, Vec::<DecoSpec>::new());
        }
    }

    /// Type `text` for good where the shown text was (it goes), and start
    /// showing again after it.
    fn commit(&mut self, app: &mut App, text: &str) {
        self.clear(app);
        for c in text.chars() {
            app.route_key(char_event(c));
        }
        self.start = app.buf_ref().point_offset();
        self.len = 0;
    }
}

/// An input method that isn't a quail table, at work.
#[derive(Debug, Clone)]
enum Native {
    Hangul {
        shown: Shown,
        h: crate::hangul::Hangul,
    },
    /// ucs: `U` and up to four hex digits typed so far.
    Ucs { shown: Shown, keys: String },
}

/// The F9 Hanja list, and where the Hangul character it replaces is.
#[derive(Debug, Clone)]
struct Hanja {
    list: crate::hangul::HanjaList,
    /// The Hangul input it came from (its syllable is replaced), or None
    /// for M-x hangul-to-hanja-conversion (the character after point).
    from: Option<Native>,
}

/// Which native engine an input method is, if it is one.
fn native_layout(name: &str) -> Option<Option<crate::hangul::Layout>> {
    use crate::hangul::Layout;
    match name {
        "korean-hangul" => Some(Some(Layout::Two)),
        "ucs" => Some(None),
        _ => None,
    }
}

/// quail-completion-max-depth.
const COMPLETION_MAX_DEPTH: usize = 5;

/// The input method on in a buffer.
pub fn current(app: &App, id: BufferId) -> Option<&str> {
    app.im.buffers.get(&id).and_then(|b| b.current.as_deref())
}

fn show_title(app: &mut App, id: BufferId) {
    let title = current(app, id).and_then(|n| {
        method(n)
            .map(|m| m.title.clone())
            .or_else(|| NATIVE.iter().find(|x| x.0 == n).map(|x| x.2.to_string()))
    });
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.input_method_title = title;
    }
}

/// activate-input-method.
pub fn activate(app: &mut App, name: &str) -> Result<(), String> {
    if !exists(name) {
        return Err(format!("Invalid input method: {name}"));
    }
    let id = app.active_id();
    finish(app);
    app.im.buffers.entry(id).or_default().current = Some(name.to_string());
    let h = &mut app.im.history;
    h.retain(|n| n != name);
    h.insert(0, name.to_string());
    show_title(app, id);
    Ok(())
}

/// deactivate-input-method.
pub fn deactivate(app: &mut App) {
    let id = app.active_id();
    finish(app);
    if let Some(b) = app.im.buffers.get_mut(&id) {
        b.current = None;
    }
    show_title(app, id);
}

/// deactivate-transient-input-method: put back what was on.
fn end_transient(app: &mut App, id: BufferId) {
    let Some(b) = app.im.buffers.get_mut(&id).filter(|b| b.transient) else {
        return;
    };
    b.transient = false;
    b.current = b.previous.take();
    show_title(app, id);
}

// ------------------------------------------------------------ keys

fn plain_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

fn char_event(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Would Forge call the input method for this key here? The first key of
/// a key sequence, a character up to U+00FF, in a buffer taking keys that
/// isn't read-only.
fn applies(app: &App, key: &KeyEvent) -> Option<String> {
    let c = plain_char(key)?;
    if (c as u32) < 0x20 || c as u32 == 0x7f || c as u32 > 0xff {
        return None;
    }
    if app.minibuffer.is_some()
        || app.query.is_some()
        || app.key_reader.is_some()
        || !app.pending.is_empty()
        || app.meta_pending
        || crate::menu::active(app)
        || app.buf_ref().read_only()
    {
        return None;
    }
    current(app, app.active_id()).map(String::from)
}

/// Show the translation's text in the buffer, underlined, in place of
/// what was shown before.
fn redisplay(app: &mut App, t: &mut Translation) {
    remember(app, t);
    let Some(b) = app.editor.state.get_buffer_mut(t.buffer) else {
        return;
    };
    b.replace_silently(t.start, t.start + t.shown, &t.current);
    t.shown = t.current.chars().count();
    b.set_decorations(
        OWNER,
        vec![DecoSpec::new(
            t.start,
            t.start + t.shown,
            DecoFace::Underline,
        )],
    );
    match t.guidance() {
        Some(g) => app.msg(g),
        None => app.message = None,
    }
    show_in_completions(app, t);
}

/// Keep the translation's picks for the next one.
fn remember(app: &mut App, t: &Translation) {
    if !t.method.forget {
        app.im
            .selections
            .insert(t.method.name.clone(), t.remembered.clone());
    }
}

/// Take the provisional text out of the buffer.
fn clear(app: &mut App, t: &Translation) {
    if let Some(b) = app.editor.state.get_buffer_mut(t.buffer) {
        b.replace_silently(t.start, t.start + t.shown, "");
        b.set_decorations(OWNER, Vec::<DecoSpec>::new());
    }
}

/// End a translation where it stands (another buffer was selected, a
/// command changed input methods): its text is typed as it shows.
pub fn finish(app: &mut App) {
    app.im.hanja = None;
    if let Some(mut c) = app.im.conv.take() {
        // As a key outside the conversion keymaps would: what has been
        // converted is typed.
        if let Some(k) = c.kkc.take() {
            c.conv_str = k.text();
        }
        let ok = app.editor.state.buffers.contains_key(&c.buffer) && app.active_id() == c.buffer;
        conv_end(app, c, ok);
    }
    if let Some(n) = app.im.native.take() {
        let (mut shown, text) = match n {
            Native::Hangul { shown, h } => (shown, h.composing().to_string()),
            Native::Ucs { shown, keys } => (shown, keys),
        };
        let id = shown.buffer;
        if app.editor.state.buffers.contains_key(&id) && app.active_id() == id {
            shown.commit(app, &text);
        } else {
            shown.clear(app);
        }
        end_transient(app, id);
    }
    if let Some(t) = app.im.translation.take() {
        clear(app, &t);
        let id = t.buffer;
        if app.editor.state.buffers.contains_key(&id) && app.active_id() == id {
            for c in t.current.chars() {
                app.route_key(char_event(c));
            }
        }
        app.message = None;
        end_transient(app, id);
    }
}

/// The input method's turn at a key: it
/// starts, continues or ends a translation, or passes the key on.
pub fn feed(app: &mut App, key: KeyEvent) {
    if let Some(h) = app.im.hanja.take() {
        return hanja_step(app, h, key);
    }
    if let Some(c) = app.im.conv.take() {
        if c.buffer != app.active_id() || !app.editor.state.buffers.contains_key(&c.buffer) {
            app.im.conv = Some(c);
            finish(app);
            return feed(app, key);
        }
        return conv_key(app, c, key);
    }
    if let Some(n) = app.im.native.take() {
        let buffer = match &n {
            Native::Hangul { shown, .. } | Native::Ucs { shown, .. } => shown.buffer,
        };
        if buffer != app.active_id() || !app.editor.state.buffers.contains_key(&buffer) {
            app.im.native = Some(n);
            finish(app);
            return feed(app, key);
        }
        return native_step(app, n, key);
    }
    if let Some(t) = app.im.translation.take() {
        if t.buffer != app.active_id() || !app.editor.state.buffers.contains_key(&t.buffer) {
            app.im.translation = Some(t);
            finish(app);
            return feed(app, key);
        }
        return step(app, t, key);
    }
    let Some(name) = applies(app, &key) else {
        return app.route_key(key);
    };
    let id = app.active_id();
    let c = plain_char(&key).expect("applies checked");
    if let Some(layout) = native_layout(&name) {
        return native_start(app, layout, c, key);
    }
    let Some(m) = method(&name) else {
        return app.route_key(key);
    };
    if !m.starts(c) {
        // Nothing to translate; still a chunk, which ends a transient
        // input method.
        app.route_key(key);
        end_transient(app, id);
        return;
    }
    let start = app.buf_ref().point_offset();
    let t = Translation {
        buffer: id,
        method: m,
        start,
        shown: 0,
        keys: String::new(),
        current: String::new(),
        candidates: Vec::new(),
        index: 0,
        remembered: app.im.selections.get(&m.name).cloned().unwrap_or_default(),
    };
    if m.conversion {
        let c = Conv {
            buffer: id,
            start,
            shown: 0,
            text: Vec::new(),
            point: 0,
            conv_str: String::new(),
            translating: false,
            t,
            tstart: 0,
            tlen: 0,
            kkc: None,
        };
        return conv_key(app, c, key);
    }
    step(app, t, key);
}

// ------------------------------------------------ Japanese

/// A function rule's turn (quail-lookup-key calls it): Japanese's
/// package switches. The translation is already gone.
fn function_rule(app: &mut App, f: &str, keys: &str) {
    let last = keys.chars().last().unwrap_or(' ');
    match f {
        // quail-japanese-self-insert-and-switch-to-alpha: to
        // japanese-ascii, where the key is read again.
        "quail-japanese-self-insert-and-switch-to-alpha" => {
            switch_package(app, 'q');
            feed(app, char_event(last));
        }
        "quail-japanese-switch-package" => switch_package(app, last),
        _ => {
            app.msg(format!("Forge doesn't have {f}"));
            app.bell = true;
        }
    }
}

/// quail-japanese-switch-package: `qz` to japanese-zenkaku, `qk` to
/// japanese-hankaku-kana, `qh` to japanese, and `qq` to japanese-ascii
/// and back.
fn switch_package(app: &mut App, key: char) {
    let id = app.active_id();
    let target = match key {
        'z' => "japanese-zenkaku".to_string(),
        'k' => "japanese-hankaku-kana".to_string(),
        'h' => "japanese".to_string(),
        'q' => {
            let cur = current(app, id).map(String::from);
            if cur.as_deref() == Some("japanese-ascii") {
                match app
                    .im
                    .buffers
                    .get(&id)
                    .and_then(|b| b.japanese_saved.clone())
                {
                    Some(saved) => saved,
                    None => return,
                }
            } else {
                app.im.buffers.entry(id).or_default().japanese_saved = cur;
                "japanese-ascii".to_string()
            }
        }
        _ => {
            app.msg("No package to be switched");
            app.bell = true;
            return;
        }
    };
    let _ = activate(app, &target);
}

/// The conversion region of an input method with conversion keys
/// (`japanese`): what's typed gathers in it, transliterated to kana, until
/// RET accepts it as it is, SPC converts it to kanji (kkc), or a key it
/// has no use for ends it.
#[derive(Debug, Clone)]
struct Conv {
    buffer: BufferId,
    /// Where it starts in the buffer, and how much of the buffer it shows.
    start: usize,
    shown: usize,
    /// What it shows, and the cursor in it.
    text: Vec<char>,
    point: usize,
    /// quail-conversion-str: what it comes to.
    conv_str: String,
    /// A translation is open (quail-translating): its keys, and where in
    /// the region its text is.
    translating: bool,
    t: Translation,
    tstart: usize,
    tlen: usize,
    /// SPC's kana-kanji conversion, while it's on.
    kkc: Option<crate::kkc::Kkc>,
}

impl Conv {
    fn text(&self) -> String {
        self.text.iter().collect()
    }

    /// Show the translation's current text in its place.
    fn show_translation(&mut self) {
        let cur: Vec<char> = self.t.current.chars().collect();
        let end = (self.tstart + self.tlen).min(self.text.len());
        let start = self.tstart.min(end);
        self.text.splice(start..end, cur.iter().copied());
        self.tlen = cur.len();
        self.point = start + self.tlen;
    }
}

/// Show the region in the buffer: underlined, or (converting to kanji)
/// the current phrase highlighted.
fn conv_show(app: &mut App, c: &mut Conv) {
    let text = c.text();
    let n = c.text.len();
    let Some(b) = app.editor.state.get_buffer_mut(c.buffer) else {
        return;
    };
    b.replace_silently(c.start, c.start + c.shown, &text);
    c.shown = n;
    let decos = match &c.kkc {
        Some(k) => {
            let (a, z, u) = k.spans();
            vec![
                DecoSpec::new(c.start + a, c.start + z, DecoFace::Highlight),
                DecoSpec::new(c.start + u.max(z), c.start + n, DecoFace::Underline),
            ]
        }
        None => vec![DecoSpec::new(c.start, c.start + n, DecoFace::Underline)],
    };
    b.set_decorations(OWNER, decos);
    b.set_offset(c.start + c.point);
}

/// The conversion is over: the region goes, and what it came to is typed.
fn conv_end(app: &mut App, mut c: Conv, type_it: bool) {
    c.text.clear();
    c.point = 0;
    c.kkc = None;
    conv_show(app, &mut c);
    if let Some(b) = app.editor.state.get_buffer_mut(c.buffer) {
        b.set_decorations(OWNER, Vec::<DecoSpec>::new());
    }
    if type_it {
        for ch in c.conv_str.chars() {
            app.route_key(char_event(ch));
        }
    }
    end_transient(app, c.buffer);
}

/// Translate the keys typed so far in the region (quail-self-insert-command
/// and quail-delete-last-char): the keys to read again, or the function
/// rule they are.
fn conv_translate(c: &mut Conv) -> Result<String, String> {
    match c.t.translate_updated() {
        Control::Func(f) => Err(f),
        Control::More => {
            c.show_translation();
            Ok(String::new())
        }
        Control::Done => {
            c.show_translation();
            c.translating = false;
            Ok(String::new())
        }
        Control::Head(n) => {
            if n == 0 {
                c.t.current = c.t.keys.clone();
            }
            c.show_translation();
            c.translating = false;
            Ok(suffix(&c.t.keys, n).to_string())
        }
    }
}

/// A key while the conversion region is up (quail-conversion-keymap, with
/// the method's conversion keys).
fn conv_key(app: &mut App, mut c: Conv, key: KeyEvent) {
    if let Some(k) = c.kkc.take() {
        return kkc_key(app, c, k, key);
    }
    if !c.translating {
        c.t.keys.clear();
        c.t.current.clear();
        c.t.candidates.clear();
        c.translating = true;
        c.tstart = c.point;
        c.tlen = 0;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let len = c.text.len();
    let mut converting = true;
    let mut rest = String::new();
    let mut error: Option<&str> = None;
    // Whether a command of the keymap ran (then the translation, if
    // over, joins the conversion string).
    let mut ran = true;
    let mut unread = None;
    match (key.code, ctrl, alt) {
        (_, _, true) => {
            converting = false;
            ran = false;
            unread = Some(key);
        }
        // K: hiragana to katakana or back (quail-japanese-toggle-kana).
        (KeyCode::Char('K'), false, _) => {
            c.translating = false;
            let text = c.text();
            let text = if text.chars().any(crate::kkc::is_hiragana) {
                crate::kkc::katakana(&text)
            } else {
                crate::kkc::hiragana(&text)
            };
            c.text = text.chars().collect();
            c.point = c.point.min(c.text.len());
            c.conv_str = text;
        }
        // SPC: to kanji (quail-japanese-kanji-kkc).
        (KeyCode::Char(' '), false, _) => {
            if c.text.last() == Some(&'n') {
                *c.text.last_mut().expect("not empty") = 'ん';
            }
            c.t.current.clear();
            let cols = app.editor.windows.frame().width.max(1) as usize;
            let k = crate::kkc::Kkc::new(&c.text(), &mut app.im.learned, cols);
            c.text = k.text().chars().collect();
            c.point = c.text.len();
            c.kkc = Some(k);
            app.message = None;
            conv_show(app, &mut c);
            app.im.conv = Some(c);
            return;
        }
        (KeyCode::Char(ch), false, _)
            if (' '..='~').contains(&ch) || ('\u{80}'..='\u{ff}').contains(&ch) =>
        {
            c.t.keys.push(ch);
            match conv_translate(&mut c) {
                Ok(r) => rest = r,
                Err(f) => {
                    // quail-japanese-switch-package: the keys' text goes;
                    // switching input methods drops the region's overlay,
                    // so the rest stays in the buffer, as typed.
                    let keys = c.t.keys.clone();
                    let end = (c.tstart + c.tlen).min(c.text.len());
                    c.text.drain(c.tstart.min(end)..end);
                    c.conv_str = c.text();
                    conv_end(app, c, true);
                    app.message = None;
                    function_rule(app, &f, &keys);
                    return;
                }
            }
        }
        // RET: as it is (quail-no-conversion).
        (KeyCode::Enter, _, _) | (KeyCode::Char('m'), true, _) => {
            converting = false;
            ran = false;
        }
        (KeyCode::Char('b'), true, _) => {
            if c.point == 0 {
                error = Some("Beginning of conversion region");
            } else {
                c.translating = false;
                c.point -= 1;
            }
        }
        (KeyCode::Char('f'), true, _) => {
            if c.point >= len {
                error = Some("End of conversion region");
            } else {
                c.translating = false;
                c.point += 1;
            }
        }
        (KeyCode::Char('a'), true, _) => {
            c.translating = false;
            c.point = 0;
        }
        (KeyCode::Char('e'), true, _) => {
            c.translating = false;
            c.point = len;
        }
        (KeyCode::Char('d'), true, _) => {
            c.translating = false;
            if c.point >= len {
                error = Some("End of conversion region");
            } else {
                c.text.remove(c.point);
                c.conv_str = c.text();
                converting = !c.text.is_empty();
            }
        }
        (KeyCode::Char('k'), true, _) => {
            if c.point >= len {
                error = Some("End of conversion region");
            } else {
                c.text.truncate(c.point);
                c.conv_str = c.text();
                converting = !c.text.is_empty();
            }
        }
        (KeyCode::Char('h'), true, _) => {
            app.msg(c.t.method.doc.clone());
        }
        (KeyCode::Backspace | KeyCode::Delete, _, _) => {
            let n = c.t.keys.chars().count();
            if n == 1 {
                let end = (c.tstart + c.tlen).min(c.text.len());
                c.text.drain(c.tstart.min(end)..end);
                c.point = c.tstart.min(c.text.len());
                c.tlen = 0;
                c.t.current.clear();
                c.translating = false;
            } else if n > 1 {
                c.t.keys.pop();
                rest = conv_translate(&mut c).unwrap_or_default();
            } else if c.point == 0 {
                error = Some("Beginning of conversion region");
            } else {
                c.point -= 1;
                c.text.remove(c.point);
                // The (empty) translation is where point is.
                c.tstart = c.point;
                c.conv_str = c.text();
                converting = !c.text.is_empty();
            }
        }
        // Any other key ends the conversion, and is read again.
        _ => {
            converting = false;
            ran = false;
            unread = Some(key);
        }
    }
    if let Some(e) = error {
        app.msg(e);
        app.bell = true;
    } else if ran {
        app.message = None;
    }
    if ran && !c.translating {
        c.conv_str.push_str(&c.t.current);
        if c.conv_str.is_empty() {
            converting = false;
        }
    }
    if !converting {
        conv_end(app, c, true);
        for ch in rest.chars() {
            feed(app, char_event(ch));
        }
        if let Some(k) = unread {
            feed(app, k);
        }
        return;
    }
    conv_show(app, &mut c);
    app.im.conv = Some(c);
    for ch in rest.chars() {
        feed(app, char_event(ch));
    }
}

/// A key while converting to kanji (kkc-keymap).
fn kkc_key(app: &mut App, mut c: Conv, mut k: crate::kkc::Kkc, key: KeyEvent) {
    use crate::kkc::{Key, Out};
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let kk = match (key.code, ctrl, alt) {
        (_, _, true) => Key::Other,
        (KeyCode::Char(' '), false, _) | (KeyCode::Char('n'), true, _) => Key::Next,
        (KeyCode::Char('p'), true, _) => Key::Prev,
        (KeyCode::Char(d), false, _) if d.is_ascii_digit() => Key::Digit(d),
        (KeyCode::Enter, _, _) | (KeyCode::Char('m'), true, _) => Key::Terminate,
        (KeyCode::Char(' ' | '@'), true, _) => Key::FirstChar,
        (KeyCode::Tab, false, _) | (KeyCode::Char('i'), true, _) => Key::Shorter,
        (KeyCode::Char('o'), true, _) => Key::Longer,
        (KeyCode::Char('I'), false, _) => Key::ShorterConversion,
        (KeyCode::Char('O'), false, _) => Key::LongerPhrase,
        (KeyCode::Char('c'), true, _) | (KeyCode::Backspace | KeyCode::Delete, _, _) => Key::Cancel,
        (KeyCode::Char('f'), true, _) => Key::NextPhrase,
        (KeyCode::Char('K'), false, _) => Key::Katakana,
        (KeyCode::Char('H'), false, _) => Key::Hiragana,
        (KeyCode::Char('l'), false, _) => Key::NextGroup,
        (KeyCode::Char('L'), false, _) => Key::PrevGroup,
        (KeyCode::Char('h'), true, _) => Key::Help,
        _ => Key::Other,
    };
    let out = k.key(&mut app.im.learned, kk);
    if k.bell {
        app.bell = true;
    }
    match &k.message {
        Some(m) => app.msg(m.clone()),
        None if out == Out::More => app.message = None,
        None => {}
    }
    c.text = k.text().chars().collect();
    c.point = c.text.len();
    match out {
        Out::More => {
            c.kkc = Some(k);
            conv_show(app, &mut c);
            app.im.conv = Some(c);
        }
        Out::Cancelled => {
            // Back to the kana.
            c.conv_str = c.text();
            c.translating = false;
            c.t.current.clear();
            conv_show(app, &mut c);
            app.im.conv = Some(c);
        }
        Out::Done | Out::Unread => {
            c.conv_str = c.text();
            conv_end(app, c, true);
            if out == Out::Unread {
                feed(app, key);
            }
        }
    }
}

// ------------------------------------------------- Hangul and ucs

/// The first key for a native input method: Hangul's jamo, or ucs's `U`.
fn native_start(app: &mut App, layout: Option<crate::hangul::Layout>, c: char, key: KeyEvent) {
    let id = app.active_id();
    match layout {
        Some(layout) if crate::hangul::Hangul::takes(layout, c) => {
            let n = Native::Hangul {
                shown: Shown::at_point(app),
                h: crate::hangul::Hangul::new(layout),
            };
            native_step(app, n, key);
        }
        None if c == 'U' || c == 'u' => {
            let mut shown = Shown::at_point(app);
            let keys = c.to_string();
            shown.show(app, &keys);
            app.im.native = Some(Native::Ucs { shown, keys });
        }
        // Not a key it takes: as typed. (Hangul runs no chunk hook here;
        // ucs runs it whatever the key.)
        _ => {
            app.route_key(key);
            if layout.is_none() {
                end_transient(app, id);
            }
        }
    }
}

/// A key while Hangul or ucs is at work.
fn native_step(app: &mut App, n: Native, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match n {
        Native::Hangul { mut shown, mut h } => {
            let layout = h_layout(&h);
            match (key.code, plain_char(&key)) {
                (_, Some(c)) if crate::hangul::Hangul::takes(layout, c) => {
                    let out = h.key(c);
                    if !out.done.is_empty() {
                        shown.commit(app, &out.done);
                    }
                    shown.show(app, &out.composing);
                    app.im.native = Some(Native::Hangul { shown, h });
                }
                (KeyCode::Backspace, _) => {
                    match h.delete() {
                        Some(out) => shown.show(app, &out.composing),
                        // Nothing left of the syllable: the character
                        // before point goes, as DEL does.
                        None => {
                            shown.clear(app);
                            app.route_key(key);
                            shown.start = app.buf_ref().point_offset();
                        }
                    }
                    app.im.native = Some(Native::Hangul { shown, h });
                }
                // F9: the Hanja for the syllable (or the character before
                // point).
                (KeyCode::F(9), _) => {
                    let ch = h.composing().chars().next().or_else(|| {
                        let b = app.buf_ref();
                        let p = b.point_offset();
                        (p > 0)
                            .then(|| b.text_offsets(p - 1, p).chars().next())
                            .flatten()
                    });
                    let list = ch.map(crate::hangul::hanja).unwrap_or_default();
                    let n = Native::Hangul { shown, h };
                    if list.is_empty() {
                        app.im.native = Some(n);
                    } else {
                        let list = crate::hangul::HanjaList {
                            base: 0,
                            offset: 0,
                            list,
                        };
                        app.msg(list.show());
                        app.im.hanja = Some(Hanja {
                            list,
                            from: Some(n),
                        });
                    }
                }
                // Anything else: the syllable is done, and the key does
                // what it does.
                _ => {
                    let text = h.composing().to_string();
                    shown.commit(app, &text);
                    shown.clear(app);
                    end_transient(app, shown.buffer);
                    let _ = ctrl;
                    feed(app, key);
                }
            }
        }
        Native::Ucs {
            mut shown,
            mut keys,
        } => {
            match plain_char(&key) {
                Some(c) if c.is_ascii_hexdigit() => {
                    keys.push(c);
                    if keys.chars().count() == 5 {
                        let code = u32::from_str_radix(&keys[1..], 16)
                            .ok()
                            .and_then(char::from_u32);
                        shown.clear(app);
                        if let Some(c) = code {
                            shown.commit(app, &c.to_string());
                        }
                        end_transient(app, shown.buffer);
                    } else {
                        shown.show(app, &keys);
                        app.im.native = Some(Native::Ucs { shown, keys });
                    }
                }
                // Not a hex digit: what was typed, then the key, as they
                // are (not to the input method again).
                _ => {
                    shown.commit(app, &keys);
                    shown.clear(app);
                    end_transient(app, shown.buffer);
                    app.route_key(key);
                }
            }
        }
    }
}

fn h_layout(h: &crate::hangul::Hangul) -> crate::hangul::Layout {
    h.layout()
}

/// A key while the Hanja list is up (hanja-keymap).
fn hanja_step(app: &mut App, mut hj: Hanja, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let chosen = match (key.code, ctrl) {
        (KeyCode::Enter, _) | (KeyCode::Char('m'), true) => Some(hj.list.selected()),
        (KeyCode::Up, _) | (KeyCode::Char('p'), true) => {
            hj.list.prev_group();
            None
        }
        (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
            hj.list.next_group();
            None
        }
        (KeyCode::Left, _) | (KeyCode::Char('b'), true) => {
            hj.list.prev();
            None
        }
        (KeyCode::Right, _) | (KeyCode::Char('f'), true) => {
            hj.list.next();
            None
        }
        (KeyCode::Char(d), false) if d.is_ascii_digit() => {
            hj.list.offset = d.to_digit(10).unwrap_or(0) as usize;
            Some(hj.list.selected())
        }
        _ => {
            // Not a key of the list: no conversion, and the key does what
            // it does.
            app.message = None;
            app.im.native = hj.from;
            return feed(app, key);
        }
    };
    match chosen {
        None => {
            app.msg(hj.list.show());
            app.im.hanja = Some(hj);
        }
        Some(c) => {
            app.message = None;
            match hj.from {
                Some(Native::Hangul { mut shown, mut h }) => {
                    if let Some(c) = c {
                        if h.composing().is_empty() {
                            // The character before point was converted.
                            app.buf().backspace();
                        }
                        h.reset();
                        shown.clear(app);
                        shown.commit(app, &c.to_string());
                    }
                    app.im.native = Some(Native::Hangul { shown, h });
                }
                _ => {
                    // M-x hangul-to-hanja-conversion: the character after
                    // point.
                    if let Some(c) = c {
                        let b = app.buf();
                        b.delete_char();
                        b.insert_str(&c.to_string());
                    }
                }
            }
        }
    }
}

/// One key during a translation.
fn step(app: &mut App, mut t: Translation, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let tab = key.code == KeyCode::Tab || (ctrl && key.code == KeyCode::Char('i'));
    if !tab {
        app.im.completion_again = false;
    }
    let complex = !t.method.simple;
    match (key.code, ctrl) {
        (KeyCode::Char(c), false) if !key.modifiers.contains(KeyModifiers::ALT) => {
            t.keys.push(c);
            let control = t.translate_updated();
            update(app, t, control);
        }
        (KeyCode::Backspace, _) => {
            // quail-delete-last-char: one key back, or no translation.
            if t.keys.chars().count() <= 1 {
                clear(app, &t);
                app.message = None;
                end_transient(app, t.buffer);
                return;
            }
            t.keys.pop();
            let control = t.translate_updated();
            update(app, t, control);
        }
        (KeyCode::Char('f'), true) | (KeyCode::Right, false)
            if complex && !t.candidates.is_empty() =>
        {
            if t.index + 1 < t.candidates.len() {
                t.pick(t.index + 1);
            } else {
                app.bell = true;
            }
            t.current = t.candidates[t.index].clone();
            redisplay(app, &mut t);
            app.im.translation = Some(t);
        }
        (KeyCode::Char('b'), true) | (KeyCode::Left, false)
            if complex && !t.candidates.is_empty() =>
        {
            if t.index > 0 {
                t.pick(t.index - 1);
            } else {
                app.bell = true;
            }
            t.current = t.candidates[t.index].clone();
            redisplay(app, &mut t);
            app.im.translation = Some(t);
        }
        (KeyCode::Char('n'), true) | (KeyCode::Down, false)
            if complex && t.candidates.len() > 10 =>
        {
            t.pick((t.index / 10 * 10 + 10).min(t.candidates.len() - 1));
            t.current = t.candidates[t.index].clone();
            redisplay(app, &mut t);
            app.im.translation = Some(t);
        }
        (KeyCode::Char('p'), true) | (KeyCode::Up, false) if complex && t.candidates.len() > 10 => {
            t.pick((t.index / 10 * 10).saturating_sub(10));
            t.current = t.candidates[t.index].clone();
            redisplay(app, &mut t);
            app.im.translation = Some(t);
        }
        // TAB: list the completions (quail-completion), in the methods
        // whose translation keymap has it.
        (KeyCode::Tab, _) | (KeyCode::Char('i'), true)
            if complex || app.im.completion_everywhere =>
        {
            completion(app, &t);
            app.im.translation = Some(t);
        }
        // C-SPC: take the current candidate.
        (KeyCode::Char(' ' | '@'), true) if complex => commit(app, t, ""),
        // Anything else ends the translation, then does what it does.
        _ => {
            commit(app, t, "");
            // Read again (after a transient input method, as an ordinary
            // key).
            feed(app, key);
        }
    }
}

/// Act on what translate said: show more, or commit (re-reading the keys
/// left over).
fn update(app: &mut App, mut t: Translation, control: Control) {
    match control {
        Control::More => {
            redisplay(app, &mut t);
            app.im.translation = Some(t);
        }
        Control::Done => commit(app, t, ""),
        Control::Func(f) => {
            clear(app, &t);
            app.message = None;
            function_rule(app, &f, &t.keys);
        }
        Control::Head(n) => {
            if n == 0 {
                t.current = t.keys.clone();
            }
            let rest = suffix(&t.keys, n).to_string();
            commit(app, t, &rest);
        }
    }
}

/// End the translation: its text is typed (each character self-inserts,
/// as the events an input method returns do), the chunk is over (a
/// transient input method goes off), and `rest` is read again.
fn commit(app: &mut App, t: Translation, rest: &str) {
    remember(app, &t);
    clear(app, &t);
    app.message = None;
    for c in t.current.chars() {
        app.route_key(char_event(c));
    }
    end_transient(app, t.buffer);
    for c in rest.chars() {
        feed(app, char_event(c));
    }
}

// ---------------------------------------------------------- commands

// ------------------------------------------------------- completion

/// The keys that can follow `keys` directly, in order.
fn next_chars(m: &Method, keys: &str) -> Vec<char> {
    m.node(keys)
        .map(|n| n.kids.iter().map(|(c, _)| *c).collect())
        .unwrap_or_default()
}

/// quail-completion-1: `keys`, what they translate to, and (to
/// COMPLETION_MAX_DEPTH) the keys that can follow, indented.
fn completion_tree(
    m: &Method,
    keys: &str,
    indent: usize,
    out: &mut String,
    spans: &mut Vec<(usize, usize, usize, String)>,
) {
    let line = out.matches('\n').count();
    let head = format!("{}{keys}:", " ".repeat(indent));
    out.push_str(&head);
    let mut col = head.chars().count();
    match m.def(keys) {
        Some(cands) => {
            // Ten to a line, each line after the first under the first.
            let pad = indent + keys.chars().count() + 1;
            let blocks = cands.len() / 10 + 1;
            let mut line = line;
            for (i, c) in cands.iter().enumerate() {
                if i % 10 == 0 {
                    if i >= 10 {
                        out.push('\n');
                        out.push_str(&" ".repeat(pad));
                        line += 1;
                        col = pad;
                    }
                    let b = format!("({}/{blocks})", i / 10 + 1);
                    col += b.chars().count();
                    out.push_str(&b);
                }
                let n = format!(" {}.", (i + 1) % 10);
                col += n.chars().count();
                out.push_str(&n);
                let w = c.chars().count();
                spans.push((line, col, col + w, c.clone()));
                col += w;
                out.push_str(c);
            }
        }
        None => out.push_str(" -"),
    }
    out.push('\n');
    let indent = indent + 2;
    if (indent - 1) / 2 < COMPLETION_MAX_DEPTH {
        for c in next_chars(m, keys) {
            completion_tree(m, &format!("{keys}{c}"), indent, out, spans);
        }
    }
}

/// The window showing *Quail Completions*, if one does.
fn completions_window(app: &App) -> Option<(forge_core::WindowId, BufferId)> {
    let id = app.editor.find_buffer(COMPLETIONS)?;
    app.editor
        .windows
        .iter()
        .find(|w| w.buffer == id)
        .map(|w| (w.id, id))
}

/// TAB during a translation (quail-completion): every completion of the
/// keys typed so far, and what each translates to, in *Quail
/// Completions* (shown, not selected). TAB again scrolls it.
fn completion(app: &mut App, t: &Translation) {
    if app.im.completion_again
        && app.im.completion_key.as_deref() == Some(t.keys.as_str())
        && let Some((win, _)) = completions_window(app)
    {
        // Already up: scroll it a page, and back to the top at the end.
        let lines = app
            .editor
            .state
            .get_buffer(app.editor.find_buffer(COMPLETIONS).unwrap())
            .map_or(0, |b| b.line_count());
        if let Some(w) = app.editor.windows.get_mut(win) {
            let page = w.height.max(1);
            w.scroll_top = if w.scroll_top + page >= lines {
                0
            } else {
                w.scroll_top + page.saturating_sub(2).max(1)
            };
            w.scroll_y_px = 0;
            let top = w.scroll_top;
            let id = w.buffer;
            let offset = app
                .editor
                .state
                .get_buffer(id)
                .map_or(0, |b| b.pos_to_offset(forge_core::Pos::new(top, 0)));
            app.editor.set_window_point(win, offset);
        }
        return;
    }
    let mut text = String::from("Possible completion and corresponding characters are:\n");
    let mut spans = Vec::new();
    completion_tree(t.method, &t.keys, 1, &mut text, &mut spans);
    let id = match app.editor.find_buffer(COMPLETIONS) {
        Some(id) => id,
        None => app.editor.add_buffer(forge_core::Buffer::new(COMPLETIONS)),
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_read_only(false);
        b.set_contents(&text);
        b.set_major_mode("special-mode");
        b.set_read_only(true);
        b.set_modified(false);
    }
    app.im.completion_spans = spans;
    app.im.completion_key = Some(t.keys.clone());
    app.im.completion_again = true;
    crate::compile::display_buffer(app, id);
    if let Some((win, _)) = completions_window(app) {
        if let Some(w) = app.editor.windows.get_mut(win) {
            w.scroll_top = 0;
            w.scroll_y_px = 0;
        }
        app.editor.set_window_point(win, 0);
    }
    show_in_completions(app, t);
}

/// While *Quail Completions* is up, highlight the keys' entry and the
/// candidate shown, and keep them in view (quail-update-guidance).
fn show_in_completions(app: &mut App, t: &Translation) {
    let Some((win, id)) = completions_window(app) else {
        return;
    };
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let text = b.get_contents();
    let needle = format!(" {}:", t.keys);
    let Some(at) = text.find(&needle) else {
        b.set_decorations(OWNER, Vec::<DecoSpec>::new());
        return;
    };
    let key_end = at + needle.len();
    let target = format!(".{}", t.current);
    let (start, end) = match text[key_end..]
        .find(&target)
        .filter(|_| !t.current.is_empty())
    {
        Some(i) => (key_end + i + 1, key_end + i + target.len()),
        None => (at, key_end),
    };
    let chars = |byte: usize| text[..byte].chars().count();
    let (a, z) = (chars(start), chars(end));
    b.set_decorations(OWNER, vec![DecoSpec::new(a, z, DecoFace::Highlight)]);
    let line_start = chars(text[..at].rfind('\n').map_or(0, |i| i + 1));
    app.editor.set_window_point(win, line_start);
}

/// A click on a candidate in *Quail Completions* chooses it
/// (quail-mouse-choose-completion). True if it was one.
pub fn choose_at(app: &mut App, buffer: BufferId, line: usize, col: usize) -> bool {
    if app.editor.find_buffer(COMPLETIONS) != Some(buffer) {
        return false;
    }
    let Some(t) = app.im.translation.take() else {
        return false;
    };
    let chosen = app
        .im
        .completion_spans
        .iter()
        .find(|(l, a, z, _)| *l == line && (*a..*z).contains(&col))
        .map(|(.., c)| c.clone());
    let Some(chosen) = chosen else {
        app.im.translation = Some(t);
        return false;
    };
    let mut t = t;
    t.current = chosen;
    // Typed where the translation is, whichever window was clicked.
    let window = app
        .editor
        .windows
        .iter()
        .find(|w| w.buffer == t.buffer)
        .map(|w| w.id);
    if app.active_id() != t.buffer
        && let Some(w) = window
    {
        app.select_window(w);
    }
    commit(app, t, "");
    true
}

/// What an input-method prompt is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Set,
    Toggle,
    Transient,
    Describe,
}

/// Rebuilds the list only when this changes.
pub fn list_key(_ask: &Ask) -> String {
    "input-methods".into()
}

pub fn candidates() -> Vec<String> {
    names().into_iter().map(String::from).collect()
}

/// Each method's annotation: its title and language.
pub fn notes(all: &[String]) -> Vec<String> {
    let s = summaries();
    all.iter()
        .map(|n| {
            s.iter()
                .find(|(name, ..)| name == n)
                .map_or_else(String::new, |(_, lang, title, _)| {
                    format!("{title}  {lang}")
                })
        })
        .collect()
}

/// The default a prompt offers: the last method used, else the default.
fn prompt_default(app: &App) -> Option<String> {
    app.im
        .history
        .first()
        .cloned()
        .or_else(|| app.im.default.clone())
}

fn ask(app: &mut App, what: &str, ask: Ask, default: Option<String>) {
    let prompt = match &default {
        Some(d) => format!("{what} (default {d}): "),
        None => format!("{what}: "),
    };
    app.prompt(prompt, Purpose::InputMethodAsk { what: ask, default });
}

pub fn answered(app: &mut App, what: Ask, default: Option<String>, input: &str) {
    let input = input.trim();
    let name = if input.is_empty() {
        default
    } else {
        Some(input.to_string())
    };
    let Some(name) = name else { return };
    let Some(m) = method(&name) else {
        return app.error(format!("Invalid input method: {name}"));
    };
    match what {
        Ask::Set | Ask::Toggle => {
            app.im.default = Some(m.name.clone());
            if let Err(e) = activate(app, &m.name) {
                app.error(e);
            }
        }
        Ask::Transient => {
            app.im.default_transient = Some(m.name.clone());
            transient(app, &m.name.clone());
        }
        Ask::Describe => describe(app, m),
    }
}

/// Turn `name` on for one chunk, remembering what was on.
fn transient(app: &mut App, name: &str) {
    let id = app.active_id();
    finish(app);
    end_transient(app, id);
    let previous = current(app, id).map(String::from);
    if let Err(e) = activate(app, name) {
        return app.error(e);
    }
    let b = app.im.buffers.entry(id).or_default();
    b.transient = true;
    b.previous = previous;
}

fn describe(app: &mut App, m: &Method) {
    let mut text = format!(
        "Input method: {} (`{}' in mode line) for {}\n  {}\n\n",
        m.name, m.title, m.language, m.doc
    );
    if m.name == current(app, app.active_id()).unwrap_or("") {
        text.push_str("It is on in this buffer (C-\\ turns it off).\n\n");
    }
    text.push_str("KEY SEQUENCE\tTRANSLATION\n------------\t-----------\n");
    // Maps can loop (Tibetan's states): eight keys deep is plenty.
    for (keys, cands) in &m.rules(8) {
        let shown = keys.replace(' ', "SPC");
        text.push_str(&format!("{shown}\t\t{}\n", cands.join(" ")));
    }
    if !m.simple {
        text.push_str(
            "\nWith several translations, a digit picks one, and C-f / C-b (C-n / C-p by tens) move through them.\n",
        );
    }
    app.show_special_buffer("*Help*", "help-mode", &text);
}

fn list(app: &mut App) {
    let mut by_language: BTreeMap<&str, Vec<(&str, &str, &str)>> = BTreeMap::new();
    for (name, lang, title, doc) in summaries() {
        by_language
            .entry(lang)
            .or_default()
            .push((name, title, doc));
    }
    let mut text = String::from(
        "Input methods: C-x RET C-\\ turns one on, C-\\ toggles it, C-x \\ is one for the next character only, C-h I describes one.\n",
    );
    for (lang, methods) in by_language {
        text.push_str(&format!("\n{lang}\n"));
        for (name, title, doc) in methods {
            text.push_str(&format!("  {name} (`{title}' in mode line)\n\t{doc}\n"));
        }
    }
    app.show_special_buffer("*Input Methods*", "help-mode", &text);
}

/// Run one of this module's commands; false if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "toggle-input-method" => {
            let id = app.active_id();
            if current(app, id).is_some() && !app.has_arg() {
                deactivate(app);
            } else if let (Some(d), false) = (app.im.default.clone(), app.has_arg()) {
                if let Err(e) = activate(app, &d) {
                    app.error(e);
                }
            } else {
                let d = prompt_default(app);
                ask(app, "Select input method", Ask::Toggle, d);
            }
        }
        "set-input-method" => {
            let d = prompt_default(app);
            ask(app, "Select input method", Ask::Set, d);
        }
        "activate-transient-input-method" => match app.im.default_transient.clone() {
            Some(name) if !app.has_arg() => transient(app, &name),
            _ => {
                let d = prompt_default(app);
                ask(app, "Transient input method", Ask::Transient, d);
            }
        },
        "describe-input-method" => {
            let d = current(app, app.active_id())
                .map(String::from)
                .or_else(|| prompt_default(app));
            ask(app, "Describe input method", Ask::Describe, d);
        }
        "list-input-methods" => list(app),
        _ => return false,
    }
    true
}

/// Bind C-\, C-x \, C-x RET C-\ and C-h I.
pub fn install_keymaps(keymaps: &mut forge_core::keymap::Keymaps) {
    use forge_core::keymap::{Origin, Scope};
    for (keys, cmd) in BINDINGS {
        keymaps
            .bind(&Scope::Global, keys, cmd, Origin::Package)
            .expect("valid input method binding");
    }
}

/// init.rhai's default_input_method and default_transient_input_method.
pub fn apply_settings(app: &mut App, settings: &forge_core::rhai::Settings) -> Result<(), String> {
    if let Some(b) = settings.flags.get("input_method_completion_everywhere") {
        app.im.completion_everywhere = *b;
    }
    for (key, slot) in [
        ("default_input_method", 0),
        ("default_transient_input_method", 1),
    ] {
        if let Some(name) = settings.strings.get(key).and_then(|v| v.first()) {
            if method(name).is_none() {
                return Err(format!(
                    "forge.opt.{key}: no input method {name:?} (M-x list-input-methods)"
                ));
            }
            let target = if slot == 0 {
                &mut app.im.default
            } else {
                &mut app.im.default_transient
            };
            *target = Some(name.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed keys to a translation from scratch: what gets committed, and
    /// what is still shown.
    fn run(name: &str, keys: &str) -> (String, String) {
        let m = method(name).unwrap();
        let mut out = String::new();
        let mut t: Option<Translation> = None;
        let mut queue: std::collections::VecDeque<char> = keys.chars().collect();
        while let Some(c) = queue.pop_front() {
            let mut tr = match t.take() {
                Some(tr) => tr,
                None if !m.starts(c) => {
                    out.push(c);
                    continue;
                }
                None => Translation {
                    buffer: BufferId(0),
                    method: m,
                    start: 0,
                    shown: 0,
                    keys: String::new(),
                    current: String::new(),
                    candidates: Vec::new(),
                    index: 0,
                    remembered: HashMap::new(),
                },
            };
            tr.keys.push(c);
            match tr.translate() {
                Control::More => t = Some(tr),
                Control::Done | Control::Func(_) => out.push_str(&tr.current),
                Control::Head(n) => {
                    out.push_str(if n == 0 { &tr.keys[..] } else { &tr.current });
                    for (i, r) in suffix(&tr.keys, n).chars().enumerate() {
                        queue.insert(i, r);
                    }
                }
            }
        }
        (out, t.map(|t| t.current).unwrap_or_default())
    }

    /// Type `keys` into a new buffer with input method `name` on, then
    /// C-g: what the buffer holds. One app for a run of them, as one Forge
    /// session typed them (quail remembers picks across translations).
    pub(crate) fn typed_in(app: &mut App, name: &str, keys: &str) -> String {
        let id = app.editor.create_buffer("case");
        app.switch_to(id);
        activate(app, name).unwrap();
        for c in keys.chars() {
            app.handle_key(char_event(c));
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        let text = app.buf_ref().get_contents();
        let scratch = app.editor.find_buffer("*scratch*");
        app.switch_to(scratch.unwrap());
        app.editor.kill_buffer(id, scratch);
        text
    }

    pub(crate) fn new_app() -> App {
        let mut editor = forge_core::Editor::new();
        editor.create_buffer("*scratch*");
        App::new(editor)
    }

    #[test]
    fn completion_spans_are_where_the_candidates_are() {
        let m = method("latin-prefix").unwrap();
        let mut text = String::new();
        let mut spans = Vec::new();
        completion_tree(m, "''", 1, &mut text, &mut spans);
        let line: Vec<char> = text.chars().collect();
        assert!(!spans.is_empty());
        for (l, a, z, c) in &spans {
            assert_eq!(*l, 0);
            assert_eq!(line[*a..*z].iter().collect::<String>(), *c);
        }
    }

    #[test]
    fn the_methods_are_here() {
        let names = names();
        for n in [
            "TeX",
            "latin-postfix",
            "latin-prefix",
            "greek",
            "russian-translit",
            "japanese",
            "japanese-katakana",
            "korean-hangul",
            "ucs",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        let tex = method("TeX").unwrap();
        assert_eq!(tex.title, "\\");
        assert!(tex.deterministic);
        assert_eq!(method("latin-postfix").unwrap().title, "L<");
    }

    #[test]
    fn postfix_methods_wait_for_the_modifier() {
        // e' shows é, but e'' is longer (it types e'): nothing committed
        // yet.
        assert_eq!(run("latin-postfix", "e'"), (String::new(), "é".into()));
        assert_eq!(run("latin-postfix", "e' "), ("é ".into(), String::new()));
        // "e" alone could still become é: it shows, uncommitted.
        assert_eq!(run("latin-postfix", "e"), (String::new(), "e".into()));
        // A key that can't follow commits the e and is read again.
        assert_eq!(run("latin-postfix", "e1"), ("e1".into(), String::new()));
        assert_eq!(
            run("latin-postfix", "cafe' "),
            ("café ".into(), String::new())
        );
        // Doubling the modifier gets the plain characters back.
        assert_eq!(run("latin-postfix", "e''"), ("e'".into(), String::new()));
    }

    #[test]
    fn prefix_methods_and_tex() {
        assert_eq!(run("latin-prefix", "'e"), ("é".into(), String::new()));
        assert_eq!(run("TeX", "\\alpha"), ("α".into(), String::new()));
        assert_eq!(
            run("TeX", "\\to"),
            (String::new(), "→".into()),
            "\\top is longer: wait"
        );
        assert_eq!(run("TeX", "\\to "), ("→ ".into(), String::new()));
        assert_eq!(run("TeX", "\\\"o"), ("ö".into(), String::new()));
        // TeX is deterministic: an unknown name commits what it can.
        assert_eq!(run("TeX", "\\qq"), ("\\qq".into(), String::new()));
    }

    #[test]
    fn digits_pick_among_candidates() {
        // latin-prefix's '' is ´ or ': the first shows (the digit that picks is handled by the key loop).
        assert_eq!(run("latin-prefix", "''"), (String::new(), "´".into()));
    }

    #[test]
    fn greek_and_russian() {
        assert_eq!(run("greek", "ab"), ("αβ".into(), String::new()));
        assert_eq!(run("russian-translit", "privet "), ("привет ".into(), String::new()));
        assert_eq!(run("russian-translit", "sh"), (String::new(), "ш".into()));
    }

    #[test]
    fn japanese_kana() {
        assert_eq!(typed_in(&mut new_app(), "japanese-hiragana", "kyou"), "きょう");
        assert_eq!(typed_in(&mut new_app(), "japanese-hiragana", "kka"), "っか");
        assert_eq!(typed_in(&mut new_app(), "japanese-katakana", "kanzi"), "カンジ");
        assert_eq!(typed_in(&mut new_app(), "japanese-hankaku-kana", "ga"), "ｶﾞ");
    }
}
