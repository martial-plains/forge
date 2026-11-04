// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org semantics beyond the outline (Phase 11, stage 2): tags, property
//! drawers, planning lines and timestamps, TODO side effects, links,
//! drawers and blocks. Everything is a method on [`Buffer`], in the same
//! style as `org.rs`; the editor binds keys and asks questions.

use crate::buffer::{Buffer, Pos};
use crate::org::{TAGS, keywords, parse_heading};
use crate::org_time::{
    Date, Field, Hm, Timestamp, adjust, field_at, inactive_now, timestamp_at, timestamps_in,
};
use regex::Regex;
use std::sync::LazyLock;

/// Tags are right-aligned so the last colon sits in this column.
pub const TAGS_COLUMN: usize = 77;

// ------------------------------------------------------------------ tags

/// `#+TAGS:` / `#+FILETAGS:` words.
fn option_words(buf: &Buffer, key: &str) -> Vec<String> {
    let prefix = format!("#+{key}:");
    (0..buf.line_count())
        .map(|l| buf.line(l))
        .filter(|l| {
            l.get(..prefix.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(&prefix))
        })
        .flat_map(|l| {
            l[prefix.len()..]
                .split_whitespace()
                .map(|w| {
                    w.split('(')
                        .next()
                        .unwrap_or(w)
                        .trim_matches([':', '{', '}'])
                        .to_string()
                })
                .filter(|w| !w.is_empty())
                .flat_map(|w| w.split(':').map(String::from).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[derive(Debug, Clone)]
enum Atom {
    Tag(String),
    TagRe(Regex),
    Prop {
        name: String,
        op: String,
        value: String,
    },
}

#[derive(Debug, Clone)]
struct Factor {
    negate: bool,
    atom: Atom,
}

/// OR of ANDs.
type MatchExpr = Vec<Vec<Factor>>;

fn parse_match(expr: &str) -> Result<MatchExpr, String> {
    let chars: Vec<char> = expr.chars().collect();
    let mut groups: MatchExpr = vec![Vec::new()];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '|' {
            groups.push(Vec::new());
            i += 1;
            continue;
        }
        let mut negate = false;
        if matches!(c, '+' | '-' | '&') {
            negate = c == '-';
            i += 1;
        }
        let start = i;
        let (mut in_quote, mut in_brace) = (false, false);
        while i < chars.len() {
            let ch = chars[i];
            if ch == '"' {
                in_quote = !in_quote;
            } else if ch == '{' && !in_quote {
                in_brace = true;
            } else if ch == '}' && !in_quote {
                in_brace = false;
            } else if !in_quote && !in_brace && matches!(ch, '+' | '-' | '&' | '|') {
                break;
            }
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        if word.is_empty() {
            continue;
        }
        let atom = if word.starts_with('{') && word.ends_with('}') && word.len() >= 2 {
            Atom::TagRe(Regex::new(&word[1..word.len() - 1]).map_err(|e| e.to_string())?)
        } else if let Some(p) = word.find(['=', '<', '>']) {
            let name = word[..p].to_string();
            let rest = &word[p..];
            let op_len =
                if rest.starts_with("<>") || rest.starts_with("<=") || rest.starts_with(">=") {
                    2
                } else {
                    1
                };
            Atom::Prop {
                name,
                op: rest[..op_len].to_string(),
                value: rest[op_len..].trim_matches('"').to_string(),
            }
        } else {
            Atom::Tag(word)
        };
        groups
            .last_mut()
            .expect("a group")
            .push(Factor { negate, atom });
    }
    groups.retain(|g| !g.is_empty());
    if groups.is_empty() {
        groups.push(Vec::new()); // matches everything
    }
    Ok(groups)
}

fn compare(a: &str, op: &str, b: &str) -> bool {
    let ord = match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y),
        _ => Some(a.cmp(b)),
    };
    let Some(ord) = ord else { return false };
    use std::cmp::Ordering::*;
    match op {
        "=" => ord == Equal,
        "<>" => ord != Equal,
        "<" => ord == Less,
        ">" => ord == Greater,
        "<=" => ord != Greater,
        ">=" => ord != Less,
        _ => false,
    }
}

impl Buffer {
    /// The tags written on heading `h`.
    pub fn org_heading_tags(&self, h: usize) -> Vec<String> {
        parse_heading(&self.line(h), &keywords(self)).map_or_else(Vec::new, |hd| hd.tags)
    }

    /// Own tags plus the ancestors' and `#+FILETAGS:`.
    pub fn org_inherited_tags(&self, h: usize) -> Vec<String> {
        let mut out = option_words(self, "FILETAGS");
        let mut chain = vec![h];
        let mut cur = h;
        while let Some(p) = self.org_up_heading(cur) {
            chain.push(p);
            cur = p;
        }
        for hh in chain.into_iter().rev() {
            for t in self.org_heading_tags(hh) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// Every tag in use or declared with `#+TAGS:`, sorted.
    pub fn org_all_tags(&self) -> Vec<String> {
        let mut all = option_words(self, "TAGS");
        all.extend(option_words(self, "FILETAGS"));
        for l in 0..self.line_count() {
            if self.org_level(l).is_some() {
                all.extend(self.org_heading_tags(l));
            }
        }
        all.sort();
        all.dedup();
        all
    }

    /// Replace the tags of heading `h` (empty removes them), aligned.
    pub fn org_set_tags(&mut self, h: usize, tags: &[String]) {
        let text = self.line(h);
        let base = match TAGS.find(&text) {
            Some(m) => text[..m.start()].trim_end().to_string(),
            None => text.trim_end().to_string(),
        };
        let new = if tags.is_empty() {
            base
        } else {
            let tagstr = format!(":{}:", tags.join(":"));
            let used = base.chars().count() + tagstr.chars().count();
            let pad = TAGS_COLUMN.saturating_sub(used).max(1);
            format!("{base}{}{tagstr}", " ".repeat(pad))
        };
        self.org_set_line(h, &new);
    }

    /// Add `tag` to heading `h` or remove it. True if it was added.
    pub fn org_toggle_tag(&mut self, h: usize, tag: &str) -> bool {
        let mut tags = self.org_heading_tags(h);
        let added = if let Some(i) = tags.iter().position(|t| t == tag) {
            tags.remove(i);
            false
        } else {
            tags.push(tag.to_string());
            true
        };
        self.org_set_tags(h, &tags);
        added
    }

    /// Replace line `l` with `text` (no newline).
    pub fn org_set_line(&mut self, l: usize, text: &str) {
        let (a, z) = (self.line_start_offset(l), self.line_end_offset(l));
        if self.text_offsets(a, z) != text {
            self.replace_offsets(a, z, text);
        }
    }

    /// Insert `text` as a new line after line `l`.
    pub fn org_insert_line_after(&mut self, l: usize, text: &str) {
        let at = self.line_end_offset(l);
        self.replace_offsets(at, at, &format!("\n{text}"));
    }

    /// Delete line `l` entirely.
    pub fn org_delete_line(&mut self, l: usize) {
        let n = self.line_count();
        if l + 1 < n {
            let (a, z) = (self.line_start_offset(l), self.line_start_offset(l + 1));
            self.replace_offsets(a, z, "");
        } else if l > 0 {
            let (a, z) = (self.line_end_offset(l - 1), self.line_end_offset(l));
            self.replace_offsets(a, z, "");
        } else {
            let (a, z) = (self.line_start_offset(l), self.line_end_offset(l));
            self.replace_offsets(a, z, "");
        }
    }

    /// Headings (all, or only TODO ones) matching a tags/properties/todo
    /// match string such as `work+urgent-home`, `{^pro}`, `LEVEL=2`,
    /// `TODO="NEXT"|+Effort>=2`.
    pub fn org_match_entries(&self, expr: &str, todo_only: bool) -> Result<Vec<usize>, String> {
        let m = parse_match(expr)?;
        let kw = keywords(self);
        let mut out = Vec::new();
        for h in 0..self.line_count() {
            let Some(hd) =
                parse_heading(&self.line(h), &kw).filter(|_| self.org_level(h).is_some())
            else {
                continue;
            };
            if todo_only && !hd.keyword.as_deref().is_some_and(|k| !kw.is_done(k)) {
                continue;
            }
            let tags = self.org_inherited_tags(h);
            let eval = |f: &Factor| -> bool {
                let v = match &f.atom {
                    Atom::Tag(t) => tags.contains(t),
                    Atom::TagRe(re) => tags.iter().any(|t| re.is_match(t)),
                    Atom::Prop { name, op, value } => {
                        let have = match name.as_str() {
                            "TODO" => hd.keyword.clone().unwrap_or_default(),
                            "LEVEL" => hd.level.to_string(),
                            n => self.org_get_property(h, n, true).unwrap_or_default(),
                        };
                        compare(&have, op, value)
                    }
                };
                v != f.negate
            };
            if m.iter().any(|g| g.iter().all(&eval)) {
                out.push(h);
            }
        }
        Ok(out)
    }

    /// `org-tags-sparse-tree` / `org-match-sparse-tree`: fold everything
    /// but the matching entries. Returns how many matched.
    pub fn org_match_sparse_tree(&mut self, expr: &str, todo_only: bool) -> Result<usize, String> {
        let hits = self.org_match_entries(expr, todo_only)?;
        if hits.is_empty() {
            return Ok(0);
        }
        let mut matched = vec![false; self.line_count()];
        for &h in &hits {
            matched[h] = true;
        }
        self.org_apply_sparse(&matched);
        Ok(hits.len())
    }
}

// ------------------------------------------------------------ properties

static PROP_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*:([^\s:]+?)(\+?):(?:\s+(.*?))?\s*$").unwrap());
static PLANNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:(?:SCHEDULED|DEADLINE|CLOSED):\s*[<\[][^>\]]*[>\]]\s*)+$").unwrap()
});

