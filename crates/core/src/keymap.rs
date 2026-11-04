// SPDX-License-Identifier: MIT OR Apache-2.0
//! Layered, traditional keymaps.
//!
//! Keys are written them ("C-x", "M-f", "C-M-/",
//! "RET", "`<left>`") and bound as space-separated sequences ("C-x C-s").
//! Any sequence that starts a longer binding is a prefix key.
//!
//! When a key is pressed, the active maps are searched in priority order.
//! The order comes from `PRIORITY_*` rather than from the order the layers
//! happen to be collected in, so a layer can be given a number instead of
//! needing a new branch in [`Keymaps::active`]. Higher wins; the defaults
//! are:
//!
//! 1. transient maps
//! 2. the prompt map
//! 3. the region map (the keymap of the text point is in)
//! 4. the buffer-local map (`local-set-key`)
//! 5. a conditional layer
//! 6. minor-mode maps, most recently enabled first
//! 7. the project map (the keymap of the directory the file is in)
//! 8. the major-mode map, then the maps of the modes it derives from
//! 9. the global map
//!
//! The sort is stable, so maps that share a priority keep being searched in
//! the order above (most recent minor mode before the major mode, major
//! mode before the mode it derives from).
//!
//! The first map that binds the sequence (to a command *or* as a prefix)
//! decides. After that, command remapping (`[remap kill-line]`) is applied
//! once, again searching the maps in priority order.
//!
//! By convention `C-c <letter>` belongs to the user: bindings made
//! with [`Origin::Package`] (built-in modes, plugins) may not use it.

use crate::app::BufferId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Binding a key to this command makes it undefined in that map, and
/// shadows bindings for the same key in lower-priority maps.
pub const UNDEFINED: &str = "undefined";

// ---------------------------------------------------------- layer priority

/// Where a layer sits relative to the others when a key is looked up.
/// Higher wins. The gaps are deliberate: a plugin that wants its modal map
/// above the major mode but below a minor mode picks a number in between
/// rather than renumbering anything.
pub const PRIORITY_TRANSIENT: i32 = 1000;
pub const PRIORITY_PROMPT: i32 = 900;
pub const PRIORITY_REGION: i32 = 600;
pub const PRIORITY_BUFFER_LOCAL: i32 = 500;
pub const PRIORITY_CONDITIONAL: i32 = 450;
pub const PRIORITY_MINOR_MODE: i32 = 400;
pub const PRIORITY_PROJECT: i32 = 350;
pub const PRIORITY_MAJOR_MODE: i32 = 300;
pub const PRIORITY_GLOBAL: i32 = 100;

/// The prompt map every prompt falls back to. A prompt named after its
/// purpose ("find-file", "dired") with no bindings of its own uses this one,
/// so naming a prompt costs nothing until something rebinds it.
pub const PROMPT_DEFAULT: &str = "prompt";

/// The priority a scope gets when nothing has overridden it. Modes are
/// resolved further up ([`Keymaps::active`]) because the same map can be a
/// minor mode in one buffer and the major mode in another.
fn default_priority(scope: &Scope) -> i32 {
    match scope {
        Scope::Global => PRIORITY_GLOBAL,
        Scope::Buffer(_) => PRIORITY_BUFFER_LOCAL,
        Scope::Mode(_) => PRIORITY_MINOR_MODE,
        Scope::Transient(_) => PRIORITY_TRANSIENT,
        Scope::Prompt(_) => PRIORITY_PROMPT,
        Scope::Layer(_) => PRIORITY_CONDITIONAL,
        Scope::Region(_) => PRIORITY_REGION,
        Scope::Project(_) => PRIORITY_PROJECT,
    }
}

/// What happens to a live transient map when a key resolves elsewhere, or
/// when it has been up for longer than it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUnbound {
    /// Leave the transient in place; the key carries on to the layers below.
    Stay,
    /// Leave it in place and swallow the key, so the layers below never see
    /// a key that was meant for the map that is up.
    Swallow,
    /// Pop the transient, then let the key resolve again in what's left.
    Exit,
}

/// A transient map that is installed right now, and what it does when a key
/// doesn't reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTransient {
    pub name: String,
    /// A map that ends after the first key it resolves.
    pub one_shot: bool,
    pub on_unbound: OnUnbound,
    /// Pop after this long, if it is still up. `None` means it stays until
    /// something pops it.
    pub timeout: Option<Duration>,
    /// When it was pushed, for [`LiveTransient::timeout`].
    pub pushed_at: Instant,
}

impl LiveTransient {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            one_shot: false,
            on_unbound: OnUnbound::Exit,
            timeout: None,
            pushed_at: Instant::now(),
        }
    }

    /// Has this been up for longer than its timeout?
    pub fn expired(&self) -> bool {
        self.timeout
            .is_some_and(|limit| self.pushed_at.elapsed() >= limit)
    }
}

/// A transient map and how long it should stay up. [`LiveTransient::new`]
/// gives the defaults (`Exit`, not one-shot, no timeout).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransientOpts {
    pub one_shot: bool,
    pub on_unbound: Option<OnUnbound>,
    /// Seconds; `None` (or 0) means the map stays until it is popped.
    pub timeout: Option<f64>,
}

impl TransientOpts {
    pub fn one_shot(mut self, yes: bool) -> Self {
        self.one_shot = yes;
        self
    }

    pub fn on_unbound(mut self, policy: OnUnbound) -> Self {
        self.on_unbound = Some(policy);
        self
    }

    pub fn timeout(mut self, seconds: f64) -> Self {
        self.timeout = Some(seconds);
        self
    }

    pub fn to_live(self, name: String) -> LiveTransient {
        let mut live = LiveTransient::new(name);
        live.one_shot = self.one_shot;
        if let Some(policy) = self.on_unbound {
            live.on_unbound = policy;
        }
        live.timeout = self
            .timeout
            .filter(|s| *s > 0.0)
            .map(Duration::from_secs_f64);
        live
    }
}

/// Which map a binding goes into.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    /// The keymap of a major or minor mode, by mode name ("help-mode").
    Mode(String),
    /// Bindings that apply only in one buffer.
    Buffer(BufferId),
    /// A map installed on top of everything else for as long as it is live
    ///: ["meow-keypad"], ["repeat"].
    Transient(String),
    /// A map that is in charge while something is asking a question, named
    /// by purpose: ["find-file"], ["dired"], ["m-x"], ["completion-popup"].
    Prompt(String),
    /// A map that only applies while a [`Condition`] holds, by layer name
    /// ("when-region", "when-read-only"). Its condition lives in the layer,
    /// not here: see [`Keymaps::add_layer`].
    Layer(String),
    /// The keymap a stretch of text brings with it, by name ("dired-entry",
    /// "magit-section"). It is up while point is inside a decoration that
    /// names it, the way text properties are, and sits at
    /// [`PRIORITY_REGION`].
    Region(String),
    /// A project's own keymap, by its root directory. It is up for every
    /// file inside that directory, the way `.dir-locals.el` bindings are,
    /// and sits at [`PRIORITY_PROJECT`].
    Project(std::path::PathBuf),
}

impl Scope {
    pub fn label(&self) -> String {
        match self {
            Scope::Global => "global-map".to_string(),
            Scope::Mode(m) => format!("{m}-map"),
            Scope::Buffer(_) => "buffer-local map".to_string(),
            Scope::Transient(name) => format!("transient map ({name})"),
            Scope::Prompt(name) => format!("prompt map ({name})"),
            Scope::Layer(name) => format!("layer map ({name})"),
            Scope::Region(name) => format!("region map ({name})"),
            Scope::Project(root) => format!("project map ({})", root.display()),
        }
    }
}

/// When a layer's bindings apply. Built-in conditions only: they are
/// answered from a snapshot taken once per keypress, so nothing here costs
/// a walk or a parse per key (see [`Context`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// A region is selected.
    RegionActive,
    /// The buffer is marked read-only.
    ReadOnly,
    /// The character before point is inside a comment.
    InComment,
    /// Point is within a string literal.
    InString,
    /// The buffer's file is in a directory with a `.git`.
    InGitRepo,
    /// The major mode is `mode`, or derives from it.
    MajorDerivesFrom(String),
    /// The minor mode is on in this buffer.
    MinorModeOn(String),
    Not(Box<Condition>),
    All(Vec<Condition>),
    Any(Vec<Condition>),
}

impl Condition {
    pub fn label(&self) -> String {
        match self {
            Condition::RegionActive => "region-active".into(),
            Condition::ReadOnly => "read-only".into(),
            Condition::InComment => "in-comment".into(),
            Condition::InString => "in-string".into(),
            Condition::InGitRepo => "in-git-repo".into(),
            Condition::MajorDerivesFrom(m) => format!("major-derives-from({m})"),
            Condition::MinorModeOn(m) => format!("minor-mode-on({m})"),
            Condition::Not(c) => format!("not({})", c.label()),
            Condition::All(cs) => format!("all({})", join(cs)),
            Condition::Any(cs) => format!("any({})", join(cs)),
        }
    }

    /// Every condition in the tree, outer first, so a caller looking for one
    /// kind of condition finds it however deeply it is nested.
    pub fn walk(&self, f: &mut impl FnMut(&Condition)) {
        f(self);
        match self {
            Condition::Not(c) => c.walk(f),
            Condition::All(cs) | Condition::Any(cs) => cs.iter().for_each(|c| c.walk(f)),
            _ => {}
        }
    }

