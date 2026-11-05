// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal org-mode: enough to live in an Org file like a TODO file.
//!
//! - Outline: headings are lines starting with stars. TAB cycles a
//!   subtree (folded, children, everything), S-TAB the whole buffer
//!   (overview, contents, everything). Folding uses fold.rs.
//! - TODO keywords: `C-c C-t` cycles none -> TODO -> DONE -> none, or the
//!   sequence from `#+TODO: A B | C D` lines.
//! - Checkboxes (`- [ ] item`) and statistics cookies (`[1/3]`, `[33%]`)
//!   on headings and list items: `C-c C-c` toggles a checkbox and every
//!   cookie is recounted.
//! - Structure editing: new heading or item (M-RET), promote and demote
//!   (M-left, M-right, with the subtree: M-S-left, M-S-right), move a
//!   subtree among its siblings (M-up, M-down).
//! - Highlighting without a grammar: headings by level, keywords, tags,
//!   links, verbatim, dates, comments, drawers.
//!
//! Everything here works on a [`Buffer`]; the frontend binds keys.

use crate::buffer::{Buffer, Pos};
use crate::syntax::SyntaxFace;
use regex::Regex;
use std::sync::LazyLock;

/// The major mode (for files ending in .org).
pub const MODE: &str = "org-mode";

/// TODO keywords: not done, then done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keywords {
    pub todo: Vec<String>,
    pub done: Vec<String>,
}

impl Default for Keywords {
    fn default() -> Self {
        Self {
            todo: vec!["TODO".into()],
            done: vec!["DONE".into()],
        }
    }
}

impl Keywords {
    /// From `#+TODO:` (or `#+SEQ_TODO:`, `#+TYP_TODO:`) lines: words, a
    /// `|`, done words (without `|`, the last word is the done one).
    /// Shortcut keys like `TODO(t)` are allowed and ignored. The first
    /// such line wins; none means TODO | DONE.
    pub fn from_text<'a>(lines: impl Iterator<Item = &'a str>) -> Self {
        for line in lines {
            let l = line.trim_start();
            let Some(rest) = ["#+TODO:", "#+SEQ_TODO:", "#+TYP_TODO:"]
                .iter()
                .find_map(|p| {
                    l.get(..p.len())
                        .filter(|h| h.eq_ignore_ascii_case(p))
                        .map(|_| &l[p.len()..])
                })
            else {
                continue;
            };
            let words: Vec<String> = rest
                .split_whitespace()
                .map(|w| w.split('(').next().unwrap_or(w).to_string())
                .collect();
            let (todo, done) = match words.iter().position(|w| w == "|") {
                Some(i) => (words[..i].to_vec(), words[i + 1..].to_vec()),
                None if words.len() > 1 => (
                    words[..words.len() - 1].to_vec(),
                    words[words.len() - 1..].to_vec(),
                ),
                None => (words.clone(), Vec::new()),
            };
            if !todo.is_empty() || !done.is_empty() {
                return Self { todo, done };
            }
        }
        Self::default()
    }

    pub fn is_keyword(&self, w: &str) -> bool {
        self.todo.iter().chain(&self.done).any(|k| k == w)
    }

    pub fn is_done(&self, w: &str) -> bool {
        self.done.iter().any(|k| k == w)
    }

    /// The keyword after `current` in the cycle (None: no keyword).
    pub fn next(&self, current: Option<&str>, forward: bool) -> Option<String> {
        let all: Vec<&String> = self.todo.iter().chain(&self.done).collect();
        let n = all.len() as isize;
        let i = current
            .and_then(|c| all.iter().position(|k| *k == c))
            .map_or(-1, |i| i as isize);
        let j = if forward {
            i + 1
        } else if i == -1 {
            n - 1
        } else {
            i - 1
        };
        (0..n).contains(&j).then(|| all[j as usize].clone())
    }
}

/// A parsed heading line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    pub keyword: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
}

/// Every folding word `#+STARTUP:` and `org-startup-folded` take.
pub const STARTUP_FOLDED_MODES: &[&str] = &[
    "overview",
    "content",
    "contents",
    "showall",
    "show-all",
    "showeverything",
    "everything",
    "all",
    "nil",
    "show2levels",
    "show3levels",
    "show4levels",
    "show5levels",
    "folded",
];

/// `show2levels` … `show5levels` to its number; anything else to None.
pub fn show_levels(mode: &str) -> Option<usize> {
    mode.strip_prefix("show")
        .and_then(|rest| rest.strip_suffix("levels"))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (2..=5).contains(n))
}

/// The number of stars if `line` is a heading ("* ", "** x"; a lone "*"
/// too).
pub fn heading_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    (stars > 0 && matches!(line.as_bytes().get(stars), None | Some(b' ') | Some(b'\t')))
        .then_some(stars)
}

pub fn parse_heading(line: &str, kw: &Keywords) -> Option<Heading> {
    let level = heading_level(line)?;
    let mut rest = line[level..].trim();
    let mut keyword = None;
    if let Some(first) = rest.split_whitespace().next()
        && kw.is_keyword(first)
    {
        keyword = Some(first.to_string());
        rest = rest[first.len()..].trim_start();
    }
    let mut tags = Vec::new();
    if let Some(m) = TAGS.find(rest) {
        tags = m
            .as_str()
            .trim()
            .trim_matches(':')
            .split(':')
            .map(String::from)
            .collect();
        rest = rest[..m.start()].trim_end();
    }
    Some(Heading {
        level,
        keyword,
        title: rest.to_string(),
        tags,
    })
}

pub(crate) static TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)(:[\w@#%:]+:)\s*$").unwrap());
static ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([-+]|\d+[.)]|\*)\s+(?:(\[[ xX-]\])(?:\s|$))?").unwrap());
static FOOTREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[fn:[^\]\s]*\]").unwrap());
static CITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[cite(?:/[^\]:]*)?:[^\]]*\]").unwrap());
static MATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\$[^$]+\$\$|\\\([^)]*\\\)|\\\[[^\]]*\\\]|(?:^|[\s(\[{'\x22])\$[^\s$.,;:?!\x22'][^$]*?[^\s$.,;:\x22']\$(?:[\s.,;:?!\x22')\]}-]|$)").unwrap()
});
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[A-Za-z]+(?:\{\})?").unwrap());
static COOKIE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d*%|\d*/\d*)\]").unwrap());

/// A plain list item: (indentation, bullet, checkbox state if any).
/// Lines starting with "*" at column 0 are headings, not items.
pub fn list_item(line: &str) -> Option<(usize, String, Option<char>)> {
    let c = ITEM.captures(line)?;
    let indent = c[1].chars().count();
    if &c[2] == "*" && indent == 0 {
        return None;
    }
    let checkbox = c.get(3).map(|m| m.as_str().chars().nth(1).unwrap_or(' '));
    Some((indent, c[2].to_string(), checkbox))
}

/// The keys `#+TODO: TODO(t) NEXT(n) | DONE(d)` give the states, for
/// fast selection: `(keyword, key)`, in the order written.
pub fn fast_todo_keys(buf: &Buffer) -> Vec<(String, char)> {
    for l in 0..buf.line_count() {
        let line = buf.line(l);
        let t = line.trim_start();
        let Some(rest) = ["#+TODO:", "#+SEQ_TODO:", "#+TYP_TODO:"]
            .iter()
            .find_map(|p| {
                t.get(..p.len())
                    .filter(|h| h.eq_ignore_ascii_case(p))
                    .map(|_| &t[p.len()..])
            })
        else {
            continue;
        };
        let keys: Vec<(String, char)> = rest
            .split_whitespace()
            .filter_map(|w| {
                let (name, key) = w.split_once('(')?;
                let c = key.chars().next()?;
                (!name.is_empty() && key.contains(')')).then(|| (name.to_string(), c))
            })
            .collect();
        return keys;
    }
    Vec::new()
}

/// What `#+STARTUP:` says about logging: `logdone` / `lognotedone` and
/// `logrepeat` (on unless `nologrepeat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogOptions {
    /// Stamp `CLOSED:` when finishing.
    pub closed: bool,
    /// Also ask for a closing note.
    pub note: bool,
    /// Log a state change in the drawer when a repeating task is finished.
    pub repeat: bool,
}

static DEFAULT_LOG_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `org-log-done` for every file: stamp `CLOSED:` when finishing, unless a
/// file's `#+STARTUP:` says otherwise (`forge.opt.org_log_done`).
pub fn set_default_log_done(on: bool) {
    DEFAULT_LOG_DONE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn log_options(buf: &Buffer) -> LogOptions {
    let mut o = LogOptions {
        closed: DEFAULT_LOG_DONE.load(std::sync::atomic::Ordering::Relaxed),
        note: false,
        repeat: true,
    };
    for l in 0..buf.line_count() {
        let line = buf.line(l);
        let t = line.trim_start();
        let Some(rest) = t
            .get(..10)
            .filter(|h| h.eq_ignore_ascii_case("#+STARTUP:"))
            .map(|_| &t[10..])
        else {
            continue;
        };
        for w in rest.split_whitespace() {
            match w.to_ascii_lowercase().as_str() {
                "logdone" => (o.closed, o.note) = (true, false),
                "lognotedone" => (o.closed, o.note) = (true, true),
                "nologdone" => (o.closed, o.note) = (false, false),
                "logrepeat" | "lognoterepeat" => o.repeat = true,
                "nologrepeat" => o.repeat = false,
                _ => {}
            }
        }
    }
    o
}

/// The buffer's TODO keywords.
pub fn keywords(buf: &Buffer) -> Keywords {
    let lines: Vec<String> = (0..buf.line_count())
        .map(|l| buf.line(l))
        .filter(|l| l.trim_start().starts_with("#+"))
        .collect();
    Keywords::from_text(lines.iter().map(String::as_str))
}

/// What TAB did to a subtree / S-TAB to the buffer (for the echo area).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Folded,
    Children,
    Subtree,
    Overview,
    Contents,
    ShowAll,
    /// `#+STARTUP: show2levels` … `show5levels`: headings down to level N.
    Levels(usize),
}

impl Visibility {
    pub fn name(self) -> &'static str {
        match self {
            Self::Folded => "FOLDED",
            Self::Children => "CHILDREN",
            Self::Subtree => "SUBTREE",
            Self::Overview => "OVERVIEW",
            Self::Contents => "CONTENTS",
            Self::ShowAll => "SHOW ALL",
            Self::Levels(2) => "SHOW2LEVELS",
            Self::Levels(3) => "SHOW3LEVELS",
            Self::Levels(4) => "SHOW4LEVELS",
            Self::Levels(5) => "SHOW5LEVELS",
            Self::Levels(_) => "SHOW LEVELS",
        }
    }
}

impl Buffer {
    pub fn org_level(&self, line: usize) -> Option<usize> {
        heading_level(&self.line(line))
    }

    /// The heading `line` belongs to (itself if it's one).
    pub fn org_heading_at(&self, line: usize) -> Option<usize> {
        (0..=line.min(self.line_count().saturating_sub(1)))
            .rev()
            .find(|&l| self.org_level(l).is_some())
    }

    /// The last line of the subtree under heading `h`.
    pub fn org_subtree_end(&self, h: usize) -> usize {
        let level = self.org_level(h).unwrap_or(0);
        let next =
            (h + 1..self.line_count()).find(|&l| self.org_level(l).is_some_and(|n| n <= level));
        let mut end = next.unwrap_or(self.line_count()).saturating_sub(1);
        // A final empty line (the file's trailing newline) isn't content.
        while end > h && next.is_none() && self.line(end).is_empty() {
            end -= 1;
        }
        end
    }

    /// The last line of heading `h`'s own text (before any sub-heading).
    pub(crate) fn org_body_end(&self, h: usize) -> usize {
        let next = (h + 1..self.line_count()).find(|&l| self.org_level(l).is_some());
        match next {
            Some(n) => n - 1,
            None => self.org_subtree_end(h),
        }
    }