impl Buffer {
    /// The line holding the SCHEDULED/DEADLINE/CLOSED entries of heading
    /// `h`, if there is one (right under the heading).
    pub fn org_planning_line(&self, h: usize) -> Option<usize> {
        (h + 1 < self.line_count() && PLANNING.is_match(&self.line(h + 1))).then_some(h + 1)
    }

    /// Lines `(first, last)` of heading `h`'s property drawer, from the
    /// `:PROPERTIES:` line to the `:END:` line.
    pub fn org_property_drawer(&self, h: usize) -> Option<(usize, usize)> {
        let first = self.org_planning_line(h).map_or(h + 1, |p| p + 1);
        if first >= self.line_count()
            || !self.line(first).trim().eq_ignore_ascii_case(":PROPERTIES:")
        {
            return None;
        }
        let limit = self.org_body_end(h);
        (first + 1..=limit)
            .find(|&l| self.line(l).trim().eq_ignore_ascii_case(":END:"))
            .map(|end| (first, end))
    }

    /// Own properties of heading `h`, in order.
    pub fn org_properties(&self, h: usize) -> Vec<(String, String)> {
        let Some((a, z)) = self.org_property_drawer(h) else {
            return Vec::new();
        };
        (a + 1..z)
            .filter_map(|l| {
                let line = self.line(l);
                let c = PROP_LINE.captures(&line)?;
                Some((
                    format!("{}{}", &c[1], &c[2]),
                    c.get(3).map_or(String::new(), |m| m.as_str().to_string()),
                ))
            })
            .collect()
    }

    /// `#+PROPERTY: NAME value` lines.
    fn org_file_property(&self, name: &str) -> Option<String> {
        (0..self.line_count()).find_map(|l| {
            let line = self.line(l);
            let rest = line
                .get(..11)
                .filter(|p| p.eq_ignore_ascii_case("#+PROPERTY:"))
                .map(|_| &line[11..])?;
            let rest = rest.trim();
            let (n, v) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            n.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }

    /// A property of heading `h`; with `inherit`, from the nearest
    /// ancestor (then the file's `#+PROPERTY:`) when `h` has none.
    pub fn org_get_property(&self, h: usize, name: &str, inherit: bool) -> Option<String> {
        let own = |hh: usize| {
            self.org_properties(hh)
                .into_iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v)
        };
        if let Some(v) = own(h) {
            return Some(v);
        }
        if !inherit {
            return None;
        }
        let mut cur = h;
        while let Some(p) = self.org_up_heading(cur) {
            if let Some(v) = own(p) {
                return Some(v);
            }
            cur = p;
        }
        self.org_file_property(name)
    }

    /// `org-insert-property-drawer`: an empty drawer under the heading
    /// (and its planning line). Returns the line of `:PROPERTIES:`.
    pub fn org_insert_property_drawer(&mut self, h: usize) -> usize {
        if let Some((a, _)) = self.org_property_drawer(h) {
            return a;
        }
        let after = self.org_planning_line(h).unwrap_or(h);
        self.org_insert_line_after(after, ":PROPERTIES:\n:END:");
        after + 1
    }

    /// `org-set-property`: set `name` on heading `h`, making the drawer if
    /// needed. Keeps the drawer's alignment style (`:NAME:  value`).
    pub fn org_set_property(&mut self, h: usize, name: &str, value: &str) {
        let name = name.trim().trim_matches(':');
        let a = self.org_insert_property_drawer(h);
        let z = self.org_property_drawer(h).map_or(a + 1, |(_, z)| z);
        let new = if value.is_empty() {
            format!(":{name}:")
        } else {
            format!(":{name}: {value}")
        };
        for l in a + 1..z {
            if PROP_LINE
                .captures(&self.line(l))
                .is_some_and(|c| format!("{}{}", &c[1], &c[2]).eq_ignore_ascii_case(name))
            {
                self.org_set_line(l, &new);
                return;
            }
        }
        self.org_insert_line_after(z - 1, &new);
    }

    /// `org-delete-property`. False if there was no such property.
    pub fn org_delete_property(&mut self, h: usize, name: &str) -> bool {
        let name = name.trim().trim_matches(':');
        let Some((a, z)) = self.org_property_drawer(h) else {
            return false;
        };
        let Some(l) = (a + 1..z).find(|&l| {
            PROP_LINE
                .captures(&self.line(l))
                .is_some_and(|c| format!("{}{}", &c[1], &c[2]).eq_ignore_ascii_case(name))
        }) else {
            return false;
        };
        self.org_delete_line(l);
        // An empty drawer isn't worth keeping.
        if z - a == 2 {
            self.org_delete_line(a);
            self.org_delete_line(a);
        }
        true
    }

    /// Values `name` may take: `NAME_ALL` (inherited), `#+PROPERTY:
    /// NAME_ALL`, and the built-ins for `ORDERED`, `TODO` and `STYLE`.
    pub fn org_allowed_values(&self, h: usize, name: &str) -> Vec<String> {
        if let Some(all) = self.org_get_property(h, &format!("{name}_ALL"), true) {
            return all
                .split_whitespace()
                .map(|w| w.trim_matches('"').to_string())
                .collect();
        }
        match name.to_uppercase().as_str() {
            "ORDERED" => vec!["t".into()],
            "TODO" => {
                let kw = keywords(self);
                kw.todo.iter().chain(&kw.done).cloned().collect()
            }
            "STYLE" => vec!["habit".into()],
            _ => Vec::new(),
        }
    }

    /// `org-property-next-allowed-value` / `-previous-`: the property
    /// cycles through its allowed values (and back to unset). Returns the
    /// new value.
    pub fn org_property_cycle(
        &mut self,
        h: usize,
        name: &str,
        forward: bool,
    ) -> Result<Option<String>, String> {
        let allowed = self.org_allowed_values(h, name);
        if allowed.is_empty() {
            return Err(format!("No allowed values for {name}"));
        }
        let cur = self.org_get_property(h, name, false);
        let n = allowed.len() as isize;
        // Positions: 0..n are values, n is "unset".
        let pos = cur
            .as_ref()
            .and_then(|c| allowed.iter().position(|a| a == c))
            .map_or(n, |i| i as isize);
        let next = (pos + if forward { 1 } else { -1 }).rem_euclid(n + 1);
        if next == n {
            self.org_delete_property(h, name);
            Ok(None)
        } else {
            let v = allowed[next as usize].clone();
            self.org_set_property(h, name, &v);
            Ok(Some(v))
        }
    }

    /// `org-toggle-ordered-property`: ORDERED on or off. True if now on.
    pub fn org_toggle_ordered(&mut self, h: usize) -> bool {
        if self.org_get_property(h, "ORDERED", false).is_some() {
            self.org_delete_property(h, "ORDERED");
            false
        } else {
            self.org_set_property(h, "ORDERED", "t");
            true
        }
    }

    /// `org-compute-property-at-point`: set the property on the line at
    /// `line` to the sum of the same property over the heading's
    /// children (the `+` column summary). Returns the sum.
    pub fn org_compute_property(&mut self, line: usize) -> Result<f64, String> {
        let h = self.org_heading_at(line).ok_or("not in an outline entry")?;
        let text = self.line(line);
        let name = PROP_LINE
            .captures(&text)
            .map(|c| format!("{}{}", &c[1], &c[2]))
            .ok_or("not on a property line")?;
        let name = name.trim_end_matches('+').to_string();
        let mut sum = 0.0;
        let mut found = false;
        for c in self.org_children(h) {
            if let Some(v) = self.org_get_property(c, &name, false)
                && let Some(x) = parse_effort(&v)
            {
                sum += x;
                found = true;
            }
        }
        if !found {
            return Err(format!("No children with {name}"));
        }
        let shown = if sum.fract() == 0.0 {
            format!("{}", sum as i64)
        } else {
            format!("{sum}")
        };
        self.org_set_property(h, &name, &shown);
        Ok(sum)
    }
}

/// "3", "1.5", "2:30" (hours:minutes) as a number.
pub fn parse_effort(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((h, m)) = s.split_once(':') {
        return Some(h.parse::<f64>().ok()? + m.parse::<f64>().ok()? / 60.0);
    }
    s.parse().ok()
}

// ------------------------------------------------- planning, timestamps

/// SCHEDULED, DEADLINE and CLOSED of an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Planning {
    pub scheduled: Option<Timestamp>,
    pub deadline: Option<Timestamp>,
    pub closed: Option<Timestamp>,
}