    /// Read a condition back from the notation [`Condition::label`] prints,
    /// so a plugin can be handed the same text the host shows.
    ///
    /// The WIT carries conditions as strings rather than as a recursive
    /// variant: a type that contains itself needs a `box<>`, which the
    /// plugin toolchain here can't read. Keys travel the same way, as
    /// "C-x C-s", so this is the same kind of notation as everywhere else.
    pub fn parse(s: &str) -> Result<Condition, String> {
        let s = s.trim();
        let (head, args) = match s.find('(') {
            Some(i) if s.ends_with(')') => (&s[..i], &s[i + 1..s.len() - 1]),
            _ if s.contains('(') => return Err(format!("unbalanced parentheses in {s:?}")),
            _ => (s, ""),
        };
        let bad = |what: &str| Err(format!("unknown condition {what:?} in {s:?}"));
        let no_args = |c: Condition| {
            if args.trim().is_empty() {
                Ok(c)
            } else {
                Err(format!("{head} takes no arguments: {s:?}"))
            }
        };
        let one_name = |f: fn(String) -> Condition| -> Result<Condition, String> {
            let name = args.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(format!("{head} takes one mode name: {s:?}"));
            }
            Ok(f(name.to_string()))
        };
        match head {
            "region-active" => no_args(Condition::RegionActive),
            "read-only" => no_args(Condition::ReadOnly),
            "in-comment" => no_args(Condition::InComment),
            "in-string" => no_args(Condition::InString),
            "in-git-repo" => no_args(Condition::InGitRepo),
            "major-derives-from" => one_name(Condition::MajorDerivesFrom),
            "minor-mode-on" => one_name(Condition::MinorModeOn),
            "not" => Ok(Condition::Not(Box::new(Condition::parse(args)?))),
            "all" | "any" => {
                let cs: Result<Vec<Condition>, String> =
                    args.split_whitespace().map(Condition::parse).collect();
                Ok(if head == "all" {
                    Condition::All(cs?)
                } else {
                    Condition::Any(cs?)
                })
            }
            _ => bad(head),
        }
    }
}

fn join(cs: &[Condition]) -> String {
    cs.iter()
        .map(Condition::label)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The facts conditions are answered from, gathered once per keypress.
///
/// [`Syntax::faces`](crate::syntax::Syntax::faces) needs `&mut self`, and the
/// git check walks the filesystem, so neither can be asked from inside
/// [`Keymaps::active`], which is `&self`. `dispatch_key` fills this in and
/// the lookup reads it, so a condition costs a field read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub region_active: bool,
    pub read_only: bool,
    pub in_git_repo: bool,
    /// The syntax face of the character before point, if the buffer has a
    /// grammar and there is a character there.
    pub face_at_point: Option<crate::syntax::SyntaxFace>,
    /// The keymaps of the decorations point is inside, widest first. See
    /// [`Scope::Region`].
    pub region_maps: Vec<String>,
    /// The root of the project the active buffer is in, if it has bindings
    /// of its own. See [`Scope::Project`].
    pub project: Option<PathBuf>,
}

impl Context {
    /// The conditions that need nothing but this snapshot and the modes.
    fn holds(&self, c: &Condition) -> bool {
        match c {
            Condition::RegionActive => self.region_active,
            Condition::ReadOnly => self.read_only,
            Condition::InComment => self.face_at_point == Some(crate::syntax::SyntaxFace::Comment),
            // An escape counts as inside the string it belongs to, which is
            // where a cursor usually sits in one.
            Condition::InString => matches!(
                self.face_at_point,
                Some(crate::syntax::SyntaxFace::String | crate::syntax::SyntaxFace::Escape)
            ),
            Condition::InGitRepo => self.in_git_repo,
            Condition::MajorDerivesFrom(_) | Condition::MinorModeOn(_) => false,
            Condition::Not(c) => !self.holds(c),
            Condition::All(cs) => cs.iter().all(|c| self.holds(c)),
            Condition::Any(cs) => cs.iter().any(|c| self.holds(c)),
        }
    }
}

/// A conditional layer: bindings that only apply while `when` holds.
#[derive(Debug, Default, Clone)]
pub struct Layer {
    /// None until the layer's condition is set. A layer with no condition
    /// is never active, so binding into a layer that doesn't exist can't
    /// shadow anything by accident.
    pub when: Option<Condition>,
    pub map: Keymap,
}

/// Who is making a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The user: init.rhai, `global-set-key`, `local-set-key`. Anything goes.
    User,
    /// Built-in modes and (later) plugins. May not take `C-c <letter>`.
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapError {
    /// Not valid key notation.
    BadKey(String),
    /// `C-c <letter>` is reserved for the user.
    Reserved(String),
    /// Binding "C-x C-s x" when "C-x C-s" already runs a command.
    NonPrefix { keys: String, prefix: String },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadKey(msg) => write!(f, "{msg}"),
            Self::Reserved(keys) => {
                write!(
                    f,
                    "{keys}: C-c followed by a letter is reserved for user bindings"
                )
            }
            Self::NonPrefix { keys, prefix } => {
                write!(f, "Key sequence {keys} starts with non-prefix key {prefix}")
            }
        }
    }
}

impl std::error::Error for KeymapError {}

// ------------------------------------------------------------ key notation

const NAMED_KEYS: &[&str] = &["SPC", "RET", "TAB", "DEL", "ESC"];
const BRACKETED_KEYS: &[&str] = &[
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "prior",
    "next",
    "insert",
    "deletechar",
    "backtab",
    // Mouse buttons, so a click can be bound like a key: a region map
    // (see `Scope::Region`) is what puts one where it means something.
    "mouse-1",
    "mouse-2",
    "mouse-3",
];

/// Parse and normalize a key sequence: "C-x  C-s" -> ["C-x", "C-s"].
///
/// Normalization matches what the terminal reports, so bindings written
/// by hand always match real key presses: modifiers come out as `C-M-`,
/// Control+letter is lower case ("C-X" is "C-x"), and "C-@" is "C-SPC".
pub fn parse_keys(keys: &str) -> Result<Vec<String>, KeymapError> {
    let seq: Vec<String> = keys
        .split_whitespace()
        .map(normalize_key)
        .collect::<Result<_, _>>()?;
    if seq.is_empty() {
        return Err(KeymapError::BadKey("Empty key sequence".into()));
    }
    Ok(seq)
}

/// A modifier a key may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Shift,
    Ctrl,
    /// Option/Alt: `M-`, and what a terminal sends for it.
    Alt,
    /// The terminal's own Meta (kitty's meta bit, usually Command on
    /// macOS), kept apart from `M-` so both can be bound.
    Meta,
    /// The Super key: Win, or Command on macOS.
    Super,
    Hyper,
}

/// Every modifier Forge understands: the spellings it accepts, and what it
/// is. Entries whose spelling starts with another (`s-super-` and `s-m-`
/// against `s-`) come first, so the longer one is read.
const MODIFIERS: &[(&[&str], &str, Modifier)] = &[
    (&["C-", "Control-", "Ctrl-"], "C-", Modifier::Ctrl),
    (&["M-", "Meta-", "A-", "Alt-"], "M-", Modifier::Alt),
    (
        &["s-super-", "super-", "Cmd-", "Command-"],
        "s-super-",
        Modifier::Super,
    ),
    (&["s-m-"], "s-m-", Modifier::Meta),
    (&["H-", "Hyper-"], "H-", Modifier::Hyper),
    (&["S-", "s-", "Shift-", "shift-"], "S-", Modifier::Shift),
];

/// The modifier names, in the order a key is written back in.
const MODIFIER_ORDER: &[(Modifier, &str)] = &[
    (Modifier::Ctrl, "C-"),
    (Modifier::Alt, "M-"),
    (Modifier::Shift, "S-"),
    (Modifier::Hyper, "H-"),
    (Modifier::Meta, "s-m-"),
    (Modifier::Super, "s-super-"),
];

/// What Shift makes of a key that isn't a letter, on a US layout: the
/// shifted symbol, so `S-2` is `@` and both spellings mean the same key.
///
/// A terminal with the kitty keyboard protocol reports the key *under* the
/// finger and Shift as a modifier; one without folds Shift into the
/// character and sends `@`. Fold it back the same way here, or the two
/// terminals would disagree about every shifted symbol.
pub fn shifted(base: char) -> Option<char> {
    Some(match base {
        // The digits, under the symbols above them: ")!@#$%^&*(".
        '0'..='9' => ")!@#$%^&*(".chars().nth(base as usize - '0' as usize)?,
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        _ => return None,
    })
}

/// The modifier `key` starts with: the spelling that matched, so a long
/// form ("Control-x") is cut by its own length, and what it means.
fn spelling(key: &str) -> Option<(&'static str, Modifier)> {
    MODIFIERS.iter().find_map(|(names, _, m)| {
        names
            .iter()
            .find(|n| key.starts_with(**n))
            .map(|n| (*n, *m))
    })
}

