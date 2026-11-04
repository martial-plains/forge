// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Org agenda (Phase 11, stage 4): collecting entries from buffers and
//! rendering the day/week/month/year agenda, the global TODO list, match
//! and search views and stuck projects as text. Each rendered line says
//! which entry it came from, so the editor can jump to it or act on it.

use crate::buffer::Buffer;
use crate::org::{Keywords, keywords, parse_heading};
use crate::org_ext::parse_effort;
use crate::org_time::{Date, Hm, Period, Timestamp, Unit, hmm, month_name, timestamps_in};
use regex::Regex;

/// One agenda file, already in memory.
pub struct Source<'a> {
    /// Name used for the category when the file has none.
    pub name: &'a str,
    pub buf: &'a Buffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Scheduled,
    Deadline,
    Timestamp,
    Closed,
    Clocked,
    Todo,
    Match,
    Search,
    Stuck,
}

/// One line of an agenda.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub source: usize,
    /// The heading line in its buffer.
    pub line: usize,
    pub heading: String,
    pub category: String,
    pub keyword: Option<String>,
    pub done: bool,
    /// Title with the priority cookie, without keyword or tags.
    pub title: String,
    pub tags: Vec<String>,
    pub priority: Option<char>,
    /// Minutes.
    pub effort: Option<i64>,
    pub kind: Kind,
    pub date: Option<Date>,
    pub time: Option<Hm>,
    pub end_time: Option<Hm>,
    /// "Scheduled:", "In 3 d.:", "Sched. 2x:", ...
    pub label: String,
    /// The entry's first lines of text, for entry-text mode.
    pub text: Vec<String>,
}

/// Filters, `/`, `<`, `=`, `_` and `^` in the agenda.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// `(include, tag)`.
    pub tags: Vec<(bool, String)>,
    pub category: Option<(bool, String)>,
    pub regexp: Option<(bool, String)>,
    /// `(operator, minutes)` with `<`, `>` or `=`.
    pub effort: Option<(char, i64)>,
    /// Only entries under this heading `(source, line)`.
    pub top_headline: Option<(usize, usize)>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    pub fn describe(&self) -> String {
        let mut p = Vec::new();
        for (inc, t) in &self.tags {
            p.push(format!("{}{t}", if *inc { '+' } else { '-' }));
        }
        if let Some((inc, c)) = &self.category {
            p.push(format!("{}cat:{c}", if *inc { '+' } else { '-' }));
        }
        if let Some((inc, r)) = &self.regexp {
            p.push(format!("{}/{r}/", if *inc { '+' } else { '-' }));
        }
        if let Some((op, m)) = &self.effort {
            p.push(format!("effort{op}{}", hmm(*m)));
        }
        if self.top_headline.is_some() {
            p.push("top-headline".into());
        }
        p.join(" ")
    }

    pub fn accepts(&self, e: &Entry, sources: &[Source]) -> bool {
        for (inc, t) in &self.tags {
            let has = e.tags.iter().any(|x| x.eq_ignore_ascii_case(t));
            if has != *inc {
                return false;
            }
        }
        if let Some((inc, c)) = &self.category
            && e.category.eq_ignore_ascii_case(c) != *inc
        {
            return false;
        }
        if let Some((inc, r)) = &self.regexp {
            let re = Regex::new(&format!("(?i){r}")).ok();
            let hit = re
                .is_some_and(|re| re.is_match(&e.heading) || e.text.iter().any(|l| re.is_match(l)));
            if hit != *inc {
                return false;
            }
        }
        if let Some((op, m)) = self.effort {
            match e.effort {
                None => return false,
                Some(x) => {
                    let ok = match op {
                        '<' => x < m,
                        '>' => x > m,
                        _ => x == m,
                    };
                    if !ok {
                        return false;
                    }
                }
            }
        }
        if let Some((s, l)) = self.top_headline {
            if e.source != s {
                return false;
            }
            let Some(b) = sources.get(s) else {
                return false;
            };
            let end = b.buf.org_subtree_end(l);
            if e.line < l || e.line > end {
                return false;
            }
        }
        true
    }
}

/// What a view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Day,
    Week,
    Month,
    Year,
}