static PLAN_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(SCHEDULED|DEADLINE|CLOSED):\s*([<\[][^>\]]*[>\]])").unwrap());

impl Buffer {
    pub fn org_planning(&self, h: usize) -> Planning {
        let mut p = Planning::default();
        let Some(l) = self.org_planning_line(h) else {
            return p;
        };
        let line = self.line(l);
        for c in PLAN_ITEM.captures_iter(&line) {
            let ts = timestamps_in(&c[2]).into_iter().next();
            match &c[1] {
                "SCHEDULED" => p.scheduled = ts,
                "DEADLINE" => p.deadline = ts,
                _ => p.closed = ts,
            }
        }
        p
    }

    /// Write the planning line of `h` (none at all when `p` is empty).
    pub fn org_write_planning(&mut self, h: usize, p: &Planning) {
        let mut parts = Vec::new();
        if let Some(t) = &p.closed {
            parts.push(format!("CLOSED: {}", t.text()));
        }
        if let Some(t) = &p.scheduled {
            parts.push(format!("SCHEDULED: {}", t.text()));
        }
        if let Some(t) = &p.deadline {
            parts.push(format!("DEADLINE: {}", t.text()));
        }
        match (self.org_planning_line(h), parts.is_empty()) {
            (Some(l), true) => self.org_delete_line(l),
            (Some(l), false) => self.org_set_line(l, &parts.join(" ")),
            (None, true) => {}
            (None, false) => self.org_insert_line_after(h, &parts.join(" ")),
        }
    }

    /// `org-schedule` / `org-deadline`: set (or with `None` remove) one
    /// of the planning entries. An existing repeater and warning period
    /// are kept when the new stamp has none.
    pub fn org_set_planning(&mut self, h: usize, kind: &str, ts: Option<Timestamp>) {
        let mut p = self.org_planning(h);
        let slot = match kind {
            "SCHEDULED" => &mut p.scheduled,
            "DEADLINE" => &mut p.deadline,
            _ => &mut p.closed,
        };
        *slot = ts.map(|mut t| {
            if let Some(old) = slot.as_ref() {
                if t.repeater.is_none() {
                    t.repeater = old.repeater.clone();
                }
                if t.warning.is_none() {
                    t.warning = old.warning.clone();
                }
            }
            t
        });
        self.org_write_planning(h, &p);
    }

    /// The timestamp under `p`, if any.
    pub fn org_timestamp_at_point(&self, p: Pos) -> Option<Timestamp> {
        timestamp_at(&self.line(p.line), p.col)
    }

    /// `org-time-stamp`: put `ts` at point, replacing the timestamp point
    /// is in. Point ends after it.
    pub fn org_put_timestamp(&mut self, p: Pos, ts: &Timestamp) {
        let line = self.line(p.line);
        let text = ts.text();
        match timestamp_at(&line, p.col) {
            Some(old) => {
                self.replace_range(
                    Pos::new(p.line, old.start),
                    Pos::new(p.line, old.end),
                    &text,
                );
                self.set_point(Pos::new(p.line, old.start + text.chars().count()));
            }
            None => {
                self.replace_range(p, p, &text);
                self.set_point(Pos::new(p.line, p.col + text.chars().count()));
            }
        }
    }

    /// `org-timestamp-up` / `-down` (and the `-day` ones with
    /// `whole_day`): change the field under point by `delta`. False when
    /// point isn't on a timestamp. Point stays on the same field.
    pub fn org_timestamp_shift(&mut self, p: Pos, delta: i64, whole_day: bool) -> bool {
        let line = self.line(p.line);
        let Some(old) = timestamp_at(&line, p.col) else {
            return false;
        };
        let field = if whole_day {
            Field::Day
        } else {
            field_at(&old, p.col)
        };
        let new = adjust(&old, field, delta);
        let text = new.text();
        self.replace_range(
            Pos::new(p.line, old.start),
            Pos::new(p.line, old.end),
            &text,
        );
        self.set_point(Pos::new(
            p.line,
            p.col
                .min(old.start + text.chars().count().saturating_sub(1)),
        ));
        true
    }