fn normalize_key(key: &str) -> Result<String, KeymapError> {
    let bad = |why: &str| KeymapError::BadKey(format!("Invalid key \"{key}\": {why}"));
    let mut mods: Vec<Modifier> = Vec::new();
    let mut rest = key;
    // "C--" is Control+minus, so only strip a modifier when something follows it.
    loop {
        if rest.len() <= 2 {
            break;
        }
        // The spelling that matched, so a long form ("Control-x") is cut
        // by its own length and not by the first one in the table.
        let Some((matched, m)) = spelling(rest) else {
            // Something that looks like a modifier but isn't one: say so,
            // rather than silently reading it as a key of its own.
            if rest.as_bytes().get(1) == Some(&b'-') && rest.starts_with(char::is_uppercase) {
                return Err(bad(
                    "the modifiers are C-, M- (A-), S-, H-, s-m- and s-super- (Cmd-)",
                ));
            }
            break;
        };
        if mods.contains(&m) {
            return Err(bad("the same modifier twice"));
        }
        mods.push(m);
        rest = &rest[matched.len()..];
    }

    let mut base = if rest.chars().count() == 1 || NAMED_KEYS.contains(&rest) {
        rest.to_string()
    } else if let Some(name) = rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        let is_fkey = name
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=24).contains(&n));
        if !is_fkey && !BRACKETED_KEYS.contains(&name) {
            return Err(bad("unknown key name"));
        }
        rest.to_string()
    } else {
        return Err(bad(
            "expected one character, a name like RET or SPC, or <name>",
        ));
    };

    let has = |m| mods.contains(&m);
    if has(Modifier::Ctrl) {
        match base.as_str() {
            "@" | " " => base = "SPC".into(),
            b if b.len() == 1 && b.as_bytes()[0].is_ascii_alphabetic() => {
                base = b.to_ascii_lowercase()
            }
            _ => {}
        }
    } else if base == " " {
        base = "SPC".into();
    }
    // Shift is written on a letter in lower case it,
    // and the same whichever terminal sent it: a terminal with the kitty
    // keyboard protocol reports Shift+a as 'a' with the Shift bit, and one
    // without as 'A' with it. On a symbol it is folded into the character
    // instead -- "S-2" is "@" -- since that character is the one to insert
    // and a terminal without the protocol sends it and nothing else. Named
    // keys keep the prefix ("S-<left>"); there is no character to fold.
    if has(Modifier::Shift) && base.len() == 1 {
        let c = base.as_bytes()[0];
        if c.is_ascii_alphabetic() {
            base = base.to_ascii_lowercase();
        } else if let Some(symbol) = base.chars().next().and_then(shifted) {
            base = symbol.to_string();
            mods.retain(|m| *m != Modifier::Shift);
        }
    }

    let mut out = String::new();
    for (m, name) in MODIFIER_ORDER {
        if mods.contains(m) {
            out.push_str(name);
        }
    }
    out.push_str(&base);
    Ok(out)
}

/// The two names one key is written as: TAB is C-i, RET is C-m. A
/// terminal that can't tell them apart sends the same byte for both, so a
/// binding in either name has to answer for the other.
///
/// One spelling, not every combination: a sequence with both TAB and C-i
/// in it has one alternative, which is the one nobody writes.
///
/// So are Shift+a and A: bindings are often written with the capital
/// letter, and a keypress of Shift with a
/// letter arrives as `S-a` (see `normalize_key`), whichever way the
/// terminal reported it.
pub fn alias_keys(seq: &[String]) -> Option<Vec<String>> {
    let swap = |k: &str| -> Option<String> {
        match k {
            "TAB" => return Some("C-i".into()),
            "C-i" => return Some("TAB".into()),
            "RET" => return Some("C-m".into()),
            "C-m" => return Some("RET".into()),
            _ => {}
        }
        let mut cs = k.chars();
        if let (Some(c), None) = (cs.next(), cs.next())
            && c.is_ascii_uppercase()
        {
            return Some(format!("S-{}", c.to_ascii_lowercase()));
        }
        k.strip_prefix("S-")
            .filter(|l| l.len() == 1 && l.as_bytes()[0].is_ascii_lowercase())
            .map(|l| l.to_ascii_uppercase())
    };
    if !seq.iter().any(|k| swap(k).is_some()) {
        return None;
    }
    Some(
        seq.iter()
            .map(|k| swap(k).unwrap_or_else(|| k.clone()))
            .collect(),
    )
}

/// True for `C-c` followed by a plain letter: reserved for the user.
/// True for `C-c` followed by a plain letter: reserved for the user.
pub fn is_user_reserved(seq: &[String]) -> bool {
    seq.first().is_some_and(|k| k == "C-c")
        && seq
            .get(1)
            .is_some_and(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_alphabetic())
}

/// Sort order for listing keys: plain keys, then one modifier, then more;
/// named keys after printable ones. The number is how many modifiers the
/// key has, so a plain key comes before any decorated one.
pub fn key_sort_key(key: &str) -> (u8, bool, String) {
    let mut rest = key;
    let mut mods = 0;
    // In MODIFIER_ORDER, so C- counts before M- and every spelling of a
    // modifier counts once.
    while let Some((_, name)) = MODIFIER_ORDER
        .iter()
        .find(|(_, name)| rest.starts_with(name))
    {
        mods += 1;
        rest = &rest[name.len()..];
    }
    (mods, rest.chars().count() > 1, rest.to_string())
}

// ------------------------------------------------------------------ Keymap