impl Span {
    pub fn name(self) -> &'static str {
        match self {
            Span::Day => "Day-agenda",
            Span::Week => "Week-agenda",
            Span::Month => "Month-agenda",
            Span::Year => "Year-agenda",
        }
    }

    /// First day and number of days for a span containing `anchor`.
    pub fn range(self, anchor: Date) -> (Date, i64) {
        match self {
            Span::Day => (anchor, 1),
            Span::Week => (anchor.week_start(), 7),
            Span::Month => {
                let first = Date::new(anchor.y, anchor.m, 1);
                (first, Date::days_in_month(anchor.y, anchor.m))
            }
            Span::Year => {
                let first = Date::new(anchor.y, 1, 1);
                (first, Date::new(anchor.y + 1, 1, 1).days() - first.days())
            }
        }
    }

    /// Move the anchor one span forward or back.
    pub fn step(self, anchor: Date, forward: bool) -> Date {
        let n = if forward { 1 } else { -1 };
        match self {
            Span::Day => anchor.add_days(n),
            Span::Week => anchor.add_days(7 * n),
            Span::Month => anchor.add_months(n),
            Span::Year => anchor.add_years(n),
        }
    }
}

/// Settings for collecting a span.
#[derive(Debug, Clone)]
pub struct Options {
    /// Days before a deadline it starts showing on today.
    pub deadline_warning: i64,
    /// Show closed entries and clock lines (log mode).
    pub log: bool,
    /// Include entries tagged ARCHIVE (and in archived subtrees).
    pub archives: bool,
    /// Lines of entry text to keep per entry.
    pub text_lines: usize,
    /// The time now, for the "now" line of the time grid. None leaves it out.
    pub now: Option<Hm>,
    /// Columns the agenda is shown in: tags are flushed to the right edge
    /// (`org-agenda-tags-column` auto). 0 means 80.
    pub width: usize,
    /// `org-stuck-projects`: keywords that mark a project (empty: any open
    /// TODO keyword) and keywords of a task that keeps it from being stuck.
    /// None is the default: NEXT or STARTED.
    pub stuck: Option<(Vec<String>, Vec<String>)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            deadline_warning: 14,
            log: false,
            archives: false,
            text_lines: 3,
            now: None,
            width: 0,
            stuck: None,
        }
    }
}

/// `org-agenda-time-grid`: the hours shown on today's agenda, and the text
/// of the row.
const GRID_HOURS: [i64; 7] = [8, 10, 12, 14, 16, 18, 20];
const GRID_FILL: &str = "----------------";
/// `org-agenda-current-time-string`.
const NOW_LABEL: &str = "now - - - - - - - - - - - - - - - - - - - - - - - - -";

fn category_of(src: &Source, h: usize) -> String {
    let b = src.buf;
    let mut cur = Some(h);
    while let Some(c) = cur {
        if let Some(v) = b
            .org_properties(c)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("CATEGORY"))
        {
            return v.1;
        }
        cur = b.org_up_heading(c);
    }
    let stem = std::path::Path::new(src.name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(src.name);
    b.org_category(stem)
}

fn base_entry(src: &Source, si: usize, h: usize, kw: &Keywords, kind: Kind) -> Option<Entry> {
    let b = src.buf;
    let hd = parse_heading(&b.line(h), kw)?;
    let done = hd.keyword.as_deref().is_some_and(|k| kw.is_done(k));
    let priority = b.org_priority(h);
    let effort = b
        .org_get_property(h, "Effort", false)
        .and_then(|e| parse_effort(&e))
        .map(|hours| (hours * 60.0).round() as i64);
    let body_end = b
        .org_subtree_end(h)
        .min(next_heading(b, h).saturating_sub(1))
        .max(h);
    let text = (h + 1..=body_end)
        .map(|l| b.line(l))
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with(':')
                && !t.starts_with("CLOCK:")
                && !t.starts_with("SCHEDULED:")
                && !t.starts_with("DEADLINE:")
                && !t.starts_with("CLOSED:")
        })
        .take(3)
        .map(|l| l.trim_end().to_string())
        .collect();
    Some(Entry {
        source: si,
        line: h,
        heading: b.line(h),
        category: category_of(src, h),
        keyword: hd.keyword,
        done,
        title: strip_cookies(&hd.title),
        tags: b.org_inherited_tags(h),
        priority,
        effort,
        kind,
        date: None,
        time: None,
        end_time: None,
        label: String::new(),
        text,
    })
}

fn next_heading(b: &Buffer, h: usize) -> usize {
    (h + 1..b.line_count())
        .find(|&l| b.org_level(l).is_some())
        .unwrap_or(b.line_count())
}