    /// Timestamps in the entry's own lines (not its sub-headings), with
    /// their lines.
    pub fn org_entry_timestamps(&self, h: usize) -> Vec<(usize, Timestamp)> {
        (h..=self
            .org_body_end(h)
            .min(self.line_count().saturating_sub(1)))
            .flat_map(|l| {
                timestamps_in(&self.line(l))
                    .into_iter()
                    .map(move |t| (l, t))
            })
            .collect()
    }

    /// `org-check-deadlines`: open entries whose deadline is within its
    /// warning period (default `default_warn` days) or is past. Returns
    /// `(heading, deadline, days left)`, soonest first.
    pub fn org_check_deadlines(&self, today: Date, default_warn: i64) -> Vec<(usize, Date, i64)> {
        let kw = keywords(self);
        let mut out = Vec::new();
        for h in 0..self.line_count() {
            if self.org_level(h).is_none() {
                continue;
            }
            let Some(hd) = parse_heading(&self.line(h), &kw) else {
                continue;
            };
            if hd.keyword.as_deref().is_some_and(|k| kw.is_done(k)) {
                continue;
            }
            let Some(dl) = self.org_planning(h).deadline else {
                continue;
            };
            let left = dl.date.days() - today.days();
            let warn = dl.warning.as_ref().map_or(default_warn, |w| w.days());
            if left <= warn {
                out.push((h, dl.date, left));
            }
        }
        out.sort_by_key(|&(h, _, left)| (left, h));
        out
    }

    /// `org-check-before-date` / `org-check-after-date`: show only
    /// entries with an active timestamp before (or after) `date`.
    pub fn org_check_date_sparse(&mut self, date: Date, before: bool) -> usize {
        let mut matched = vec![false; self.line_count()];
        let mut count = 0;
        for (h, m) in matched.iter_mut().enumerate() {
            if self.org_level(h).is_none() {
                continue;
            }
            let hit = self
                .org_entry_timestamps(h)
                .iter()
                .any(|(_, t)| t.active && if before { t.date < date } else { t.date > date });
            if hit {
                *m = true;
                count += 1;
            }
        }
        if count > 0 {
            self.org_apply_sparse(&matched);
        }
        count
    }
}

// ------------------------------------------------------------ TODO states

impl Buffer {
    /// The earlier sibling that blocks `h` from being done, when the
    /// parent is ORDERED.
    pub fn org_blocked_by(&self, h: usize) -> Option<String> {
        let parent = self.org_up_heading(h)?;
        self.org_get_property(parent, "ORDERED", false)?;
        let kw = keywords(self);
        for s in self.org_children(parent) {
            if s == h {
                return None;
            }
            let hd = parse_heading(&self.line(s), &kw)?;
            if hd.keyword.as_deref().is_some_and(|k| !kw.is_done(k)) {
                return Some(hd.title);
            }
        }
        None
    }

    /// `org-todo` with Org's side effects: an ORDERED parent can block it,
    /// finishing stamps `CLOSED:` (and a repeating entry moves its
    /// timestamps on and goes back to the first TODO state instead),
    /// leaving a done state removes the stamp. Returns the new keyword.
    pub fn org_todo_logged(
        &mut self,
        line: usize,
        forward: bool,
        today: Date,
        now: Hm,
    ) -> Result<Option<String>, String> {
        self.org_todo_logged_to(line, forward, None, today, now)
    }

    /// `org_todo_logged`, optionally going straight to `explicit` (the
    /// state picked with fast selection; `Some(None)` clears it).
    pub fn org_todo_logged_to(
        &mut self,
        line: usize,
        forward: bool,
        explicit: Option<Option<String>>,
        today: Date,
        now: Hm,
    ) -> Result<Option<String>, String> {
        let h = self.org_heading_at(line).ok_or("not in an outline entry")?;
        let kw = keywords(self);
        let old = parse_heading(&self.line(h), &kw).and_then(|hd| hd.keyword);
        let next = match &explicit {
            Some(state) => state.clone(),
            None => kw.next(old.as_deref(), forward),
        };
        let log = crate::org::log_options(self);
        let finishing = next.as_deref().is_some_and(|k| kw.is_done(k));
        let was_done = old.as_deref().is_some_and(|k| kw.is_done(k));
        if finishing && let Some(b) = self.org_blocked_by(h) {
            return Err(format!("Blocked by sibling \"{b}\""));
        }
        self.org_todo_to(line, forward, explicit)?;
        let mut p = self.org_planning(h);
        if finishing && !was_done {
            let repeats = p.scheduled.as_ref().is_some_and(|t| t.repeater.is_some())
                || p.deadline.as_ref().is_some_and(|t| t.repeater.is_some());
            if repeats {
                for t in [&mut p.scheduled, &mut p.deadline].into_iter().flatten() {
                    if let Some(n) = t.next_repeat(today, now) {
                        *t = n;
                    }
                }
                p.closed = None;
                self.org_write_planning(h, &p);
                self.org_set_property(h, "LAST_REPEAT", &inactive_now(today, now));
                if log.repeat {
                    let to = next.clone().unwrap_or_default();
                    let from = old.clone().unwrap_or_default();
                    self.org_add_log_entry(
                        h,
                        &format!(
                            "- State {:<12} from {:<12} {}",
                            format!("\"{to}\""),
                            format!("\"{from}\""),
                            inactive_now(today, now)
                        ),
                    );
                }
                // Back to the first TODO state.
                let first = kw.todo.first().cloned();
                while parse_heading(&self.line(h), &kw).and_then(|hd| hd.keyword) != first {
                    if self.org_todo(line, true).is_err() {
                        break;
                    }
                }
                return Ok(first);
            }
            if log.closed {
                p.closed = Some(Timestamp::new(false, today, Some(now)));
                self.org_write_planning(h, &p);
            }
        } else if was_done && !finishing && p.closed.is_some() {
            p.closed = None;
            self.org_write_planning(h, &p);
        }
        Ok(next)
    }

    /// Add `entry` (a `- ...` list item, possibly several lines) at the top
    /// of the heading's `:LOGBOOK:`, making the drawer when it's missing.
    pub fn org_add_log_entry(&mut self, h: usize, entry: &str) {
        let limit = self.org_subtree_end(h).min(
            (h + 1..self.line_count())
                .find(|&l| self.org_level(l).is_some())
                .map_or(usize::MAX, |l| l - 1),
        );
        let logbook = (h + 1..=limit.min(self.line_count().saturating_sub(1)))
            .find(|&l| self.line(l).trim().eq_ignore_ascii_case(":LOGBOOK:"));
        match logbook {
            Some(l) => self.org_insert_line_after(l, entry),
            None => {
                let mut after = self.org_planning_line(h).unwrap_or(h);
                if let Some((_, z)) = self.org_property_drawer(h) {
                    after = z;
                }
                self.org_insert_line_after(after, &format!(":LOGBOOK:\n{entry}\n:END:"));
            }
        }
    }

    /// Every entry with a not-done TODO keyword:
    /// `(line, keyword, title, tags)`.
    pub fn org_todo_entries(&self) -> Vec<(usize, String, String, Vec<String>)> {
        let kw = keywords(self);
        (0..self.line_count())
            .filter(|&l| self.org_level(l).is_some())
            .filter_map(|l| {
                let hd = parse_heading(&self.line(l), &kw)?;
                let k = hd.keyword?;
                (!kw.is_done(&k)).then_some((l, k, hd.title, hd.tags))
            })
            .collect()
    }
}

