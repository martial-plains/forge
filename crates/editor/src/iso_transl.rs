// SPDX-License-Identifier: MIT OR Apache-2.0
//! C-x 8.
//!
//! - **Key translations**: `C-x 8 ' e` types `é`,
//!   `C-x 8 " u` types `ü`, `C-x 8 a >` types `→`, `C-x 8 _ m` types `—`,
//!   and so on, about 300 of them (`C-x 8 C-h` lists them all).
//!   - A translation produces the character as if it had been typed, so it
//!     works wherever typing does: in a buffer (where it self-inserts and
//!     takes a prefix argument, `C-u 3 C-x 8 o`), in the minibuffer and in
//!     isearch, and in a keyboard macro.
//!   - `M-x iso-transl-set-language` adds a language's shorter keys
//!     (German `C-x 8 a` for `ä`).
//! - **Commands under C-x 8.**
//!   - `C-x 8 RET` (`insert-char`) reads a character by its Unicode name,
//!     with completion, or by its code (`e9`, `#xe9`, `#o351`, `#10r233`).
//!   - `C-x 8 e` holds the emoji commands: `e`/`i` insert by group, `s`
//!     searches by name, `l` lists them all, `r` offers recent ones, `d`
//!     describes one.
//! - **Inspecting characters.** `C-x =` (`what-cursor-position`) says
//!   which character is after point; with `C-u`, or as `M-x describe-char`,
//!   it says everything, including how to type it.
//!
//! Alt-key and dead-key variants of the translations aren't here:
//! a terminal doesn't send those keys.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::{BufferId, Pos, Scope};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthChar;

type Entry = (&'static str, char);

/// Accent keys and the combining mark each one adds to the letter typed
/// after it. The base letters are composed with Unicode normalization
/// (NFC), so the table covers every precomposed Latin letter Unicode has.
const ACCENTS: &[(char, char)] = &[
    ('\'', '\u{301}'), // acute
    ('`', '\u{300}'),  // grave
    ('"', '\u{308}'),  // diaeresis
    ('^', '\u{302}'),  // circumflex
    ('~', '\u{303}'),  // tilde
    (',', '\u{327}'),  // cedilla
    (')', '\u{306}'),  // breve
    ('v', '\u{30c}'),  // caron
    ('-', '\u{304}'),  // macron
    ('.', '\u{307}'),  // dot above
    (';', '\u{328}'),  // ogonek
    ('o', '\u{30a}'),  // ring above
];

/// Characters that don't decompose, typed with a short mnemonic.
const SYMBOLS: &[Entry] = &[
    (" ", '\u{a0}'),
    ("!", '¡'),
    ("?", '¿'),
    ("''", '´'),
    ("\"\"", '¨'),
    ("``", '`'),
    (",,", '¸'),
    ("--", '¯'),
    ("' ", '\''),
    ("` ", '`'),
    ("\" ", '"'),
    ("^ ", '^'),
    ("~ ", '~'),
    ("*E", '€'),
    ("*L", '£'),
    ("*Y", '¥'),
    ("*c", '¢'),
    ("*$", '¤'),
    ("c", '©'),
    ("r", '®'),
    ("tm", '™'),
    ("s", '§'),
    ("p", '¶'),
    ("0", '°'),
    ("+-", '±'),
    ("x", '×'),
    ("%", '÷'),
    ("u", 'μ'),
    ("1/2", '½'),
    ("1/4", '¼'),
    ("3/4", '¾'),
    ("1s", '¹'),
    ("2s", '²'),
    ("3s", '³'),
    ("<<", '«'),
    (">>", '»'),
    ("q<", '“'),
    ("q>", '”'),
    ("q,", '„'),
    ("Q<", '‘'),
    ("Q>", '’'),
    ("Q,", '‚'),
    ("_m", '—'),
    ("_n", '–'),
    ("_.", '…'),
    ("bu", '•'),
    ("a>", '→'),
    ("a<", '←'),
    ("a^", '↑'),
    ("av", '↓'),
    ("ne", '≠'),
    ("le", '≤'),
    ("ge", '≥'),
    ("ap", '≈'),
    ("8", '∞'),
    ("AE", 'Æ'),
    ("ae", 'æ'),
    ("OE", 'Œ'),
    ("oe", 'œ'),
    ("\"S", 'ẞ'),
    ("\"s", 'ß'),
    ("/o", 'ø'),
    ("/O", 'Ø'),
    ("/l", 'ł'),
    ("/L", 'Ł'),
    ("/d", 'đ'),
    ("/D", 'Đ'),
    ("dh", 'ð'),
    ("DH", 'Ð'),
    ("th", 'þ'),
    ("TH", 'Þ'),
];

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The translations typed after C-x 8: the symbols plus every accent key
/// followed by a letter that composes with it.
pub fn char_map() -> &'static [Entry] {
    static MAP: OnceLock<Vec<Entry>> = OnceLock::new();
    MAP.get_or_init(|| {
        use unicode_normalization::UnicodeNormalization;
        let mut v: Vec<Entry> = SYMBOLS.to_vec();
        for &(key, mark) in ACCENTS {
            for base in ('A'..='Z').chain('a'..='z') {
                let mut composed = [base, mark].into_iter().nfc();
                if let (Some(c), None) = (composed.next(), composed.next())
                    && c != base
                {
                    let k = leak(format!("{key}{base}"));
                    if !v.iter().any(|(e, _)| *e == k) {
                        v.push((k, c));
                    }
                }
            }
        }
        v.sort_by_key(|(k, _)| *k);
        v
    })
}