fn strip_cookies(title: &str) -> String {
    let re = Regex::new(r"\s*\[(?:\d*/\d*|\d*%)\]").unwrap();
    re.replace_all(title, "").trim().to_string()
}

/// Dates a timestamp (with its repeater) falls on within `from..from+days`.
pub fn occurrences(ts: &Timestamp, from: Date, days: i64) -> Vec<Date> {
    let to = from.add_days(days - 1);
    let Some(r) = ts.repeater.as_ref() else {
        return if ts.date >= from && ts.date <= to {
            vec![ts.date]
        } else {
            Vec::new()
        };
    };
    step_dates(ts.date, r, from, to)
}

fn step_dates(base: Date, r: &Period, from: Date, to: Date) -> Vec<Date> {
    let mut out = Vec::new();
    let mut n = 0;
    loop {
        let d = match r.unit {
            Unit::Hour | Unit::Day => base.add_days(n * r.n),
            Unit::Week => base.add_days(n * r.n * 7),
            Unit::Month => base.add_months(n * r.n),
            Unit::Year => base.add_years(n * r.n),
        };
        if d > to || n > 100_000 {
            break;
        }
        if d >= from {
            out.push(d);
        }
        n += 1;
        if r.n <= 0 {
            break;
        }
    }
    out
}

/// A heading is skipped unless archives are shown.
fn archived(b: &Buffer, h: usize) -> bool {
    b.org_inherited_tags(h).iter().any(|t| t == "ARCHIVE")
}

/// Collect what the agenda for `days` days from `start` shows.
pub fn collect_span(
    sources: &[Source],
    start: Date,
    days: i64,
    today: Date,
    opts: &Options,
) -> Vec<Entry> {
    let mut out = Vec::new();
    let end = start.add_days(days - 1);
    for (si, src) in sources.iter().enumerate() {
        let b = src.buf;
        let kw = keywords(b);
        for h in 0..b.line_count() {
            if b.org_level(h).is_none() {
                continue;
            }
            if !opts.archives && archived(b, h) {
                continue;
            }
            let Some(base) = base_entry(src, si, h, &kw, Kind::Timestamp) else {
                continue;
            };
            let plan = b.org_planning(h);
            let planning_line = b.org_planning_line(h);
            if let Some(s) = plan.scheduled.as_ref().filter(|_| !base.done) {
                for d in occurrences(s, start, days) {
                    let mut e = base.clone();
                    e.kind = Kind::Scheduled;
                    e.date = Some(d);
                    e.time = s.time;
                    e.end_time = s.end_time;
                    e.label = "Scheduled:".into();
                    out.push(e);
                }
                // Overdue items show on today.
                if today >= start && today <= end && s.date < today && s.repeater.is_none() {
                    let mut e = base.clone();
                    e.kind = Kind::Scheduled;
                    e.date = Some(today);
                    e.label = format!("Sched.{:>2}x:", today.days() - s.date.days());
                    out.push(e);
                }
            }
            if let Some(dl) = plan.deadline.as_ref().filter(|_| !base.done) {
                for d in occurrences(dl, start, days) {
                    let mut e = base.clone();
                    e.kind = Kind::Deadline;
                    e.date = Some(d);
                    e.time = dl.time;
                    e.end_time = dl.end_time;
                    e.label = if d == today {
                        "Deadline:".into()
                    } else if d < today {
                        format!("{:>2} d. ago:", today.days() - d.days())
                    } else {
                        "Deadline:".into()
                    };
                    out.push(e);
                }
                if today >= start && today <= end && dl.repeater.is_none() {
                    let left = dl.date.days() - today.days();
                    let warn = dl
                        .warning
                        .as_ref()
                        .map_or(opts.deadline_warning, Period::days);
                    if left < 0 || (left > 0 && left <= warn) {
                        // Past deadlines already show on their own day if it's in
                        // the span; show on today either way.
                        let mut e = base.clone();
                        e.kind = Kind::Deadline;
                        e.date = Some(today);
                        e.label = if left < 0 {
                            format!("{:>2} d. ago:", -left)
                        } else {
                            format!("In {left:>3} d.:")
                        };
                        // Avoid repeating the same row when the deadline day is today.
                        if !(left == 0)
                            && !(dl.date >= start && dl.date <= end && left < 0 && dl.date == today)
                        {
                            out.push(e);
                        }
                    }
                }
            }
            // Plain active timestamps in the heading and body (not planning
            // or clock lines).
            let body_end = next_heading(b, h).saturating_sub(1);
            for l in h..=body_end.min(b.line_count().saturating_sub(1)) {
                if Some(l) == planning_line || b.line(l).trim_start().starts_with("CLOCK:") {
                    continue;
                }
                let text = b.line(l);
                let stamps = timestamps_in(&text);
                // `<a>--<b>` ranges.
                let mut skip_next = false;
                for (k, ts) in stamps.iter().enumerate() {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if !ts.active || base.done {
                        continue;
                    }
                    let range_end = stamps.get(k + 1).filter(|n| {
                        n.active
                            && text[..]
                                .chars()
                                .skip(ts.end)
                                .take(n.start.saturating_sub(ts.end))
                                .collect::<String>()
                                == "--"
                    });
                    if let Some(re) = range_end {
                        skip_next = true;
                        let (a, z) = (ts.date, re.date);
                        let total = z.days() - a.days() + 1;
                        for i in 0..total {
                            let d = a.add_days(i);
                            if d >= start && d <= end {
                                let mut e = base.clone();
                                e.kind = Kind::Timestamp;
                                e.date = Some(d);
                                e.label = format!("({}/{total}):", i + 1);
                                out.push(e);
                            }
                        }
                        continue;
                    }
                    for d in occurrences(ts, start, days) {
                        let mut e = base.clone();
                        e.kind = Kind::Timestamp;
                        e.date = Some(d);
                        e.time = ts.time;
                        e.end_time = ts.end_time;
                        out.push(e);
                    }
                }
            }
            // Log mode: closed entries and clock lines.
            if opts.log {
                if let Some(c) = plan.closed.as_ref()
                    && c.date >= start
                    && c.date <= end
                {
                    let mut e = base.clone();
                    e.kind = Kind::Closed;
                    e.date = Some(c.date);
                    e.time = c.time;
                    e.label = "Closed:".into();
                    out.push(e);
                }
                for cl in b.org_clock_lines(h) {
                    if cl.start.date >= start && cl.start.date <= end {
                        let mut e = base.clone();
                        e.kind = Kind::Clocked;
                        e.date = Some(cl.start.date);
                        e.time = cl.start.time;
                        e.label = match &cl.end {
                            Some(end) => format!(
                                "Clocked: ({})",
                                hmm((end.minutes() - cl.start.minutes()).max(0))
                            ),
                            None => "Clocked: (running)".into(),
                        };
                        out.push(e);
                    }
                }
            }
        }
    }
    out
}