// ------------------------------------------------------------------ links

/// A link in a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Character columns of the whole link text.
    pub start: usize,
    pub end: usize,
    /// Character columns of the visible text: the description when there
    /// is one, else the target. This is what Forge shows and styles with
    /// `org-link`, and what a descriptive link collapses to.
    pub text_start: usize,
    pub text_end: usize,
    pub target: String,
    pub description: Option<String>,
}

impl Link {
    /// The text shown for this link: its description, or the target.
    pub fn visible(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.target)
    }
}

static BRACKET_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\[\]]+)\](?:\[([^\[\]]*)\])?\]").unwrap());
static ANGLE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<((?:https?|ftp|mailto|file|id):[^<>\s]+)>").unwrap());
static PLAIN_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:https?|ftp|mailto):[^\s<>()\[\]"']*[^\s<>()\[\]"'.,;:!?]"#).unwrap()
});

/// Links in `line`, in order.
pub fn links_in(line: &str) -> Vec<Link> {
    let col = |b: usize| line[..b].chars().count();
    let mut out: Vec<Link> = Vec::new();
    for c in BRACKET_LINK.captures_iter(line) {
        let m = c.get(0).unwrap();
        // With a description, the description is what shows; without one,
        // the target shows on its own.
        let shown = c.get(2).or_else(|| c.get(1)).expect("a target");
        out.push(Link {
            start: col(m.start()),
            end: col(m.end()),
            text_start: col(shown.start()),
            text_end: col(shown.end()),
            target: c[1].to_string(),
            description: c.get(2).map(|d| d.as_str().to_string()),
        });
    }
    for c in ANGLE_LINK.captures_iter(line) {
        let m = c.get(0).unwrap();
        let shown = c.get(1).expect("a target");
        out.push(Link {
            start: col(m.start()),
            end: col(m.end()),
            text_start: col(shown.start()),
            text_end: col(shown.end()),
            target: c[1].to_string(),
            description: None,
        });
    }
    for m in PLAIN_LINK.find_iter(line) {
        let (s, e) = (col(m.start()), col(m.end()));
        if !out.iter().any(|l| s >= l.start && e <= l.end) {
            out.push(Link {
                start: s,
                end: e,
                text_start: s,
                text_end: e,
                target: m.as_str().to_string(),
                description: None,
            });
        }
    }
    out.sort_by_key(|l| l.start);
    out
}

pub fn link_at(line: &str, col: usize) -> Option<Link> {
    links_in(line)
        .into_iter()
        .find(|l| col >= l.start && col < l.end)
}

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File {
        path: String,
        search: Option<String>,
    },
    Id(String),
    CustomId(String),
    /// `*Heading` in this buffer.
    Heading(String),
    /// A bare `[[text]]`: a `<<target>>`, a name, a heading or text.
    Fuzzy(String),
    Shell(String),
}

pub fn resolve_link(target: &str) -> LinkTarget {
    let t = target.trim();
    if let Some(r) = t.strip_prefix("file:") {
        return split_file(r);
    }
    if let Some(r) = t.strip_prefix("id:") {
        return LinkTarget::Id(r.to_string());
    }
    if let Some(r) = t.strip_prefix("shell:") {
        return LinkTarget::Shell(r.to_string());
    }
    if let Some(r) = t.strip_prefix('#') {
        return LinkTarget::CustomId(r.to_string());
    }
    if let Some(r) = t.strip_prefix('*') {
        return LinkTarget::Heading(r.to_string());
    }
    if t.contains("://") || t.starts_with("mailto:") {
        return LinkTarget::Url(t.to_string());
    }
    if t.starts_with('/') || t.starts_with("./") || t.starts_with("../") || t.starts_with("~/") {
        return split_file(t);
    }
    LinkTarget::Fuzzy(t.to_string())
}

fn split_file(r: &str) -> LinkTarget {
    match r.split_once("::") {
        Some((p, s)) => LinkTarget::File {
            path: p.to_string(),
            search: Some(s.to_string()),
        },
        None => LinkTarget::File {
            path: r.to_string(),
            search: None,
        },
    }
}

/// A heading's text as links name it: no keyword, tags or cookie.
fn heading_text(buf: &Buffer, h: usize) -> String {
    let kw = keywords(buf);
    let t = parse_heading(&buf.line(h), &kw).map_or_else(String::new, |hd| hd.title);
    static COOKIE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\[(?:\d*/\d*|\d*%)\]").unwrap());
    COOKIE.replace_all(&t, "").trim().to_string()
}