/// Language names and their extra, shorter keys, as "key=char" words.
const LANGUAGE_KEYS: &[(&str, &str)] = &[
    (
        "Esperanto",
        "C=Ĉ G=Ĝ H=Ĥ J=Ĵ S=Ŝ U=Ŭ c=ĉ g=ĝ h=ĥ j=ĵ s=ŝ u=ŭ",
    ),
    ("French", "C=Ç c=ç"),
    ("German", "A=Ä O=Ö U=Ü a=ä o=ö u=ü S=ẞ s=ß"),
    ("Portuguese", "C=Ç c=ç"),
    ("Spanish", "N=Ñ n=ñ"),
];

/// Languages with shorter keys (M-x iso-transl-set-language).
pub fn languages() -> &'static [(&'static str, &'static [Entry])] {
    static LANGS: OnceLock<Vec<(&'static str, &'static [Entry])>> = OnceLock::new();
    LANGS.get_or_init(|| {
        LANGUAGE_KEYS
            .iter()
            .map(|(name, words)| {
                let entries: Vec<Entry> = words
                    .split_whitespace()
                    .filter_map(|w| {
                        let (k, c) = w.split_once('=')?;
                        Some((leak(k.to_string()), c.chars().next()?))
                    })
                    .collect();
                (*name, &*Box::leak(entries.into_boxed_slice()))
            })
            .collect()
    })
}

/// The commands bound under C-x 8 (the translations come first).
pub const PREFIX_COMMANDS: &[(&str, &str)] = &[
    ("C-x 8 RET", "insert-char"),
    ("C-x 8 e e", "emoji-insert"),
    ("C-x 8 e i", "emoji-insert"),
    ("C-x 8 e s", "emoji-search"),
    ("C-x 8 e l", "emoji-list"),
    ("C-x 8 e r", "emoji-recent"),
    ("C-x 8 e d", "emoji-describe"),
];

/// The other bindings this module brings.
pub const BINDINGS: &[(&str, &str)] = &[("C-x =", "what-cursor-position")];

pub const EMOJI_LIST_MODE: &str = "emoji-list-mode";

/// *Emoji*'s keys (it derives from special-mode: q, n, p...).
pub const EMOJI_LIST_BINDINGS: &[(&str, &str)] = &[
    ("RET", "emoji-list-select"),
    ("<mouse-1>", "emoji-list-select"),
    ("d", "emoji-describe"),
];

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new("describe-char", "Describe the character after point: its code, Unicode name,").body("and how to type it (C-u C-x =)."),
    forge_docs::CommandDoc::line(
        "emoji-describe",
        "Say what the emoji at point is (its name and group), or ask for one.",
    ),
    forge_docs::CommandDoc::line(
        "emoji-insert",
        "Insert an emoji, picked from a group and then by name (C-x 8 e e).",
    ),
    forge_docs::CommandDoc::new("emoji-list", "List every emoji, by group, in *Emoji*; RET on one inserts it where you were").body("(C-x 8 e l)."),
    forge_docs::CommandDoc::line(
        "emoji-list-select",
        "Insert the emoji at point in the buffer emoji-list was called from.",
    ),
    forge_docs::CommandDoc::line(
        "emoji-recent",
        "Insert one of the emoji inserted recently (C-x 8 e r).",
    ),
    forge_docs::CommandDoc::line(
        "emoji-search",
        "Insert an emoji, found by its name with completion (C-x 8 e s).",
    ),
    forge_docs::CommandDoc::new("insert-char", "Insert a character, read by its Unicode name (with completion) or its code point").body("(C-x 8 RET).\n\nThe code is hexadecimal (e9), or #x, #o or #NrN notation (#xe9, #o351, #10r233). With an argument, insert that many."),
    forge_docs::CommandDoc::new("iso-transl-set-language", "Add a language's shorter C-x 8 keys (German: C-x 8 a for ä;").body("Spanish: C-x 8 n for ñ...)."),
    forge_docs::CommandDoc::new("what-cursor-position", "Say which character is after point (its code in decimal, octal and hex),").body("and where point is (C-x =).\n\nWith C-u, describe the character in full (describe-char)."),
];

/// A key sequence part way through C-x 8, outside the buffer's keymaps
/// (in the minibuffer and isearch, which read one key at a time).
#[derive(Default, Debug, Clone, PartialEq)]
enum Pending {
    #[default]
    None,
    /// C-x was typed: C-x 8 starts a translation, anything else is C-x
    /// followed by that key.
    HeldCx,
    /// The keys typed after C-x 8.
    Seq(Vec<String>),
}

#[derive(Default)]
pub struct State {
    pending: Pending,
    /// iso-transl-set-language's choice.
    pub language: Option<&'static str>,
    /// Emoji inserted lately, the latest first.
    pub recent_emoji: Vec<String>,
    /// The buffer *Emoji* inserts into.
    pub emoji_source: Option<BufferId>,
}

const RECENT_EMOJI: usize = 20;

// ------------------------------------------------------------ the table