/// Collect for the global TODO list. `keyword` narrows to one keyword.
pub fn collect_todos(sources: &[Source], keyword: Option<&str>, opts: &Options) -> Vec<Entry> {
    let mut out = Vec::new();
    // Where each entry's keyword sits in its file's sequence, for the sort.
    let mut rank: Vec<usize> = Vec::new();
    for (si, src) in sources.iter().enumerate() {
        let b = src.buf;
        let kw = keywords(b);
        for h in 0..b.line_count() {
            if b.org_level(h).is_none() || (!opts.archives && archived(b, h)) {
                continue;
            }
            let Some(mut e) = base_entry(src, si, h, &kw, Kind::Todo) else {
                continue;
            };
            let Some(k) = e.keyword.clone() else { continue };
            if e.done || keyword.is_some_and(|w| w != k) {
                continue;
            }
            let plan = b.org_planning(h);
            e.date = plan
                .scheduled
                .as_ref()
                .or(plan.deadline.as_ref())
                .map(|t| t.date);
            rank.push(kw.todo.iter().position(|w| *w == k).unwrap_or(0));
            out.push(e);
        }
    }
    sort_todos(&mut out, &rank);
    out
}

/// `(todo-state-down priority-down category-keep)`: later states
/// first, then the higher priority, and otherwise the order they were found
/// in (file by file, top to bottom).
fn sort_todos(v: &mut Vec<Entry>, rank: &[usize]) {
    let mut keyed: Vec<(usize, Entry)> = rank.iter().copied().zip(v.drain(..)).collect();
    keyed.sort_by_key(|(r, e)| (std::cmp::Reverse(*r), e.priority.unwrap_or('B')));
    v.extend(keyed.into_iter().map(|(_, e)| e));
}