    /// Direct children of heading `h`.
    pub fn org_children(&self, h: usize) -> Vec<usize> {
        let level = self.org_level(h).unwrap_or(0);
        let end = self.org_subtree_end(h);
        let mut out = Vec::new();
        let mut child_level = usize::MAX;
        for l in h + 1..=end {
            if let Some(n) = self.org_level(l)
                && n > level
                && n <= child_level
            {
                child_level = n;
                out.push(l);
            }
        }
        out
    }

    fn fold_subtree(&mut self, h: usize) {
        let end = self.org_subtree_end(h);
        if end > h {
            self.fold_lines(h + 1, end);
        }
    }

    /// The `:END:` line of the drawer that starts at `line` (`:NAME:`),
    /// or None when `line` doesn't start a closed drawer.
    pub fn org_drawer_end(&self, line: usize) -> Option<usize> {
        let text = self.line(line);
        let t = text.trim();
        let name = t.strip_prefix(':')?.strip_suffix(':')?;
        if name.is_empty()
            || name.eq_ignore_ascii_case("END")
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || "_-@#%".contains(c))
        {
            return None;
        }
        for l in line + 1..self.line_count() {
            let next = self.line(l);
            let n = next.trim();
            if n.eq_ignore_ascii_case(":END:") {
                return Some(l);
            }
            // A drawer holds no headings and no other drawer.
            if self.org_level(l).is_some() {
                return None;
            }
        }
        None
    }

    /// TAB on a drawer line: show or hide its contents. Some(true) when it
    /// is now hidden, None if `line` isn't a drawer.
    pub fn org_cycle_drawer(&mut self, line: usize) -> Option<bool> {
        let end = self.org_drawer_end(line)?;
        if self.line_folded(line) {
            self.unfold_lines(line, end);
            Some(false)
        } else {
            self.fold_lines(line + 1, end);
            Some(true)
        }
    }

    /// Collapse the drawers in `first..=last` that are on show, each to
    /// its first line.
    pub fn org_hide_drawers(&mut self, first: usize, last: usize) {
        let mut l = first;
        while l <= last {
            if !self.line_hidden(l)
                && let Some(end) = self.org_drawer_end(l)
            {
                self.fold_lines(l + 1, end);
                l = end;
            }
            l += 1;
        }
    }

    /// TAB on a heading: folded -> children -> subtree -> folded. None if
    /// `line` isn't a heading.
    pub fn org_cycle(&mut self, line: usize) -> Option<Visibility> {
        self.org_level(line)?;
        let end = self.org_subtree_end(line);
        if end == line {
            return Some(Visibility::Subtree); // nothing to hide
        }
        let hidden = (line + 1..=end).any(|l| self.line_hidden(l));
        Some(if self.line_folded(line) {
            self.unfold_lines(line, end);
            let children = self.org_children(line);
            for c in &children {
                self.fold_subtree(*c);
            }
            self.org_hide_drawers(line + 1, end);
            if children.is_empty() {
                Visibility::Subtree
            } else {
                Visibility::Children
            }
        } else if hidden {
            self.unfold_lines(line, end);
            self.org_hide_drawers(line + 1, end);
            Visibility::Subtree
        } else {
            self.fold_subtree(line);
            Visibility::Folded
        })
    }

    /// S-TAB: overview (top-level headings) -> contents (all headings) ->
    /// everything.
    pub fn org_global_cycle(&mut self) -> Visibility {
        let headings: Vec<usize> = (0..self.line_count())
            .filter(|&l| self.org_level(l).is_some())
            .collect();
        if self.folds().is_empty() {
            for &h in &headings {
                if self.org_level(h) == Some(1) {
                    self.fold_subtree(h);
                }
            }
            return Visibility::Overview;
        }
        let all_visible = headings.iter().all(|&h| !self.line_hidden(h));
        self.unfold_all();
        if all_visible {
            return Visibility::ShowAll;
        }
        for &h in &headings {
            let end = self.org_body_end(h);
            if end > h {
                self.fold_lines(h + 1, end);
            }
        }
        Visibility::Contents
    }

    /// C-c C-t: the next (or previous) TODO keyword on the heading at or
    /// above `line`. Returns the new keyword, or Err if there's no heading.
    pub fn org_todo(&mut self, line: usize, forward: bool) -> Result<Option<String>, String> {
        self.org_todo_to(line, forward, None)
    }

    /// `org_todo`, or with `explicit` the given state (None: no keyword)
    /// instead of the next one in the sequence.
    pub fn org_todo_to(
        &mut self,
        line: usize,
        forward: bool,
        explicit: Option<Option<String>>,
    ) -> Result<Option<String>, String> {
        let h = self.org_heading_at(line).ok_or("not in an outline entry")?;
        let kw = keywords(self);
        let text = self.line(h);
        let head = parse_heading(&text, &kw).expect("a heading");
        let next = match explicit {
            Some(state) => state,
            None => kw.next(head.keyword.as_deref(), forward),
        };
        let stars = head.level;
        // The keyword sits after the stars and one space.
        let after_stars = text[stars..].len() - text[stars..].trim_start().len();
        let start = stars + after_stars;
        let old_len = head.keyword.as_ref().map_or(0, |k| {
            k.len() + usize::from(text[start + k.len()..].starts_with(' '))
        });
        let new = next.as_ref().map_or(String::new(), |k| format!("{k} "));
        let (a, z) = (
            text[..start].chars().count(),
            text[..start + old_len].chars().count(),
        );
        let point = self.point_offset();
        let line_start = self.line_start_offset(h);
        let prefix = if after_stars == 0 { " " } else { "" };
        self.replace_range(Pos::new(h, a), Pos::new(h, z), &format!("{prefix}{new}"));
        // Keep point on the same text.
        let delta = (prefix.len() + new.chars().count()) as isize - (z - a) as isize;
        let p = if point >= line_start + z {
            (point as isize + delta) as usize
        } else {
            point.min(line_start + a)
        };
        self.set_offset(p);
        self.org_update_cookies();
        Ok(next)
    }

    /// C-c C-c on a list item with a checkbox: toggle it. Returns false
    /// if `line` has none. Cookies are recounted either way.
    pub fn org_toggle_checkbox(&mut self, line: usize) -> bool {
        let text = self.line(line);
        let toggled = match ITEM.captures(&text).and_then(|c| c.get(3)) {
            Some(m) => {
                let state = text[m.start() + 1..].chars().next().unwrap_or(' ');
                let col = text[..m.start() + 1].chars().count();
                let point = self.point_offset();
                self.replace_range(
                    Pos::new(line, col),
                    Pos::new(line, col + 1),
                    if state == ' ' { "X" } else { " " },
                );
                self.set_offset(point);
                true
            }
            None => false,
        };
        self.org_update_cookies();
        toggled
    }

    /// Recount every statistics cookie: on a heading, its checkboxes if
    /// its text has any, else its children's TODO keywords; on a list
    /// item, the checkboxes of the items under it. Returns how many
    /// changed.
    pub fn org_update_cookies(&mut self) -> usize {
        let kw = keywords(self);
        let mut changed = 0;
        for l in 0..self.line_count() {
            let text = self.line(l);
            let Some(m) = COOKIE.find(&text) else {
                continue;
            };
            let (done, total) = if let Some(level) = heading_level(&text) {
                let _ = level;
                let body_end = self.org_body_end(l);
                let boxes: Vec<char> = (l + 1..=body_end)
                    .filter_map(|b| list_item(&self.line(b)).and_then(|i| i.2))
                    .collect();
                if boxes.is_empty() {
                    let kids: Vec<Heading> = self
                        .org_children(l)
                        .into_iter()
                        .filter_map(|c| parse_heading(&self.line(c), &kw))
                        .collect();
                    let with_kw: Vec<&Heading> =
                        kids.iter().filter(|h| h.keyword.is_some()).collect();
                    (
                        with_kw
                            .iter()
                            .filter(|h| kw.is_done(h.keyword.as_deref().unwrap_or("")))
                            .count(),
                        with_kw.len(),
                    )
                } else {
                    (
                        boxes.iter().filter(|&&c| c == 'X' || c == 'x').count(),
                        boxes.len(),
                    )
                }
            } else if let Some((indent, _, _)) = list_item(&text) {
                // Checkboxes in this item's sub-list: every item more
                // indented than this one, at any depth, up to the next
                // item at this level or less (or the next heading).
                let mut boxes = Vec::new();
                for b in l + 1..self.line_count() {
                    let t = self.line(b);
                    if t.trim().is_empty() {
                        continue;
                    }
                    if heading_level(&t).is_some() {
                        break;
                    }
                    let ind = t.chars().take_while(|c| c.is_whitespace()).count();
                    if ind <= indent {
                        break;
                    }
                    if let Some((_, _, Some(c))) = list_item(&t) {
                        boxes.push(c);
                    }
                }
                (
                    boxes.iter().filter(|&&c| c == 'X' || c == 'x').count(),
                    boxes.len(),
                )
            } else {
                continue;
            };
            let new = if m.as_str().contains('%') {
                format!("[{}%]", (done * 100).checked_div(total).unwrap_or(0))
            } else {
                format!("[{done}/{total}]")
            };
            if new != m.as_str() {
                let (a, z) = (
                    text[..m.start()].chars().count(),
                    text[..m.end()].chars().count(),
                );
                let point = self.point_offset();
                let line_start = self.line_start_offset(l);
                self.replace_range(Pos::new(l, a), Pos::new(l, z), &new);
                let delta = new.chars().count() as isize - (z - a) as isize;
                self.set_offset(if point > line_start + a {
                    (point as isize + delta).max(0) as usize
                } else {
                    point
                });
                changed += 1;
            }
        }
        changed
    }

    /// M-RET: on a list item, a new item after it (same bullet, an empty
    /// checkbox if it had one); elsewhere a new heading at the current
    /// level after the current subtree. Point ends up ready to type.
    pub fn org_insert_heading_or_item(&mut self, line: usize) {
        if heading_level(&self.line(line)).is_none() && self.org_insert_item_after(line, false) {
            return;
        }
        let (level, end) = match self.org_heading_at(line) {
            Some(h) => (self.org_level(h).unwrap_or(1), self.org_subtree_end(h)),
            None => (1, self.line_count().saturating_sub(1)),
        };
        let at = self.line_end_offset(end);
        let stars = "*".repeat(level);
        if self.line(end).is_empty() && end + 1 >= self.line_count() {
            self.replace_offsets(at, at, &format!("{stars} "));
        } else {
            self.replace_offsets(at, at, &format!("\n{stars} "));
        }
    }

    /// Promote (`delta` < 0) or demote a heading, or its whole subtree.
    /// Items are indented by two columns instead. False if `line` is
    /// neither, or a heading would go above level 1.
    pub fn org_shift_level(&mut self, line: usize, delta: isize, subtree: bool) -> bool {
        let point = self.point_offset();
        let before = self.char_count();
        let ok = if let Some(level) = self.org_level(line) {
            if delta < 0 && level == 1 {
                return false;
            }
            let last = if subtree {
                self.org_subtree_end(line)
            } else {
                line
            };
            for l in line..=last {
                if self.org_level(l).is_some() {
                    if delta < 0 {
                        self.replace_range(Pos::new(l, 0), Pos::new(l, 1), "");
                    } else {
                        self.replace_range(Pos::new(l, 0), Pos::new(l, 0), "*");
                    }
                }
            }
            true
        } else if let Some((indent, _, _)) = list_item(&self.line(line)) {
            if delta < 0 && indent == 0 {
                return false;
            }
            if delta < 0 {
                let n = indent.min(2);
                self.replace_range(Pos::new(line, 0), Pos::new(line, n), "");
            } else {
                self.replace_range(Pos::new(line, 0), Pos::new(line, 0), "  ");
            }
            true
        } else {
            false
        };
        let line_start = self.line_start_offset(line);
        let shift = self.char_count() as isize - before as isize;
        let p = if point > line_start && !subtree {
            (point as isize + shift).max(line_start as isize) as usize
        } else {
            point
        };
        self.set_offset(p.min(self.char_count()));
        ok
    }

    /// M-x org-convert-to-odd-levels: rewrite every heading so levels are
    /// 1, 3, 5, ... (level L becomes `2L-1`), the double-star layout Org
    /// shows with `#+STARTUP: odd`. Returns how many headings changed.
    pub fn org_convert_to_odd_levels(&mut self) -> usize {
        self.org_convert_levels(|l| 2 * l - 1)
    }

    /// M-x org-convert-to-oddeven-levels: the inverse, consecutive levels
    /// 1, 2, 3, ... (level L becomes `ceil(L/2)`). Returns how many
    /// headings changed.
    pub fn org_convert_to_oddeven_levels(&mut self) -> usize {
        self.org_convert_levels(|l| l.div_ceil(2))
    }

    /// Rewrite every heading's star count with `new_level`. Headings keep
    /// their line, so point follows the text it was on.
    fn org_convert_levels(&mut self, new_level: impl Fn(usize) -> usize) -> usize {
        let point = self.point();
        let mut changed = 0;
        for l in 0..self.line_count() {
            let line = self.line(l);
            let Some(level) = heading_level(&line) else {
                continue;
            };
            let new = new_level(level);
            if new == 0 || new == level {
                continue;
            }
            let rebuilt = format!("{}{}", "*".repeat(new), &line[level..]);
            self.org_set_line(l, &rebuilt);
            changed += 1;
        }
        if point.line < self.line_count() {
            let col = point.col.min(self.line_len(point.line));
            self.set_point(Pos::new(point.line, col));
        }
        changed
    }

    /// M-up / M-down: swap the subtree at `line` with the previous or next
    /// sibling. Returns false if there's none.
    pub fn org_move_subtree(&mut self, line: usize, down: bool) -> bool {
        let Some(h) = self.org_heading_at(line) else {
            return false;
        };
        let level = self.org_level(h).expect("a heading");
        let end = self.org_subtree_end(h);
        let (a_start, a_end, b_start, b_end) = if down {
            let next = end + 1;
            if next >= self.line_count() || self.org_level(next) != Some(level) {
                return false;
            }
            (h, end, next, self.org_subtree_end(next))
        } else {
            let prev = (0..h)
                .rev()
                .find(|&l| self.org_level(l).is_some_and(|n| n <= level));
            let Some(prev) = prev.filter(|&p| self.org_level(p) == Some(level)) else {
                return false;
            };
            (prev, h - 1, h, end)
        };
        let point = self.point_offset() - self.line_start_offset(h);
        let first = self.text_range(Pos::new(a_start, 0), Pos::new(a_end, self.line_len(a_end)));
        let second = self.text_range(Pos::new(b_start, 0), Pos::new(b_end, self.line_len(b_end)));
        self.unfold_lines(a_start, b_end);
        self.replace_range(
            Pos::new(a_start, 0),
            Pos::new(b_end, self.line_len(b_end)),
            &format!("{second}\n{first}"),
        );
        // Point stays in the subtree it was in.
        let moved_to = if down {
            a_start + (b_end - b_start + 1)
        } else {
            a_start
        };
        self.set_offset(self.line_start_offset(moved_to) + point);
        true
    }

    /// The first heading after (`forward`) or before `line` at `level` or
    /// any level (None). For C-c C-n / C-c C-p / C-c C-f / C-c C-b.
    pub fn org_next_heading(&self, line: usize, forward: bool, same_level: bool) -> Option<usize> {
        let level = if same_level {
            self.org_heading_at(line).and_then(|h| self.org_level(h))
        } else {
            None
        };
        let ok = |l: usize| match (self.org_level(l), level) {
            (Some(n), Some(want)) => n == want,
            (Some(_), None) => !self.line_hidden(l),
            _ => false,
        };
        if forward {
            // Siblings stop at the end of the parent.
            let limit = match (same_level, self.org_heading_at(line)) {
                (true, Some(h)) => (h + 1..self.line_count())
                    .find(|&l| self.org_level(l).is_some_and(|n| n < level.unwrap_or(1))),
                _ => None,
            };
            (line + 1..limit.unwrap_or(self.line_count())).find(|&l| ok(l))
        } else {
            let start = if same_level {
                self.org_heading_at(line).unwrap_or(line)
            } else {
                line
            };
            let parent = |l: usize| {
                self.org_level(l)
                    .is_some_and(|n| level.is_some_and(|want| n < want))
            };
            for l in (0..start).rev() {
                if same_level && parent(l) {
                    return None;
                }
                if ok(l) {
                    return Some(l);
                }
            }
            None
        }
    }

    /// C-c C-u: the parent heading.
    pub fn org_up_heading(&self, line: usize) -> Option<usize> {
        let h = self.org_heading_at(line)?;
        let level = self.org_level(h)?;
        (0..h)
            .rev()
            .find(|&l| self.org_level(l).is_some_and(|n| n < level))
    }

    // ------------------------------------------- Document Structure (Phase 11.1)

    /// `org-up-element`: the parent element of `line`: the enclosing list
    /// item when inside a nested item, else the parent heading.
    pub fn org_up_element(&self, line: usize) -> Option<usize> {
        let line = line.min(self.line_count().saturating_sub(1));
        if let Some((indent, _, _)) = list_item(&self.line(line)) {
            // Nearest item above with a smaller indent.
            for l in (0..line).rev() {
                let t = self.line(l);
                if self.org_level(l).is_some() {
                    return self.org_heading_at(l);
                }
                if let Some((i, _, _)) = list_item(&t)
                    && i < indent
                {
                    return Some(l);
                }
                if !t.trim().is_empty()
                    && t.chars().take_while(|c| c.is_whitespace()).count() < indent
                    && heading_level(&t).is_none()
                    && list_item(&t).is_none()
                {
                    // A paragraph line outdented past the item: stop at heading.
                    return self.org_heading_at(line);
                }
            }
            return self.org_heading_at(line);
        }
        self.org_up_heading(line)
    }

    /// Targets for `org-goto`: (line, label) for every heading.
    pub fn org_goto_targets(&self) -> Vec<(usize, String)> {
        (0..self.line_count())
            .filter_map(|l| {
                let level = self.org_level(l)?;
                Some((
                    l,
                    format!("{}{}", "  ".repeat(level - 1), self.line(l).trim()),
                ))
            })
            .collect()
    }

    /// `org-goto-sibling`: next/previous heading at the same level.
    pub fn org_goto_sibling(&self, line: usize, forward: bool) -> Option<usize> {
        self.org_next_heading(line, forward, true)
    }

    /// `org-mark-subtree`: the (first, last) lines of the subtree at `line`.
    /// None when `line` is in no heading.
    pub fn org_mark_subtree(&self, line: usize) -> Option<(usize, usize)> {
        let h = self.org_heading_at(line)?;
        Some((h, self.org_subtree_end(h)))
    }

    /// The text of the subtree at heading `h` (lines joined, no trailing newline).
    pub fn org_subtree_text(&self, h: usize) -> String {
        let end = self.org_subtree_end(h);
        self.text_range(Pos::new(h, 0), Pos::new(end, self.line_len(end)))
    }

    /// `org-tree-to-indirect-buffer`: (buffer name, text) for the subtree
    /// at `line`. None when outside any heading.
    pub fn org_tree_to_indirect_text(&self, line: usize) -> Option<(String, String)> {
        let h = self.org_heading_at(line)?;
        let kw = keywords(self);
        let title = parse_heading(&self.line(h), &kw)
            .map(|x| x.title)
            .unwrap_or_default();
        let name = if title.is_empty() {
            self.line(h).trim().to_string()
        } else {
            title
        };
        Some((name, self.org_subtree_text(h)))
    }

    /// `org-cycle-set-startup-visibility`: fold according to `mode`
    /// ("overview", "content"/"contents", "showall"/"showeverything",
    /// "show2levels" … "show5levels", "folded"). Unknown modes fold
    /// every subtree. Returns the resulting visibility label.
    pub fn org_cycle_set_startup_visibility(&mut self, mode: &str) -> Visibility {
        let mode = mode.to_ascii_lowercase();
        match mode.as_str() {
            "overview" => {
                self.unfold_all();
                for h in 0..self.line_count() {
                    if self.org_level(h) == Some(1) {
                        self.fold_subtree(h);
                    }
                }
                Visibility::Overview
            }
            "content" | "contents" => {
                self.unfold_all();
                for h in 0..self.line_count() {
                    if self.org_level(h).is_some() {
                        let end = self.org_body_end(h);
                        if end > h {
                            self.fold_lines(h + 1, end);
                        }
                    }
                }
                Visibility::Contents
            }
            "showall" | "show-all" | "showeverything" | "everything" | "all" | "nil" => {
                self.unfold_all();
                Visibility::ShowAll
            }
            _ => match show_levels(&mode) {
                // `show2levels` …: headings down to level N stay visible.
                Some(n) => {
                    self.unfold_all();
                    for h in 0..self.line_count() {
                        if self.org_level(h) == Some(n) {
                            self.fold_subtree(h);
                        }
                    }
                    Visibility::Levels(n)
                }
                // "folded" and anything unknown: every subtree folded.
                None => {
                    self.unfold_all();
                    for h in 0..self.line_count() {
                        if self.org_level(h).is_some() {
                            self.fold_subtree(h);
                        }
                    }
                    Visibility::Folded
                }
            },
        }
    }

    /// `#+STARTUP:` words in the buffer, in order, lowercased: folding
    /// ("overview", "show2levels", …), indent ("indent", "noindent") and
    /// anything else (ignored here). Later words win per axis.
    pub fn startup_options(&self) -> Vec<String> {
        let mut out = Vec::new();
        for l in 0..self.line_count() {
            let line = self.line(l);
            let t = line.trim_start();
            let Some(rest) = ["#+STARTUP:"].iter().find_map(|p| {
                t.get(..p.len())
                    .filter(|h| h.eq_ignore_ascii_case(p))
                    .map(|_| &t[p.len()..])
            }) else {
                continue;
            };
            out.extend(rest.split_whitespace().map(|w| w.to_ascii_lowercase()));
        }
        out
    }

    /// Visiting an Org file: fold it and set indent mode from its
    /// `#+STARTUP:` keywords, falling back to the configured defaults.
    /// Later keywords win per axis (`indent`/`noindent`, one folding word).
    pub fn apply_org_startup(&mut self, folded_default: &str, indented_default: bool) {
        let mut folded = folded_default.to_ascii_lowercase();
        let mut indented = indented_default;
        for o in self.startup_options() {
            match o.as_str() {
                "indent" => indented = true,
                "noindent" => indented = false,
                w if STARTUP_FOLDED_MODES.contains(&w) => folded = w.to_string(),
                _ => {}
            }
        }
        self.org_cycle_set_startup_visibility(&folded);
        self.org_display.indent = indented;
    }

    /// `org-fold-show-all`: show everything.
    pub fn org_fold_show_all(&mut self) {
        self.unfold_all();
    }
    /// `org-reveal`: open the folds around point. True if any opened.
    pub fn org_reveal_at_point(&mut self) -> bool {
        self.reveal(self.point_offset())
    }

    /// `org-show-entry`: show the body of the entry at `line` (not its children).
    pub fn org_show_entry(&mut self, line: usize) {
        if let Some(h) = self.org_heading_at(line) {
            let end = self.org_body_end(h);
            self.unfold_lines(h, end);
        }
    }

    /// `org-show-children`: show the heading's direct children (as folded headings).
    pub fn org_show_children(&mut self, line: usize) {
        let Some(h) = self.org_heading_at(line) else {
            return;
        };
        let end = self.org_subtree_end(h);
        self.unfold_lines(h, end);
        for c in self.org_children(h) {
            self.fold_subtree(c);
        }
    }

    /// `org-show-subtree`: show the whole subtree.
    pub fn org_show_subtree(&mut self, line: usize) {
        if let Some(h) = self.org_heading_at(line) {
            let end = self.org_subtree_end(h);
            self.unfold_lines(h, end);
        }
    }

    /// `org-hide-subtree`: hide the subtree. False when there is nothing to hide.
    pub fn org_hide_subtree(&mut self, line: usize) -> bool {
        if let Some(h) = self.org_heading_at(line) {
            let end = self.org_subtree_end(h);
            if end > h {
                self.fold_lines(h + 1, end);
                return true;
            }
        }
        false
    }

    /// Is the subtree at `h` archived (`:ARCHIVE:` tag on it or an ancestor)?
    pub fn org_is_archived(&self, h: usize) -> bool {
        let kw = keywords(self);
        let mut cur = Some(h);
        while let Some(c) = cur {
            if let Some(head) = parse_heading(&self.line(c), &kw)
                && head.tags.iter().any(|t| t.eq_ignore_ascii_case("ARCHIVE"))
            {
                return true;
            }
            cur = self.org_up_heading(c);
        }
        false
    }

    /// `org-cycle-force-archived`: like `org-cycle`, but archived subtrees
    /// are shown too (ours skips nothing, so reveal the subtree fully).
    pub fn org_cycle_force_archived(&mut self, line: usize) -> Option<Visibility> {
        let v = self.org_cycle(line)?;
        if let Some(h) = self.org_heading_at(line)
            && self.org_is_archived(h)
        {
            let end = self.org_subtree_end(h);
            self.unfold_lines(h, end);
            return Some(Visibility::Subtree);
        }
        Some(v)
    }

    fn org_insert_heading_at(&mut self, after: usize, level: usize, keyword: Option<&str>) {
        let stars = "*".repeat(level.max(1));
        let kw = match keyword {
            Some(k) => format!("{k} "),
            None => String::new(),
        };
        let at = self.line_end_offset(after.min(self.line_count().saturating_sub(1)));
        if self.line(after).is_empty() && after + 1 >= self.line_count() {
            self.replace_offsets(at, at, &format!("{stars} {kw}"));
        } else {
            self.replace_offsets(at, at, &format!("\n{stars} {kw}"));
        }
    }

    /// `org-insert-heading`: a new heading after the current subtree.
    pub fn org_insert_heading(&mut self, line: usize) {
        let (level, end) = match self.org_heading_at(line) {
            Some(h) => (self.org_level(h).unwrap_or(1), self.org_subtree_end(h)),
            None => (1, self.line_count().saturating_sub(1)),
        };
        self.org_insert_heading_at(end, level, None);
    }

    /// `org-insert-heading-respect-content`: after the entry's own text,
    /// before its first child.
    pub fn org_insert_heading_respect_content(&mut self, line: usize) {
        let (level, after) = match self.org_heading_at(line) {
            Some(h) => (self.org_level(h).unwrap_or(1), self.org_body_end(h)),
            None => (1, self.line_count().saturating_sub(1)),
        };
        self.org_insert_heading_at(after, level, None);
    }

    fn org_first_todo_keyword(&self) -> Option<String> {
        let kw = keywords(self);
        kw.todo.first().cloned()
    }

    /// `org-insert-todo-heading` / `...-respect-content`.
    pub fn org_insert_todo_heading(&mut self, line: usize, respect_content: bool) {
        // On a list item the "TODO" is a checkbox.
        if heading_level(&self.line(line)).is_none() && self.org_insert_item_after(line, true) {
            return;
        }
        let todo = self.org_first_todo_keyword();
        let (level, after) = match self.org_heading_at(line) {
            Some(h) => (
                self.org_level(h).unwrap_or(1),
                if respect_content {
                    self.org_body_end(h)
                } else {
                    self.org_subtree_end(h)
                },
            ),
            None => (1, self.line_count().saturating_sub(1)),
        };
        self.org_insert_heading_at(after, level, todo.as_deref());
    }

    /// `org-do-promote` / `org-do-demote`: one heading line.
    pub fn org_do_promote(&mut self, line: usize) -> bool {
        self.org_shift_level(line, -1, false)
    }

    pub fn org_do_demote(&mut self, line: usize) -> bool {
        self.org_shift_level(line, 1, false)
    }

    /// `org-promote-subtree` / `org-demote-subtree`.
    pub fn org_promote_subtree(&mut self, line: usize) -> bool {
        self.org_shift_level(line, -1, true)
    }

    pub fn org_demote_subtree(&mut self, line: usize) -> bool {
        self.org_shift_level(line, 1, true)
    }

    /// `org-move-subtree-up` / `org-move-subtree-down`.
    pub fn org_move_subtree_up(&mut self, line: usize) -> bool {
        self.org_move_subtree(line, false)
    }

    pub fn org_move_subtree_down(&mut self, line: usize) -> bool {
        self.org_move_subtree(line, true)
    }

    /// `org-sort`: sort the child subtrees of the heading at `line`
    /// alphabetically by title (case-insensitive). Before the first
    /// heading, the top-level headings are sorted instead.
    /// `reverse` with C-u. Returns how many entries were sorted.
    pub fn org_sort_children(&mut self, line: usize, reverse: bool) -> usize {
        let kids = match self.org_heading_at(line) {
            Some(h) => self.org_children(h),
            // Before any heading: sort the top-level headings.
            None => (0..self.line_count())
                .filter(|&l| self.org_level(l) == Some(1) && self.org_up_heading(l).is_none())
                .collect(),
        };
        if kids.len() < 2 {
            return 0;
        }
        let kw = keywords(self);
        let mut blocks: Vec<(String, String)> = kids
            .iter()
            .map(|&c| {
                let title = parse_heading(&self.line(c), &kw)
                    .map(|x| x.title.to_lowercase())
                    .unwrap_or_default();
                (title, self.org_subtree_text(c))
            })
            .collect();
        blocks.sort_by(|a, b| a.0.cmp(&b.0));
        if reverse {
            blocks.reverse();
        }
        let first = kids[0];
        let last = self.org_subtree_end(*kids.last().expect("kids"));
        self.unfold_lines(first, last);
        let text = blocks
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.replace_range(
            Pos::new(first, 0),
            Pos::new(last, self.line_len(last)),
            &text,
        );
        kids.len()
    }

    /// `org-copy-subtree`: the subtree text (for the kill ring).
    pub fn org_copy_subtree(&self, line: usize) -> Option<String> {
        let h = self.org_heading_at(line)?;
        Some(self.org_subtree_text(h))
    }

    /// `org-cut-subtree`: delete the subtree, returning its text.
    pub fn org_cut_subtree(&mut self, line: usize) -> Option<String> {
        let h = self.org_heading_at(line)?;
        let end = self.org_subtree_end(h);
        let text = self.org_subtree_text(h);
        self.unfold_lines(h, end);
        let start = self.line_start_offset(h);
        let after = if end + 1 < self.line_count() {
            self.line_start_offset(end + 1)
        } else {
            self.char_count()
        };
        // Avoid leaving a leading blank gap: also eat one preceding newline
        // when cutting at end of buffer.
        let (s, e) = if end + 1 >= self.line_count() && h > 0 {
            (self.line_end_offset(h - 1), after)
        } else {
            (start, after)
        };
        self.replace_offsets(s, e, "");
        let lc = self.line_count();
        self.set_point(Pos::new(h.min(lc.saturating_sub(1)), 0));
        Some(text)
    }

    /// `org-paste-subtree` / `org-yank`: insert subtree `text` after the
    /// current subtree (or at `line` when outside any heading).
    pub fn org_paste_subtree(&mut self, line: usize, text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let end = match self.org_heading_at(line) {
            Some(h) => self.org_subtree_end(h),
            None => line.min(self.line_count().saturating_sub(1)),
        };
        let at = self.line_end_offset(end);
        // Separate from surrounding text with blank handling like Org.
        let sep = if self.line(end).trim().is_empty() {
            ""
        } else {
            "\n"
        };
        self.replace_offsets(at, at, &format!("{sep}{text}"));
        true
    }

    /// `org-toggle-heading`: heading <-> list item.
    pub fn org_toggle_heading(&mut self, line: usize) -> bool {
        if let Some(level) = self.org_level(line) {
            // Heading -> "- title" (drop stars, keep keyword+title, drop tags).
            let kw = keywords(self);
            let head = parse_heading(&self.line(line), &kw);
            let rest = match head {
                Some(h) => {
                    let k = h.keyword.map(|k| format!("{k} ")).unwrap_or_default();
                    format!("{k}{}", h.title)
                }
                None => self.line(line)[level..].trim_start().to_string(),
            };
            let indent = "  ".repeat(level.saturating_sub(1));
            self.replace_range(
                Pos::new(line, 0),
                Pos::new(line, self.line_len(line)),
                &format!("{indent}- {rest}"),
            );
            return true;
        }
        if let Some((indent, _, _)) = list_item(&self.line(line)) {
            // Item -> heading one deeper than the enclosing heading.
            let depth = self
                .org_heading_at(line)
                .and_then(|h| self.org_level(h))
                .unwrap_or(0);
            let level = (depth + 1).max(1);
            let text = self.line(line);
            let body = ITEM.replace(&text, "").trim_start().to_string();
            let _ = indent;
            self.replace_range(
                Pos::new(line, 0),
                Pos::new(line, self.line_len(line)),
                &format!("{} {body}", "*".repeat(level)),
            );
            return true;
        }
        false
    }

    /// `org-clone-subtree-with-time-shift`: duplicate the subtree below
    /// itself, shifting every YYYY-MM-DD date by `days`.
    pub fn org_clone_subtree_with_time_shift(&mut self, line: usize, days: i64) -> bool {
        let Some(h) = self.org_heading_at(line) else {
            return false;
        };
        let end = self.org_subtree_end(h);
        let text = self.org_subtree_text(h);
        let shifted = shift_org_dates(&text, days);
        let at = self.line_end_offset(end);
        let sep = if self.line(end).trim().is_empty() {
            ""
        } else {
            "\n"
        };
        self.replace_offsets(at, at, &format!("{sep}{shifted}"));
        true
    }

    // ------------------------------------------------ sparse trees

    fn org_line_matches(&self, line: usize, re: &Regex) -> bool {
        re.is_match(&self.line(line))
    }

    /// `org-sparse-tree` / `org-match-sparse-tree`: show only subtrees
    /// containing `pattern` (regexp, else literal substring). Returns the
    /// number of matching lines.
    pub fn org_sparse_tree(&mut self, pattern: &str) -> usize {
        let re = Regex::new(pattern)
            .or_else(|_| Regex::new(&regex::escape(pattern)))
            .unwrap_or_else(|_| Regex::new(".*").expect(".*"));
        let n = self.line_count();
        let mut matched = vec![false; n];
        let mut count = 0;
        for (l, m) in matched.iter_mut().enumerate() {
            if self.org_line_matches(l, &re) {
                *m = true;
                count += 1;
            }
        }
        if count == 0 {
            return 0;
        }
        self.org_apply_sparse(&matched);
        count
    }

    /// Show only the entries whose lines are marked in `matched` (and the
    /// headings leading to them); fold the rest. Shared by the sparse
    /// tree commands (regexp, tags, deadlines).
    pub fn org_apply_sparse(&mut self, matched: &[bool]) {
        let n = self.line_count();
        let mut matched = matched.to_vec();
        matched.resize(n, false);
        let mut contains = vec![false; n];
        for h in (0..n).rev() {
            if matched[h] {
                contains[h] = true;
            }
            if self.org_level(h).is_some() {
                let end = self.org_subtree_end(h);
                if (h..=end)
                    .any(|l| matched[l] || (l != h && self.org_level(l).is_some() && contains[l]))
                {
                    contains[h] = true;
                }
            }
        }
        self.unfold_all();
        // Fold deepest-first every heading subtree without a match.
        let mut heads: Vec<usize> = (0..n).filter(|&l| self.org_level(l).is_some()).collect();
        heads.sort_by_key(|&h| std::cmp::Reverse(h));
        for h in heads {
            if h < self.line_count() && !contains[h] {
                let end = self
                    .org_subtree_end(h)
                    .min(self.line_count().saturating_sub(1));
                if end > h {
                    self.fold_lines(h + 1, end);
                }
            } else if h < self.line_count() {
                // Keep the path open but fold non-matching child subtrees.
                for c in self.org_children(h) {
                    if c < self.line_count() && !contains[c] {
                        let end = self
                            .org_subtree_end(c)
                            .min(self.line_count().saturating_sub(1));
                        if end > c {
                            self.fold_lines(c + 1, end);
                        }
                    }
                }
            }
        }
    }

    /// `org-occur`: lines matching `pattern`, like `occur` but for Org
    /// (headings and body alike). Err on a bad regexp.
    pub fn org_occur_lines(&self, pattern: &str) -> Result<Vec<(usize, String)>, String> {
        let re = Regex::new(pattern).map_err(|e| e.to_string())?;
        Ok((0..self.line_count())
            .filter(|&l| re.is_match(&self.line(l)))
            .map(|l| (l, self.line(l)))
            .collect())
    }

    // ------------------------------------------------ plain lists

    // ------------------------------------------------ plain list items

    /// A new item after the item at `line` and everything under it, with
    /// the same bullet (numbers go up by one and the items after renumber).
    /// `checkbox` forces an empty checkbox, as M-S-RET does. False when
    /// `line` isn't an item.
    pub fn org_insert_item_after(&mut self, line: usize, checkbox: bool) -> bool {
        let Some((indent, bullet, had_box)) = list_item(&self.line(line)) else {
            return false;
        };
        let Some((_, first, last)) = self.org_item_range(line) else {
            return false;
        };
        let bullet = match bullet.trim_end_matches(['.', ')']).parse::<u64>() {
            Ok(n) => format!("{}{}", n + 1, &bullet[bullet.len() - 1..]),
            Err(_) => bullet,
        };
        let item = format!(
            "\n{}{} {}",
            " ".repeat(indent),
            bullet,
            if had_box.is_some() || checkbox {
                "[ ] "
            } else {
                ""
            }
        );
        let end = self.line_end_offset(last);
        self.replace_offsets(end, end, &item);
        self.org_renumber_list(first);
        // Renumbering rewrites text but not lines: point is on the new item.
        let new_line = last + 1;
        let col = self.line(new_line).chars().count();
        self.set_point(Pos::new(new_line, col));
        true
    }

    /// The first lines of the items that share the list level of the item
    /// at `line` (the item itself among them), top to bottom.
    pub fn org_item_siblings(&self, line: usize) -> Vec<usize> {
        let Some((indent, _, _)) = list_item(&self.line(line)) else {
            return Vec::new();
        };
        let belongs = |t: &str| -> Option<bool> {
            // Some(true): a sibling; Some(false): part of the list, deeper
            // or a continuation; None: the list ended.
            if t.trim().is_empty() {
                return Some(false);
            }
            if heading_level(t).is_some() {
                return None;
            }
            match list_item(t) {
                Some((i, _, _)) if i == indent => Some(true),
                Some((i, _, _)) if i > indent => Some(false),
                Some(_) => None,
                None => {
                    let ind = t.chars().take_while(|c| c.is_whitespace()).count();
                    (ind > indent).then_some(false)
                }
            }
        };
        let mut first = line;
        let mut l = line;
        let mut blanks = 0;
        while l > 0 {
            l -= 1;
            let t = self.line(l);
            blanks = if t.trim().is_empty() { blanks + 1 } else { 0 };
            if blanks > 1 {
                break;
            }
            match belongs(&t) {
                Some(true) => first = l,
                Some(false) => {}
                None => break,
            }
        }
        let mut out = vec![first];
        let mut blanks = 0;
        for l in first + 1..self.line_count() {
            let t = self.line(l);
            blanks = if t.trim().is_empty() { blanks + 1 } else { 0 };
            if blanks > 1 {
                break;
            }
            match belongs(&t) {
                Some(true) => out.push(l),
                Some(false) => {}
                None => break,
            }
        }
        out
    }

    /// The item before (or after) the one at `line` at the same level.
    pub fn org_neighbour_item(&self, line: usize, down: bool) -> Option<usize> {
        let sibs = self.org_item_siblings(line);
        let at = sibs.iter().rposition(|&s| s <= line)?;
        if down {
            sibs.get(at + 1).copied()
        } else {
            at.checked_sub(1).map(|i| sibs[i])
        }
    }

    /// M-up / M-down on an item: swap it (with its children) with the
    /// neighbouring item. Point travels with it. False at either end.
    pub fn org_move_item(&mut self, line: usize, down: bool) -> bool {
        let sibs = self.org_item_siblings(line);
        let Some(at) = sibs.iter().rposition(|&s| s <= line) else {
            return false;
        };
        let (a, b) = if down {
            (at, at + 1)
        } else {
            (at.wrapping_sub(1), at)
        };
        if b >= sibs.len() || a == usize::MAX {
            return false;
        }
        let (a_first, b_first) = (sibs[a], sibs[b]);
        let a_last = self.org_item_range(a_first).map_or(a_first, |r| r.2);
        let b_last = self.org_item_range(b_first).map_or(b_first, |r| r.2);
        let lines = |me: &Self, x: usize, y: usize| -> Vec<String> {
            (x..=y).map(|l| me.line(l)).collect()
        };
        let block_a = lines(self, a_first, a_last);
        let gap = lines(self, a_last + 1, b_first.saturating_sub(1).max(a_last));
        let gap = if b_first > a_last + 1 {
            gap
        } else {
            Vec::new()
        };
        let block_b = lines(self, b_first, b_last);
        let mut out = block_b.clone();
        out.extend(gap.iter().cloned());
        out.extend(block_a.iter().cloned());
        let p = self.point();
        let from = Pos::new(a_first, 0);
        let to = Pos::new(b_last, self.line(b_last).chars().count());
        self.replace_range(from, to, &out.join("\n"));
        let moved_to = if down {
            a_first + block_b.len() + gap.len()
        } else {
            a_first
        };
        let moved_from = if down { a_first } else { b_first };
        let _ = moved_from;
        let offset = p.line.saturating_sub(if down { a_first } else { b_first });
        let col = p.col.min(self.line(moved_to + offset).chars().count());
        self.set_point(Pos::new(moved_to + offset, col));
        self.org_renumber_list(a_first);
        true
    }

    /// Number the ordered items at the level of the one at `line` 1, 2, 3 ...
    pub fn org_renumber_list(&mut self, line: usize) {
        let sibs = self.org_item_siblings(line);
        let mut n = 0u64;
        for l in sibs {
            let text = self.line(l);
            let Some((indent, bullet, _)) = list_item(&text) else {
                continue;
            };
            let Some(mark) = bullet.chars().last().filter(|c| matches!(c, '.' | ')')) else {
                return;
            };
            if bullet.trim_end_matches(['.', ')']).parse::<u64>().is_err() {
                return;
            }
            n += 1;
            let want = format!("{n}{mark}");
            if want != bullet {
                self.replace_range(
                    Pos::new(l, indent),
                    Pos::new(l, indent + bullet.chars().count()),
                    &want,
                );
            }
        }
    }

    /// S-left / S-right (and `C-c -`) on an item: cycle the bullet of the
    /// whole list level through `-`, `+`, `1.`, `1)`.
    pub fn org_cycle_bullet(&mut self, line: usize, backwards: bool) -> bool {
        let Some((_, bullet, _)) = list_item(&self.line(line)) else {
            return false;
        };
        const KINDS: [&str; 4] = ["-", "+", "1.", "1)"];
        let at = KINDS
            .iter()
            .position(|k| {
                if bullet.trim_end_matches(['.', ')']).parse::<u64>().is_ok() {
                    bullet.ends_with(&k[k.len() - 1..]) && k.len() == 2
                } else {
                    *k == bullet || (bullet == "*" && *k == "-")
                }
            })
            .unwrap_or(0);
        let next = KINDS[(at + if backwards { 3 } else { 1 }) % 4];
        let sibs = self.org_item_siblings(line);
        let at_point = self.point();
        let mut n = 0u64;
        for l in sibs {
            let text = self.line(l);
            let Some((indent, old, _)) = list_item(&text) else {
                continue;
            };
            n += 1;
            let new = if next.len() == 2 {
                format!("{n}{}", &next[1..])
            } else {
                next.to_string()
            };
            self.replace_range(
                Pos::new(l, indent),
                Pos::new(l, indent + old.chars().count()),
                &new,
            );
        }
        let col = at_point.col.min(self.line_len(at_point.line));
        self.set_point(Pos::new(at_point.line, col));
        true
    }

    fn org_item_range(&self, line: usize) -> Option<(usize, usize, usize)> {
        let (indent, _, _) = list_item(&self.line(line))?;
        let mut end = line;
        for l in line + 1..self.line_count() {
            let t = self.line(l);
            if t.trim().is_empty() {
                // Blank lines belong to the item only if followed by a deeper line.
                let mut j = l + 1;
                while j < self.line_count() && self.line(j).trim().is_empty() {
                    j += 1;
                }
                if j >= self.line_count() {
                    break;
                }
                let nt = self.line(j);
                let nindent = nt.chars().take_while(|c| c.is_whitespace()).count();
                if heading_level(&nt).is_some()
                    || nindent <= indent && list_item(&nt).is_some_and(|(i, _, _)| i <= indent)
                {
                    break;
                }
                end = j;
                continue;
            }
            if heading_level(&t).is_some() {
                break;
            }
            let nindent = t.chars().take_while(|c| c.is_whitespace()).count();
            if let Some((i, _, _)) = list_item(&t) {
                if i <= indent {
                    break;
                }
            } else if nindent <= indent {
                break;
            }
            end = l;
        }
        Some((indent, line, end))
    }

    fn org_shift_item(&mut self, line: usize, delta: isize) -> bool {
        let Some((_, first, last)) = self.org_item_range(line) else {
            return false;
        };
        for l in first..=last {
            if delta > 0 {
                self.replace_range(Pos::new(l, 0), Pos::new(l, 0), "  ");
            } else {
                let t = self.line(l);
                let n = t.chars().take_while(|c| *c == ' ').count().min(2);
                if n > 0 {
                    self.replace_range(Pos::new(l, 0), Pos::new(l, n), "");
                }
            }
        }
        true
    }

    /// `org-list-indent-item-generic`: indent the item (and its children).
    pub fn org_list_indent_item(&mut self, line: usize) -> bool {
        if list_item(&self.line(line)).is_none() {
            return false;
        }
        self.org_shift_item(line, 1)
    }

    /// `org-list-outdent-item`.
    pub fn org_list_outdent_item(&mut self, line: usize) -> bool {
        if list_item(&self.line(line)).is_none() {
            return false;
        }
        self.org_shift_item(line, -1)
    }

    /// `org-list-insert-item`: a new item after this one (checkbox-aware,
    /// number-aware). False when not on an item.
    pub fn org_list_insert_item(&mut self, line: usize) -> bool {
        let text = self.line(line);
        let Some((indent, bullet, checkbox)) = list_item(&text) else {
            return false;
        };
        let bullet = match bullet.trim_end_matches(['.', ')']).parse::<u64>() {
            Ok(n) => format!("{}{}", n + 1, &bullet[bullet.len() - 1..]),
            Err(_) => bullet,
        };
        let item = format!(
            "\n{}{} {}",
            " ".repeat(indent),
            bullet,
            if checkbox.is_some() { "[ ] " } else { "" }
        );
        let end = self.line_end_offset(line);
        self.replace_offsets(end, end, &item);
        true
    }

    /// `org-list-item-to-subtree`: the item (plus its children) becomes
    /// heading(s). Returns false when not on an item.
    pub fn org_list_item_to_subtree(&mut self, line: usize) -> bool {
        let Some((_, first, last)) = self.org_item_range(line) else {
            return false;
        };
        let depth = self
            .org_heading_at(line)
            .and_then(|h| self.org_level(h))
            .unwrap_or(0);
        let base_indent = list_item(&self.line(first)).map(|i| i.0).unwrap_or(0);
        for l in first..=last {
            let t = self.line(l);
            match list_item(&t) {
                Some((indent, _, _)) => {
                    let rel = indent.saturating_sub(base_indent) / 2;
                    let level = (depth + 1 + rel).max(1);
                    let body = ITEM.replace(&t, "").trim_start().to_string();
                    self.replace_range(
                        Pos::new(l, 0),
                        Pos::new(l, self.line_len(l)),
                        &format!("{} {body}", "*".repeat(level)),
                    );
                }
                None => {
                    // Continuation line: leave as body text.
                }
            }
        }
        self.org_update_cookies();
        true
    }

    /// `org-list-repair`: renumber ordered sibling items and recount
    /// cookies. Returns how many lines changed.
    pub fn org_list_repair(&mut self) -> usize {
        let mut changed = 0;
        let n = self.line_count();
        let mut l = 0;
        while l < n {
            let t = self.line(l);
            if let Some((indent, bullet, _)) = list_item(&t)
                && bullet.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                let suffix = if bullet.ends_with(')') { ")" } else { "." };
                let mut num = 1u64;
                let mut j = l;
                while j < n {
                    let jt = self.line(j);
                    match list_item(&jt) {
                        Some((i, b, _))
                            if i == indent
                                && b.ends_with(suffix)
                                && b.trim_end_matches(['.', ')']).parse::<u64>().is_ok() =>
                        {
                            let want = format!("{num}{suffix}");
                            if b != want {
                                let col_a = indent;
                                let col_b = indent + b.chars().count();
                                self.replace_range(Pos::new(j, col_a), Pos::new(j, col_b), &want);
                                changed += 1;
                            }
                            num += 1;
                            j += 1;
                        }
                        Some((i, _, _)) if i > indent => {
                            j += 1;
                        }
                        _ => break,
                    }
                }
                l = j;
                continue;
            }
            l += 1;
        }
        changed + self.org_update_cookies()
    }

    /// `org-sort-list`: sort sibling items alphabetically. False when not
    /// on an item.
    pub fn org_sort_list(&mut self, line: usize, reverse: bool) -> bool {
        let text = self.line(line);
        let Some((indent, _, _)) = list_item(&text) else {
            return false;
        };
        // Sibling block: consecutive items at this indent.
        let mut first = line;
        while first > 0 {
            let t = self.line(first - 1);
            if list_item(&t).is_some_and(|(i, _, _)| i == indent) {
                first -= 1;
            } else {
                break;
            }
        }
        let mut last = line;
        while last + 1 < self.line_count() {
            let t = self.line(last + 1);
            if list_item(&t).is_some_and(|(i, _, _)| i == indent) {
                last += 1;
            } else {
                break;
            }
        }
        if last <= first {
            return false;
        }
        let mut rows: Vec<String> = (first..=last).map(|l| self.line(l)).collect();
        rows.sort_by_key(|r| r.to_lowercase());
        if reverse {
            rows.reverse();
        }
        let text = rows.join("\n");
        self.replace_range(
            Pos::new(first, 0),
            Pos::new(last, self.line_len(last)),
            &text,
        );
        true
    }

    // ------------------------------------------------ drawers and blocks

    /// `org-insert-drawer`: a `:NAME:` ... `:END:` drawer after `line`.
    /// The name is uppercased; defaults to LOGBOOK.
    pub fn org_insert_drawer(&mut self, line: usize, name: &str) {
        let name = {
            let n = name.trim().trim_matches(':').to_uppercase();
            if n.is_empty() { "LOGBOOK".into() } else { n }
        };
        let line = line.min(self.line_count().saturating_sub(1));
        let base = self.line(line);
        let indent = base.chars().take_while(|c| c.is_whitespace()).count();
        let pad = " ".repeat(indent);
        let at = self.line_end_offset(line);
        self.replace_offsets(at, at, &format!("\n{pad}:{name}:\n{pad}:END:"));
        self.set_point(Pos::new(line + 1, self.line_len(line + 1)));
    }
}