/// What a key typed after C-x 8 contributes to the translation's string:
/// its character ("SPC" is a space, "S-a" an A). None for keys with
/// modifiers, which no translation uses.
pub fn key_char(desc: &str) -> Option<char> {
    if desc == "SPC" {
        return Some(' ');
    }
    if let Some(l) = desc.strip_prefix("S-") {
        let mut cs = l.chars();
        return match (cs.next(), cs.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
            _ => None,
        };
    }
    let mut cs = desc.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// The string the keys after C-x 8 spell, if they all are characters.
fn typed(keys: &[String]) -> Option<String> {
    keys.iter().map(|k| key_char(k)).collect()
}

/// The keys of a translation, as typed after C-x 8 ("' e", "SPC").
pub fn keys_of(typed: &str) -> String {
    typed
        .chars()
        .map(|c| {
            if c == ' ' {
                "SPC".to_string()
            } else {
                c.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// A whole translation: this character.
    Char(char),
    /// The start of one or more.
    Prefix,
    /// Nothing starts this way.
    None,
}

fn language_map(language: Option<&str>) -> &'static [(&'static str, char)] {
    language
        .and_then(|l| languages().iter().find(|(n, _)| *n == l))
        .map_or(&[], |(_, m)| *m)
}

/// Look up what's been typed after C-x 8. A language's keys win over the
/// standard ones, as define-key over them does.
pub fn lookup(typed: &str, language: Option<&str>) -> Step {
    let lang = language_map(language);
    if let Some((_, c)) = lang.iter().chain(char_map()).find(|(k, _)| *k == typed) {
        return Step::Char(*c);
    }
    let longer = |(k, _): &&(&str, char)| k.len() > typed.len() && k.starts_with(typed);
    if lang.iter().chain(char_map()).any(|e| longer(&e)) {
        Step::Prefix
    } else {
        Step::None
    }
}

/// Every translation in force, as (string typed after C-x 8, character).
pub fn translations(language: Option<&str>) -> Vec<(&'static str, char)> {
    let lang = language_map(language);
    let mut v: Vec<(&str, char)> = lang.to_vec();
    v.extend(
        char_map()
            .iter()
            .filter(|(k, _)| !lang.iter().any(|(l, _)| l == k)),
    );
    v
}

/// The ways to type `c` with C-x 8 (shortest first).
pub fn ways_to_type(c: char, language: Option<&str>) -> Vec<String> {
    let mut v: Vec<&str> = translations(language)
        .into_iter()
        .filter(|(_, t)| *t == c)
        .map(|(k, _)| k)
        .collect();
    v.sort_by_key(|k| (k.len(), *k));
    v.into_iter()
        .map(|k| format!("C-x 8 {}", keys_of(k)))
        .collect()
}

// ------------------------------------------------------- the key filter

fn char_event(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn cx_event() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
}

/// Is the main keymap (not a prompt, a question or the menu) taking keys?
fn at_keymaps(app: &App) -> bool {
    app.minibuffer.is_none()
        && app.query.is_none()
        && app.key_reader.is_none()
        && !crate::menu::active(app)
}

/// Show where a translation read in the minibuffer has got to.
fn note(app: &mut App, text: String) {
    match app.minibuffer.as_mut() {
        Some(mb) => mb.note = Some(format!(" [{text}]")),
        None => app.msg(text),
    }
}

/// An error while a prompt is up shows in it over the
/// minibuffer for a moment.
fn fail(app: &mut App, text: String) {
    if app.minibuffer.is_some() {
        note(app, text);
        app.bell = true;
    } else {
        app.error(text);
    }
}

/// key-translation-map for C-x 8: called with each key before it
/// goes anywhere. Returns the key to handle: the same one, the character
/// a translation produced, or None when the key was taken (part of a
/// sequence, or an error).
pub fn filter(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if app.key_reader.is_some() || app.query.is_some() {
        return Some(key);
    }
    let desc = crate::keymap::describe_key(&key, false).unwrap_or_default();
    if app.iso.pending != Pending::None
        && let Some(mb) = app.minibuffer.as_mut()
    {
        // The note from the last key of the sequence is stale now.
        mb.note = None;
    }
    match std::mem::take(&mut app.iso.pending) {
        Pending::None => {}
        Pending::HeldCx => {
            if desc == "8" {
                app.iso.pending = Pending::Seq(Vec::new());
                note(app, "C-x 8-".into());
                return None;
            }
            // Not ours after all: C-x, then this key.
            app.route_key(cx_event());
            return Some(key);
        }
        Pending::Seq(mut seq) => {
            if desc == "C-g" {
                note(app, "Quit".into());
                return None;
            }
            if desc == "C-h" && typed(&seq).is_some_and(|t| t.is_empty()) {
                describe_prefix(app);
                return None;
            }
            seq.push(desc);
            let spelled = typed(&seq);
            match spelled.as_deref().map(|t| lookup(t, app.iso.language)) {
                Some(Step::Char(c)) => return Some(char_event(c)),
                Some(Step::Prefix) => {
                    note(app, format!("C-x 8 {}-", seq.join(" ")));
                    app.iso.pending = Pending::Seq(seq);
                }
                // C-x 8 RET and C-x 8 e read from the minibuffer, and this
                // is one already.
                _ if seq[0] == "RET" || seq[0] == "e" => {
                    fail(
                        app,
                        "Command attempted to use minibuffer while in minibuffer".into(),
                    );
                }
                _ => fail(app, format!("C-x 8 {} is undefined", seq.join(" "))),
            }
            return None;
        }
    }

    if app.minibuffer.is_some() {
        // The minibuffer and isearch read keys one at a time, so C-x is
        // held back to see whether 8 follows.
        if desc == "C-x" {
            app.iso.pending = Pending::HeldCx;
            return None;
        }
        return Some(key);
    }

    if !at_keymaps(app)
        || app.meta_pending
        || app.pending.len() < 2
        || app.pending[0] != "C-x"
        || app.pending[1] != "8"
    {
        return Some(key);
    }
    // In the buffer, C-x 8 is a prefix of the keymaps (C-x 8 RET, C-x 8 e
    // ...), so the sequence so far is in app.pending.
    if desc == "C-h" && app.pending.len() == 2 {
        app.pending.clear();
        app.message = None;
        describe_prefix(app);
        return None;
    }
    let mut seq = app.pending[2..].to_vec();
    seq.push(desc.clone());
    match typed(&seq).map(|t| lookup(&t, app.iso.language)) {
        Some(Step::Char(c)) => {
            app.pending.clear();
            app.message = None;
            Some(char_event(c))
        }
        Some(Step::Prefix) => {
            app.pending.push(desc);
            let arg = app.arg_echo();
            app.msg(format!("{arg}{}-", app.pending.join(" ")));
            None
        }
        // The keymaps say what it is: C-x 8 RET, C-x 8 e s, or undefined.
        _ => Some(key),
    }
}

/// What can follow `keys` (typed after C-x 8): (next key, the character
/// it types, or "+prefix"), for the prefix hints.
pub fn continuations(keys: &[String], language: Option<&str>) -> Vec<(String, String)> {
    let Some(so_far) = typed(keys) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, c) in translations(language) {
        let Some(rest) = k.strip_prefix(so_far.as_str()) else {
            continue;
        };
        let Some(next) = rest.chars().next() else {
            continue;
        };
        let key = keys_of(&next.to_string());
        let what = if rest.chars().count() == 1 {
            glyph(c)
        } else {
            "+prefix".into()
        };
        match out.iter_mut().find(|(o, _)| *o == key) {
            Some(slot) if what == "+prefix" => slot.1 = what,
            Some(_) => {}
            None => out.push((key, what)),
        }
    }
    out.sort();
    out
}

/// For C-h k: what a key sequence starting with C-x 8 comes to so far.
/// None when the keymaps decide (C-x 8 RET, C-x 8 e ...).
pub fn describe_keys(app: &App, keys: &[String]) -> Option<Step> {
    if keys.len() < 2 || keys[0] != "C-x" || keys[1] != "8" {
        return None;
    }
    if keys.len() == 2 {
        return Some(Step::Prefix);
    }
    match lookup(&typed(&keys[2..])?, app.iso.language) {
        Step::None => None,
        step => Some(step),
    }
}

/// C-x 8 C-h: the translations and the commands under C-x 8.
pub fn describe_prefix(app: &mut App) {
    let mut text = String::from(
        "Key translations starting with C-x 8:\n\
         (each types its character, wherever you type)\n\n\
         key             binding\n\
         ---             -------\n",
    );
    let mut all = translations(app.iso.language);
    all.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in all {
        let shown = if c.is_whitespace() || c.width() == Some(0) {
            format!("U+{:04X}", c as u32)
        } else {
            c.to_string()
        };
        let name = char_name(c).unwrap_or_default();
        text.push_str(&format!(
            "{:<16}{shown}  {name}\n",
            format!("C-x 8 {}", keys_of(k))
        ));
    }
    text.push_str("\nCommands under C-x 8:\n\n");
    for (k, cmd) in PREFIX_COMMANDS {
        text.push_str(&format!("{k:<16}{cmd}\n"));
    }
    app.show_special_buffer("*Help*", "help-mode", &text);
}

// ---------------------------------------------------------- characters

/// The Unicode name of a character.
pub fn char_name(c: char) -> Option<String> {
    unicode_names2::name(c).map(|n| n.to_string())
}

/// Every named character, by name: insert-char's completion table.
fn names() -> &'static [(String, char)] {
    static NAMES: OnceLock<Vec<(String, char)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        (0..=0x10FFFFu32)
            .filter_map(char::from_u32)
            .filter_map(|c| unicode_names2::name(c).map(|n| (n.to_string(), c)))
            .collect()
    })
}