impl Buffer {
    /// Find what a search string (the part after `::`, or a `*Heading`,
    /// `#custom-id`, `[[fuzzy]]`) names in this buffer.
    pub fn org_find_link_target(&self, t: &LinkTarget) -> Option<Pos> {
        let n = self.line_count();
        let find_heading = |name: &str| {
            (0..n).find(|&l| {
                self.org_level(l).is_some()
                    && heading_text(self, l).eq_ignore_ascii_case(name.trim())
            })
        };
        let find_prop = |prop: &str, val: &str| {
            (0..n).find(|&l| {
                self.org_level(l).is_some()
                    && self
                        .org_get_property(l, prop, false)
                        .is_some_and(|v| v == val)
            })
        };
        let find_text = |s: &str| {
            (0..n).find_map(|l| {
                self.line(l)
                    .find(s)
                    .map(|b| Pos::new(l, self.line(l)[..b].chars().count()))
            })
        };
        match t {
            LinkTarget::Heading(name) => find_heading(name).map(|l| Pos::new(l, 0)),
            LinkTarget::CustomId(id) => find_prop("CUSTOM_ID", id).map(|l| Pos::new(l, 0)),
            LinkTarget::Id(id) => find_prop("ID", id).map(|l| Pos::new(l, 0)),
            LinkTarget::Fuzzy(s) => {
                let dedicated = format!("<<{s}>>");
                if let Some(p) = find_text(&dedicated) {
                    return Some(p);
                }
                let named = (0..n).find(|&l| {
                    let line = self.line(l);
                    let lt = line.trim();
                    lt.len() > 7
                        && lt
                            .get(..7)
                            .is_some_and(|head| head.eq_ignore_ascii_case("#+NAME:"))
                        && lt[7..].trim().eq_ignore_ascii_case(s.trim())
                });
                if let Some(l) = named {
                    return Some(Pos::new(l, 0));
                }
                find_heading(s)
                    .map(|l| Pos::new(l, 0))
                    .or_else(|| find_text(s))
            }
            LinkTarget::File { search, .. } => {
                let s = search.as_deref()?;
                if let Ok(line) = s.trim().parse::<usize>() {
                    return Some(Pos::new(line.saturating_sub(1).min(n.saturating_sub(1)), 0));
                }
                let sub = resolve_link(s);
                match sub {
                    LinkTarget::Fuzzy(_)
                    | LinkTarget::Heading(_)
                    | LinkTarget::CustomId(_)
                    | LinkTarget::Id(_) => self.org_find_link_target(&sub),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Link at `p`.
    pub fn org_link_at_point(&self, p: Pos) -> Option<Link> {
        link_at(&self.line(p.line), p.col)
    }

    /// `org-next-link` / `org-previous-link`: the start of the next link
    /// after (or before) `p`.
    pub fn org_next_link(&self, p: Pos, forward: bool) -> Option<Pos> {
        if forward {
            for l in p.line..self.line_count() {
                for k in links_in(&self.line(l)) {
                    if l > p.line || k.start > p.col {
                        return Some(Pos::new(l, k.start));
                    }
                }
            }
        } else {
            for l in (0..=p.line).rev() {
                for k in links_in(&self.line(l)).into_iter().rev() {
                    if l < p.line || k.start < p.col {
                        return Some(Pos::new(l, k.start));
                    }
                }
            }
        }
        None
    }

    /// `org-insert-link`: put `[[target][description]]` at `p`, replacing
    /// the link there. A description-less link is written bare.
    pub fn org_insert_link(&mut self, p: Pos, target: &str, description: &str) {
        let text = if description.trim().is_empty() {
            format!("[[{target}]]")
        } else {
            format!("[[{target}][{description}]]")
        };
        match self.org_link_at_point(p) {
            Some(old) => {
                self.replace_range(
                    Pos::new(p.line, old.start),
                    Pos::new(p.line, old.end),
                    &text,
                );
                self.set_point(Pos::new(p.line, old.start + text.chars().count()));
            }
            None => {
                self.replace_range(p, p, &text);
                self.set_point(Pos::new(p.line, p.col + text.chars().count()));
            }
        }
    }

    /// `org-remove-link`: replace the link at `p` by its description (or
    /// its target). False if point isn't on one.
    pub fn org_remove_link(&mut self, p: Pos) -> bool {
        let Some(k) = self.org_link_at_point(p) else {
            return false;
        };
        let keep = k.description.clone().unwrap_or_else(|| k.target.clone());
        self.replace_range(Pos::new(p.line, k.start), Pos::new(p.line, k.end), &keep);
        self.set_point(Pos::new(p.line, k.start));
        true
    }

    /// `org-store-link`: a link to the entry at `line`, as
    /// `(target, description)`. `file` is this buffer's file.
    pub fn org_store_link(&self, line: usize, file: Option<&str>) -> (String, String) {
        let file = file.map(str::to_string);
        match self.org_heading_at(line) {
            Some(h) => {
                let name = heading_text(self, h);
                let id = self.org_get_property(h, "CUSTOM_ID", false);
                let target = match (&file, id) {
                    (Some(f), Some(id)) => format!("file:{f}::#{id}"),
                    (Some(f), None) => format!("file:{f}::*{name}"),
                    (None, Some(id)) => format!("#{id}"),
                    (None, None) => format!("*{name}"),
                };
                (target, name)
            }
            None => {
                let text = self.line(line).trim().to_string();
                match file {
                    Some(f) => (
                        format!("file:{f}::{}", line + 1),
                        if text.is_empty() { f } else { text },
                    ),
                    None => (format!("{}", line + 1), text),
                }
            }
        }
    }
}

// --------------------------------------------------- drawers and blocks

static BEGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*#\+BEGIN_(\S+)(?:\s+(.*))?$").unwrap());
static DRAWER_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*:([A-Za-z0-9_-]+):\s*$").unwrap());

/// A `#+BEGIN_x` ... `#+END_x` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub first: usize,
    pub last: usize,
    /// Lower-cased: "src", "quote", "example".
    pub kind: String,
    /// What follows the kind on the begin line ("rust", "html").
    pub arg: String,
}

impl Buffer {
    /// The block `line` is in (its begin and end lines included).
    pub fn org_block_at(&self, line: usize) -> Option<Block> {
        let n = self.line_count();
        let line = line.min(n.saturating_sub(1));
        for first in (0..=line).rev() {
            let text = self.line(first);
            if let Some(c) = BEGIN.captures(&text) {
                let kind = c[1].to_lowercase();
                let end_re = format!("#+end_{kind}");
                let last =
                    (first + 1..n).find(|&l| self.line(l).trim().eq_ignore_ascii_case(&end_re))?;
                return (last >= line).then(|| Block {
                    first,
                    last,
                    kind,
                    arg: c
                        .get(2)
                        .map_or(String::new(), |m| m.as_str().trim().to_string()),
                });
            }
            if self.line(first).trim().to_lowercase().starts_with("#+end_") && first < line {
                return None;
            }
        }
        None
    }

    /// The drawer `line` starts (`:NAME:` ... `:END:`), as its lines.
    pub fn org_drawer_at(&self, line: usize) -> Option<(usize, usize)> {
        let text = self.line(line);
        let c = DRAWER_START.captures(&text)?;
        if c[1].eq_ignore_ascii_case("END") {
            return None;
        }
        let limit = self
            .org_body_end(self.org_heading_at(line).unwrap_or(line))
            .max(line);
        (line + 1..=limit.min(self.line_count().saturating_sub(1)))
            .find(|&l| self.line(l).trim().eq_ignore_ascii_case(":END:"))
            .map(|end| (line, end))
    }

    /// TAB on a drawer's first line or a block's `#+BEGIN` line folds or
    /// unfolds it. Returns what happened, or None when `line` is neither.
    pub fn org_toggle_drawer_or_block(&mut self, line: usize) -> Option<&'static str> {
        let text = self.line(line);
        let (first, last) = if BEGIN.is_match(&text) {
            let b = self.org_block_at(line)?;
            (b.first, b.last)
        } else {
            self.org_drawer_at(line)?
        };
        if last <= first {
            return None;
        }
        if self.line_folded(first) {
            self.unfold_line(first);
            Some("Shown")
        } else {
            self.fold_lines(first + 1, last);
            Some("Folded")
        }
    }

    /// `org-insert-structure-template` / blocks: wrap lines `first..=last`
    /// (or open an empty block after `first` when `last` is None) in
    /// `#+BEGIN_KIND arg` ... `#+END_KIND`. Returns the line to put point
    /// on.
    pub fn org_insert_block(
        &mut self,
        first: usize,
        last: Option<usize>,
        kind: &str,
        arg: &str,
    ) -> usize {
        let kind_up = kind.to_uppercase();
        let begin = if arg.trim().is_empty() {
            format!("#+BEGIN_{kind_up}")
        } else {
            format!("#+BEGIN_{kind_up} {}", arg.trim())
        };
        let end = format!("#+END_{kind_up}");
        match last {
            Some(last) => {
                let (a, z) = (self.line_start_offset(first), self.line_end_offset(last));
                let body = self.text_offsets(a, z);
                self.replace_offsets(a, z, &format!("{begin}\n{body}\n{end}"));
                first + 1
            }
            None => {
                let on_empty = self.line(first).trim().is_empty();
                if on_empty {
                    let (a, z) = (self.line_start_offset(first), self.line_end_offset(first));
                    self.replace_offsets(a, z, &format!("{begin}\n\n{end}"));
                    first + 1
                } else {
                    self.org_insert_line_after(first, &format!("{begin}\n\n{end}"));
                    first + 2
                }
            }
        }
    }
}

/// The structure templates: key -> (block kind, argument).
pub fn structure_template(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "s" => ("src", ""),
        "e" => ("example", ""),
        "q" => ("quote", ""),
        "v" => ("verse", ""),
        "c" => ("center", ""),
        "C" => ("comment", ""),
        "l" => ("export", "latex"),
        "h" => ("export", "html"),
        "a" => ("export", "ascii"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org_time::Date;

    fn org(text: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b
    }

    const TODAY: Date = Date {
        y: 2026,
        m: 10,
        d: 12,
    };

    #[test]
    fn tags() {
        let mut b = org("#+FILETAGS: :f:\n* TODO Task [1/2]\n** Sub :a:\n* Other\n");
        assert!(b.org_toggle_tag(1, "work"));
        let l = b.line(1);
        assert!(l.starts_with("* TODO Task [1/2]"));
        assert!(l.ends_with(":work:"));
        assert_eq!(l.chars().count(), TAGS_COLUMN);
        assert!(!b.org_toggle_tag(1, "work"));
        assert_eq!(b.line(1), "* TODO Task [1/2]");
        b.org_set_tags(2, &["a".into(), "b".into()]);
        assert_eq!(b.org_heading_tags(2), ["a", "b"]);
        b.org_set_tags(1, &["proj".into()]);
        assert_eq!(b.org_inherited_tags(2), ["f", "proj", "a", "b"]);
        assert_eq!(b.org_all_tags(), ["a", "b", "f", "proj"]);
    }

    #[test]
    fn matching() {
        let mut b = org("* TODO A :work:\n** NEXT B :urgent:\n* DONE C :work:\n* D :home:\n");
        let m = |b: &Buffer, e: &str| b.org_match_entries(e, false).unwrap();
        assert_eq!(m(&b, "work"), [0, 1, 2]);
        assert_eq!(m(&b, "work+urgent"), [1]);
        assert_eq!(m(&b, "work-urgent"), [0, 2]);
        assert_eq!(m(&b, "home|urgent"), [1, 3]);
        assert_eq!(m(&b, "{^ho}"), [3]);
        assert_eq!(m(&b, "LEVEL=2"), [1]);
        assert_eq!(m(&b, "TODO=\"DONE\""), [2]);
        assert_eq!(b.org_match_entries("work", true).unwrap(), [0]);
        b.org_set_property(3, "Effort", "3");
        assert_eq!(m(&b, "Effort>=2"), [3]);
        assert_eq!(b.org_match_sparse_tree("home", false).unwrap(), 1);
        assert!(b.line_hidden(1));
    }

    #[test]
    fn properties() {
        let mut b = org("* Task\nbody\n** Child\n");
        assert_eq!(b.org_get_property(0, "X", false), None);
        b.org_set_property(0, "X", "1");
        b.org_set_property(0, "Y", "two");
        assert_eq!(
            b.get_contents(),
            "* Task\n:PROPERTIES:\n:X: 1\n:Y: two\n:END:\nbody\n** Child\n"
        );
        b.org_set_property(0, "X", "5");
        assert_eq!(
            b.org_properties(0),
            [("X".into(), "5".into()), ("Y".into(), "two".into())]
        );
        let child = b.org_children(0)[0];
        assert_eq!(b.org_get_property(child, "Y", true).as_deref(), Some("two"));
        assert_eq!(b.org_get_property(child, "Y", false), None);
        assert!(b.org_delete_property(0, "Y"));
        assert!(b.org_delete_property(0, "X"));
        assert_eq!(b.get_contents(), "* Task\nbody\n** Child\n");
        assert!(!b.org_delete_property(0, "X"));
    }

    #[test]
    fn allowed_values_and_ordered() {
        let mut b = org("* T\n:PROPERTIES:\n:Effort_ALL: 1 2 3\n:END:\n** C\n");
        let c = b.org_children(0)[0];
        assert_eq!(
            b.org_property_cycle(c, "Effort", true).unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(
            b.org_property_cycle(c, "Effort", true).unwrap().as_deref(),
            Some("2")
        );
        assert_eq!(
            b.org_property_cycle(c, "Effort", false).unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(b.org_property_cycle(c, "Effort", false).unwrap(), None);
        assert!(b.org_property_cycle(c, "Nope", true).is_err());
        assert!(b.org_toggle_ordered(c));
        assert!(!b.org_toggle_ordered(c));
    }

    #[test]
    fn compute_property_sums_children() {
        let mut b = org(
            "* P\n:PROPERTIES:\n:Effort: 0\n:END:\n** A\n:PROPERTIES:\n:Effort: 2\n:END:\n** B\n:PROPERTIES:\n:Effort: 1:30\n:END:\n",
        );
        assert_eq!(b.org_compute_property(2).unwrap(), 3.5);
        assert_eq!(
            b.org_get_property(0, "Effort", false).as_deref(),
            Some("3.5")
        );
    }

    #[test]
    fn planning_and_timestamps() {
        let mut b = org("* TODO Task\nbody <2026-10-20 Tue>\n");
        let t = Timestamp::new(true, Date::new(2026, 10, 15), None);
        b.org_set_planning(0, "DEADLINE", Some(t));
        b.org_set_planning(
            0,
            "SCHEDULED",
            Some(Timestamp::new(true, Date::new(2026, 10, 13), Some((9, 0)))),
        );
        assert_eq!(
            b.line(1),
            "SCHEDULED: <2026-10-13 Tue 09:00> DEADLINE: <2026-10-15 Thu>"
        );
        let mut r = Timestamp::new(true, Date::new(2026, 10, 14), None);
        r.repeater = None;
        b.org_set_planning(0, "DEADLINE", Some(r));
        assert!(b.line(1).ends_with("DEADLINE: <2026-10-14 Wed>"));
        b.org_set_planning(0, "SCHEDULED", None);
        b.org_set_planning(0, "DEADLINE", None);
        assert_eq!(b.get_contents(), "* TODO Task\nbody <2026-10-20 Tue>\n");

        let mut b = org("<2026-10-12 Mon 14:00>\n");
        assert!(b.org_timestamp_shift(Pos::new(0, 7), 1, false));
        assert_eq!(b.line(0), "<2026-11-12 Thu 14:00>");
        assert!(b.org_timestamp_shift(Pos::new(0, 7), 3, true));
        assert_eq!(b.line(0), "<2026-11-15 Sun 14:00>");
        assert!(!b.org_timestamp_shift(Pos::new(0, 40), 1, false));
        let mut b = org("due: x\n");
        let t = Timestamp::new(true, Date::new(2026, 1, 2), None);
        b.org_put_timestamp(Pos::new(0, 5), &t);
        assert_eq!(b.line(0), "due: <2026-01-02 Fri>x");
        b.org_put_timestamp(
            Pos::new(0, 8),
            &Timestamp::new(false, Date::new(2026, 3, 4), None),
        );
        assert_eq!(b.line(0), "due: [2026-03-04 Wed]x");
    }

    #[test]
    fn deadlines_and_date_checks() {
        let mut b = org(
            "* TODO A\nDEADLINE: <2026-10-14 Wed>\n* TODO B\nDEADLINE: <2026-12-25 Fri>\n* DONE C\nDEADLINE: <2026-10-10 Sat>\n* TODO D\nDEADLINE: <2026-10-20 Tue -10d>\n* E\n<2026-09-01 Tue>\n",
        );
        let got = b.org_check_deadlines(TODAY, 7);
        assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), [0, 6]);
        assert_eq!(got[0].2, 2);
        assert_eq!(b.org_check_date_sparse(Date::new(2026, 10, 1), true), 1);
        assert!(b.line_hidden(1));
        // Entries with a stamp after Oct 1: the three open deadlines' entries.
        let mut b2 = org("* A\n<2026-10-14 Wed>\n* B\n<2026-09-01 Tue>\n");
        assert_eq!(b2.org_check_date_sparse(Date::new(2026, 10, 1), false), 1);
        assert!(b2.line_hidden(3));
    }

    #[test]
    fn todo_closed_repeat_and_ordered() {
        // Without `logdone` finishing leaves no CLOSED stamp.
        let mut plain = org("* TODO Task\n");
        plain.org_todo_logged(0, true, TODAY, (14, 8)).unwrap();
        assert_eq!(plain.get_contents(), "* DONE Task\n");
        let mut b = org(
            "#+STARTUP: logdone\n* TODO Task\n* TODO Weekly\nSCHEDULED: <2026-10-12 Mon +1w>\n",
        );
        let r = b.org_todo_logged(1, true, TODAY, (14, 8)).unwrap();
        assert_eq!(r.as_deref(), Some("DONE"));
        assert_eq!(b.line(1), "* DONE Task");
        assert_eq!(b.line(2), "CLOSED: [2026-10-12 Mon 14:08]");
        b.org_todo_logged(1, true, TODAY, (14, 8)).unwrap(); // DONE -> none
        assert_eq!(b.get_contents().lines().nth(1), Some("* Task"));
        assert!(!b.get_contents().contains("CLOSED"));
        // Repeating entry.
        let w = b
            .get_contents()
            .lines()
            .position(|l| l == "* TODO Weekly")
            .unwrap();
        b.org_todo_logged(w, true, TODAY, (14, 8)).unwrap();
        assert_eq!(b.line(w), "* TODO Weekly");
        assert_eq!(b.line(w + 1), "SCHEDULED: <2026-10-19 Mon +1w>");
        assert!(b.line(w + 3).starts_with(":LAST_REPEAT: [2026-10-12"));
        // And the state change is logged the way Org does.
        assert!(
            b.get_contents()
                .contains("- State \"DONE\"       from \"TODO\"       [2026-10-12 Mon 14:08]"),
            "{}",
            b.get_contents()
        );

        // Ordered parent.
        let mut b = org("* P\n:PROPERTIES:\n:ORDERED: t\n:END:\n** TODO One\n** TODO Two\n");
        let err = b.org_todo_logged(5, true, TODAY, (0, 0)).unwrap_err();
        assert!(err.contains("One"));
        b.org_todo_logged(4, true, TODAY, (0, 0)).unwrap();
        assert!(
            b.org_todo_logged(
                b.get_contents()
                    .lines()
                    .position(|l| l.contains("Two"))
                    .unwrap(),
                true,
                TODAY,
                (0, 0)
            )
            .is_ok()
        );
    }

    #[test]
    fn links() {
        let line =
            "see [[https://a.b/c][site]] and [[*Heading]] and https://x.y/z, ok <mailto:me@x.y>";
        let all = links_in(line);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].description.as_deref(), Some("site"));
        assert_eq!(all[2].target, "https://x.y/z");
        assert_eq!(link_at(line, 6).unwrap().target, "https://a.b/c");
        assert!(link_at(line, 1).is_none());
        assert_eq!(
            resolve_link("https://a.b"),
            LinkTarget::Url("https://a.b".into())
        );
        assert_eq!(
            resolve_link("file:notes.org::*Top"),
            LinkTarget::File {
                path: "notes.org".into(),
                search: Some("*Top".into())
            }
        );
        assert_eq!(resolve_link("#cid"), LinkTarget::CustomId("cid".into()));
        assert_eq!(resolve_link("*H"), LinkTarget::Heading("H".into()));
        assert_eq!(
            resolve_link("some text"),
            LinkTarget::Fuzzy("some text".into())
        );
    }

    #[test]
    fn link_targets_and_editing() {
        let mut b = org(
            "* Top\n:PROPERTIES:\n:CUSTOM_ID: top-id\n:END:\ntext <<anchor>> x\n* TODO Other [1/2] :t:\n#+NAME: tbl\n[[*Other]] [[#top-id]]\n",
        );
        let find = |b: &Buffer, t: &str| b.org_find_link_target(&resolve_link(t));
        assert_eq!(find(&b, "*other"), Some(Pos::new(5, 0)));
        assert_eq!(find(&b, "#top-id"), Some(Pos::new(0, 0)));
        assert_eq!(find(&b, "anchor"), Some(Pos::new(4, 5)));
        assert_eq!(find(&b, "tbl"), Some(Pos::new(6, 0)));
        assert_eq!(find(&b, "file:x.org::8"), Some(Pos::new(7, 0)));
        assert_eq!(find(&b, "file:x.org::*Top"), Some(Pos::new(0, 0)));
        assert_eq!(find(&b, "nothing here"), None);
        assert_eq!(b.org_next_link(Pos::new(0, 0), true), Some(Pos::new(7, 0)));
        assert_eq!(b.org_next_link(Pos::new(7, 0), true), Some(Pos::new(7, 11)));
        assert_eq!(
            b.org_next_link(Pos::new(7, 11), false),
            Some(Pos::new(7, 0))
        );
        assert_eq!(b.org_next_link(Pos::new(7, 11), true), None);

        b.org_insert_link(Pos::new(7, 2), "*Top", "go");
        assert_eq!(b.line(7), "[[*Top][go]] [[#top-id]]");
        assert!(b.org_remove_link(Pos::new(7, 3)));
        assert_eq!(b.line(7), "go [[#top-id]]");
        let (t, d) = b.org_store_link(5, Some("/p/t.org"));
        assert_eq!((t.as_str(), d.as_str()), ("file:/p/t.org::*Other", "Other"));
        let (t, _) = b.org_store_link(1, None);
        assert_eq!(t, "#top-id");
    }

    #[test]
    fn multibyte_name_line_is_not_a_panic() {
        let b = org("#+NAMEéx\n");
        assert!(b.org_find_link_target(&resolve_link("zzz")).is_none());
    }

    #[test]
    fn one_character_name_is_found() {
        let b = org("#+NAME:x\n");
        assert_eq!(
            b.org_find_link_target(&resolve_link("x")),
            Some(Pos::new(0, 0))
        );
    }

    #[test]
    fn drawers_and_blocks() {
        let mut b =
            org("* H\n:LOGBOOK:\nnote\n:END:\n#+BEGIN_SRC rust\nfn x() {}\n#+END_SRC\nafter\n");
        assert_eq!(b.org_drawer_at(1), Some((1, 3)));
        assert_eq!(b.org_drawer_at(3), None);
        let blk = b.org_block_at(5).unwrap();
        assert_eq!(
            (blk.first, blk.last, blk.kind.as_str(), blk.arg.as_str()),
            (4, 6, "src", "rust")
        );
        assert!(b.org_block_at(7).is_none());
        assert_eq!(b.org_toggle_drawer_or_block(1), Some("Folded"));
        assert!(b.line_hidden(2));
        assert_eq!(b.org_toggle_drawer_or_block(1), Some("Shown"));
        assert!(!b.line_hidden(2));
        assert_eq!(b.org_toggle_drawer_or_block(4), Some("Folded"));
        assert_eq!(b.org_toggle_drawer_or_block(7), None);

        let mut b = org("a\nb\nc\n");
        assert_eq!(b.org_insert_block(0, Some(1), "quote", ""), 1);
        assert_eq!(b.get_contents(), "#+BEGIN_QUOTE\na\nb\n#+END_QUOTE\nc\n");
        let mut b = org("x\n\ny\n");
        assert_eq!(b.org_insert_block(1, None, "src", "python"), 2);
        assert_eq!(b.line(1), "#+BEGIN_SRC python");
        let mut b = org("x\n");
        assert_eq!(b.org_insert_block(0, None, "example", ""), 2);
        assert_eq!(b.get_contents(), "x\n#+BEGIN_EXAMPLE\n\n#+END_EXAMPLE\n");
        assert_eq!(structure_template("h"), Some(("export", "html")));
    }
}