static DATE_YMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap());

/// Shift every YYYY-MM-DD date in `text` by `days` (for
/// `org-clone-subtree-with-time-shift`).
pub fn shift_org_dates(text: &str, days: i64) -> String {
    DATE_YMD
        .replace_all(text, |caps: &regex::Captures| {
            let y: i64 = caps[1].parse().unwrap_or(2000);
            let m: i64 = caps[2].parse().unwrap_or(1);
            let d: i64 = caps[3].parse().unwrap_or(1);
            let (y2, m2, d2) = add_days(y, m, d, days);
            format!("{y2:04}-{m2:02}-{d2:02}")
        })
        .into_owned()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn add_days(y: i64, m: i64, d: i64, days: i64) -> (i64, i64, i64) {
    civil_from_days(days_from_civil(y, m, d) + days)
}

// ------------------------------------------------------------ highlighting

static VERBATIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s(\x22'{])([=~][^\s=~](?:[^\n]*?[^\s])??[=~])(?:$|[\s\-.,;:!?'\x22)}\]])")
        .unwrap()
});
/// One regex per emphasis marker (Org's `org-emphasis-regexp-components`):
/// the marker follows the start of the line, whitespace or one of `-({'"`;
/// its text neither starts nor ends with whitespace; and it is followed by
/// whitespace, the end of the line or one of `-.,;:!?'")}[\`.
fn emphasis_regex(marker: &str) -> Regex {
    let m = regex::escape(marker);
    Regex::new(&format!(
        r#"(?:^|[\s\-({{'"])({m}[^\s{m}](?:[^\n]*?[^\s])??{m})(?:$|[\s\-.,;:!?'")}}\[\\])"#
    ))
    .expect("valid emphasis regex")
}
static EMPH_BOLD: LazyLock<Regex> = LazyLock::new(|| emphasis_regex("*"));
static EMPH_ITALIC: LazyLock<Regex> = LazyLock::new(|| emphasis_regex("/"));
static EMPH_UNDERLINE: LazyLock<Regex> = LazyLock::new(|| emphasis_regex("_"));
static EMPH_STRIKE: LazyLock<Regex> = LazyLock::new(|| emphasis_regex("+"));