/// How a character shows in an annotation: itself, on a dotted circle if
/// it combines, or nothing for spaces and controls.
fn glyph(c: char) -> String {
    if c.is_control() || c.is_whitespace() {
        String::new()
    } else if c.width() == Some(0) {
        format!("\u{25cc}{c}")
    } else {
        c.to_string()
    }
}

/// What insert-char was given: a name (any case; Unicode's loose matching
/// of spaces, hyphens and underscores) or a code point. Hexadecimal
/// digits are a code, as in read-char-by-name.
pub fn parse_char(input: &str) -> Option<char> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let code = |digits: &str, radix: u32| {
        u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
    };
    if let Some(rest) = s.strip_prefix('#') {
        let (radix, digits) = match rest.chars().next()? {
            'x' | 'X' => (16, &rest[1..]),
            'o' | 'O' => (8, &rest[1..]),
            'b' | 'B' => (2, &rest[1..]),
            _ => {
                let (r, d) = rest.split_once(['r', 'R'])?;
                (r.parse::<u32>().ok().filter(|r| (2..=36).contains(r))?, d)
            }
        };
        return code(digits, radix);
    }
    if s.chars().all(|c| c.is_ascii_hexdigit()) {
        return code(s, 16);
    }
    if let Some(hex) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return code(hex, 16);
    }
    unicode_names2::character(s)
}