/// Collect entries matching a tags/properties/todo match string.
pub fn collect_match(
    sources: &[Source],
    expr: &str,
    todo_only: bool,
    opts: &Options,
) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (si, src) in sources.iter().enumerate() {
        let b = src.buf;
        let kw = keywords(b);
        for h in b.org_match_entries(expr, todo_only)? {
            if !opts.archives && archived(b, h) {
                continue;
            }
            if let Some(mut e) = base_entry(src, si, h, &kw, Kind::Match) {
                let plan = b.org_planning(h);
                e.date = plan
                    .scheduled
                    .as_ref()
                    .or(plan.deadline.as_ref())
                    .map(|t| t.date);
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// `org-search-view`: every word must appear (`+word` / `-word` for
/// require / forbid, `{regexp}` for a regexp) in the entry's text.
pub fn collect_search(
    sources: &[Source],
    query: &str,
    opts: &Options,
) -> Result<Vec<Entry>, String> {
    enum Term {
        Must(Regex),
        Not(Regex),
    }
    let mut terms = Vec::new();
    for tok in query.split_whitespace() {
        let (neg, body) = match tok.chars().next() {
            Some('-') => (true, &tok[1..]),
            Some('+') => (false, &tok[1..]),
            _ => (false, tok),
        };
        let pat = match body.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
            Some(re) => re.to_string(),
            None => regex::escape(body),
        };
        let re =
            Regex::new(&format!("(?i){pat}")).map_err(|e| format!("Bad search term {tok}: {e}"))?;
        terms.push(if neg { Term::Not(re) } else { Term::Must(re) });
    }
    if terms.is_empty() {
        return Err("Empty search".into());
    }
    let mut out = Vec::new();
    for (si, src) in sources.iter().enumerate() {
        let b = src.buf;
        let kw = keywords(b);
        for h in 0..b.line_count() {
            if b.org_level(h).is_none() || (!opts.archives && archived(b, h)) {
                continue;
            }
            let end = next_heading(b, h)
                .saturating_sub(1)
                .min(b.line_count().saturating_sub(1));
            let text: String = (h..=end).map(|l| b.line(l)).collect::<Vec<_>>().join("\n");
            let ok = terms.iter().all(|t| match t {
                Term::Must(re) => re.is_match(&text),
                Term::Not(re) => !re.is_match(&text),
            });
            if ok && let Some(e) = base_entry(src, si, h, &kw, Kind::Search) {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// Stuck projects: a TODO entry with sub-entries, none of which is NEXT
/// or STARTED, and which is not itself done.
pub fn collect_stuck(sources: &[Source], opts: &Options) -> Vec<Entry> {
    let mut out = Vec::new();
    for (si, src) in sources.iter().enumerate() {
        let b = src.buf;
        let kw = keywords(b);
        for h in 0..b.line_count() {
            if b.org_level(h).is_none() || (!opts.archives && archived(b, h)) {
                continue;
            }
            let Some(e) = base_entry(src, si, h, &kw, Kind::Stuck) else {
                continue;
            };
            if e.done || e.keyword.is_none() {
                continue;
            }
            if let Some((projects, _)) = &opts.stuck
                && !projects.is_empty()
                && !projects.iter().any(|p| Some(p) == e.keyword.as_ref())
            {
                continue;
            }
            let end = b.org_subtree_end(h);
            let subs: Vec<Option<String>> = (h + 1..=end)
                .filter(|&l| b.org_level(l).is_some())
                .map(|l| parse_heading(&b.line(l), &kw).and_then(|hd| hd.keyword))
                .collect();
            if subs.is_empty() {
                continue;
            }
            let active = subs.iter().flatten().any(|k| match &opts.stuck {
                Some((_, active)) => active.contains(k),
                None => matches!(k.as_str(), "NEXT" | "STARTED"),
            });
            if !active {
                out.push(e);
            }
        }
    }
    out
}

// -------------------------------------------------------------- rendering

/// A rendered agenda: text lines and, per line, the entry it shows.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub lines: Vec<String>,
    /// Parallel to `lines`: index into `entries`.
    pub refs: Vec<Option<usize>>,
    pub entries: Vec<Entry>,
    /// Per line: the date a day header stands for.
    pub dates: Vec<Option<Date>>,
}

impl Rendered {
    fn push(&mut self, line: String, r: Option<usize>, date: Option<Date>) {
        self.lines.push(line);
        self.refs.push(r);
        self.dates.push(date);
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

fn time_text(e: &Entry) -> String {
    match (e.time, e.end_time) {
        (Some((h, m)), Some((h2, m2))) => format!("{h:>2}:{m:02}-{h2:>2}:{m2:02}"),
        (Some((h, m)), None) => format!("{h:>2}:{m:02}......"),
        _ => String::new(),
    }
}

fn item_text(e: &Entry) -> String {
    let mut s = String::new();
    if let Some(k) = &e.keyword {
        s.push_str(k);
        s.push(' ');
    }
    // The title still has its `[#A]` cookie.
    s.push_str(&e.title);
    if !e.tags.is_empty() {
        s.push_str(&format!("  :{}:", e.tags.join(":")));
    }
    s
}

/// `line` (ending in the entry's heading) with the entry's tags moved to
/// the right edge aligns them. Entries without tags are
/// left alone.
fn flush_tags(line: String, e: &Entry, width: usize) -> String {
    if e.tags.is_empty() {
        return line;
    }
    let tags = format!(":{}:", e.tags.join(":"));
    let Some(at) = line.rfind(&format!("  {tags}")) else {
        return line;
    };
    let head = line[..at].to_string();
    let edge = if width == 0 { 80 } else { width }.saturating_sub(1);
    let gap = edge
        .saturating_sub(crate::width::line_width(&head) + crate::width::line_width(&tags))
        .max(1);
    format!("{head}{}{tags}", " ".repeat(gap))
}

fn item_line(e: &Entry) -> String {
    // " %i %-12:c%?-12t% s": the category, the time of day padded
    // to 12 when there is one, then the leader ("Scheduled:", "In   2 d.:")
    // with a space after it, so the heading never runs into either.
    let t = time_text(e);
    let when = format!(
        "{}{}",
        if t.is_empty() {
            String::new()
        } else {
            format!("{t:<12}")
        },
        if e.label.is_empty() {
            String::new()
        } else {
            format!("{:<12}", e.label)
        },
    );
    format!(
        "  {:<12}{}{}",
        format!("{}:", e.category),
        when,
        item_text(e)
    )
}

fn order_in_day(e: &Entry) -> (u8, i64, char, String) {
    let t = e.time.map_or(i64::MAX, |(h, m)| h * 60 + m);
    let group = match e.kind {
        Kind::Timestamp if e.time.is_some() => 0,
        _ => 1,
    };
    (group, t, e.priority.unwrap_or('B'), e.category.clone())
}

/// Render a day/week/month/year agenda. Entries are filtered first.
pub fn render_span(
    sources: &[Source],
    span: Span,
    anchor: Date,
    today: Date,
    opts: &Options,
    filter: &Filter,
    show_text: bool,
) -> Rendered {
    let (start, days) = span.range(anchor);
    let mut entries = collect_span(sources, start, days, today, opts);
    entries.retain(|e| filter.accepts(e, sources));
    let mut r = Rendered::default();
    let title = match span {
        Span::Week => format!("{} (W{:02}):", span.name(), start.iso_week()),
        Span::Month => format!("{} ({} {}):", span.name(), month_name(start.m), start.y),
        Span::Year => format!("{} ({}):", span.name(), start.y),
        Span::Day => format!("{}:", span.name()),
    };
    r.push(title, None, None);
    for i in 0..days {
        let d = start.add_days(i);
        let mut day: Vec<&Entry> = entries.iter().filter(|e| e.date == Some(d)).collect();
        day.sort_by_key(|e| order_in_day(e));
        let header = format!(
            "{:<10} {:>2} {} {}{}",
            long_day(d),
            d.d,
            month_name(d.m),
            d.y,
            ""
        );
        if span == Span::Month || span == Span::Year {
            // Long spans skip empty days.
            if day.is_empty() {
                continue;
            }
        }
        r.push(header, None, Some(d));
        // The time grid: today only, and only when something that day has
        // a time (`(daily today require-timed)`). Rows for the hours no
        // entry sits on, plus the "now" line, merge in by time of day.
        let grid_day = matches!(span, Span::Day | Span::Week)
            && d == today
            && day.iter().any(|e| e.time.is_some());
        let mut grid: Vec<(i64, String)> = Vec::new();
        if grid_day {
            let taken: Vec<i64> = day
                .iter()
                .filter_map(|e| e.time.map(|(h, m)| h * 60 + m))
                .collect();
            for h in GRID_HOURS {
                if !taken.contains(&(h * 60)) {
                    grid.push((h * 60, grid_line(h * 60, GRID_FILL)));
                }
            }
            if let Some((h, m)) = opts.now {
                grid.push((h * 60 + m, grid_line(h * 60 + m, NOW_LABEL)));
            }
            grid.sort_by_key(|g| g.0);
        }
        let mut grid = grid.into_iter().peekable();
        for e in day {
            let t = e.time.map_or(i64::MAX, |(h, m)| h * 60 + m);
            while let Some((_, line)) = grid.next_if(|(gt, _)| *gt <= t) {
                r.push(line, None, Some(d));
            }
            let idx = r.entries.len();
            r.entries.push(e.clone());
            r.push(flush_tags(item_line(e), e, opts.width), Some(idx), Some(d));
            if show_text {
                for t in &e.text {
                    r.push(format!("{:<26}{}", "", t.trim()), Some(idx), Some(d));
                }
            }
        }
        for (_, line) in grid {
            r.push(line, None, Some(d));
        }
    }
    r
}

/// A time-grid row: no category, the time with dots, then `label`.
fn grid_line(minutes: i64, label: &str) -> String {
    let t = format!("{:>2}:{:02}......", minutes / 60, minutes % 60);
    format!("  {:<12}{:<12}{label}", "", t)
}

fn long_day(d: Date) -> &'static str {
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ][d.weekday()]
}

/// Render a list of entries that have no day structure.
pub fn render_list(
    title: &str,
    entries: Vec<Entry>,
    sources: &[Source],
    filter: &Filter,
    show_text: bool,
) -> Rendered {
    render_list_in(title, entries, sources, filter, show_text, 0)
}

/// `render_list` for a window `opts_width` columns wide (tags go to its
/// right edge).
pub fn render_list_in(
    title: &str,
    mut entries: Vec<Entry>,
    sources: &[Source],
    filter: &Filter,
    show_text: bool,
    opts_width: usize,
) -> Rendered {
    entries.retain(|e| filter.accepts(e, sources));
    let mut r = Rendered::default();
    r.push(title.to_string(), None, None);
    for e in entries {
        let idx = r.entries.len();
        r.entries.push(e.clone());
        // " %i %-12:c": the category and the heading, no date.
        let mut line = format!("  {:<12}", format!("{}:", e.category));
        line.push_str(&item_text(&e));
        let line = flush_tags(line, &e, opts_width);
        r.push(line, Some(idx), None);
        if show_text {
            for t in &e.text {
                r.push(format!("{:<24}{}", "", t.trim()), Some(idx), None);
            }
        }
    }
    r
}

/// The time at the agenda's front: when it was made.
pub fn stamp_line(today: Date, now: Hm) -> String {
    format!("Agenda made {} {:02}:{:02}", today.iso(), now.0, now.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: Date = Date {
        y: 2026,
        m: 10,
        d: 12,
    }; // a Monday

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("plan.org");
        b.set_contents(text);
        b
    }

    fn src(b: &Buffer) -> Vec<Source<'_>> {
        vec![Source {
            name: "plan.org",
            buf: b,
        }]
    }

    const DOC: &str = "#+CATEGORY: work\n\
* TODO Write report\nSCHEDULED: <2026-10-12 Mon 10:00>\n\
* TODO [#A] File taxes :money:\nDEADLINE: <2026-10-15 Thu>\n\
* TODO Old thing\nSCHEDULED: <2026-10-09 Fri>\n\
* Meeting\n<2026-10-13 Tue 14:00-15:00>\n\
* DONE Finished\nCLOSED: [2026-10-12 Mon 08:00] SCHEDULED: <2026-10-12 Mon>\n\
* TODO Weekly\nSCHEDULED: <2026-10-05 Mon +1w>\n\
* Trip\n<2026-10-14 Wed>--<2026-10-16 Fri>\n";

    #[test]
    fn week_agenda_places_everything() {
        let b = buf(DOC);
        let s = src(&b);
        let r = render_span(
            &s,
            Span::Week,
            TODAY,
            TODAY,
            &Options::default(),
            &Filter::default(),
            false,
        );
        let text = r.text();
        assert!(text.starts_with("Week-agenda (W42):"), "{text}");
        assert!(text.contains("Monday     12 October 2026"));
        // Monday: the timed task, the overdue one, the weekly repeat.
        let monday: Vec<&String> = r
            .lines
            .iter()
            .skip(2)
            .filter(|l| !l.contains("-------"))
            .take(4)
            .collect();
        assert!(
            monday[0].contains("10:00...... Scheduled:") && monday[0].contains("Write report"),
            "{monday:?}"
        );
        assert!(text.contains("Sched. 3x:") && text.contains("Old thing"));
        assert!(text.contains("Weekly"));
        // Deadline shows on its day and as a warning today.
        assert!(text.contains("In   3 d.:") && text.contains("[#A] File taxes"));
        assert!(
            text.lines()
                .any(|l| l.contains("Deadline:") && l.contains("File taxes"))
        );
        // The timed meeting and the three-day trip.
        assert!(text.contains("14:00-15:00") && text.contains("Meeting"));
        assert!(text.contains("(1/3):") && text.contains("(3/3):"));
        // Done entries stay out without log mode.
        assert!(!text.contains("Finished"));
        assert!(text.contains(":money:"));
    }

    #[test]
    fn agenda_month_with_an_invalid_date_is_not_a_panic() {
        let b = buf(DOC);
        let s = src(&b);
        let r = render_span(
            &s,
            Span::Month,
            Date::new(2026, 0, 12),
            Date::new(2026, 0, 12),
            &Options::default(),
            &Filter::default(),
            false,
        );
        assert!(
            r.text().starts_with("Month-agenda (January 2026):"),
            "{}",
            r.text()
        );
    }

    #[test]
    fn log_mode_filters_and_text() {
        let b = buf(DOC);
        let s = src(&b);
        let log = Options {
            log: true,
            ..Default::default()
        };
        let r = render_span(&s, Span::Day, TODAY, TODAY, &log, &Filter::default(), false);
        assert!(r.text().contains("Closed:") && r.text().contains("Finished"));
        let f = Filter {
            tags: vec![(true, "money".into())],
            ..Default::default()
        };
        let r = render_span(&s, Span::Week, TODAY, TODAY, &Options::default(), &f, false);
        assert!(r.text().contains("File taxes") && !r.text().contains("Meeting"));
        let f = Filter {
            category: Some((false, "work".into())),
            ..Default::default()
        };
        let r = render_span(&s, Span::Week, TODAY, TODAY, &Options::default(), &f, false);
        assert!(!r.text().contains("File taxes"));
        assert_eq!(f.describe(), "-cat:work");
    }

    #[test]
    fn todo_match_search_stuck() {
        let b = buf(
            "#+TODO: TODO NEXT | DONE\n* TODO [#C] low\n* TODO [#A] high :x:\nbody words\n* Proj\n** TODO sub\n* TODO P2\n** NEXT go\n* TODO P3\n** TODO later\n",
        );
        let s = src(&b);
        let opts = Options::default();
        let todos = collect_todos(&s, None, &opts);
        // Later states first, then priority.
        assert_eq!(todos[0].title, "go", "NEXT comes before TODO");
        assert_eq!(todos[1].title, "[#A] high", "priority order");
        assert_eq!(collect_todos(&s, Some("NEXT"), &opts).len(), 1);
        let m = collect_match(&s, "x", false, &opts).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(collect_search(&s, "body +words", &opts).unwrap().len(), 1);
        assert_eq!(collect_search(&s, "body -words", &opts).unwrap().len(), 0);
        let stuck = collect_stuck(&s, &opts);
        assert_eq!(stuck.len(), 1);
        assert_eq!(stuck[0].title, "P3");
        let r = render_list(
            "Global list of TODO items of type: ALL",
            todos,
            &s,
            &Filter::default(),
            false,
        );
        assert!(
            r.lines[1].contains("NEXT go") && r.lines[2].contains("TODO [#A] high"),
            "{:?}",
            r.lines
        );
    }

    #[test]
    fn occurrences_and_spans() {
        let ts = timestamps_in("<2026-10-05 Mon +1w>").remove(0);
        let d = occurrences(&ts, Date::new(2026, 10, 10), 14);
        assert_eq!(d, vec![Date::new(2026, 10, 12), Date::new(2026, 10, 19)]);
        assert_eq!(Span::Week.range(TODAY), (Date::new(2026, 10, 12), 7));
        assert_eq!(Span::Month.range(TODAY), (Date::new(2026, 10, 1), 31));
        assert_eq!(Span::Week.step(TODAY, false), Date::new(2026, 10, 5));
    }
}