/// The byte ranges of `marker`-delimited emphasis in `line`. The search
/// resumes where the emphasis ended, not after the character that follows
/// it, so `*a* *b*` finds both.
fn emphasis_ranges(re: &Regex, line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < line.len() {
        let Some(c) = re.captures_at(line, at) else {
            break;
        };
        let m = c.get(1).expect("group");
        out.push((m.start(), m.end()));
        at = m.end();
    }
    out
}

static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[<\[]\d{4}-\d{2}-\d{2}[^>\]\n]*[>\]]").unwrap());

/// Faces for one line of Org text, one per char.
pub fn line_faces(line: &str, kw: &Keywords) -> Vec<Option<SyntaxFace>> {
    let n = line.chars().count();
    let mut faces = vec![None; n];
    // Byte range -> char range, painted.
    let paint = |a: usize, z: usize, f: SyntaxFace, faces: &mut Vec<Option<SyntaxFace>>| {
        let (a, z) = (line[..a].chars().count(), line[..z].chars().count());
        for slot in &mut faces[a..z] {
            *slot = Some(f);
        }
    };
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.starts_with("#+") || trimmed == "#" || trimmed.starts_with("# ") {
        paint(indent, line.len(), SyntaxFace::Comment, &mut faces);
        return faces;
    }
    if trimmed.starts_with(':') && trimmed.trim_end().ends_with(':') && !trimmed.contains(' ') {
        paint(indent, line.len(), SyntaxFace::Property, &mut faces);
        return faces;
    }
    if let Some(h) = parse_heading(line, kw) {
        let face = [
            SyntaxFace::Function,
            SyntaxFace::Type,
            SyntaxFace::Builtin,
            SyntaxFace::Property,
        ][(h.level - 1) % 4];
        paint(0, line.len(), face, &mut faces);
        if let Some(k) = &h.keyword {
            let at = line.find(k.as_str()).unwrap_or(0);
            let f = if kw.is_done(k) {
                SyntaxFace::Comment
            } else {
                SyntaxFace::Keyword
            };
            paint(at, at + k.len(), f, &mut faces);
        }
        if let Some(m) = TAGS.captures(line).and_then(|c| c.get(1)) {
            paint(m.start(), m.end(), SyntaxFace::Attribute, &mut faces);
        }
    } else if let Some(c) = ITEM.captures(line) {
        let b = c.get(2).expect("bullet");
        paint(b.start(), b.end(), SyntaxFace::Keyword, &mut faces);
        if let Some(cb) = c.get(3) {
            paint(cb.start(), cb.end(), SyntaxFace::Constant, &mut faces);
        }
    }
    for m in COOKIE.find_iter(line) {
        paint(m.start(), m.end(), SyntaxFace::Constant, &mut faces);
    }
    for m in DATE.find_iter(line) {
        paint(m.start(), m.end(), SyntaxFace::Constant, &mut faces);
    }
    for c in VERBATIM.captures_iter(line) {
        let m = c.get(1).expect("group");
        paint(m.start(), m.end(), SyntaxFace::String, &mut faces);
    }
    // Emphasis, markers included. An emphasised word on a heading line takes the emphasis
    // face, as a face covers a char at a time here.
    for (re, face) in [
        (&*EMPH_BOLD, SyntaxFace::OrgBold),
        (&*EMPH_ITALIC, SyntaxFace::OrgItalic),
        (&*EMPH_UNDERLINE, SyntaxFace::OrgUnderline),
        (&*EMPH_STRIKE, SyntaxFace::OrgStrike),
    ] {
        for (a, z) in emphasis_ranges(re, line) {
            paint(a, z, face, &mut faces);
        }
    }
    // Links: like org-link face, style only the visible text (the
    // description, or the target when there is none), not the brackets.
    // These are char columns already, so fill them directly.
    for l in crate::org_ext::links_in(line) {
        for slot in &mut faces[l.text_start.min(n)..l.text_end.min(n)] {
            *slot = Some(SyntaxFace::Link);
        }
    }
    // Footnote references, citations, LaTeX fragments and entities.
    for m in FOOTREF.find_iter(line) {
        paint(m.start(), m.end(), SyntaxFace::Attribute, &mut faces);
    }
    for m in CITE.find_iter(line) {
        paint(m.start(), m.end(), SyntaxFace::Type, &mut faces);
    }
    for m in MATH.find_iter(line) {
        // The match may include a boundary char on each side of `$..$`.
        let t = m.as_str();
        let a = m.start() + t.len() - t.trim_start_matches(|c: char| c != '$' && c != '\\').len();
        let z = m.start()
            + t.trim_end_matches(|c: char| c != '$' && c != '\\' && c != ')' && c != ']')
                .len();
        if z > a {
            paint(a, z, SyntaxFace::Builtin, &mut faces);
        }
    }
    for m in ENTITY.find_iter(line) {
        if crate::export::entities::find(m.as_str().trim_start_matches('\\').trim_end_matches("{}"))
            .is_some()
        {
            paint(m.start(), m.end(), SyntaxFace::Builtin, &mut faces);
        }
    }
    faces
}