// --------------------------------------------------------------- emoji

pub const GROUPS: &[(emojis::Group, &str)] = &[
    (emojis::Group::SmileysAndEmotion, "Smileys & Emotion"),
    (emojis::Group::PeopleAndBody, "People & Body"),
    (emojis::Group::AnimalsAndNature, "Animals & Nature"),
    (emojis::Group::FoodAndDrink, "Food & Drink"),
    (emojis::Group::TravelAndPlaces, "Travel & Places"),
    (emojis::Group::Activities, "Activities"),
    (emojis::Group::Objects, "Objects"),
    (emojis::Group::Symbols, "Symbols"),
    (emojis::Group::Flags, "Flags"),
];

fn group_name(g: emojis::Group) -> &'static str {
    GROUPS.iter().find(|(x, _)| *x == g).map_or("", |(_, n)| n)
}

/// Every emoji with its skin-tone variants, as (name, glyph).
fn all_emoji() -> &'static [(String, &'static str)] {
    static ALL: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = Vec::new();
        for e in emojis::iter() {
            v.push((e.name().to_string(), e.as_str()));
            if let Some(tones) = e.skin_tones() {
                for t in tones.skip(1) {
                    v.push((t.name().to_string(), t.as_str()));
                }
            }
        }
        v
    })
}

fn emoji_by_name(name: &str) -> Option<&'static str> {
    all_emoji()
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, g)| *g)
}

/// The emoji at `text`'s start, if one is: the longest that is one.
pub fn emoji_prefix(text: &str) -> Option<&'static emojis::Emoji> {
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take(16)
        .collect();
    ends.into_iter()
        .rev()
        .find_map(|end| emojis::get(&text[..end]))
}

fn describe_emoji(e: &emojis::Emoji) -> String {
    let codes: Vec<String> = e
        .as_str()
        .chars()
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    format!(
        "{}  {} ({}), {}",
        e.as_str(),
        e.name(),
        group_name(e.group()),
        codes.join(" ")
    )
}

fn insert_emoji(app: &mut App, glyph: &str) {
    if !insert_text(app, glyph, 1) {
        return;
    }
    let r = &mut app.iso.recent_emoji;
    r.retain(|g| g != glyph);
    r.insert(0, glyph.to_string());
    r.truncate(RECENT_EMOJI);
}

/// Insert `text` `count` times at point; false (with message) in
/// a read-only buffer.
fn insert_text(app: &mut App, text: &str, count: usize) -> bool {
    let b = app.buf();
    b.take_refused();
    b.insert_str(&text.repeat(count.max(1)));
    if app.buf().take_refused() {
        let name = app.buf_ref().name().to_string();
        app.error(format!("Buffer is read-only: {name}"));
        return false;
    }
    true
}

// ------------------------------------------------------------- prompts

/// What a C-x 8 prompt is asking for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    InsertChar { count: usize },
    EmojiSearch,
    EmojiGroup,
    EmojiInGroup(usize),
    EmojiRecent,
    EmojiDescribe,
    Language,
}

/// Rebuilds the list only when this changes.
pub fn list_key(ask: &Ask) -> String {
    match ask {
        Ask::InsertChar { .. } => "unicode-names".into(),
        Ask::EmojiInGroup(g) => format!("emoji-group-{g}"),
        other => format!("{other:?}"),
    }
}

pub fn category(ask: &Ask) -> &'static str {
    match ask {
        Ask::InsertChar { .. } => "unicode-name",
        Ask::EmojiGroup | Ask::Language => "choice",
        _ => "emoji",
    }
}

pub fn candidates(app: &App, ask: &Ask) -> Vec<String> {
    match ask {
        Ask::InsertChar { .. } => names().iter().map(|(n, _)| n.clone()).collect(),
        Ask::EmojiSearch | Ask::EmojiDescribe => {
            all_emoji().iter().map(|(n, _)| n.clone()).collect()
        }
        Ask::EmojiGroup => GROUPS.iter().map(|(_, n)| n.to_string()).collect(),
        Ask::EmojiInGroup(g) => GROUPS[*g]
            .0
            .emojis()
            .map(|e| e.name().to_string())
            .collect(),
        Ask::EmojiRecent => app
            .iso
            .recent_emoji
            .iter()
            .map(|g| emojis::get(g).map_or_else(|| g.clone(), |e| e.name().to_string()))
            .collect(),
        Ask::Language => languages().iter().map(|(n, _)| n.to_string()).collect(),
    }
}