/// One keymap: key sequences to command names, plus command remappings.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: BTreeMap<String, String>,
    remaps: BTreeMap<String, String>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// The command bound to exactly this sequence ("C-x C-s").
    pub fn get(&self, keys: &str) -> Option<&str> {
        self.bindings.get(keys).map(String::as_str)
    }

    /// True if some longer binding starts with `keys`.
    pub fn is_prefix(&self, keys: &str) -> bool {
        let p = format!("{keys} ");
        self.bindings
            .range(p.clone()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with(&p))
    }

    pub fn remap_of(&self, command: &str) -> Option<&str> {
        self.remaps.get(command).map(String::as_str)
    }

    /// All bindings, sorted by key sequence.
    pub fn bindings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.bindings.iter().map(|(k, c)| (k.as_str(), c.as_str()))
    }

    /// Every command this map names, as a binding or on either side of a
    /// remapping.
    pub fn bound_commands(&self) -> impl Iterator<Item = &str> {
        self.bindings()
            .map(|(_, command)| command)
            .chain(self.remaps().flat_map(|(from, to)| [from, to]))
    }

    /// All remappings (from, to).
    pub fn remaps(&self) -> impl Iterator<Item = (&str, &str)> {
        self.remaps.iter().map(|(k, c)| (k.as_str(), c.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.remaps.is_empty()
    }

    /// Bind a normalized sequence. Binding a prefix replaces everything
    /// under it.
    fn bind(&mut self, seq: &[String], command: &str) -> Result<(), KeymapError> {
        for i in 1..seq.len() {
            let prefix = seq[..i].join(" ");
            if self.bindings.contains_key(&prefix) {
                return Err(KeymapError::NonPrefix {
                    keys: seq.join(" "),
                    prefix,
                });
            }
        }
        let keys = seq.join(" ");
        let under = format!("{keys} ");
        self.bindings.retain(|k, _| !k.starts_with(&under));
        self.bindings.insert(keys, command.to_string());
        Ok(())
    }

    fn unbind(&mut self, seq: &[String]) -> bool {
        self.bindings.remove(&seq.join(" ")).is_some()
    }
}

// ----------------------------------------------------------------- Keymaps

/// Every keymap the editor knows about.
#[derive(Debug, Default, Clone)]
pub struct Keymaps {
    pub global: Keymap,
    modes: HashMap<String, Keymap>,
    /// Derived modes: "help-mode" -> "special-mode". The parent's keymap is
    /// searched after the child's.
    parents: HashMap<String, String>,
    buffers: HashMap<BufferId, Keymap>,
    /// Transient maps, by name. How long each is up is the caller's
    /// business ([`Keymaps::active`] takes the live ones).
    transients: HashMap<String, Keymap>,
    /// Prompt maps, by purpose name. See [`Scope::Prompt`].
    prompts: HashMap<String, Keymap>,
    /// Conditional layers, by name. See [`Scope::Layer`].
    layers: HashMap<String, Layer>,
    /// Keymaps a stretch of text brings with it, by name. Which ones are
    /// up is the caller's business: [`Keymaps::active`] takes the ones the
    /// decorations under point name.
    regions: HashMap<String, Keymap>,
    /// A project's keymap, by its root directory. Which are up is again
    /// the caller's: [`Keymaps::active`] takes the one the active buffer
    /// is inside.
    projects: HashMap<PathBuf, Keymap>,
    /// Layers whose priority isn't the default for their scope.
    priorities: HashMap<Scope, i32>,
}

impl Keymaps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, scope: &Scope) -> Option<&Keymap> {
        match scope {
            Scope::Global => Some(&self.global),
            Scope::Mode(m) => self.modes.get(m),
            Scope::Buffer(id) => self.buffers.get(id),
            Scope::Transient(name) => self.transients.get(name),
            Scope::Prompt(name) => self.prompts.get(name),
            Scope::Layer(name) => self.layers.get(name).map(|l| &l.map),
            Scope::Region(name) => self.regions.get(name),
            Scope::Project(root) => self.projects.get(root),
        }
    }

    /// The map for `scope`, created if needed.
    pub fn get_mut(&mut self, scope: &Scope) -> &mut Keymap {
        match scope {
            Scope::Global => &mut self.global,
            Scope::Mode(m) => self.modes.entry(m.clone()).or_default(),
            Scope::Buffer(id) => self.buffers.entry(*id).or_default(),
            Scope::Transient(name) => self.transients.entry(name.clone()).or_default(),
            Scope::Prompt(name) => self.prompts.entry(name.clone()).or_default(),
            Scope::Layer(name) => &mut self.layers.entry(name.clone()).or_default().map,
            Scope::Region(name) => self.regions.entry(name.clone()).or_default(),
            Scope::Project(root) => self.projects.entry(root.clone()).or_default(),
        }
    }

    /// Define the conditional layer `name`: its bindings apply while `when`
    /// holds. Calling it again replaces the condition, keeping the bindings,
    /// so a plugin can narrow a layer it already filled.
    pub fn add_layer(&mut self, name: &str, when: Condition) {
        self.layers.entry(name.to_string()).or_default().when = Some(when);
    }

    /// `name`'s condition, if the layer has one.
    pub fn layer_condition(&self, name: &str) -> Option<&Condition> {
        self.layers.get(name).and_then(|l| l.when.as_ref())
    }

    /// Drop a layer and its bindings.
    pub fn remove_layer(&mut self, name: &str) -> bool {
        self.layers.remove(name).is_some()
    }

    /// Is any region map bound at all? Asked once per keypress, so the
    /// decorations under point aren't walked when nothing could use them.
    /// A map whose bindings are all gone doesn't count.
    pub fn has_regions(&self) -> bool {
        self.regions.values().any(|m| !m.bindings.is_empty())
    }

    /// The project map for the file at `path`: the one whose root is the
    /// longest prefix of it, so a repository inside a directory that has
    /// bindings of its own uses its own.
    pub fn project_for(&self, path: &std::path::Path) -> Option<(&PathBuf, &Keymap)> {
        self.projects
            .iter()
            .filter(|(root, map)| !map.bindings.is_empty() && path.starts_with(root))
            .max_by_key(|(root, _)| root.as_os_str().len())
    }

    /// Is any project map bound at all? Asked once per keypress, so the
    /// buffer's path isn't walked for nothing.
    pub fn has_projects(&self) -> bool {
        self.projects.values().any(|m| !m.bindings.is_empty())
    }

    /// Forget a project's map and its bindings.
    pub fn remove_project(&mut self, root: &std::path::Path) -> bool {
        self.projects.remove(root).is_some()
    }

    /// The project roots that have bindings, for listings.
    pub fn bound_projects(&self) -> Vec<&PathBuf> {
        let mut roots: Vec<&PathBuf> = self
            .projects
            .iter()
            .filter(|(_, m)| !m.bindings.is_empty())
            .map(|(root, _)| root)
            .collect();
        roots.sort();
        roots
    }

    /// Names of the region maps that have bindings.
    pub fn region_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.regions.keys().map(String::as_str).collect();
        names.sort();
        names
    }

    /// Forget a region map and its bindings.
    pub fn remove_region(&mut self, name: &str) -> bool {
        self.regions.remove(name).is_some()
    }

    /// Names of the conditional layers that have bindings.
    pub fn layer_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.layers.keys().map(String::as_str).collect();
        names.sort();
        names
    }

    /// Bind `keys` to `command`. Returns the normalized sequence.
    pub fn bind(
        &mut self,
        scope: &Scope,
        keys: &str,
        command: &str,
        origin: Origin,
    ) -> Result<Vec<String>, KeymapError> {
        let seq = parse_keys(keys)?;
        self.bind_seq(scope, &seq, command, origin)?;
        Ok(seq)
    }

    /// Like [`Keymaps::bind`] for an already-parsed sequence.
    pub fn bind_seq(
        &mut self,
        scope: &Scope,
        seq: &[String],
        command: &str,
        origin: Origin,
    ) -> Result<(), KeymapError> {
        let takes_user_keys = is_user_reserved(seq) || (seq.len() == 1 && seq[0] == "C-c");
        if origin == Origin::Package && takes_user_keys {
            return Err(KeymapError::Reserved(seq.join(" ")));
        }
        self.get_mut(scope).bind(seq, command)
    }

    /// Remove a binding from one map (lower-priority maps show through again).
    /// Returns false if it wasn't bound there.
    pub fn unbind(&mut self, scope: &Scope, keys: &str) -> Result<bool, KeymapError> {
        let seq = parse_keys(keys)?;
        Ok(self.get_mut(scope).unbind(&seq))
    }

    /// `[remap from]`: keys that would run `from` run `to` instead.
    pub fn remap(&mut self, scope: &Scope, from: &str, to: &str) {
        self.get_mut(scope)
            .remaps
            .insert(from.to_string(), to.to_string());
    }

    pub fn unremap(&mut self, scope: &Scope, from: &str) -> bool {
        self.get_mut(scope).remaps.remove(from).is_some()
    }

    /// Put `scope`'s layer at `priority`, above or below the scopes it would
    /// otherwise be searched among. This is how a plugin's modal map gets
    /// ahead of the major mode without rebinding the keys it wants.
    pub fn set_priority(&mut self, scope: &Scope, priority: i32) {
        self.priorities.insert(scope.clone(), priority);
    }

    /// `scope`'s priority: the override if there is one, else the default
    /// for that scope.
    pub fn priority(&self, scope: &Scope) -> i32 {
        self.priorities
            .get(scope)
            .copied()
            .unwrap_or_else(|| default_priority(scope))
    }

    /// Drop an override, going back to the default for that scope.
    pub fn reset_priority(&mut self, scope: &Scope) -> bool {
        self.priorities.remove(scope).is_some()
    }

    /// Make `mode`'s keymap fall back to `parent`'s (a derived mode).
    pub fn set_mode_parent(&mut self, mode: &str, parent: &str) {
        self.parents.insert(mode.to_string(), parent.to_string());
    }

    pub fn mode_parent(&self, mode: &str) -> Option<&str> {
        self.parents.get(mode).map(String::as_str)
    }

    /// Clear the global and mode maps (and mode parents), keeping
    /// buffer-local maps. Used before reloading the defaults and init.rhai.
    pub fn reset_shared(&mut self) {
        self.global = Keymap::new();
        self.modes.clear();
        self.parents.clear();
        self.layers.clear();
    }

    /// Does `mode`, or anything it derives from, have `ancestor`?
    fn derives_from(&self, mode: &str, ancestor: &str) -> bool {
        let mut m = Some(mode);
        let mut seen = BTreeSet::new();
        while let Some(name) = m {
            if name == ancestor {
                return true;
            }
            if !seen.insert(name) {
                return false;
            }
            m = self.mode_parent(name);
        }
        false
    }

    /// Does `when` hold right now? The mode conditions are answered from
    /// what the caller already passed to [`Keymaps::active`]; the rest come
    /// from the snapshot it took for this keypress.
    fn condition_holds(
        &self,
        when: &Condition,
        major: &str,
        minors: &[String],
        ctx: &Context,
    ) -> bool {
        match when {
            // These two are answered here rather than by `Context::holds`,
            // which has no mode chain to consult.
            Condition::MajorDerivesFrom(a) => self.derives_from(major, a),
            Condition::MinorModeOn(m) => minors.iter().any(|x| x == m),
            Condition::Not(c) => !self.condition_holds(c, major, minors, ctx),
            Condition::All(cs) => cs
                .iter()
                .all(|c| self.condition_holds(c, major, minors, ctx)),
            Condition::Any(cs) => cs
                .iter()
                .any(|c| self.condition_holds(c, major, minors, ctx)),
            other => ctx.holds(other),
        }
    }

    /// Forget a killed buffer's local map.
    pub fn remove_buffer(&mut self, id: BufferId) {
        self.buffers.remove(&id);
    }

    /// Names of the prompt maps that have bindings.
    pub fn prompt_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.prompts.keys().map(String::as_str).collect();
        names.sort();
        names
    }

    /// Whether a prompt has a map of its own. A prompt without one borrows
    /// the default prompt map; isearch checks this so it only answers to
    /// keys bound for it, not the default prompt's RET and C-g.
    pub fn prompt_map_exists(&self, name: &str) -> bool {
        self.prompts.contains_key(name)
    }

    /// Names of the transient maps that have bindings.
    pub fn transient_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.transients.keys().map(String::as_str).collect();
        names.sort();
        names
    }

    /// Does any layer's condition mention something `f` recognizes? Used to
    /// skip gathering a fact nothing asks for: the syntax face and the git
    /// check both cost more than the rest.
    pub fn any_layer_needs(&self, f: impl Fn(&Condition) -> bool) -> bool {
        self.layers
            .values()
            .filter_map(|l| l.when.as_ref())
            .any(|when| {
                let mut found = false;
                when.walk(&mut |c| found |= f(c));
                found
            })
    }

    /// Every command any keymap names, wherever it applies.
    pub fn bound_commands(&self) -> impl Iterator<Item = &str> {
        std::iter::once(&self.global)
            .chain(self.modes.values())
            .chain(self.buffers.values())
            .chain(self.transients.values())
            .chain(self.prompts.values())
            .chain(self.layers.values().map(|layer| &layer.map))
            .chain(self.regions.values())
            .chain(self.projects.values())
            .flat_map(|map| map.bound_commands())
    }

    /// Names of modes that have keymaps.
    pub fn mode_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.modes.keys().map(String::as_str).collect();
        names.sort();
        names
    }

    /// The maps that apply in a buffer, highest priority first. `live` are the
    /// transient maps that are up right now, outermost first; each one
    /// outranks everything else, and among themselves the order they were
    /// pushed in.
    pub fn active(
        &self,
        buffer: BufferId,
        major: &str,
        minors: &[String],
        live: &[LiveTransient],
        prompt: Option<&str>,
        ctx: &Context,
    ) -> ActiveKeymaps<'_> {
        let mut maps = Vec::new();
        // The text point is in can bring its own keys: a Dired entry, a
        // magit section, an org link. Widest span first, so a name inside
        // a listing is searched before the listing around it.
        for name in &ctx.region_maps {
            let Some(m) = self.regions.get(name) else {
                continue;
            };
            let scope = Scope::Region(name.clone());
            maps.push(ActiveMap {
                priority: self.priority(&scope),
                scope,
                map: m,
            });
        }
        // The project the file is in brings bindings of its own, the way a
        // .dir-locals.el does.
        if let Some(root) = &ctx.project
            && let Some(m) = self.projects.get(root)
        {
            let scope = Scope::Project(root.clone());
            maps.push(ActiveMap {
                priority: self.priority(&scope),
                scope,
                map: m,
            });
        }
        // The prompt is in charge while it is up, above everything but a
        // transient map (which is there to override even this).
        if let Some(name) = prompt {
            // A prompt with no map of its own borrows the default one; the
            // scope it is reported under stays the purpose name, so
            // where-is and describe-key name the prompt the user is in.
            let found = self
                .prompts
                .get(name)
                .or_else(|| self.prompts.get(PROMPT_DEFAULT));
            if let Some(m) = found {
                let scope = Scope::Prompt(name.to_string());
                maps.push(ActiveMap {
                    priority: self.priority(&scope),
                    scope,
                    map: m,
                });
            }
        }
        // Push order below is only the tie-break: the sort by priority is
        // what decides. Layers that share a priority keep this order.
        for t in live {
            let scope = Scope::Transient(t.name.clone());
            if let Some(m) = self.transients.get(&t.name) {
                maps.push(ActiveMap {
                    priority: self.priority(&scope),
                    scope,
                    map: m,
                });
            }
        }
        if let Some(m) = self.buffers.get(&buffer) {
            let scope = Scope::Buffer(buffer);
            maps.push(ActiveMap {
                priority: self.priority(&scope),
                scope,
                map: m,
            });
        }
        // Conditional layers, those whose condition holds right now. One
        // whose condition isn't set is skipped, so binding into a layer that
        // doesn't exist can't shadow anything.
        for (name, layer) in &self.layers {
            let Some(when) = &layer.when else { continue };
            if !self.condition_holds(when, major, minors, ctx) {
                continue;
            }
            let scope = Scope::Layer(name.clone());
            maps.push(ActiveMap {
                priority: self.priority(&scope),
                scope,
                map: &layer.map,
            });
        }
        for minor in minors {
            if let Some(m) = self.modes.get(minor) {
                let scope = Scope::Mode(minor.clone());
                maps.push(ActiveMap {
                    priority: self.priority(&scope),
                    scope,
                    map: m,
                });
            }
        }
        // The major mode, then what it derives from (guarding against cycles).
        let mut mode = Some(major);
        let mut seen = BTreeSet::new();
        while let Some(name) = mode {
            if !seen.insert(name) {
                break;
            }
            if let Some(m) = self.modes.get(name) {
                let scope = Scope::Mode(name.to_string());
                maps.push(ActiveMap {
                    // A mode map defaults to the major-mode priority here: if
                    // this same mode is also a minor mode, the caller's
                    // override still wins, because set_priority is per scope.
                    priority: self
                        .priorities
                        .get(&scope)
                        .copied()
                        .unwrap_or(PRIORITY_MAJOR_MODE),
                    scope,
                    map: m,
                });
            }
            mode = self.mode_parent(name);
        }
        maps.push(ActiveMap {
            priority: self.priority(&Scope::Global),
            scope: Scope::Global,
            map: &self.global,
        });
        // Stable, so equal priorities keep the push order above: most recent
        // minor mode first, major mode before the mode it derives from.
        maps.sort_by_key(|m| std::cmp::Reverse(m.priority));
        ActiveKeymaps { maps }
    }
}