#[cfg(test)]
mod tests {
    #[test]
    fn tab_on_a_drawer_line_shows_and_hides_its_contents() {
        let mut b = org("* H\n:LOGBOOK:\n- note\n:END:\nafter\n");
        assert_eq!(b.org_drawer_end(1), Some(3));
        assert_eq!(b.org_drawer_end(0), None);
        assert_eq!(b.org_drawer_end(3), None, ":END: is not a drawer start");
        assert_eq!(b.org_cycle_drawer(1), Some(true));
        assert!(b.line_hidden(2) && b.line_hidden(3) && !b.line_hidden(4));
        assert_eq!(b.org_cycle_drawer(1), Some(false));
        assert!(!b.line_hidden(2));
        assert_eq!(b.org_cycle_drawer(0), None);
        // Opening a folded heading collapses the drawers inside it.
        b.org_cycle(0);
        b.org_cycle(0);
        b.unfold_all();
        b.fold_subtree(0);
        b.org_cycle(0);
        assert!(
            b.line_hidden(2),
            "the drawer is collapsed when the entry opens"
        );
    }

    use super::*;

    #[test]
    fn emphasis_is_styled_with_its_markers() {
        let kw = Keywords::default();
        let line = "a *bold* /it/ _un_ +st+ and *two* *words*";
        let f = line_faces(line, &kw);
        let at = |needle: &str| f[line.find(needle).unwrap()];
        assert_eq!(at("*bold*"), Some(SyntaxFace::OrgBold));
        assert_eq!(at("bold"), Some(SyntaxFace::OrgBold));
        assert_eq!(at("/it/"), Some(SyntaxFace::OrgItalic));
        assert_eq!(at("_un_"), Some(SyntaxFace::OrgUnderline));
        assert_eq!(at("+st+"), Some(SyntaxFace::OrgStrike));
        // Two emphasised words next to each other are both found.
        assert_eq!(at("*two*"), Some(SyntaxFace::OrgBold));
        assert_eq!(at("*words*"), Some(SyntaxFace::OrgBold));
        // Plain text around them is untouched.
        assert_eq!(at("a "), None);
        assert_eq!(at(" and"), None);
    }