/// The annotations of a list (the character, its code).
pub fn notes(category: &str, all: &[String]) -> Vec<String> {
    match category {
        "unicode-name" => {
            let note = |c: char| format!("{}  U+{:04X}", glyph(c), c as u32);
            let table = names();
            if all.len() == table.len() {
                table.iter().map(|(_, c)| note(*c)).collect()
            } else {
                all.iter()
                    .map(|n| unicode_names2::character(n).map_or_else(String::new, note))
                    .collect()
            }
        }
        "emoji" => all
            .iter()
            .map(|n| emoji_by_name(n).unwrap_or("").to_string())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn answered(app: &mut App, ask: Ask, input: &str) {
    let input = input.trim();
    match ask {
        Ask::InsertChar { count } => match parse_char(input) {
            Some(c) => {
                insert_text(app, &c.to_string(), count);
            }
            None if input.is_empty() => {}
            None => app.error(format!("Invalid character: {input}")),
        },
        Ask::EmojiSearch | Ask::EmojiInGroup(_) | Ask::EmojiRecent => {
            let glyph = emoji_by_name(input).or_else(|| emojis::get(input).map(|e| e.as_str()));
            match glyph {
                Some(g) => insert_emoji(app, g),
                None if input.is_empty() => {}
                None => app.error(format!("No emoji called {input}")),
            }
        }
        Ask::EmojiDescribe => {
            let e = emoji_by_name(input)
                .and_then(emojis::get)
                .or_else(|| emoji_prefix(input));
            match e {
                Some(e) => app.msg(describe_emoji(e)),
                None if input.is_empty() => {}
                None => app.error(format!("No emoji called {input}")),
            }
        }
        Ask::EmojiGroup => match GROUPS
            .iter()
            .position(|(_, n)| n.eq_ignore_ascii_case(input))
        {
            Some(g) => app.prompt(
                format!("Insert emoji ({}): ", GROUPS[g].1),
                Purpose::CharAsk(Ask::EmojiInGroup(g)),
            ),
            None if input.is_empty() => {}
            None => app.error(format!("No emoji group {input}")),
        },
        Ask::Language => match languages()
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(input))
        {
            Some((name, map)) => {
                app.iso.language = Some(name);
                let keys: Vec<String> = map
                    .iter()
                    .map(|(k, c)| format!("{} {c}", keys_of(k)))
                    .collect();
                app.msg(format!("C-x 8 for {name}: {}", keys.join(", ")));
            }
            None if input.is_empty() => {}
            None => app.error(format!("No C-x 8 keys for {input}")),
        },
    }
}

// ------------------------------------------------------------ commands

/// Run one of this module's commands; false if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "insert-char" => {
            let count = app.count();
            if count <= 0 {
                return true;
            }
            app.prompt(
                "Insert character (Unicode name or hex): ",
                Purpose::CharAsk(Ask::InsertChar {
                    count: count as usize,
                }),
            );
        }
        "what-cursor-position" => {
            if app.raw_arg() || app.has_arg() {
                describe_char(app);
            } else {
                let text = what_cursor_position(app.buf_ref());
                app.msg(text);
            }
        }
        "describe-char" => describe_char(app),
        "emoji-search" => app.prompt("Insert emoji: ", Purpose::CharAsk(Ask::EmojiSearch)),
        "emoji-insert" => app.prompt("Emoji group: ", Purpose::CharAsk(Ask::EmojiGroup)),
        "emoji-recent" => {
            if app.iso.recent_emoji.is_empty() {
                app.error("No recently used emoji");
            } else {
                app.prompt("Insert recent emoji: ", Purpose::CharAsk(Ask::EmojiRecent));
            }
        }
        "emoji-describe" => {
            let b = app.buf_ref();
            let p = b.point_offset();
            let ahead = b.text_offsets(p, (p + 16).min(b.char_count()));
            match emoji_prefix(&ahead) {
                Some(e) => {
                    let text = describe_emoji(e);
                    app.msg(text);
                }
                None => app.prompt("Describe emoji: ", Purpose::CharAsk(Ask::EmojiDescribe)),
            }
        }
        "emoji-list" => emoji_list(app),
        "emoji-list-select" => emoji_list_select(app),
        "iso-transl-set-language" => {
            app.prompt("Set which language? ", Purpose::CharAsk(Ask::Language));
        }
        _ => return false,
    }
    true
}

/// C-x =: "Char: é (233, #o351, #xe9, file ...) point=5 of 20 (20%)
/// column=3" puts it.
pub fn what_cursor_position(b: &forge_core::Buffer) -> String {
    let offset = b.point_offset();
    let total = b.char_count();
    let p = b.point();
    let column = forge_core::width::display_col(&b.line(p.line), p.col);
    let percent = (100 * offset + total / 2).checked_div(total).unwrap_or(0);
    let place = format!(
        "point={} of {total} ({percent}%) column={column}",
        offset + 1
    );
    match b.char_at(p) {
        None => place,
        Some(c) => {
            let n = c as u32;
            let shown = match c {
                '\n' => "C-j".to_string(),
                '\t' => "TAB".to_string(),
                ' ' => "SPC".to_string(),
                c if c.is_control() => format!("^{}", ((n as u8) ^ 0x40) as char),
                c => glyph(c),
            };
            // "file ..." says the character takes several bytes in
            // the file's encoding (UTF-8 here).
            let file = if n < 128 { "" } else { ", file ..." };
            format!("Char: {shown} ({n}, #o{n:o}, #x{n:x}{file}) {place}")
        }
    }
}