// ---------------------------------------------------------- active keymaps

pub struct ActiveMap<'a> {
    /// Where this layer sits; higher is searched first. See [`PRIORITY_GLOBAL`].
    pub priority: i32,
    pub scope: Scope,
    pub map: &'a Keymap,
}

/// What a key sequence means right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Command {
        command: String,
        /// Where the binding was found.
        scope: Scope,
        /// Set when a `[remap ...]` replaced the bound command.
        remapped_from: Option<String>,
    },
    Prefix,
    Undefined,
}

/// Next key after a prefix, for prefix hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    Command(String),
    Prefix,
}

/// The keymaps active in one buffer, in priority order.
pub struct ActiveKeymaps<'a> {
    pub maps: Vec<ActiveMap<'a>>,
}

impl ActiveKeymaps<'_> {
    /// Like [`ActiveKeymaps::lookup`], but only in the layers at or above
    /// `min_priority`. A prompt uses this: a key its map doesn't bind is
    /// text to type, not a global binding to run.
    pub fn lookup_above(&self, seq: &[String], min_priority: i32) -> Lookup {
        self.search(seq, Some(min_priority))
    }

    pub fn lookup(&self, seq: &[String]) -> Lookup {
        self.search(seq, None)
    }

    /// Look `seq` up in the maps, highest priority first, and if nothing
    /// binds it that way, look for the other name of the same key (see
    /// [`alias_keys`]). A binding in either spelling answers for both;
    /// one in both answers for itself, because the spelling asked for is
    /// searched first.
    fn search(&self, seq: &[String], min_priority: Option<i32>) -> Lookup {
        // One spelling, across every map, before the other: a key bound in
        // both spellings means itself, and the layer order is the same
        // either way.
        let spelling = |candidate: &[String]| -> Option<Lookup> {
            let keys = candidate.join(" ");
            self.maps
                .iter()
                .filter(|m| min_priority.is_none_or(|min| m.priority >= min))
                .find_map(|m| {
                    let cmd = m.map.get(&keys)?;
                    Some(Lookup::Command {
                        command: self.remap(cmd).unwrap_or(cmd).to_string(),
                        scope: m.scope.clone(),
                        remapped_from: self.remap(cmd).map(|_| cmd.to_string()),
                    })
                })
                .or_else(|| {
                    self.maps
                        .iter()
                        .filter(|m| min_priority.is_none_or(|min| m.priority >= min))
                        .find(|m| m.map.is_prefix(&keys))
                        .map(|_| Lookup::Prefix)
                })
        };
        if let Some(found) = spelling(seq) {
            return found;
        }
        // TAB is C-i and RET is C-m: which one arrives depends on the
        // terminal, not on what the user meant.
        alias_keys(seq)
            .and_then(|alias| spelling(&alias))
            .unwrap_or(Lookup::Undefined)
    }

    /// Every command named by the maps that apply right now.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.maps.iter().flat_map(|map| map.map.bound_commands())
    }

    /// Where `command` is remapped to, if anywhere.
    pub fn remap(&self, command: &str) -> Option<&str> {
        self.maps.iter().find_map(|m| m.map.remap_of(command))
    }

    /// Commands that are remapped to `command`.
    pub fn remapped_from(&self, command: &str) -> Vec<String> {
        let mut from: BTreeSet<String> = BTreeSet::new();
        for m in &self.maps {
            for (f, _) in m.map.remaps() {
                if self.remap(f) == Some(command) {
                    from.insert(f.to_string());
                }
            }
        }
        from.into_iter().collect()
    }

    /// Every key sequence that runs `command` (after shadowing and
    /// remapping), shortest first.
    pub fn where_is(&self, command: &str) -> Vec<String> {
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for m in &self.maps {
            keys.extend(m.map.bindings().map(|(k, _)| k));
        }
        let mut out: Vec<String> = keys
            .into_iter()
            .filter(|k| {
                let seq: Vec<String> = k.split(' ').map(String::from).collect();
                matches!(self.lookup(&seq), Lookup::Command { command: c, .. } if c == command)
            })
            .map(String::from)
            .collect();
        out.sort_by_key(|k| (k.split(' ').count(), k.len(), k.clone()));
        out
    }

    /// The keys that can follow `prefix`, with what each does. Shadowed
    /// and `undefined` keys are left out.
    pub fn continuations(&self, prefix: &[String]) -> Vec<(String, Continuation)> {
        let p = if prefix.is_empty() {
            String::new()
        } else {
            format!("{} ", prefix.join(" "))
        };
        let mut next: BTreeSet<&str> = BTreeSet::new();
        for m in &self.maps {
            for (k, _) in m.map.bindings() {
                if let Some(rest) = k.strip_prefix(p.as_str())
                    && let Some(first) = rest.split(' ').next().filter(|f| !f.is_empty())
                {
                    next.insert(first);
                }
            }
        }
        let mut out: Vec<(String, Continuation)> = next
            .into_iter()
            .filter_map(|k| {
                let mut seq = prefix.to_vec();
                seq.push(k.to_string());
                match self.lookup(&seq) {
                    Lookup::Command { command, .. } if command != UNDEFINED => {
                        Some((k.to_string(), Continuation::Command(command)))
                    }
                    Lookup::Prefix => Some((k.to_string(), Continuation::Prefix)),
                    _ => None,
                }
            })
            .collect();
        out.sort_by_key(|(k, _)| key_sort_key(k));
        out
    }

    /// Is the binding of `keys` in map number `index` the one that is
    /// actually used (not shadowed by a higher-priority map)?
    pub fn is_effective(&self, index: usize, keys: &str) -> bool {
        let seq: Vec<String> = keys.split(' ').map(String::from).collect();
        // Any higher map binding or prefixing a leading part of the sequence wins.
        for m in &self.maps[..index] {
            for i in 1..=seq.len() {
                let part = seq[..i].join(" ");
                if m.map.get(&part).is_some() || (i == seq.len() && m.map.is_prefix(&part)) {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn cmd(l: Lookup) -> Option<String> {
        match l {
            Lookup::Command { command, .. } => Some(command),
            _ => None,
        }
    }

    #[test]
    fn parses_and_normalizes_keys() {
        assert_eq!(parse_keys("C-x  C-s").unwrap(), seq("C-x C-s"));
        assert_eq!(parse_keys("M-C-x").unwrap(), seq("C-M-x"));
        assert_eq!(parse_keys("C-X").unwrap(), seq("C-x"));
        assert_eq!(parse_keys("M-X").unwrap(), seq("M-X"));
        assert_eq!(parse_keys("C-@").unwrap(), seq("C-SPC"));
        assert_eq!(parse_keys("C--").unwrap(), seq("C--"));
        assert_eq!(
            parse_keys("<f5> <left> M-DEL").unwrap(),
            seq("<f5> <left> M-DEL")
        );
        assert!(parse_keys("").is_err());
        assert!(parse_keys("C-foo").is_err());
        assert!(parse_keys("<nope>").is_err());
        // Shift, Hyper, Meta and Super have names now (a terminal with the
        // kitty keyboard protocol can report them; without one, S-<left>
        // just never fires).
        assert_eq!(parse_keys("S-a  s-b  S-A").unwrap(), seq("S-a S-b S-a"));
        assert_eq!(parse_keys("C-S-a").unwrap(), seq("C-S-a"));
        assert_eq!(
            parse_keys("S-<left> H-<left>").unwrap(),
            seq("S-<left> H-<left>")
        );
        assert_eq!(
            parse_keys("s-super-x s-m-x").unwrap(),
            seq("s-super-x s-m-x")
        );
        assert_eq!(
            parse_keys("Control-x Meta-x Shift-x").unwrap(),
            seq("C-x M-x S-x")
        );
        assert_eq!(
            parse_keys("A-x Alt-x Ctrl-x Cmd-x Command-x").unwrap(),
            seq("M-x M-x C-x s-super-x s-super-x"),
            "Alt- is M-, Command- is Super"
        );
        assert_eq!(
            parse_keys("s-C-a").unwrap(),
            seq("C-S-a"),
            "the order written in doesn't matter; the order printed does"
        );
        assert!(
            parse_keys("C-C-a").is_err(),
            "the same modifier twice is a typo"
        );
    }

    #[test]
    fn tab_and_c_i_are_one_key() {
        let mut km = Keymaps::new();
        let g = Scope::Global;
        // Only TAB is bound, as it is by default.
        km.bind(&g, "TAB", "indent", Origin::Package).unwrap();
        let finds = |km: &Keymaps, keys: &str| {
            cmd(km
                .active(BufferId(0), "m", &[], &[], None, &Context::default())
                .lookup(&seq(keys)))
        };
        assert_eq!(finds(&km, "TAB").as_deref(), Some("indent"));
        assert_eq!(
            finds(&km, "C-i").as_deref(),
            Some("indent"),
            "Ctrl+I is the same key, and finds TAB's binding"
        );
        // Binding C-i too: now each means itself.
        km.bind(&g, "C-i", "newline", Origin::Package).unwrap();
        assert_eq!(finds(&km, "TAB").as_deref(), Some("indent"));
        assert_eq!(finds(&km, "C-i").as_deref(), Some("newline"));
        // RET and C-m likewise, and the other way round.
        let mut km = Keymaps::new();
        km.bind(&g, "C-m", "newline", Origin::Package).unwrap();
        assert_eq!(finds(&km, "RET").as_deref(), Some("newline"));
        // In a sequence too, and as a prefix.
        let mut km = Keymaps::new();
        km.bind(&g, "C-x TAB", "quoted", Origin::Package).unwrap();
        let finds = |km: &Keymaps, keys: &str| {
            km.active(BufferId(0), "m", &[], &[], None, &Context::default())
                .lookup(&seq(keys))
        };
        assert_eq!(cmd(finds(&km, "C-x C-i")).as_deref(), Some("quoted"));
        // A TAB binding that is only a prefix is still a prefix when C-i
        // is what was typed.
        let mut km = Keymaps::new();
        km.bind(&g, "C-x TAB f", "fill", Origin::Package).unwrap();
        assert!(matches!(finds(&km, "C-x C-i"), Lookup::Prefix));
        assert_eq!(
            alias_keys(&seq("TAB")),
            Some(seq("C-i")),
            "the pair is symmetric"
        );
        assert_eq!(alias_keys(&seq("C-m")), Some(seq("RET")));
        assert_eq!(
            alias_keys(&seq("C-x C-s")),
            None,
            "most keys have no other name"
        );
        // Shift+i is I: a binding written "C-h I" answers to the keypress,
        // and one written "S-a" to a capital A.
        assert_eq!(alias_keys(&seq("C-h S-i")), Some(seq("C-h I")));
        assert_eq!(alias_keys(&seq("A")), Some(seq("S-a")));
        assert_eq!(alias_keys(&seq("C-S-a")), None, "only a plain Shift+letter");
        let mut km = Keymaps::new();
        km.bind(&g, "C-h I", "describe-input-method", Origin::Package)
            .unwrap();
        km.bind(&g, "C-x v I", "vc-log-incoming", Origin::Package)
            .unwrap();
        assert!(matches!(finds(&km, "C-h S-i"), Lookup::Command { .. }));
        assert!(matches!(finds(&km, "C-x v S-i"), Lookup::Command { .. }));
    }

    #[test]
    fn layers_shadow_in_priority_order() {
        let mut km = Keymaps::new();
        let buf = BufferId(1);
        let g = Scope::Global;
        km.bind(&g, "C-k", "kill-line", Origin::Package).unwrap();
        km.bind(&g, "C-x C-s", "save-buffer", Origin::Package)
            .unwrap();
        km.bind(
            &Scope::Mode("text-mode".into()),
            "C-k",
            "major-kill",
            Origin::Package,
        )
        .unwrap();
        km.bind(
            &Scope::Mode("foo-mode".into()),
            "C-k",
            "minor-kill",
            Origin::Package,
        )
        .unwrap();
        km.bind(&Scope::Buffer(buf), "C-x j", "jump", Origin::User)
            .unwrap();

        let minors = vec!["foo-mode".to_string()];
        let active = km.active(buf, "text-mode", &minors, &[], None, &Context::default());
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("minor-kill")
        );
        // Prefix maps merge: the buffer map's C-x j doesn't hide global C-x C-s.
        assert_eq!(active.lookup(&seq("C-x")), Lookup::Prefix);
        assert_eq!(cmd(active.lookup(&seq("C-x j"))).as_deref(), Some("jump"));
        assert_eq!(
            cmd(active.lookup(&seq("C-x C-s"))).as_deref(),
            Some("save-buffer")
        );

        let active = km.active(buf, "text-mode", &[], &[], None, &Context::default());
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("major-kill")
        );
        let active = km.active(
            BufferId(2),
            "fundamental-mode",
            &[],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("kill-line")
        );
        assert_eq!(active.lookup(&seq("C-x j")), Lookup::Undefined);
    }

    #[test]
    fn derived_modes_inherit_keys() {
        let mut km = Keymaps::new();
        km.bind(
            &Scope::Mode("special-mode".into()),
            "q",
            "quit-window",
            Origin::Package,
        )
        .unwrap();
        km.set_mode_parent("help-mode", "special-mode");
        let active = km.active(
            BufferId(0),
            "help-mode",
            &[],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            cmd(active.lookup(&seq("q"))).as_deref(),
            Some("quit-window")
        );
    }

    #[test]
    fn remap_and_where_is() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-k", "kill-line", Origin::Package)
            .unwrap();
        km.bind(&Scope::Global, "C-x C-k", "kill-line", Origin::Package)
            .unwrap();
        km.remap(&Scope::Mode("m".into()), "kill-line", "kill-whole-line");
        let minors = vec!["m".to_string()];
        let active = km.active(
            BufferId(0),
            "fundamental-mode",
            &minors,
            &[],
            None,
            &Context::default(),
        );
        match active.lookup(&seq("C-k")) {
            Lookup::Command {
                command,
                remapped_from,
                ..
            } => {
                assert_eq!(command, "kill-whole-line");
                assert_eq!(remapped_from.as_deref(), Some("kill-line"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(active.where_is("kill-whole-line"), vec!["C-k", "C-x C-k"]);
        assert!(active.where_is("kill-line").is_empty());
        assert_eq!(active.remapped_from("kill-whole-line"), vec!["kill-line"]);
    }

    #[test]
    fn a_layer_can_be_given_its_own_priority() {
        let mut km = Keymaps::new();
        let buf = BufferId(1);
        let g = Scope::Global;
        km.bind(&g, "C-k", "global-kill", Origin::Package).unwrap();
        km.bind(
            &Scope::Mode("text-mode".into()),
            "C-k",
            "major-kill",
            Origin::Package,
        )
        .unwrap();
        km.bind(
            &Scope::Mode("modal".into()),
            "C-k",
            "modal-kill",
            Origin::Package,
        )
        .unwrap();

        // Defaults first: a minor mode beats the major mode.
        let active = km.active(
            buf,
            "text-mode",
            &["modal".to_string()],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("modal-kill")
        );

        // Raising one layer's number is all it takes to reorder them.
        let modal = Scope::Mode("modal".into());
        let previous = km.priority(&modal);
        assert_eq!(previous, PRIORITY_MINOR_MODE);
        km.set_priority(&modal, PRIORITY_MINOR_MODE + 25);
        let active = km.active(
            buf,
            "text-mode",
            &["modal".to_string()],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            active.maps[0].scope,
            Scope::Mode("modal".into()),
            "the raised layer is searched first"
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("modal-kill")
        );

        // ...and lowering it below the buffer map loses to the buffer.
        km.bind(&Scope::Buffer(buf), "C-k", "buffer-kill", Origin::User)
            .unwrap();
        km.set_priority(&modal, PRIORITY_BUFFER_LOCAL - 25);
        let active = km.active(
            buf,
            "text-mode",
            &["modal".to_string()],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("buffer-kill")
        );

        // Resetting goes back to the default, and to the old winner.
        assert!(km.reset_priority(&modal));
        assert_eq!(km.priority(&modal), PRIORITY_MINOR_MODE);
        let active = km.active(
            buf,
            "text-mode",
            &["modal".to_string()],
            &[],
            None,
            &Context::default(),
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("buffer-kill")
        );
    }

    #[test]
    fn equal_priorities_keep_the_order_they_always_had() {
        let mut km = Keymaps::new();
        let buf = BufferId(1);
        let minors = ["second".to_string(), "first".to_string()];
        let expected = [("second", "second-kill"), ("first", "first-kill")];
        for (m, command) in expected {
            km.bind(&Scope::Mode(m.into()), "C-k", command, Origin::Package)
                .unwrap();
        }
        let active = km.active(
            buf,
            "fundamental-mode",
            &minors,
            &[],
            None,
            &Context::default(),
        );
        // Most recently enabled minor mode first, as before.
        assert_eq!(
            active.maps[0].scope,
            Scope::Mode("second".into()),
            "the most recent minor mode is still searched first"
        );
        assert_eq!(
            cmd(active.lookup(&seq("C-k"))).as_deref(),
            Some("second-kill")
        );
    }

    #[test]
    fn undefined_shadows_lower_maps() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-z", "suspend", Origin::Package)
            .unwrap();
        km.bind(&Scope::Mode("m".into()), "C-z", UNDEFINED, Origin::Package)
            .unwrap();
        let active = km.active(BufferId(0), "m", &[], &[], None, &Context::default());
        assert_eq!(cmd(active.lookup(&seq("C-z"))).as_deref(), Some(UNDEFINED));
        assert!(active.continuations(&[]).is_empty());
    }

    #[test]
    fn c_c_letter_is_reserved_for_users() {
        let mut km = Keymaps::new();
        let m = Scope::Mode("m".into());
        assert!(matches!(
            km.bind(&m, "C-c a", "x", Origin::Package),
            Err(KeymapError::Reserved(_))
        ));
        assert!(matches!(
            km.bind(&m, "C-c", "x", Origin::Package),
            Err(KeymapError::Reserved(_))
        ));
        assert!(km.bind(&m, "C-c C-a", "x", Origin::Package).is_ok());
        assert!(km.bind(&m, "C-c .", "x", Origin::Package).is_ok());
        assert!(km.bind(&Scope::Global, "C-c a", "x", Origin::User).is_ok());
    }

    #[test]
    fn non_prefix_keys_and_rebinding_prefixes() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-x C-s", "save-buffer", Origin::Package)
            .unwrap();
        assert!(matches!(
            km.bind(&Scope::Global, "C-x C-s x", "y", Origin::User),
            Err(KeymapError::NonPrefix { .. })
        ));
        // Binding the prefix itself replaces what was under it.
        km.bind(&Scope::Global, "C-x", "x-cmd", Origin::User)
            .unwrap();
        assert!(km.global.get("C-x C-s").is_none());
    }

    #[test]
    fn continuations_after_prefix() {
        let mut km = Keymaps::new();
        let g = Scope::Global;
        km.bind(&g, "C-x C-s", "save-buffer", Origin::Package)
            .unwrap();
        km.bind(&g, "C-x 4 f", "find-file-other-window", Origin::Package)
            .unwrap();
        km.bind(&g, "C-x b", "switch-to-buffer", Origin::Package)
            .unwrap();
        km.bind(
            &Scope::Mode("m".into()),
            "C-x b",
            "mode-switch",
            Origin::Package,
        )
        .unwrap();
        let active = km.active(BufferId(0), "m", &[], &[], None, &Context::default());
        let c = active.continuations(&seq("C-x"));
        assert_eq!(
            c,
            vec![
                ("4".to_string(), Continuation::Prefix),
                ("b".to_string(), Continuation::Command("mode-switch".into())),
                (
                    "C-s".to_string(),
                    Continuation::Command("save-buffer".into())
                ),
            ]
        );
    }

    #[test]
    fn shadowed_bindings_are_not_effective() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-k", "kill-line", Origin::Package)
            .unwrap();
        km.bind(&Scope::Global, "C-f", "forward-char", Origin::Package)
            .unwrap();
        km.bind(&Scope::Mode("m".into()), "C-k", "other", Origin::Package)
            .unwrap();
        let active = km.active(BufferId(0), "m", &[], &[], None, &Context::default());
        assert!(active.is_effective(0, "C-k"));
        assert!(!active.is_effective(1, "C-k"));
        assert!(active.is_effective(1, "C-f"));
    }

    // ------------------------------------------------------ conditional layers

    fn layered() -> (Keymaps, Condition, Condition) {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global-move", Origin::Package)
            .unwrap();
        // One layer for when the region is active, one for the other way
        // round: both bind C-f, so which one shows says what held.
        km.bind(
            &Scope::Layer("region".into()),
            "C-f",
            "region-move",
            Origin::Package,
        )
        .unwrap();
        km.bind(
            &Scope::Layer("plain".into()),
            "C-f",
            "plain-move",
            Origin::Package,
        )
        .unwrap();
        (
            km,
            Condition::RegionActive,
            Condition::Not(Box::new(Condition::RegionActive)),
        )
    }

    fn active_cmd(km: &Keymaps, ctx: &Context, major: &str, minors: &[String]) -> Option<String> {
        cmd(km
            .active(BufferId(0), major, minors, &[], None, ctx)
            .lookup(&seq("C-f")))
    }

    #[test]
    fn a_layer_answers_only_while_its_condition_holds() {
        let (mut km, region, not_region) = layered();
        km.add_layer("region", region.clone());
        km.add_layer("plain", not_region);
        assert_eq!(
            active_cmd(&km, &Context::default(), "fundamental-mode", &[]).as_deref(),
            Some("plain-move"),
            "no region: the not-region layer answers"
        );
        assert_eq!(
            active_cmd(
                &km,
                &Context {
                    region_active: true,
                    ..Default::default()
                },
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("region-move"),
            "a region active: the region layer answers"
        );
    }

    #[test]
    fn a_layer_with_no_condition_is_never_active() {
        // Binding into a layer that was never given a condition must not
        // shadow the global map by accident.
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global-move", Origin::Package)
            .unwrap();
        km.bind(&Scope::Layer("typo".into()), "C-f", "typo", Origin::Package)
            .unwrap();
        assert_eq!(
            active_cmd(&km, &Context::default(), "fundamental-mode", &[]).as_deref(),
            Some("global-move")
        );
        assert_eq!(km.layer_condition("typo"), None);
    }

    #[test]
    fn the_mode_conditions_come_from_the_mode_chain() {
        let mut km = Keymaps::new();
        km.set_mode_parent("help-mode", "special-mode");
        km.bind(
            &Scope::Layer("special".into()),
            "C-f",
            "special",
            Origin::Package,
        )
        .unwrap();
        km.add_layer(
            "special",
            Condition::MajorDerivesFrom("special-mode".into()),
        );
        assert_eq!(
            active_cmd(&km, &Context::default(), "fundamental-mode", &[]).as_deref(),
            None,
            "fundamental-mode doesn't derive from special-mode"
        );
        assert_eq!(
            active_cmd(&km, &Context::default(), "help-mode", &[]).as_deref(),
            Some("special"),
            "help-mode derives from it, so the layer is up"
        );
        // A minor mode is asked by name, not derived from.
        let mut km = layered().0;
        km.add_layer("region", Condition::MinorModeOn("modal".into()));
        assert_eq!(
            active_cmd(&km, &Context::default(), "text-mode", &[]).as_deref(),
            Some("global-move"),
            "the minor mode is off, so its layer stays down"
        );
        assert_eq!(
            active_cmd(&km, &Context::default(), "text-mode", &["modal".into()]).as_deref(),
            Some("region-move"),
            "and on, so its layer answers"
        );
    }

    #[test]
    fn a_layer_sits_above_the_modes_and_below_the_buffer() {
        let mut km = Keymaps::new();
        km.bind(
            &Scope::Buffer(BufferId(0)),
            "C-f",
            "buffer",
            Origin::Package,
        )
        .unwrap();
        km.bind(&Scope::Mode("m".into()), "C-f", "mode", Origin::Package)
            .unwrap();
        km.bind(
            &Scope::Layer("region".into()),
            "C-f",
            "layer",
            Origin::Package,
        )
        .unwrap();
        km.add_layer("region", Condition::RegionActive);
        assert_eq!(
            active_cmd(&km, &Context::default(), "m", &[]).as_deref(),
            Some("buffer"),
            "the buffer-local map is above a conditional layer"
        );
        // Drop the buffer-local binding and the layer is what answers: it
        // sits above the major mode, which is where a plugin's modal-ish
        // keys belong.
        km.unbind(&Scope::Buffer(BufferId(0)), "C-f").unwrap();
        assert_eq!(
            active_cmd(&km, &Context::default(), "m", &[]).as_deref(),
            Some("mode"),
            "with the condition down, the major mode answers"
        );
        assert_eq!(
            active_cmd(
                &km,
                &Context {
                    region_active: true,
                    ..Default::default()
                },
                "m",
                &[]
            )
            .as_deref(),
            Some("layer"),
            "and a conditional layer outranks the major mode"
        );
    }

    #[test]
    fn any_and_all_combine_the_conditions() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global", Origin::Package)
            .unwrap();
        let both = Condition::All(vec![Condition::RegionActive, Condition::ReadOnly]);
        km.bind(&Scope::Layer("l".into()), "C-f", "both", Origin::Package)
            .unwrap();
        km.add_layer("l", both.clone());
        assert_eq!(
            active_cmd(
                &km,
                &Context {
                    region_active: true,
                    read_only: false,
                    ..Default::default()
                },
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("global"),
            "all of them must hold"
        );
        assert_eq!(
            active_cmd(
                &km,
                &Context {
                    region_active: true,
                    read_only: true,
                    ..Default::default()
                },
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("both")
        );
        // The mode conditions work inside the combinators too, which is what
        // makes "Not(MajorDerivesFrom(...))" expressible.
        let not_special =
            Condition::Not(Box::new(Condition::MajorDerivesFrom("special-mode".into())));
        km.set_mode_parent("help-mode", "special-mode");
        km.add_layer("l", not_special.clone());
        assert_eq!(
            active_cmd(&km, &Context::default(), "help-mode", &[]).as_deref(),
            Some("global"),
            "help-mode derives from special-mode, so the layer is down"
        );
        assert_eq!(
            active_cmd(&km, &Context::default(), "fundamental-mode", &[]).as_deref(),
            Some("both"),
            "and it isn't, so the layer is up"
        );
        assert_eq!(not_special.label(), "not(major-derives-from(special-mode))");
    }

    #[test]
    fn a_layer_syntax_condition_reads_the_face_at_point() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global", Origin::Package)
            .unwrap();
        km.bind(
            &Scope::Layer("c".into()),
            "C-f",
            "in-comment",
            Origin::Package,
        )
        .unwrap();
        km.add_layer("c", Condition::InComment);
        let face = |f| Context {
            face_at_point: Some(f),
            ..Default::default()
        };
        assert_eq!(
            active_cmd(
                &km,
                &face(crate::syntax::SyntaxFace::Comment),
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("in-comment")
        );
        assert_eq!(
            active_cmd(
                &km,
                &face(crate::syntax::SyntaxFace::Keyword),
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("global"),
            "a keyword isn't a comment"
        );
        // An escape counts as inside the string it belongs to.
        km.add_layer("c", Condition::InString);
        assert_eq!(
            active_cmd(
                &km,
                &face(crate::syntax::SyntaxFace::Escape),
                "fundamental-mode",
                &[]
            )
            .as_deref(),
            Some("in-comment"),
            "the layer was replaced, so it still answers as InComment"
        );
        assert_eq!(km.layer_condition("c"), Some(&Condition::InString));
    }

    #[test]
    fn only_the_expensive_conditions_are_worth_looking_for() {
        let mut km = Keymaps::new();
        km.add_layer("a", Condition::RegionActive);
        km.add_layer("b", Condition::MinorModeOn("modal".into()));
        assert!(
            !km.any_layer_needs(|c| matches!(c, Condition::InGitRepo)),
            "nothing asks for the git check"
        );
        km.add_layer(
            "c",
            Condition::Any(vec![Condition::ReadOnly, Condition::InString]),
        );
        assert!(
            km.any_layer_needs(|c| matches!(c, Condition::InString)),
            "a nested condition counts too"
        );
        assert!(
            !km.any_layer_needs(|c| matches!(c, Condition::InGitRepo)),
            "and still nothing asks for git"
        );
    }

    #[test]
    fn a_region_map_answers_only_where_its_text_is() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global", Origin::Package)
            .unwrap();
        km.bind(
            &Scope::Buffer(BufferId(0)),
            "C-f",
            "buffer",
            Origin::Package,
        )
        .unwrap();
        km.bind(
            &Scope::Region("entry".into()),
            "C-f",
            "region",
            Origin::Package,
        )
        .unwrap();
        // With no region under point the buffer-local map answers.
        assert_eq!(
            active_cmd(&km, &Context::default(), "m", &[]).as_deref(),
            Some("buffer")
        );
        let at = |names: &[&str]| Context {
            region_maps: names.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(
            active_cmd(&km, &at(&["entry"]), "m", &[]).as_deref(),
            Some("region"),
            "a region map is above the buffer-local map"
        );
        // A name no region map has is nothing, not an error.
        assert_eq!(
            active_cmd(&km, &at(&["typo"]), "m", &[]).as_deref(),
            Some("buffer")
        );
        assert_eq!(km.priority(&Scope::Region("entry".into())), 600);
    }

    #[test]
    fn mouse_keys_are_keys() {
        assert_eq!(
            parse_keys("<mouse-1>").unwrap(),
            vec!["<mouse-1>".to_string()],
            "a click can be bound like any other key"
        );
        assert!(
            parse_keys("<mouse-9>").is_err(),
            "there is no notation for that button"
        );
    }

    #[test]
    fn a_project_map_answers_for_the_files_inside_it() {
        let mut km = Keymaps::new();
        km.bind(&Scope::Global, "C-f", "global", Origin::Package)
            .unwrap();
        let root = PathBuf::from("/tmp/project");
        km.bind(
            &Scope::Project(root.clone()),
            "C-f",
            "project",
            Origin::Package,
        )
        .unwrap();
        let at = |dir: &str| Context {
            project: Some(PathBuf::from(dir)),
            ..Default::default()
        };
        assert_eq!(
            active_cmd(&km, &at("/tmp/project"), "m", &[]).as_deref(),
            Some("project"),
            "a file in the project sees its bindings"
        );
        assert_eq!(
            active_cmd(&km, &at("/tmp/elsewhere"), "m", &[]).as_deref(),
            Some("global"),
            "a file outside it doesn't see the bindings"
        );
        // The nearest project wins: a repository inside a directory that
        // has bindings of its own uses its own.
        let outer = PathBuf::from("/tmp/outer");
        km.bind(
            &Scope::Project(outer.clone()),
            "C-f",
            "outer",
            Origin::Package,
        )
        .unwrap();
        assert_eq!(
            active_cmd(&km, &at("/tmp/outer"), "m", &[]).as_deref(),
            Some("outer"),
            "until the inner project has bindings of its own"
        );
        let inner = PathBuf::from("/tmp/outer/inner");
        km.bind(
            &Scope::Project(inner.clone()),
            "C-f",
            "inner",
            Origin::Package,
        )
        .unwrap();
        assert_eq!(
            active_cmd(&km, &at("/tmp/outer/inner"), "m", &[]).as_deref(),
            Some("inner")
        );
        // Which root a file gets is picked by prefix, not by point: the
        // longest one above it wins.
        assert_eq!(
            km.project_for(std::path::Path::new("/tmp/outer/inner/src/a.rs"))
                .map(|(root, _)| root.as_path()),
            Some(inner.as_path())
        );
        assert_eq!(
            km.project_for(std::path::Path::new("/tmp/outer/other/a.rs"))
                .map(|(root, _)| root.as_path()),
            Some(outer.as_path()),
            "and the file's own directory is what decides"
        );
        assert_eq!(km.priority(&Scope::Project(root)), 350);
    }

    #[test]
    fn layers_are_kept_out_of_reset_shared_and_listed_by_name() {
        let mut km = Keymaps::new();
        km.add_layer("b", Condition::RegionActive);
        km.add_layer("a", Condition::ReadOnly);
        assert_eq!(km.layer_names(), ["a", "b"]);
        km.reset_shared();
        assert!(
            km.layer_names().is_empty(),
            "a reload starts with no layers, since their conditions come with them"
        );
    }
}