    #[test]
    fn emphasis_needs_its_boundaries() {
        let kw = Keywords::default();
        for line in [
            "2*3*4",
            "a * b * c",
            "path/to/file",
            "snake_case_name",
            "x *not closed",
            "** Heading",
        ] {
            let f = line_faces(line, &kw);
            assert!(
                f.iter().all(|x| !matches!(
                    x,
                    Some(
                        SyntaxFace::OrgBold
                            | SyntaxFace::OrgItalic
                            | SyntaxFace::OrgUnderline
                            | SyntaxFace::OrgStrike
                    )
                )),
                "{line:?} must not be emphasised"
            );
        }
    }

    #[test]
    fn highlights_rich_inline_objects() {
        let kw = Keywords::default();
        let f = line_faces("a[fn:1] [cite:@k] $x^2$ \\alpha \\nope b", &kw);
        let at = |i: usize| f[i];
        assert_eq!(at(1), Some(SyntaxFace::Attribute));
        assert_eq!(at(9), Some(SyntaxFace::Type));
        assert_eq!(at(19), Some(SyntaxFace::Builtin), "math");
        assert_eq!(at(26), Some(SyntaxFace::Builtin), "entity");
        assert_eq!(at(34), None, "unknown entity");
    }

    fn org(text: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b
    }