/// C-u C-x =: everything about the character after point, in *Help*.
fn describe_char(app: &mut App) {
    let b = app.buf_ref();
    let p = b.point();
    let Some(c) = b.char_at(p) else {
        return app.error("No character follows point");
    };
    let offset = b.point_offset();
    let total = b.char_count();
    let column = forge_core::width::display_col(&b.line(p.line), p.col);
    let n = c as u32;
    let percent = (100 * offset + total / 2).checked_div(total).unwrap_or(0);
    let name = char_name(c);
    let mut bytes = [0u8; 4];
    let utf8: Vec<String> = c
        .encode_utf8(&mut bytes)
        .bytes()
        .map(|x| format!("#x{x:02X}"))
        .collect();
    let mut rows = vec![
        (
            "position",
            format!("{} of {total} ({percent}%), column: {column}", offset + 1),
        ),
        (
            "character",
            format!(
                "{} (displayed as {}) (codepoint {n}, #o{n:o}, #x{n:x})",
                glyph(c),
                glyph(c)
            ),
        ),
        ("charset", "unicode (Unicode (ISO10646))".to_string()),
        ("code point in charset", format!("0x{n:04X}")),
        ("buffer code", utf8.join(" ")),
        (
            "file code",
            format!("{} (encoded by coding system utf-8-unix)", utf8.join(" ")),
        ),
    ];
    if let Some(name) = &name {
        rows.push(("name", name.clone()));
    }
    if let Some(e) = emojis::get(&c.to_string()) {
        rows.push(("emoji", format!("{} ({})", e.name(), group_name(e.group()))));
    }
    let mut ways = vec![format!("type \"C-x 8 RET {n:x}\"")];
    if let Some(name) = &name {
        ways.push(format!("or \"C-x 8 RET {name}\""));
    }
    for w in ways_to_type(c, app.iso.language) {
        ways.push(format!("or type \"{w}\""));
    }
    rows.push(("to input", ways.join("\n")));
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut text = String::new();
    for (k, v) in rows {
        let mut lines = v.lines();
        text.push_str(&format!("{k:>width$}: {}\n", lines.next().unwrap_or("")));
        for more in lines {
            text.push_str(&format!("{:width$}  {more}\n", ""));
        }
    }
    app.show_special_buffer("*Help*", "help-mode", &text);
}

/// Emoji per line in *Emoji*.
const PER_LINE: usize = 16;

fn emoji_list(app: &mut App) {
    let here = app.active_id();
    if app.buf_ref().major_mode() != EMOJI_LIST_MODE {
        app.iso.emoji_source = Some(here);
    }
    let mut text =
        String::from("RET (or a click) inserts the emoji at point; d describes it; q quits.\n");
    for (g, name) in GROUPS {
        text.push_str(&format!("\n{name}\n"));
        let glyphs: Vec<&str> = g.emojis().map(|e| e.as_str()).collect();
        for row in glyphs.chunks(PER_LINE) {
            text.push_str(&row.join(" "));
            text.push('\n');
        }
    }
    app.show_special_buffer("*Emoji*", EMOJI_LIST_MODE, &text);
    app.buf().set_point(Pos::new(3, 0));
}

/// The emoji around point in *Emoji* (or at it, in any buffer).
fn emoji_at_point(app: &App) -> Option<&'static emojis::Emoji> {
    let b = app.buf_ref();
    let p = b.point();
    let line: Vec<char> = b.line(p.line).chars().collect();
    // The space-separated token point is in, or just after.
    let mut start = p.col.min(line.len());
    if start == line.len() || line.get(start) == Some(&' ') {
        start = start.saturating_sub(1);
    }
    while start > 0 && line[start - 1] != ' ' {
        start -= 1;
    }
    let token: String = line[start..].iter().take_while(|c| **c != ' ').collect();
    emojis::get(&token).or_else(|| emoji_prefix(&token))
}

fn emoji_list_select(app: &mut App) {
    let Some(e) = emoji_at_point(app) else {
        return app.error("No emoji here");
    };
    let target = app
        .iso
        .emoji_source
        .filter(|id| app.editor.state.buffers.contains_key(id));
    let Some(target) = target else {
        return app.error("The buffer emoji-list was called from is gone");
    };
    app.switch_to(target);
    insert_emoji(app, e.as_str());
}

