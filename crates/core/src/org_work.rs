// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org workflow (Phase 11, stage 4, without the agenda): refiling,
//! archiving, clocking, priorities and capture templates. The agenda is
//! in `org_agenda.rs`. Everything is a method on [`Buffer`] or a plain
//! function over text; the editor binds keys, asks questions and opens
//! the other files.

use crate::buffer::{Buffer, Pos};
use crate::org::{heading_level, keywords, parse_heading};
use crate::org_time::{Date, Hm, Timestamp, hmm, month_name, timestamps_in};
use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------- subtrees

/// `text` (a subtree) with its top heading moved to `level` and every
/// other heading by the same amount (never above level 1).
pub fn reindent_subtree(text: &str, level: usize) -> String {
    let Some(top) = text.lines().find_map(heading_level) else {
        return text.to_string();
    };
    let delta = level as isize - top as isize;
    if delta == 0 {
        return text.to_string();
    }
    text.split('\n')
        .map(|l| match heading_level(l) {
            Some(n) => {
                let new = (n as isize + delta).max(1) as usize;
                format!("{}{}", "*".repeat(new), &l[n..])
            }
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Buffer {
    /// The last line with text on it (a file's trailing newline makes an
    /// empty last line that new text should not go after).
    fn org_last_content_line(&self) -> usize {
        let mut l = self.line_count().saturating_sub(1);
        while l > 0 && self.line(l).is_empty() {
            l -= 1;
        }
        l
    }

    /// Insert `text` (a subtree) as the last child of heading `dest`, or
    /// at the end of the buffer when `dest` is None. Returns the line it
    /// starts on.
    pub fn org_insert_subtree_under(&mut self, dest: Option<usize>, text: &str) -> usize {
        let (level, after) = match dest {
            Some(h) => (self.org_level(h).unwrap_or(0) + 1, self.org_subtree_end(h)),
            None => (1, self.org_last_content_line()),
        };
        let body = reindent_subtree(text.trim_end_matches('\n'), level);
        if self.char_count() == 0 {
            self.replace_offsets(0, 0, &format!("{body}\n"));
            return 0;
        }
        self.org_insert_line_after(after, &body);
        after + 1
    }

    /// Targets for refiling: `(heading line, "Parent/Child" path)` for
    /// headings down to `max_level`.
    pub fn org_refile_targets(&self, max_level: usize) -> Vec<(usize, String)> {
        let kw = keywords(self);
        let mut stack: Vec<(usize, String)> = Vec::new();
        let mut out = Vec::new();
        for l in 0..self.line_count() {
            let Some(hd) = parse_heading(&self.line(l), &kw) else {
                continue;
            };
            while stack.last().is_some_and(|(lv, _)| *lv >= hd.level) {
                stack.pop();
            }
            let title = clean_title(&hd.title);
            stack.push((hd.level, title));
            if hd.level <= max_level {
                out.push((
                    l,
                    stack
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("/"),
                ));
            }
        }
        out
    }

    /// `org-refile` inside one buffer: move the subtree at `src` under
    /// `dest`. Errors when `dest` is inside the subtree.
    pub fn org_refile_within(&mut self, src: usize, dest: usize) -> Result<usize, String> {
        let end = self.org_subtree_end(src);
        if dest >= src && dest <= end {
            return Err("Cannot refile to a position inside the selected subtree".into());
        }
        let text = self.org_subtree_text(src);
        let removed = end - src + 1;
        let cut = self.org_cut_subtree(src).ok_or("Not in an outline entry")?;
        debug_assert_eq!(cut, text);
        let dest = if dest > src { dest - removed } else { dest };
        Ok(self.org_insert_subtree_under(Some(dest), &text))
    }
}

pub(crate) fn clean_title(t: &str) -> String {
    static COOKIE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\[(?:\d*/\d*|\d*%)\]").unwrap());
    COOKIE.replace_all(t, "").trim().to_string()
}

// ---------------------------------------------------------------- archive

impl Buffer {
    /// The category of the file: `#+CATEGORY:` or `fallback`.
    pub fn org_category(&self, fallback: &str) -> String {
        (0..self.line_count())
            .find_map(|l| {
                let line = self.line(l);
                let rest = line
                    .get(..11)
                    .filter(|p| p.eq_ignore_ascii_case("#+CATEGORY:"))
                    .map(|_| &line[11..])?;
                Some(rest.trim().to_string())
            })
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Where heading `h` archives to: an `ARCHIVE` property (inherited), a
    /// `#+ARCHIVE:` line, or `%s_archive::`. Returns the file pattern and
    /// the heading inside it, if one is named.
    pub fn org_archive_location(&self, h: usize) -> (String, Option<String>) {
        let spec = self
            .org_get_property(h, "ARCHIVE", true)
            .or_else(|| {
                (0..self.line_count()).find_map(|l| {
                    let line = self.line(l);
                    line.get(..10)
                        .filter(|p| p.eq_ignore_ascii_case("#+ARCHIVE:"))
                        .map(|_| line[10..].trim().to_string())
                })
            })
            .unwrap_or_else(|| "%s_archive::".to_string());
        let (file, head) = match spec.split_once("::") {
            Some((f, h)) => (f.trim().to_string(), h.trim().to_string()),
            None => (spec.trim().to_string(), String::new()),
        };
        let head = head.trim_start_matches('*').trim().to_string();
        (file, (!head.is_empty()).then_some(head))
    }

    /// The subtree at `h` as it is archived: with `ARCHIVE_TIME`,
    /// `ARCHIVE_FILE`, `ARCHIVE_CATEGORY`, `ARCHIVE_TODO` and
    /// `ARCHIVE_ITAGS` properties on its top heading.
    pub fn org_prepare_archive(&self, h: usize, file_name: &str, today: Date, now: Hm) -> String {
        let kw = keywords(self);
        let text = self.org_subtree_text(h);
        let hd = parse_heading(&self.line(h), &kw);
        let mut tmp = Buffer::new("archive.org");
        tmp.set_contents(&text);
        let stamp = Timestamp::new(false, today, Some(now)).text();
        let stamp = stamp.trim_matches(['[', ']']).to_string();
        tmp.org_set_property(0, "ARCHIVE_TIME", &stamp);
        if !file_name.is_empty() {
            tmp.org_set_property(0, "ARCHIVE_FILE", file_name);
        }
        let stem = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        tmp.org_set_property(0, "ARCHIVE_CATEGORY", &self.org_category(&stem));
        if let Some(k) = hd.as_ref().and_then(|d| d.keyword.clone()) {
            tmp.org_set_property(0, "ARCHIVE_TODO", &k);
        }
        let own_tags = self.org_heading_tags(h);
        let inherited: Vec<String> = self
            .org_inherited_tags(h)
            .into_iter()
            .filter(|t| !own_tags.contains(t))
            .collect();
        if !inherited.is_empty() {
            tmp.org_set_property(0, "ARCHIVE_ITAGS", &inherited.join(" "));
        }
        let own = tmp.line_count();
        let end = (0..own)
            .rev()
            .find(|&l| !tmp.line(l).is_empty())
            .unwrap_or(0);
        (0..=end)
            .map(|l| tmp.line(l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `org-archive-to-archive-sibling`: move the subtree under an
    /// `Archive` sibling (tagged ARCHIVE) of its parent, making one at the
    /// end of the parent when there is none. Returns the new line.
    pub fn org_archive_to_sibling(
        &mut self,
        h: usize,
        today: Date,
        now: Hm,
    ) -> Result<usize, String> {
        let h = self.org_heading_at(h).ok_or("Not in an outline entry")?;
        let level = self.org_level(h).unwrap_or(1);
        let parent = self.org_up_heading(h);
        let siblings: Vec<usize> = match parent {
            Some(p) => self.org_children(p),
            None => (0..self.line_count())
                .filter(|&l| self.org_level(l) == Some(1))
                .collect(),
        };
        if self.org_heading_tags_contain(h, "ARCHIVE")
            || self.org_inherited_tags(h).iter().any(|t| t == "ARCHIVE")
        {
            return Err("This subtree is already an archive".into());
        }
        let existing = siblings.iter().copied().find(|&s| {
            s != h
                && self.org_level(s) == Some(level)
                && self.org_heading_tags_contain(s, "ARCHIVE")
        });
        let text = { self.org_prepare_archive(h, "", today, now) };
        let cut_lines = self.org_subtree_end(h) - h + 1;
        self.org_cut_subtree(h).ok_or("Not in an outline entry")?;
        let adjust = |l: usize| if l > h { l - cut_lines } else { l };
        let dest = match existing {
            Some(a) => adjust(a),
            None => {
                let at = match parent {
                    Some(p) => {
                        let p = adjust(p);
                        self.org_subtree_end(p)
                    }
                    None => self.org_last_content_line(),
                };
                self.org_insert_line_after(at, &format!("{} Archive :ARCHIVE:", "*".repeat(level)));
                at + 1
            }
        };
        Ok(self.org_insert_subtree_under(Some(dest), &text))
    }

    fn org_heading_tags_contain(&self, h: usize, tag: &str) -> bool {
        self.org_heading_tags(h).iter().any(|t| t == tag)
    }
}

// ----------------------------------------------------------------- clock

/// `org-clock-rounding-minutes`: the time to the nearest multiple of
/// `step` minutes (halves round up), carrying into the next day.
pub fn round_clock_time(date: Date, at: Hm, step: i64) -> (Date, Hm) {
    if step <= 1 {
        return (date, at);
    }
    let m = at.0 * 60 + at.1;
    let r = (m + step / 2).div_euclid(step) * step;
    let d = date.add_days(r.div_euclid(1440));
    let r = r.rem_euclid(1440);
    (d, (r / 60, r % 60))
}

/// A moment `minutes` earlier than `(date, at)`.
pub fn minutes_before(date: Date, at: Hm, minutes: i64) -> (Date, Hm) {
    let m = date.days() * 1440 + at.0 * 60 + at.1 - minutes;
    let r = m.rem_euclid(1440);
    (Date::from_days(m.div_euclid(1440)), (r / 60, r % 60))
}

/// A `CLOCK:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockLine {
    pub line: usize,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
}

impl ClockLine {
    /// Minutes between start and end (0 while running).
    pub fn minutes(&self) -> i64 {
        self.end
            .as_ref()
            .map_or(0, |e| (e.minutes() - self.start.minutes()).max(0))
    }
}

static CLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*CLOCK:\s*(\[[^\]]+\])(?:--(\[[^\]]+\]))?").unwrap());

pub fn parse_clock_line(line: &str, n: usize) -> Option<ClockLine> {
    let c = CLOCK.captures(line)?;
    let start = timestamps_in(&c[1]).into_iter().next()?;
    let end = c
        .get(2)
        .and_then(|m| timestamps_in(m.as_str()).into_iter().next());
    Some(ClockLine {
        line: n,
        start,
        end,
    })
}

impl Buffer {
    /// Clock lines in the entry's own body.
    pub fn org_clock_lines(&self, h: usize) -> Vec<ClockLine> {
        (h + 1
            ..=self
                .org_body_end(h)
                .min(self.line_count().saturating_sub(1)))
            .filter_map(|l| parse_clock_line(&self.line(l), l))
            .collect()
    }

    /// Every unfinished clock line in the buffer.
    pub fn org_open_clocks(&self) -> Vec<usize> {
        (0..self.line_count())
            .filter(|&l| parse_clock_line(&self.line(l), l).is_some_and(|c| c.end.is_none()))
            .collect()
    }

    /// `org-clock-in`: a new open `CLOCK:` line in the entry's drawer.
    pub fn org_clock_in(&mut self, h: usize, today: Date, now: Hm) -> Result<(), String> {
        let h = self.org_heading_at(h).ok_or("Not in an outline entry")?;
        if self.org_clock_lines(h).iter().any(|c| c.end.is_none()) {
            return Err("This entry is already clocked in".into());
        }
        let line = format!("CLOCK: {}", Timestamp::new(false, today, Some(now)).text());
        let limit = self.org_body_end(h);
        let logbook =
            (h + 1..=limit).find(|&l| self.line(l).trim().eq_ignore_ascii_case(":LOGBOOK:"));
        match logbook {
            Some(l) => self.org_insert_line_after(l, &line),
            None => {
                let mut after = self.org_planning_line(h).unwrap_or(h);
                if let Some((_, z)) = self.org_property_drawer(h) {
                    after = z;
                }
                self.org_insert_line_after(after, &format!(":LOGBOOK:\n{line}\n:END:"));
            }
        }
        Ok(())
    }

    /// `org-clock-out`: close the open clock of the entry. Returns the
    /// minutes it ran.
    pub fn org_clock_out(&mut self, h: usize, today: Date, now: Hm) -> Result<i64, String> {
        let h = self.org_heading_at(h).ok_or("Not in an outline entry")?;
        let open = self
            .org_clock_lines(h)
            .into_iter()
            .find(|c| c.end.is_none())
            .ok_or("No active clock in this entry")?;
        let end = Timestamp::new(false, today, Some(now));
        let mins = (end.minutes() - open.start.minutes()).max(0);
        let text = format!(
            "CLOCK: {}--{} => {:>5}",
            open.start.text(),
            end.text(),
            hmm(mins)
        );
        let indent: String = self
            .line(open.line)
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        self.org_set_line(open.line, &format!("{indent}{text}"));
        Ok(mins)
    }

    /// `org-evaluate-time-range`: rewrite the `=> h:mm` of the closed
    /// `CLOCK:` line at `line` from its two timestamps. Returns the new
    /// duration, or None if the line isn't a closed clock.
    pub fn org_update_clock_line(&mut self, line: usize) -> Option<String> {
        let c = parse_clock_line(&self.line(line), line)?;
        let end = c.end.as_ref()?;
        let mins = (end.minutes() - c.start.minutes()).max(0);
        let indent: String = self
            .line(line)
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let new = format!(
            "{indent}CLOCK: {}--{} => {:>5}",
            c.start.text(),
            end.text(),
            hmm(mins)
        );
        if new != self.line(line) {
            self.org_set_line(line, &new);
        }
        Some(hmm(mins))
    }

    /// `org-clock-cancel`: drop the open clock line (and an empty drawer).
    pub fn org_clock_cancel(&mut self, h: usize) -> bool {
        let Some(h) = self.org_heading_at(h) else {
            return false;
        };
        let Some(open) = self
            .org_clock_lines(h)
            .into_iter()
            .find(|c| c.end.is_none())
        else {
            return false;
        };
        self.org_delete_line(open.line);
        let limit = self
            .org_body_end(h)
            .min(self.line_count().saturating_sub(1));
        if let Some(lb) =
            (h + 1..=limit).find(|&l| self.line(l).trim().eq_ignore_ascii_case(":LOGBOOK:"))
            && lb + 1 < self.line_count()
            && self.line(lb + 1).trim().eq_ignore_ascii_case(":END:")
        {
            self.org_delete_line(lb + 1);
            self.org_delete_line(lb);
        }
        true
    }

    /// Minutes clocked in the entry, with its sub-entries when asked.
    pub fn org_clock_total(&self, h: usize, with_children: bool) -> i64 {
        let end = if with_children {
            self.org_subtree_end(h)
        } else {
            self.org_body_end(h)
        };
        (h + 1..=end.min(self.line_count().saturating_sub(1)))
            .filter_map(|l| parse_clock_line(&self.line(l), l))
            .map(|c| c.minutes())
            .sum()
    }

    /// Close every open clock except `keep` (a line), at `now`. Returns
    /// how many were closed. (`org-resolve-clocks`, without asking.)
    pub fn org_close_stale_clocks(&mut self, keep: Option<usize>, today: Date, now: Hm) -> usize {
        let mut n = 0;
        for l in self.org_open_clocks() {
            if Some(l) == keep {
                continue;
            }
            if let Some(h) = self.org_heading_at(l)
                && self.org_clock_out(h, today, now).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// `org-clock-report`: write a clock table. Inside an existing
    /// `#+BEGIN: clocktable` block that block is rewritten; otherwise a new
    /// one goes in at `line`. Returns the first line of the block.
    pub fn org_clock_report(&mut self, line: usize, today: Date, now: Hm) -> usize {
        let existing = crate::org_dblock::block_at(self, line)
            .filter(|k| k.name.eq_ignore_ascii_case("clocktable"));
        let at = match existing {
            Some(k) => k.begin,
            None => {
                let at = self.line_start_offset(line.min(self.line_count().saturating_sub(1)));
                self.replace_offsets(
                    at,
                    at,
                    "#+BEGIN: clocktable :maxlevel 2 :scope file\n#+END:\n",
                );
                self.set_point(Pos::new(line, 0));
                line
            }
        };
        let _ = self.org_dblock_update(at, today, now);
        at
    }
}

// -------------------------------------------------------------- priority

/// What to do to a heading's `[#A]` cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityAction {
    Up,
    Down,
    Set(char),
    Remove,
}

/// Highest, lowest and default priority.
pub const PRIORITY_HIGHEST: char = 'A';
pub const PRIORITY_LOWEST: char = 'C';
pub const PRIORITY_DEFAULT: char = 'B';

static PRIORITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[#([A-Z0-9]+)\]\s?").unwrap());

impl Buffer {
    /// The priority character of heading `h`.
    pub fn org_priority(&self, h: usize) -> Option<char> {
        let kw = keywords(self);
        let hd = parse_heading(&self.line(h), &kw)?;
        PRIORITY
            .captures(&hd.title)
            .and_then(|c| c[1].chars().next())
    }

    /// `org-priority`, `-up`, `-down`. Returns the new priority.
    pub fn org_set_priority(
        &mut self,
        h: usize,
        action: PriorityAction,
    ) -> Result<Option<char>, String> {
        let h = self.org_heading_at(h).ok_or("Not in an outline entry")?;
        let cur = self.org_priority(h);
        let new = match action {
            PriorityAction::Remove => None,
            PriorityAction::Set(c) => {
                let c = c.to_ascii_uppercase();
                if !c.is_ascii_uppercase() && !c.is_ascii_digit() {
                    return Err(format!("Bad priority {c:?}"));
                }
                Some(c)
            }
            PriorityAction::Up | PriorityAction::Down => {
                let base = cur.unwrap_or(PRIORITY_DEFAULT);
                let up = action == PriorityAction::Up;
                let n = if up {
                    (base as u8).saturating_sub(1)
                } else {
                    base as u8 + 1
                } as char;
                // Wrap like Org: past the lowest goes to the highest.
                Some(if n < PRIORITY_HIGHEST {
                    PRIORITY_LOWEST
                } else if n > PRIORITY_LOWEST {
                    PRIORITY_HIGHEST
                } else {
                    n
                })
            }
        };
        let line = self.line(h);
        let kw = keywords(self);
        let hd = parse_heading(&line, &kw).ok_or("Not on a heading")?;
        let without = PRIORITY.replace(&line, "").to_string();
        let result = match new {
            None => without,
            Some(c) => {
                let stars = hd.level;
                let rest = &without[stars..];
                let mut it = rest.trim_start().splitn(2, char::is_whitespace);
                let first = it.next().unwrap_or("");
                let cookie = format!("[#{c}]");
                if hd.keyword.as_deref() == Some(first) && !first.is_empty() {
                    format!(
                        "{} {} {} {}",
                        &without[..stars],
                        first,
                        cookie,
                        it.next().unwrap_or("").trim_start()
                    )
                    .trim_end()
                    .to_string()
                } else {
                    format!("{} {} {}", &without[..stars], cookie, rest.trim_start())
                        .trim_end()
                        .to_string()
                }
            }
        };
        self.org_set_line(h, &result);
        Ok(new)
    }
}

// --------------------------------------------------------------- capture

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Entry,
    Item,
    CheckItem,
    TableLine,
    Plain,
}

impl TemplateKind {
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "entry" => Self::Entry,
            "item" => Self::Item,
            "checkitem" => Self::CheckItem,
            "table-line" => Self::TableLine,
            "plain" => Self::Plain,
            _ => return None,
        })
    }
}

/// A capture template: where it goes and what it looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTemplate {
    pub key: String,
    pub description: String,
    pub kind: TemplateKind,
    /// File name, relative to the org directory unless absolute.
    pub file: String,
    /// The heading to file under (created if missing).
    pub headline: Option<String>,
    pub text: String,
}

/// The templates used when `forge.opt.org_capture_templates` is unset.
pub fn default_capture_templates() -> Vec<CaptureTemplate> {
    let t = |key: &str, desc: &str, head: &str, text: &str| CaptureTemplate {
        key: key.into(),
        description: desc.into(),
        kind: TemplateKind::Entry,
        file: "notes.org".into(),
        headline: Some(head.into()),
        text: text.into(),
    };
    vec![
        t("t", "Todo", "Tasks", "* TODO %?\n  %U\n  %i"),
        t("n", "Note", "Notes", "* %?\n  %U\n  %i"),
        t("l", "Todo with link", "Tasks", "* TODO %?\n  %U\n  %a"),
    ]
}

/// Parse `forge.opt.org_capture_templates` entries:
/// `key :: description :: kind :: file[::Headline] :: template`, where
/// `\n` in the template is a newline.
pub fn parse_capture_templates(lines: &[String]) -> Result<Vec<CaptureTemplate>, String> {
    let mut out = Vec::new();
    for l in lines {
        let parts: Vec<&str> = l.splitn(5, " :: ").collect();
        if parts.len() < 5 {
            return Err(format!(
                "bad capture template (want 'key :: description :: kind :: file :: template'): {l}"
            ));
        }
        let kind = TemplateKind::from_name(parts[2])
            .ok_or_else(|| format!("unknown capture kind {:?}", parts[2]))?;
        let (file, headline) = match parts[3].split_once("::") {
            Some((f, h)) => (f.trim().to_string(), Some(h.trim().to_string())),
            None => (parts[3].trim().to_string(), None),
        };
        out.push(CaptureTemplate {
            key: parts[0].trim().to_string(),
            description: parts[1].trim().to_string(),
            kind,
            file,
            headline,
            text: parts[4].replace("\\n", "\n"),
        });
    }
    Ok(out)
}

/// What the escapes in a template expand to.
#[derive(Debug, Clone, Default)]
pub struct CaptureCtx {
    pub today: Option<Date>,
    pub now: Hm,
    pub initial: String,
    pub link: String,
    pub file_name: String,
    pub full_path: String,
    pub kill: String,
    pub user: String,
    /// Answers to the `%^{...}` prompts, in order.
    pub answers: Vec<String>,
}

/// The prompts in a template: `%^{Name|default}` -> `("Name", "default")`.
pub fn template_prompts(text: &str) -> Vec<(String, String)> {
    static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%\^\{([^}]*)\}").unwrap());
    PROMPT
        .captures_iter(text)
        .map(|c| {
            let mut it = c[1].split('|');
            (
                it.next().unwrap_or("").to_string(),
                it.next().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn strftime(fmt: &str, d: Date, hm: Hm) -> String {
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&format!("{:04}", d.y)),
            Some('y') => out.push_str(&format!("{:02}", d.y % 100)),
            Some('m') => out.push_str(&format!("{:02}", d.m)),
            Some('d') => out.push_str(&format!("{:02}", d.d)),
            Some('e') => out.push_str(&format!("{:>2}", d.d)),
            Some('H') => out.push_str(&format!("{:02}", hm.0)),
            Some('M') => out.push_str(&format!("{:02}", hm.1)),
            Some('S') => out.push_str("00"),
            Some('a') => out.push_str(d.day_name()),
            Some('b') => out.push_str(&month_name(d.m)[..3]),
            Some('B') => out.push_str(month_name(d.m)),
            Some('F') => out.push_str(&d.iso()),
            Some('T') => out.push_str(&format!("{:02}:{:02}:00", hm.0, hm.1)),
            Some('R') => out.push_str(&format!("{:02}:{:02}", hm.0, hm.1)),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Expand a template. Returns the text and the offset (in characters) of
/// `%?`, if there is one.
pub fn expand_template(text: &str, ctx: &CaptureCtx) -> (String, Option<usize>) {
    let today = ctx.today.unwrap_or_else(|| Date::new(1970, 1, 1));
    let mut out = String::new();
    let mut cursor = None;
    let mut answers = ctx.answers.iter();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let c = chars[i + 1];
        i += 2;
        match c {
            '?' => cursor = Some(out.chars().count()),
            't' => out.push_str(&Timestamp::new(true, today, None).text()),
            'T' => out.push_str(&Timestamp::new(true, today, Some(ctx.now)).text()),
            'u' => out.push_str(&Timestamp::new(false, today, None).text()),
            'U' => out.push_str(&Timestamp::new(false, today, Some(ctx.now)).text()),
            'i' => out.push_str(&ctx.initial),
            'a' => out.push_str(&ctx.link),
            'f' => out.push_str(&ctx.file_name),
            'F' => out.push_str(&ctx.full_path),
            'n' => out.push_str(&ctx.user),
            'x' => out.push_str(&ctx.kill),
            '%' => out.push('%'),
            '<' => {
                let end = chars[i..]
                    .iter()
                    .position(|c| *c == '>')
                    .map_or(chars.len(), |p| i + p);
                let fmt: String = chars[i..end].iter().collect();
                out.push_str(&strftime(&fmt, today, ctx.now));
                i = (end + 1).min(chars.len());
            }
            '^' if chars.get(i) == Some(&'{') => {
                let end = chars[i..]
                    .iter()
                    .position(|c| *c == '}')
                    .map_or(chars.len(), |p| i + p);
                let spec: String = chars[i + 1..end].iter().collect();
                let default = spec.split('|').nth(1).unwrap_or("");
                out.push_str(answers.next().map(String::as_str).unwrap_or(default));
                i = (end + 1).min(chars.len());
            }
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    (out, cursor)
}

impl Buffer {
    /// The first heading whose title is `title` (a cookie or keyword does
    /// not count), at any level.
    pub fn org_find_heading(&self, title: &str) -> Option<usize> {
        let kw = keywords(self);
        (0..self.line_count()).find(|&l| {
            parse_heading(&self.line(l), &kw)
                .is_some_and(|hd| clean_title(&hd.title).eq_ignore_ascii_case(title.trim()))
        })
    }

    /// File captured `text` into the buffer. Returns the first line it
    /// landed on.
    pub fn org_capture_insert(
        &mut self,
        kind: TemplateKind,
        headline: Option<&str>,
        text: &str,
    ) -> usize {
        let text = text.trim_end_matches('\n');
        let target = match headline {
            Some(title) if !title.is_empty() => match self.org_find_heading(title) {
                Some(h) => Some(h),
                None => {
                    // Create it at the end of the file.
                    let at = self.org_last_content_line();
                    if self.char_count() == 0 {
                        self.replace_offsets(0, 0, &format!("* {title}\n"));
                        Some(0)
                    } else {
                        self.org_insert_line_after(at, &format!("* {title}"));
                        Some(at + 1)
                    }
                }
            },
            _ => None,
        };
        match kind {
            TemplateKind::Entry => self.org_insert_subtree_under(target, text),
            TemplateKind::Item | TemplateKind::CheckItem | TemplateKind::Plain => {
                // At the end of the entry's own body (before sub-headings).
                let after = match target {
                    Some(h) => {
                        let mut e = self.org_body_end(h);
                        while e > h && self.line(e).trim().is_empty() {
                            e -= 1;
                        }
                        e
                    }
                    None => self.org_last_content_line(),
                };
                let body = match kind {
                    TemplateKind::Item if !text.trim_start().starts_with(['-', '+']) => {
                        format!("- {text}")
                    }
                    TemplateKind::CheckItem if !text.trim_start().starts_with("- [") => {
                        format!("- [ ] {text}")
                    }
                    _ => text.to_string(),
                };
                if self.char_count() == 0 {
                    self.replace_offsets(0, 0, &format!("{body}\n"));
                    return 0;
                }
                self.org_insert_line_after(after, &body);
                after + 1
            }
            TemplateKind::TableLine => {
                let from = target.unwrap_or(0);
                let to = target.map_or(self.line_count(), |h| self.org_body_end(h) + 1);
                let table = (from..to.min(self.line_count()))
                    .find(|&l| crate::org_table::is_table_line(&self.line(l)));
                match table.and_then(|l| self.org_table_bounds(l)) {
                    Some((_, last)) => {
                        let row = if text.trim_start().starts_with('|') {
                            text.to_string()
                        } else {
                            format!("| {text} |")
                        };
                        self.org_insert_line_after(last, &row);
                        let (a, _) = self.org_table_bounds(last).unwrap_or((last, last));
                        let _ = self.org_table_align(a, 0);
                        last + 1
                    }
                    None => {
                        let at =
                            target.map_or(self.org_last_content_line(), |h| self.org_body_end(h));
                        let row = if text.trim_start().starts_with('|') {
                            text.to_string()
                        } else {
                            format!("| {text} |")
                        };
                        self.org_insert_line_after(at, &row);
                        at + 1
                    }
                }
            }
        }
    }
}

static CLOCKTABLE_INDENT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// `org-clocktable-indent-string`: what marks a nested row's indentation.
pub fn clocktable_indent_string() -> String {
    CLOCKTABLE_INDENT
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| "\\_".to_string())
}

pub fn set_clocktable_indent_string(s: Option<String>) {
    if let Ok(mut g) = CLOCKTABLE_INDENT.lock() {
        *g = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(text: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b
    }

    fn lines(b: &Buffer) -> Vec<String> {
        (0..b.line_count()).map(|l| b.line(l)).collect()
    }

    const D: Date = Date {
        y: 2026,
        m: 10,
        d: 12,
    };

    #[test]
    fn reindent_and_refile() {
        assert_eq!(
            reindent_subtree("* A\n** B\ntext", 3),
            "*** A\n**** B\ntext"
        );
        assert_eq!(reindent_subtree("*** A\n**** B", 1), "* A\n** B");
        let mut b = org("* One\n** Sub\n* Two\n** Child\ntext\n");
        let targets = b.org_refile_targets(2);
        assert_eq!(targets[1].1, "One/Sub");
        assert!(b.org_refile_within(0, 1).is_err(), "into itself");
        let at = b.org_refile_within(0, 2).unwrap();
        assert_eq!(lines(&b)[..2], ["* Two", "** Child"]);
        assert_eq!(b.line(at), "** One");
        assert!(lines(&b).contains(&"*** Sub".to_string()));
    }

    #[test]
    fn archive_properties_and_sibling() {
        let mut b = org("#+CATEGORY: work\n* Proj :a:\n** TODO Task :b:\nbody\n** Other\n");
        let text = b.org_prepare_archive(2, "/x/plan.org", D, (14, 30));
        assert!(text.contains(":ARCHIVE_TIME: 2026-10-12 Mon 14:30"));
        assert!(text.contains(":ARCHIVE_FILE: /x/plan.org"));
        assert!(text.contains(":ARCHIVE_CATEGORY: work"));
        assert!(text.contains(":ARCHIVE_TODO: TODO"));
        assert!(text.contains(":ARCHIVE_ITAGS: a"));
        assert_eq!(b.org_archive_location(2), ("%s_archive".into(), None));
        let at = b.org_archive_to_sibling(2, D, (14, 30)).unwrap();
        assert!(lines(&b).iter().any(|l| l == "** Archive :ARCHIVE:"));
        assert!(b.line(at).starts_with("*** TODO Task"), "{}", b.line(at));
        assert!(b.org_archive_to_sibling(at, D, (14, 30)).is_err());
    }

    #[test]
    fn clocking_in_and_out() {
        let mut b = org("* Task\n:PROPERTIES:\n:A: 1\n:END:\ntext\n** Child\n");
        b.org_clock_in(0, D, (9, 0)).unwrap();
        assert_eq!(
            lines(&b)[4..7],
            [":LOGBOOK:", "CLOCK: [2026-10-12 Mon 09:00]", ":END:"]
        );
        assert!(b.org_clock_in(0, D, (9, 5)).is_err());
        assert_eq!(b.org_open_clocks(), vec![5]);
        let m = b.org_clock_out(0, D, (10, 30)).unwrap();
        assert_eq!(m, 90);
        assert_eq!(
            b.line(5),
            "CLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 10:30] =>  1:30"
        );
        assert_eq!(b.org_clock_total(0, true), 90);
        b.org_clock_in(0, D, (11, 0)).unwrap();
        assert_eq!(
            b.line(5),
            "CLOCK: [2026-10-12 Mon 11:00]",
            "newest clock first"
        );
        assert!(b.org_clock_cancel(0));
        assert_eq!(b.org_clock_total(0, false), 90);
    }

    #[test]
    fn clock_cancel_with_a_trailing_logbook_is_not_a_panic() {
        // The drawer is the last line of the buffer: there is no `:END:`
        // line after it to look at.
        let mut b = org("* Task\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 09:00]");
        assert!(b.org_clock_cancel(0));
        assert_eq!(b.get_contents(), "* Task\n:LOGBOOK:");
    }

    #[test]
    fn template_month_out_of_range_is_not_a_panic() {
        let ctx = CaptureCtx {
            today: Some(Date::new(2026, 0, 12)),
            now: (9, 5),
            ..Default::default()
        };
        let (t, _) = expand_template("%<%B %b>", &ctx);
        assert_eq!(t, "January Jan");
    }

    #[test]
    fn clock_report_makes_and_remakes_a_table() {
        let mut b = org(
            "* A\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 10:00] =>  1:00\n:END:\n** B\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 10:00]--[2026-10-12 Mon 10:30] =>  0:30\n:END:\n",
        );
        let first = b.org_clock_report(0, D, (12, 0));
        assert_eq!(first, 0);
        let all = lines(&b);
        assert_eq!(all[0], "#+BEGIN: clocktable :maxlevel 2 :scope file");
        assert!(
            all.iter()
                .any(|l| l.contains("*Total time*") && l.contains("*1:30*")),
            "{all:?}"
        );
        assert!(
            all.iter()
                .any(|l| l.contains("\\_  B") && l.contains("0:30"))
        );
        // Running it inside the block rewrites rather than nests.
        let n = b.org_clock_report(2, D, (12, 5));
        assert_eq!(n, 0);
        assert_eq!(
            lines(&b)
                .iter()
                .filter(|l| l.starts_with("#+BEGIN: clocktable"))
                .count(),
            1
        );
    }

    #[test]
    fn rounding_and_stepping_back() {
        assert_eq!(round_clock_time(D, (9, 7), 15), (D, (9, 0)));
        assert_eq!(round_clock_time(D, (9, 8), 15), (D, (9, 15)));
        assert_eq!(round_clock_time(D, (23, 58), 30), (D.add_days(1), (0, 0)));
        assert_eq!(round_clock_time(D, (9, 8), 0), (D, (9, 8)));
        assert_eq!(minutes_before(D, (0, 10), 20), (D.add_days(-1), (23, 50)));
    }

    #[test]
    fn clock_line_is_recomputed() {
        let mut b = org(
            "* A\n:LOGBOOK:\n  CLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 11:30] =>  0:05\n:END:\n",
        );
        assert_eq!(b.org_update_clock_line(2).as_deref(), Some("2:30"));
        assert_eq!(
            b.line(2),
            "  CLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 11:30] =>  2:30"
        );
        assert_eq!(b.org_update_clock_line(0), None);
        let mut open = org("CLOCK: [2026-10-12 Mon 09:00]");
        assert_eq!(open.org_update_clock_line(0), None);
    }

    #[test]
    fn priorities() {
        let mut b = org("* TODO Write\n* Plain\n");
        assert_eq!(
            b.org_set_priority(0, PriorityAction::Up).unwrap(),
            Some('A')
        );
        assert_eq!(b.line(0), "* TODO [#A] Write");
        assert_eq!(
            b.org_set_priority(0, PriorityAction::Up).unwrap(),
            Some('C'),
            "wraps"
        );
        assert_eq!(
            b.org_set_priority(0, PriorityAction::Set('b')).unwrap(),
            Some('B')
        );
        assert_eq!(b.org_priority(0), Some('B'));
        b.org_set_priority(0, PriorityAction::Remove).unwrap();
        assert_eq!(b.line(0), "* TODO Write");
        b.org_set_priority(1, PriorityAction::Down).unwrap();
        assert_eq!(b.line(1), "* [#C] Plain");
    }

    #[test]
    fn templates_expand() {
        let ts = parse_capture_templates(&[
            "j :: Journal :: entry :: journal.org::Days :: * %<%Y-%m-%d> %^{Title|Untitled}\\n  %?"
                .into(),
        ])
        .unwrap();
        assert_eq!(ts[0].headline.as_deref(), Some("Days"));
        assert_eq!(
            template_prompts(&ts[0].text),
            vec![("Title".to_string(), "Untitled".to_string())]
        );
        let ctx = CaptureCtx {
            today: Some(D),
            now: (14, 30),
            answers: vec!["Hi".into()],
            ..Default::default()
        };
        let (text, cur) = expand_template(&ts[0].text, &ctx);
        assert_eq!(text, "* 2026-10-12 Hi\n  ");
        assert_eq!(cur, Some(text.chars().count()));
        let (t, _) = expand_template(
            "%U|%t|%T|%i|%x|%%",
            &CaptureCtx {
                today: Some(D),
                now: (9, 5),
                initial: "I".into(),
                kill: "K".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            t,
            "[2026-10-12 Mon 09:05]|<2026-10-12 Mon>|<2026-10-12 Mon 09:05>|I|K|%"
        );
        assert!(parse_capture_templates(&["bad".into()]).is_err());
    }

    #[test]
    fn capture_into_buffer() {
        let mut b = org("* Tasks\n** TODO old\n* Notes\n");
        let at = b.org_capture_insert(TemplateKind::Entry, Some("Tasks"), "* TODO new\n  note");
        assert_eq!(b.line(at), "** TODO new");
        assert_eq!(
            lines(&b)[..4],
            ["* Tasks", "** TODO old", "** TODO new", "  note"]
        );
        b.org_capture_insert(TemplateKind::CheckItem, Some("Notes"), "buy milk");
        assert!(lines(&b).contains(&"- [ ] buy milk".to_string()));
        // A missing headline is created.
        let mut e = org("");
        e.org_capture_insert(TemplateKind::Entry, Some("Inbox"), "* x");
        assert_eq!(lines(&e)[..2], ["* Inbox", "** x"]);
        let mut t = org("* Log\n| a |\n| b |\n");
        t.org_capture_insert(TemplateKind::TableLine, Some("Log"), "c");
        assert_eq!(t.line(3), "| c |");
    }
}