    const DOC: &str = "#+TITLE: x\n* TODO One [0/2]\nbody\n** TODO A\ntext a\n** B\n* Two :tag:\n- [ ] x [0/1]\n  - [ ] y\n";

    #[test]
    fn keywords_and_headings() {
        let kw = Keywords::from_text(["#+TODO: TODO(t) NEXT | DONE CANCELLED"].into_iter());
        assert_eq!(kw.todo, ["TODO", "NEXT"]);
        assert_eq!(kw.done, ["DONE", "CANCELLED"]);
        assert_eq!(kw.next(None, true).as_deref(), Some("TODO"));
        assert_eq!(kw.next(Some("CANCELLED"), true), None);
        assert_eq!(kw.next(None, false).as_deref(), Some("CANCELLED"));
        let h = parse_heading("** NEXT Write it   :work:urgent:", &kw).unwrap();
        assert_eq!(
            (h.level, h.keyword.as_deref(), h.title.as_str()),
            (2, Some("NEXT"), "Write it")
        );
        assert_eq!(h.tags, ["work", "urgent"]);
        assert_eq!(heading_level("*bold* text"), None);
        assert_eq!(heading_level("*"), Some(1));
        assert_eq!(list_item("  - [X] done"), Some((2, "-".into(), Some('X'))));
        assert_eq!(list_item("1. first"), Some((0, "1.".into(), None)));
        assert_eq!(list_item("* heading"), None);
    }

    #[test]
    fn cycling_subtrees_and_buffer() {
        let mut b = org(DOC);
        assert_eq!(b.org_subtree_end(1), 5);
        assert_eq!(b.org_children(1), [3, 5]);
        assert_eq!(b.org_cycle(1), Some(Visibility::Folded));
        assert!((2..=5).all(|l| b.line_hidden(l)));
        assert_eq!(b.org_cycle(1), Some(Visibility::Children));
        assert!(!b.line_hidden(2) && !b.line_hidden(3) && b.line_hidden(4) && !b.line_hidden(5));
        assert_eq!(b.org_cycle(1), Some(Visibility::Subtree));
        assert!(b.folds().is_empty());
        assert_eq!(b.org_cycle(2), None, "not a heading");

        assert_eq!(b.org_global_cycle(), Visibility::Overview);
        assert!(!b.line_hidden(1) && b.line_hidden(3) && !b.line_hidden(6) && b.line_hidden(7));
        assert_eq!(b.org_global_cycle(), Visibility::Contents);
        assert!(!b.line_hidden(3) && b.line_hidden(4) && !b.line_hidden(5) && b.line_hidden(2));
        assert_eq!(b.org_global_cycle(), Visibility::ShowAll);
        assert!(b.folds().is_empty());
    }

    #[test]
    fn todo_checkboxes_and_cookies() {
        let mut b = org(DOC);
        b.set_point(Pos::new(3, 5));
        assert_eq!(b.org_todo(3, true).unwrap().as_deref(), Some("DONE"));
        assert_eq!(b.line(3), "** DONE A");
        assert_eq!(
            b.line(1),
            "* TODO One [1/1]",
            "B has no keyword so isn't counted"
        );
        assert_eq!(b.org_todo(5, true).unwrap().as_deref(), Some("TODO"));
        assert_eq!(b.line(5), "** TODO B");
        assert_eq!(b.line(1), "* TODO One [1/2]");
        assert_eq!(b.org_todo(3, true).unwrap(), None);
        assert_eq!(b.line(3), "** A");
        assert!(b.org_todo(0, true).is_err());

        assert!(b.org_toggle_checkbox(8));
        assert_eq!(b.line(8), "  - [X] y");
        assert_eq!(b.line(7), "- [ ] x [1/1]");
        assert!(!b.org_toggle_checkbox(6));
    }

    #[test]
    fn list_cookie_counts_every_descendant() {
        let mut b = org("* H\n- [ ] top [0/2]\n  - [ ] mid [0/1]\n    - [ ] deep\n");
        assert!(b.org_toggle_checkbox(3));
        assert_eq!(b.line(3), "    - [X] deep");
        assert_eq!(b.line(2), "  - [ ] mid [1/1]", "mid counts its child");
        assert_eq!(b.line(1), "- [ ] top [1/2]", "top counts every descendant");
    }

    #[test]
    fn a_heading_cookie_only_counts_its_own_subtree() {
        let mut b = org("* A [0/1]\n- [ ] a\n* B [0/1]\n- [ ] b\n");
        assert!(b.org_toggle_checkbox(1));
        assert_eq!(b.line(0), "* A [1/1]");
        assert_eq!(b.line(2), "* B [0/1]", "the sibling heading is untouched");
    }

    #[test]
    fn a_list_cookie_only_counts_its_own_sublist() {
        let mut b = org("- [ ] one [0/1]\n  - [ ] child\n- [ ] two [0/1]\n  - [ ] child\n");
        assert!(b.org_toggle_checkbox(1));
        assert_eq!(b.line(0), "- [ ] one [1/1]");
        assert_eq!(
            b.line(2),
            "- [ ] two [0/1]",
            "the sibling list is untouched"
        );
    }

    #[test]
    fn structure_editing() {
        let mut b = org("* A\na\n** A1\n* B\nb\n");
        assert!(b.org_move_subtree(0, true));
        assert_eq!(b.get_contents(), "* B\nb\n* A\na\n** A1\n");
        assert_eq!(b.point().line, 2);
        assert!(!b.org_move_subtree(2, true));
        assert!(b.org_move_subtree(3, false));
        assert_eq!(b.get_contents(), "* A\na\n** A1\n* B\nb\n");
        assert!(b.org_shift_level(0, 1, true));
        assert_eq!(b.get_contents(), "** A\na\n*** A1\n* B\nb\n");
        assert!(b.org_shift_level(0, -1, false));
        assert_eq!(b.line(0), "* A");
        assert!(!b.org_shift_level(0, -1, false));
        b.org_insert_heading_or_item(1);
        assert_eq!(b.get_contents(), "* A\na\n*** A1\n* \n* B\nb\n");
        assert_eq!(b.point(), Pos::new(3, 2));

        let mut l = org("- [X] one\n");
        l.org_insert_heading_or_item(0);
        assert_eq!(l.get_contents(), "- [X] one\n- [ ] \n");
        let mut n = org("1. one");
        n.org_insert_heading_or_item(0);
        assert_eq!(n.get_contents(), "1. one\n2. ");
    }

    #[test]
    fn navigation() {
        let b = org(DOC);
        assert_eq!(b.org_next_heading(0, true, false), Some(1));
        assert_eq!(b.org_next_heading(1, true, true), Some(6));
        assert_eq!(b.org_next_heading(3, true, true), Some(5));
        assert_eq!(
            b.org_next_heading(5, true, true),
            None,
            "siblings end with the parent"
        );
        assert_eq!(b.org_next_heading(6, false, true), Some(1));
        assert_eq!(b.org_next_heading(5, false, true), Some(3));
        assert_eq!(b.org_up_heading(4), Some(1));
    }

    #[test]
    fn document_structure_motion() {
        let b = org(DOC);
        // org-up-element: nested item -> enclosing item, body -> heading.
        let items = org("- a\n  - b\n  - c\n* H\n");
        assert_eq!(items.org_up_element(2), Some(0));
        assert_eq!(b.org_up_element(4), Some(1));
        assert_eq!(b.org_up_heading(4), Some(1));
        // goto targets / siblings / mark / indirect.
        assert_eq!(b.org_goto_targets().len(), 4);
        assert_eq!(b.org_goto_sibling(3, true), Some(5));
        assert_eq!(b.org_goto_sibling(5, false), Some(3));
        assert_eq!(b.org_mark_subtree(2), Some((1, 5)));
        assert_eq!(b.org_mark_subtree(4), Some((3, 4)));
        let (name, text) = b.org_tree_to_indirect_text(2).unwrap();
        assert!(name.contains("One"));
        assert!(text.starts_with("* TODO One"));
        let (aname, _) = b.org_tree_to_indirect_text(4).unwrap();
        assert_eq!(aname, "A");
        assert!(org("plain\n").org_mark_subtree(0).is_none());
    }