/// Bind C-x 8 RET, C-x 8 e ..., C-x =, and *Emoji*'s keys.
pub fn install_keymaps(keymaps: &mut forge_core::keymap::Keymaps) {
    use forge_core::keymap::Origin;
    for (keys, cmd) in PREFIX_COMMANDS.iter().chain(BINDINGS) {
        keymaps
            .bind(&Scope::Global, keys, cmd, Origin::Package)
            .expect("valid C-x 8 binding");
    }
    let mode = Scope::Mode(EMOJI_LIST_MODE.into());
    for (keys, cmd) in EMOJI_LIST_BINDINGS {
        keymaps
            .bind(&mode, keys, cmd, Origin::Package)
            .expect("valid emoji-list binding");
    }
    keymaps.set_mode_parent(EMOJI_LIST_MODE, "special-mode");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_composes_accents() {
        assert_eq!(lookup("'e", None), Step::Char('é'));
        assert_eq!(lookup("\"u", None), Step::Char('ü'));
        assert_eq!(lookup("1/2", None), Step::Char('½'));
        assert_eq!(lookup("a>", None), Step::Char('→'));
        assert_eq!(lookup("_m", None), Step::Char('—'));
        assert_eq!(lookup("*E", None), Step::Char('€'));
        assert_eq!(lookup(" ", None), Step::Char('\u{a0}'));
        assert_eq!(lookup("' ", None), Step::Char('\''));
        assert_eq!(lookup("vc", None), Step::Char('č'));
        assert_eq!(lookup(")a", None), Step::Char('ă'));
        assert_eq!(lookup("'", None), Step::Prefix);
        assert_eq!(lookup("1", None), Step::Prefix);
        assert_eq!(lookup("1/", None), Step::Prefix);
        assert_eq!(lookup("'q", None), Step::None);
        assert_eq!(
            lookup("e", None),
            Step::None,
            "C-x 8 e is the emoji commands"
        );
        // No translation is a prefix of another: define-key would
        // refuse the table otherwise.
        for (a, _) in char_map() {
            for (b, _) in char_map() {
                assert!(a == b || !b.starts_with(a), "{a:?} is a prefix of {b:?}");
            }
        }
        assert!(char_map().len() > 250);
    }

    #[test]
    fn languages_add_shorter_keys() {
        assert_eq!(lookup("u", None), Step::Char('μ'));
        assert_eq!(lookup("u", Some("German")), Step::Char('ü'));
        assert_eq!(lookup("C", Some("French")), Step::Char('Ç'));
        assert_eq!(lookup("N", Some("Spanish")), Step::Char('Ñ'));
        assert_eq!(
            lookup("'e", Some("German")),
            Step::Char('é'),
            "the rest stay"
        );
        assert_eq!(ways_to_type('ü', Some("German"))[0], "C-x 8 u");
        assert_eq!(ways_to_type('é', None), ["C-x 8 ' e"]);
        assert_eq!(ways_to_type('\u{a0}', None), ["C-x 8 SPC"]);
    }

    #[test]
    fn keys_spell_strings() {
        assert_eq!(key_char("SPC"), Some(' '));
        assert_eq!(key_char("S-a"), Some('A'));
        assert_eq!(key_char("\""), Some('"'));
        assert_eq!(key_char("RET"), None);
        assert_eq!(key_char("C-a"), None);
        assert_eq!(
            typed(&["^".into(), "^".into(), "S-c".into()]).as_deref(),
            Some("^^C")
        );
    }

    #[test]
    fn insert_char_reads_names_and_codes() {
        assert_eq!(parse_char("LATIN SMALL LETTER E WITH ACUTE"), Some('é'));
        assert_eq!(parse_char("latin small letter e with acute"), Some('é'));
        assert_eq!(parse_char("e9"), Some('é'));
        assert_eq!(parse_char("#xe9"), Some('é'));
        assert_eq!(parse_char("#o351"), Some('é'));
        assert_eq!(parse_char("#10r233"), Some('é'));
        assert_eq!(parse_char("U+1F600"), Some('😀'));
        assert_eq!(parse_char("SNOWMAN"), Some('☃'));
        assert_eq!(parse_char("nonsense name"), None);
        assert_eq!(parse_char("d800"), None, "a surrogate is no character");
        assert_eq!(parse_char(""), None);
    }

    #[test]
    fn names_and_notes() {
        assert_eq!(
            char_name('é').as_deref(),
            Some("LATIN SMALL LETTER E WITH ACUTE")
        );
        let all = [
            "LATIN SMALL LETTER E WITH ACUTE".to_string(),
            "COMBINING ACUTE ACCENT".to_string(),
        ];
        let n = notes("unicode-name", &all);
        assert_eq!(n[0], "é  U+00E9");
        assert_eq!(n[1], "\u{25cc}\u{301}  U+0301");
        assert!(names().len() > 30_000);
    }

    #[test]
    fn emoji() {
        assert_eq!(emoji_by_name("grinning face"), Some("😀"));
        assert_eq!(
            emoji_prefix("😀 hi").map(|e| e.name()),
            Some("grinning face")
        );
        assert_eq!(emoji_prefix("hi"), None);
        let wave = emoji_prefix("👋🏽").unwrap();
        assert!(wave.name().contains("waving hand"), "{}", wave.name());
        assert!(
            describe_emoji(emojis::get("😀").unwrap())
                .starts_with("😀  grinning face (Smileys & Emotion), U+1F600")
        );
        assert!(
            all_emoji().iter().any(|(n, _)| n.contains("skin tone")),
            "variants are in the list"
        );
    }

    #[test]
    fn what_cursor_position_says_what_it_does() {
        let mut b = forge_core::Buffer::new("x");
        b.set_contents("aé\n");
        assert_eq!(
            what_cursor_position(&b),
            "Char: a (97, #o141, #x61) point=1 of 3 (0%) column=0"
        );
        b.set_point(Pos::new(0, 1));
        assert_eq!(
            what_cursor_position(&b),
            "Char: é (233, #o351, #xe9, file ...) point=2 of 3 (33%) column=1"
        );
        b.set_point(Pos::new(1, 0));
        assert_eq!(what_cursor_position(&b), "point=4 of 3 (100%) column=0");
    }
}