    #[test]
    fn document_structure_visibility() {
        let mut b = org(DOC);
        b.org_show_children(1);
        assert!(!b.line_hidden(3) && !b.line_hidden(5));
        assert!(b.line_hidden(4));
        b.org_show_subtree(1);
        assert!((2..=5).all(|l| !b.line_hidden(l)));
        assert!(b.org_hide_subtree(1));
        assert!((2..=5).all(|l| b.line_hidden(l)));
        b.org_show_entry(1);
        assert!(!b.line_hidden(2));
        b.org_fold_show_all();
        assert!(b.folds().is_empty());
        assert_eq!(
            b.org_cycle_set_startup_visibility("overview"),
            Visibility::Overview
        );
        assert!(b.line_hidden(2));
        assert_eq!(
            b.org_cycle_set_startup_visibility("showall"),
            Visibility::ShowAll
        );
        assert!(b.folds().is_empty());
        b.set_point(Pos::new(1, 0));
        b.fold_lines(2, 3);
        assert!(b.org_reveal_at_point() || !b.folds().is_empty());
        b.set_point(Pos::new(2, 0));
        assert!(b.org_reveal_at_point());
        assert_eq!(b.org_cycle_force_archived(1), Some(Visibility::Folded));
    }

    #[test]
    fn startup_visibility_modes() {
        let doc = "* A\nbody\n** A1\ndeep\n*** A1a\nx\n* B\n";
        let mut b = org(doc);
        assert_eq!(
            b.org_cycle_set_startup_visibility("showeverything"),
            Visibility::ShowAll
        );
        assert!(b.folds().is_empty());
        assert_eq!(
            b.org_cycle_set_startup_visibility("show2levels"),
            Visibility::Levels(2)
        );
        assert!(!b.line_hidden(0) && !b.line_hidden(2) && !b.line_hidden(6));
        assert!(b.line_hidden(3) && b.line_hidden(4) && b.line_hidden(5));
        assert_eq!(
            b.org_cycle_set_startup_visibility("show3levels"),
            Visibility::Levels(3)
        );
        assert!(!b.line_hidden(4) && b.line_hidden(5));
        assert_eq!(
            b.org_cycle_set_startup_visibility("content"),
            Visibility::Contents
        );
        assert!(!b.line_hidden(0) && b.line_hidden(1));
        // Unknown words fold everything, like "folded".
        assert_eq!(
            b.org_cycle_set_startup_visibility("bogus"),
            Visibility::Folded
        );
        assert_eq!(show_levels("show2levels"), Some(2));
        assert_eq!(show_levels("show5levels"), Some(5));
        assert_eq!(show_levels("show1levels"), None);
        assert_eq!(show_levels("show6levels"), None);
        assert_eq!(show_levels("overview"), None);
    }

    #[test]
    fn startup_keywords_parse_and_apply() {
        let mut b = org("#+STARTUP: overview indent\n* A\nbody\n** B\n");
        assert_eq!(
            b.startup_options(),
            vec!["overview".to_string(), "indent".to_string()]
        );
        b.apply_org_startup("showeverything", false);
        assert!(!b.line_hidden(1) && b.line_hidden(2) && b.line_hidden(3));
        assert!(b.org_display.indent);
        // Later words win per axis; unknown words are ignored.
        let mut c = org("#+startup: OVERVIEW\n#+STARTUP: content noindent bogus\n* A\nbody\n");
        assert_eq!(
            c.startup_options(),
            ["overview", "content", "noindent", "bogus"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        c.apply_org_startup("showeverything", true);
        assert!(!c.line_hidden(2) && c.line_hidden(3));
        assert!(!c.org_display.indent);
        // No keywords: the configured defaults.
        let mut d = org("* A\nbody\n");
        d.apply_org_startup("showeverything", false);
        assert!(d.folds().is_empty() && !d.org_display.indent);
        let mut e = org("* A\nbody\n");
        e.apply_org_startup("overview", true);
        assert!(e.line_hidden(1) && e.org_display.indent);
    }

    #[test]
    fn visiting_applies_startup_keywords_over_defaults() {
        use crate::Editor;
        let dir = std::env::temp_dir().join(format!("forge-org-startup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.org");
        std::fs::write(&file, "#+STARTUP: overview indent\n* A\nbody\n** B\n").unwrap();
        let mut ed = Editor::default();
        let (id, is_new) = ed.open_file(&file).unwrap();
        assert!(!is_new);
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.major_mode(), "org-mode");
        assert!(!b.line_hidden(1) && b.line_hidden(2));
        assert!(b.org_display.indent);
        // Second visit keeps the live buffer (no re-fold).
        let (again, _) = ed.open_file(&file).unwrap();
        assert_eq!(again, id);
        // Without keywords the configured defaults rule.
        let plain = dir.join("plain.org");
        std::fs::write(&plain, "* A\nbody\n").unwrap();
        ed.org_startup_folded = "content".to_string();
        let (pid, _) = ed.open_file(&plain).unwrap();
        let p = ed.state.get_buffer(pid).unwrap();
        assert!(!p.line_hidden(0) && p.line_hidden(1));
        assert!(!p.org_display.indent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn document_structure_editing() {
        let mut b = org("* A\nbody\n** A1\n* B\n");
        b.org_insert_heading(0);
        assert!(b.get_contents().contains("\n* \n* B"));
        let mut c = org("* A\nbody\n** A1\n* B\n");
        c.org_insert_heading_respect_content(0);
        let lines: Vec<String> = (0..c.line_count()).map(|l| c.line(l)).collect();
        assert_eq!(lines[2], "* ");
        let mut t = org("* A\n* B\n");
        t.org_insert_todo_heading(0, false);
        assert!(t.get_contents().contains("* TODO "));
        let mut p = org("* A\n* B\n");
        assert!(p.org_do_demote(0));
        assert_eq!(p.line(0), "** A");
        assert!(p.org_do_promote(0));
        assert!(!p.org_promote_subtree(0));
        assert!(p.org_demote_subtree(1));
        assert_eq!(p.line(1), "** B");
        let mut m = org("* A\n* B\n");
        assert!(m.org_move_subtree_down(0));
        assert!(m.line(0).starts_with("* B"));
        assert!(m.org_move_subtree_up(1));
        assert!(m.line(0).starts_with("* A"));
        let mut s = org("* P\n** zebra\n** apple\n** Mango\n");
        assert_eq!(s.org_sort_children(0, false), 3);
        assert_eq!(
            (s.line(1), s.line(2), s.line(3)),
            ("** apple".into(), "** Mango".into(), "** zebra".into())
        );
        let mut top = org("intro\n* zebra\n* apple\n");
        assert_eq!(top.org_sort_children(0, false), 2);
        assert_eq!(
            (top.line(1), top.line(2)),
            ("* apple".into(), "* zebra".into())
        );
        let mut cp = org("* A\na\n* B\n");
        let text = cp.org_copy_subtree(0).unwrap();
        assert!(text.contains("* A"));
        assert!(cp.org_paste_subtree(2, &text));
        assert_eq!(cp.line_count(), 6);
        let mut cut = org("* A\na\n* B\n");
        let yanked = cut.org_cut_subtree(0).unwrap();
        assert!(yanked.contains("a"));
        assert_eq!(cut.line(0), "* B");
        cut.org_paste_subtree(0, &yanked);
        assert!(cut.get_contents().contains("* A"));
        let mut tg = org("* Head\n");
        assert!(tg.org_toggle_heading(0));
        assert!(tg.line(0).starts_with("- Head"));
        assert!(tg.org_toggle_heading(0));
        assert!(tg.line(0).starts_with("* "));
        let mut cl = org("* A\n<2024-01-31>\n");
        assert!(cl.org_clone_subtree_with_time_shift(0, 1));
        assert!(cl.get_contents().contains("2024-02-01"));
        assert_eq!(shift_org_dates("x 2023-12-31 y", 1), "x 2024-01-01 y");
    }

    #[test]
    fn document_structure_sparse_lists_drawers() {
        let mut b = org("* apple\nbody\n* banana\ncherry\n");
        assert_eq!(b.org_sparse_tree("cherry"), 1);
        assert!(b.line_hidden(1));
        assert!(!b.line_hidden(2));
        let found = b.org_occur_lines("banana").unwrap();
        assert_eq!(found.len(), 1);
        assert!(b.org_occur_lines("[invalid").is_err());

        let mut l = org("- b\n- a\n- c\n");
        assert!(l.org_list_indent_item(1));
        assert!(l.line(1).starts_with("  "));
        assert!(l.org_list_outdent_item(1));
        assert!(l.line(1).starts_with("- a"));
        assert!(l.org_list_insert_item(0));
        assert_eq!(l.line(1), "- ");
        let mut r = org("2. b\n1. a\n");
        r.org_list_repair();
        assert_eq!((r.line(0), r.line(1)), ("1. b".into(), "2. a".into()));
        let mut s = org("- pear\n- apple\n- mango\n");
        assert!(s.org_sort_list(0, false));
        assert_eq!(
            (s.line(0), s.line(1), s.line(2)),
            ("- apple".into(), "- mango".into(), "- pear".into())
        );
        let mut conv = org("* H\n- item\n  - sub\n");
        assert!(conv.org_list_item_to_subtree(1));
        assert!(conv.line(1).starts_with("** "));
        let mut d = org("* H\nbody\n");
        d.org_insert_drawer(0, "logbook");
        assert_eq!(d.line(1), ":LOGBOOK:");
        assert_eq!(d.line(2), ":END:");
    }

    #[test]
    fn faces() {
        let kw = Keywords::default();
        let f = line_faces("* TODO Hi :x:", &kw);
        assert_eq!(f[0], Some(SyntaxFace::Function));
        assert_eq!(f[2], Some(SyntaxFace::Keyword));
        assert_eq!(f[11], Some(SyntaxFace::Attribute));
        assert_eq!(line_faces("** DONE x", &kw)[3], Some(SyntaxFace::Comment));
        let f = line_faces("see [[https://x.org][x]] and =code= [1/2]", &kw);
        // The brackets and target aren't styled; only the description is.
        assert_eq!(f[4], None);
        assert_eq!(f[21], Some(SyntaxFace::Link));
        assert_eq!(f[29], Some(SyntaxFace::String));
        assert_eq!(f[36], Some(SyntaxFace::Constant));
        assert_eq!(f[0], None);
        assert_eq!(line_faces("#+TITLE: x", &kw)[3], Some(SyntaxFace::Comment));
    }

    #[test]
    fn odd_levels_conversion_round_trips() {
        let mut b = org("* One\nbody\n** Two\n*** Three\n**** Four\n");
        assert_eq!(b.org_convert_to_odd_levels(), 3);
        assert_eq!(
            b.get_contents(),
            "* One\nbody\n*** Two\n***** Three\n******* Four\n"
        );
        // A heading with the same structure but already odd: only the
        // levels that change are counted.
        assert_eq!(b.org_convert_to_oddeven_levels(), 3);
        assert_eq!(
            b.get_contents(),
            "* One\nbody\n** Two\n*** Three\n**** Four\n"
        );
    }

    #[test]
    fn conversion_keeps_headings_without_stars_alone() {
        let mut b = org("#+TITLE: x\nnot a heading\n* A\ntext\n");
        // Nothing to do: level 1 is already odd and consecutive.
        assert_eq!(b.org_convert_to_odd_levels(), 0);
        assert_eq!(b.org_convert_to_oddeven_levels(), 0);
        b.org_shift_level(2, 1, false);
        assert_eq!(b.org_convert_to_odd_levels(), 1);
        assert_eq!(b.get_contents(), "#+TITLE: x\nnot a heading\n*** A\ntext\n");
    }

    #[test]
    fn conversion_keeps_point_on_its_line() {
        let mut b = org("* One\n** Two\nbody here\n");
        b.set_point(Pos::new(2, 3));
        b.org_convert_to_odd_levels();
        assert_eq!(b.get_contents(), "* One\n*** Two\nbody here\n");
        assert_eq!(b.point().line, 2);
        assert_eq!(b.point().col, 3);
    }
}
