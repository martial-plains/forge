// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dynamic blocks (`org-dblock-update`): `#+BEGIN: name :params` ...
//! `#+END:`, whose contents are made by a writer and replaced each time.
//! Built in: `clocktable` and `columnview`. Any other name belongs to a
//! command called `org-dblock-write:NAME`, which the editor runs.

use crate::Buffer;
use crate::org::{keywords, parse_heading};
use crate::org_time::{Date, Hm, Timestamp, hmm};
use crate::org_work::{clean_title, clocktable_indent_string, parse_clock_line};

/// A dynamic block: the `#+BEGIN:` line, the `#+END:` line, the name and
/// the text of the parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub begin: usize,
    pub end: usize,
    pub name: String,
    pub params: String,
}

/// The name and parameters of a `#+BEGIN: name params` line.
pub fn parse_begin(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let head = t.get(..8)?;
    if !head.eq_ignore_ascii_case("#+BEGIN:") {
        return None;
    }
    let rest = t[8..].trim();
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..name_end];
    (!name.is_empty()).then(|| (name.to_string(), rest[name_end..].trim().to_string()))
}

fn is_end(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("#+END:")
}

/// Every dynamic block in the buffer, in order.
pub fn all_blocks(b: &Buffer) -> Vec<Block> {
    let mut out = Vec::new();
    let mut l = 0;
    while l < b.line_count() {
        if let Some((name, params)) = parse_begin(&b.line(l))
            && let Some(z) = (l + 1..b.line_count()).find(|&i| is_end(&b.line(i)))
        {
            out.push(Block {
                begin: l,
                end: z,
                name,
                params,
            });
            l = z;
        }
        l += 1;
    }
    out
}

/// The dynamic block that has `line` in it (its first or last line too).
pub fn block_at(b: &Buffer, line: usize) -> Option<Block> {
    all_blocks(b)
        .into_iter()
        .find(|k| (k.begin..=k.end).contains(&line))
}

/// `:key value` pairs. A value is one word or a "quoted string"; a key
/// with no value gets an empty one.
pub fn parse_params(s: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_whitespace() {
            it.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            it.next();
            for c in it.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
            if let Some(last) = out.last_mut()
                && last.1.is_empty()
            {
                last.1 = word;
            }
            continue;
        }
        while let Some(&c) = it.peek() {
            if c.is_whitespace() {
                break;
            }
            word.push(c);
            it.next();
        }
        if let Some(k) = word.strip_prefix(':') {
            out.push((k.to_string(), String::new()));
        } else if let Some(last) = out.last_mut()
            && last.1.is_empty()
        {
            last.1 = word;
        }
    }
    out
}

pub fn get<'a>(ps: &'a [(String, String)], key: &str) -> Option<&'a str> {
    ps.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty() && *v != "nil")
}

pub fn truthy(ps: &[(String, String)], key: &str, default: bool) -> bool {
    match ps.iter().find(|(k, _)| k == key) {
        None => default,
        Some((_, v)) => !(v == "nil"),
    }
}

// --------------------------------------------------------- time ranges

fn day_start(d: Date) -> i64 {
    d.days() * 1440
}

/// A `:block` value as `[start, end)` dates: `today`, `thisweek`,
/// `lastmonth`, `2026`, `2026-10`, `2026-W41`, `2026-10-12`, with an
/// optional `-N`/`+N` step back or forward on the relative ones.
pub fn block_range(spec: &str, today: Date) -> Option<(Date, Date)> {
    let s = spec.trim();
    let split = s
        .find(['+', '-'])
        .filter(|_| s.starts_with(|c: char| c.is_ascii_alphabetic()))
        .unwrap_or(s.len());
    let (word, off) = s.split_at(split);
    let n: i64 = if off.is_empty() {
        0
    } else {
        off.trim_start_matches('+').parse().ok()?
    };
    let month = |d: Date| Date::new(d.y, d.m, 1);
    match word {
        "today" | "yesterday" | "thisweek" | "lastweek" | "thismonth" | "lastmonth"
        | "thisyear" | "lastyear" => {
            let back = i64::from(word.starts_with("last") || word == "yesterday");
            let k = n - back;
            Some(match &word[word.len() - 3..] {
                "day" | "ody" => {
                    let a = today.add_days(k);
                    (a, a.add_days(1))
                }
                "eek" => {
                    let a = today.week_start().add_days(7 * k);
                    (a, a.add_days(7))
                }
                "nth" => {
                    let a = month(today).add_months(k);
                    (a, a.add_months(1))
                }
                _ => {
                    let a = Date::new(today.y + k, 1, 1);
                    (a, Date::new(a.y + 1, 1, 1))
                }
            })
        }
        _ => {
            let parts: Vec<&str> = s.split('-').collect();
            let year: i64 = parts.first()?.parse().ok()?;
            match parts.as_slice() {
                [_] if s.len() == 4 => Some((Date::new(year, 1, 1), Date::new(year + 1, 1, 1))),
                [_, w] if w.starts_with(['W', 'w']) => {
                    let wk: i64 = w[1..].parse().ok()?;
                    let a = Date::new(year, 1, 4).week_start().add_days(7 * (wk - 1));
                    Some((a, a.add_days(7)))
                }
                [_, m] => {
                    let a = Date::new(year, m.parse().ok()?, 1);
                    Some((a, a.add_months(1)))
                }
                [_, _, _] => {
                    let a = Date::parse_iso(s)?;
                    Some((a, a.add_days(1)))
                }
                _ => None,
            }
        }
    }
}

/// `:tstart`/`:tend`: "2026-10-01", "2026-10-01 09:00", "<-1w>", "now".
fn instant(spec: &str, today: Date, now: Hm) -> Option<i64> {
    let s = spec
        .trim()
        .trim_start_matches(['<', '['])
        .trim_end_matches(['>', ']']);
    let now_min = day_start(today) + now.0 * 60 + now.1;
    match s {
        "now" => return Some(now_min),
        "today" => return Some(day_start(today)),
        _ => {}
    }
    if let Some(last) = s.chars().last()
        && let Ok(n) = s[..s.len() - 1].parse::<i64>()
        && (s.starts_with('+') || s.starts_with('-'))
    {
        return Some(match last {
            'h' => now_min + n * 60,
            'd' => day_start(today.add_days(n)),
            'w' => day_start(today.add_days(7 * n)),
            'm' => day_start(today.add_months(n)),
            'y' => day_start(Date::new(today.y + n, today.m, today.d.min(28))),
            _ => return None,
        });
    }
    let (d, rest) = s.split_once(' ').unwrap_or((s, ""));
    let d = Date::parse_iso(d)?;
    let (h, m) = rest.split_once(':').unwrap_or(("0", "0"));
    Some(day_start(d) + h.trim().parse::<i64>().ok()? * 60 + m.trim().parse::<i64>().ok()?)
}

/// The clock range a clocktable's parameters ask for, in minutes since
/// the epoch: `(from, to)`, each `None` when open.
pub fn clock_range(ps: &[(String, String)], today: Date, now: Hm) -> (Option<i64>, Option<i64>) {
    if let Some(b) = get(ps, "block") {
        return match block_range(b, today) {
            Some((a, z)) => (Some(day_start(a)), Some(day_start(z))),
            None => (None, None),
        };
    }
    (
        get(ps, "tstart").and_then(|s| instant(s, today, now)),
        get(ps, "tend").and_then(|s| instant(s, today, now)),
    )
}

// ---------------------------------------------------------- clocktable

/// Whether tags satisfy a `:match` expression: `a|b`, `a&b`, `+a-b`.
fn tags_match(expr: &str, tags: &[String]) -> bool {
    let has = |t: &str| tags.iter().any(|x| x == t);
    expr.split('|').any(|alt| {
        let mut ok = true;
        let mut sign = '+';
        let mut word = String::new();
        let flush = |sign: char, word: &mut String, ok: &mut bool| {
            if !word.is_empty() {
                if has(word) != (sign != '-') {
                    *ok = false;
                }
                word.clear();
            }
        };
        for c in alt.chars() {
            match c {
                '+' | '-' | '&' => {
                    flush(sign, &mut word, &mut ok);
                    sign = if c == '-' { '-' } else { '+' };
                }
                c => word.push(c),
            }
        }
        flush(sign, &mut word, &mut ok);
        ok
    })
}

impl Buffer {
    /// The tags on a heading and on the headings above it.
    fn org_tags_inherited(&self, h: usize) -> Vec<String> {
        let mut out = self.org_heading_tags(h);
        let mut level = self.org_level(h).unwrap_or(1);
        for l in (0..h).rev() {
            if let Some(lv) = self.org_level(l)
                && lv < level
            {
                level = lv;
                out.extend(self.org_heading_tags(l));
            }
        }
        out
    }

    /// The headings above `h`, outermost first (and `h`).
    fn org_ancestor_at_level(&self, h: usize, level: usize) -> Option<usize> {
        let mut cur = self.org_level(h)?;
        if cur == level {
            return Some(h);
        }
        for l in (0..h).rev() {
            if let Some(lv) = self.org_level(l)
                && lv < cur
            {
                cur = lv;
                if lv == level {
                    return Some(l);
                }
            }
        }
        None
    }
}

/// Minutes of `[s, e]` that fall inside `[lo, hi)`.
fn clipped(s: i64, e: i64, lo: Option<i64>, hi: Option<i64>) -> i64 {
    let a = lo.map_or(s, |lo| s.max(lo));
    let z = hi.map_or(e, |hi| e.min(hi));
    (z - a).max(0)
}

struct Entry {
    heading: usize,
    minutes: i64,
}

/// The clocktable's text: the caption and the table(s).
pub fn clocktable(
    b: &Buffer,
    ps: &[(String, String)],
    at: usize,
    today: Date,
    now: Hm,
) -> Vec<String> {
    let maxlevel = get(ps, "maxlevel")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(2);
    let scope = get(ps, "scope").unwrap_or("file");
    let last = b.line_count().saturating_sub(1);
    let (from, to) = match scope {
        "subtree" | "tree" => b
            .org_heading_at(at)
            .map_or((0, last), |h| (h, b.org_subtree_end(h))),
        s if s.starts_with("tree") => s[4..]
            .parse::<usize>()
            .ok()
            .and_then(|n| {
                b.org_heading_at(at)
                    .and_then(|h| b.org_ancestor_at_level(h, n))
            })
            .map_or((0, last), |h| (h, b.org_subtree_end(h))),
        _ => (0, last),
    };
    let matcher = get(ps, "match");
    let kw = keywords(b);
    // Clock lines, each owned by the heading it is under.
    let mut cur: Option<usize> = None;
    let mut clocks: Vec<(usize, i64, i64)> = Vec::new();
    for l in from..=to.min(last) {
        if b.org_level(l).is_some() {
            cur = Some(l);
        } else if let (Some(h), Some(c)) = (cur, parse_clock_line(&b.line(l), l))
            && let Some(e) = &c.end
        {
            clocks.push((h, c.start.minutes(), e.minutes()));
        }
    }
    let (lo, hi) = clock_range(ps, today, now);
    let steps = step_ranges(ps, lo, hi, today);
    let indent = truthy(ps, "indent", true);
    let stamp = Timestamp::new(false, today, Some(now)).text();
    let mut out = vec![format!("#+CAPTION: Clock summary at {stamp}")];
    let ranges: Vec<(Option<i64>, Option<i64>, Option<String>)> = if steps.is_empty() {
        vec![(lo, hi, None)]
    } else {
        steps
            .into_iter()
            .map(|(a, z, t)| (Some(a), Some(z), Some(t)))
            .collect()
    };
    for (lo, hi, title) in ranges {
        let entries: Vec<Entry> = clocks
            .iter()
            .filter(|(h, ..)| matcher.is_none_or(|m| tags_match(m, &b.org_tags_inherited(*h))))
            .map(|&(h, s, e)| Entry {
                heading: h,
                minutes: clipped(s, e, lo, hi),
            })
            .filter(|e| e.minutes > 0)
            .collect();
        let total: i64 = entries.iter().map(|e| e.minutes).sum();
        if title.is_some() {
            if total == 0 && truthy(ps, "stepskip0", false) {
                continue;
            }
            out.push(String::new());
            out.push(title.unwrap_or_default());
        }
        let mut rows: Vec<(usize, String, i64)> = Vec::new();
        for h in from..=to.min(last) {
            if let Some(hd) = parse_heading(&b.line(h), &kw) {
                let end = b.org_subtree_end(h);
                let t: i64 = entries
                    .iter()
                    .filter(|e| (h..=end).contains(&e.heading))
                    .map(|e| e.minutes)
                    .sum();
                if t > 0 && hd.level <= maxlevel {
                    rows.push((hd.level, clean_title(&hd.title), t));
                }
            }
        }
        let base = rows.iter().map(|(l, _, _)| *l).min().unwrap_or(1);
        let mut table = vec![
            crate::org_table::Row::Data(vec!["Headline".into(), "Time".into()]),
            crate::org_table::Row::Rule,
            crate::org_table::Row::Data(vec!["*Total time*".into(), format!("*{}*", hmm(total))]),
            crate::org_table::Row::Rule,
        ];
        for (lv, title, t) in &rows {
            let pad = if indent && *lv > base {
                format!(
                    "{}{}",
                    clocktable_indent_string(),
                    " ".repeat(2 * (lv - base))
                )
            } else {
                String::new()
            };
            table.push(crate::org_table::Row::Data(vec![
                format!("{pad}{title}"),
                hmm(*t),
            ]));
        }
        out.extend(crate::org_table::format_rows(&table, ""));
    }
    out
}

/// `:step day|week|month|year` over a closed range: each step's start,
/// end and the line that heads its table.
fn step_ranges(
    ps: &[(String, String)],
    lo: Option<i64>,
    hi: Option<i64>,
    _today: Date,
) -> Vec<(i64, i64, String)> {
    let (Some(lo), Some(hi), Some(step)) = (lo, hi, get(ps, "step")) else {
        return Vec::new();
    };
    let (label, next): (&str, fn(Date) -> Date) = match step {
        "day" => ("Daily report: ", |d| d.add_days(1)),
        "week" => ("Weekly report starting on: ", |d| d.add_days(7)),
        "month" => ("Monthly report starting on: ", |d| d.add_months(1)),
        "year" => ("Yearly report starting on: ", |d| d.add_months(12)),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut d = Date::from_days(lo.div_euclid(1440));
    for _ in 0..800 {
        if day_start(d) >= hi {
            break;
        }
        let n = next(d);
        let a = day_start(d).max(lo);
        let z = day_start(n).min(hi);
        out.push((
            a,
            z,
            format!("{label}{}", Timestamp::new(false, d, None).text()),
        ));
        d = n;
    }
    out
}

// ---------------------------------------------------------- columnview

/// The `columnview` writer: a table of the columns for a subtree.
pub fn columnview(b: &Buffer, ps: &[(String, String)], at: usize) -> Result<Vec<String>, String> {
    use crate::org_columns::{DEFAULT_FORMAT, parse_format};
    let last = b.line_count().saturating_sub(1);
    let id = get(ps, "id").unwrap_or("global");
    let section = match id {
        "global" => None,
        "local" => Some(
            b.org_heading_at(at)
                .ok_or("No heading to take the columns from")?,
        ),
        id => Some(
            (0..=last)
                .find(|&l| {
                    b.org_level(l).is_some()
                        && (b.org_get_property(l, "ID", false).as_deref() == Some(id)
                            || b.org_get_property(l, "CUSTOM_ID", false).as_deref() == Some(id))
                })
                .ok_or_else(|| format!("Cannot find entry with ID \"{id}\""))?,
        ),
    };
    let fmt = get(ps, "format").map(String::from).unwrap_or_else(|| {
        let f = b.org_columns_format(section);
        if f.is_empty() {
            DEFAULT_FORMAT.to_string()
        } else {
            f
        }
    });
    let cols = parse_format(&fmt)?;
    let (from, to) = section.map_or((0, last), |h| (h, b.org_subtree_end(h)));
    let maxlevel = get(ps, "maxlevel").and_then(|n| n.parse::<usize>().ok());
    let skip_empty = truthy(ps, "skip-empty-rows", false);
    let hlines: Option<usize> = match get(ps, "hlines") {
        Some("t") => Some(usize::MAX),
        Some(n) => n.parse().ok(),
        None => None,
    };
    let rows: Vec<_> = b
        .org_columns_rows(&cols, from, to)
        .into_iter()
        .filter(|r| maxlevel.is_none_or(|m| r.level <= m))
        .filter(|r| {
            !skip_empty
                || r.cells
                    .iter()
                    .zip(&cols)
                    .any(|(v, c)| c.prop != "ITEM" && !v.trim().is_empty())
        })
        .collect();
    let base = rows.iter().map(|r| r.level).min().unwrap_or(1);
    use crate::org_table::Row as T;
    let mut table = vec![
        T::Data(cols.iter().map(|c| c.title.clone()).collect()),
        T::Rule,
    ];
    for (i, r) in rows.iter().enumerate() {
        if i > 0 && hlines.is_some_and(|n| r.level <= n) {
            table.push(T::Rule);
        }
        table.push(T::Data(
            cols.iter()
                .zip(&r.cells)
                .map(|(c, v)| {
                    if c.prop == "ITEM" && r.level > base {
                        format!(
                            "{}{}{v}",
                            clocktable_indent_string(),
                            " ".repeat(2 * (r.level - base))
                        )
                    } else {
                        v.clone()
                    }
                })
                .collect(),
        ));
    }
    Ok(crate::org_table::format_rows(&table, ""))
}

// --------------------------------------------------------- the update

/// What `org_dblock_update` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Updated {
    /// The block was rewritten (its begin line).
    Done(usize),
    /// Not a built-in: the editor runs `org-dblock-write:NAME`; the block
    /// is empty and `end` is its `#+END:` line.
    Custom(Block),
}

impl Buffer {
    /// Rewrite the dynamic block around `line`. A name that isn't built in
    /// empties the block and comes back as [`Updated::Custom`].
    pub fn org_dblock_update(
        &mut self,
        line: usize,
        today: Date,
        now: Hm,
    ) -> Result<Updated, String> {
        let blk = block_at(self, line).ok_or("Not at a dynamic block")?;
        let ps = parse_params(&blk.params);
        let text = match blk.name.to_lowercase().as_str() {
            "clocktable" => Some(clocktable(self, &ps, blk.begin, today, now)),
            "columnview" => Some(columnview(self, &ps, blk.begin)?),
            _ => None,
        };
        let lines = text.unwrap_or_default();
        let custom = !matches!(
            blk.name.to_lowercase().as_str(),
            "clocktable" | "columnview"
        );
        let a = self.line_start_offset(blk.begin + 1);
        let z = self.line_start_offset(blk.end);
        let body = if lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", lines.join("\n"))
        };
        self.replace_offsets(a, z, &body);
        if custom {
            return Ok(Updated::Custom(Block {
                end: blk.begin + 1,
                ..blk
            }));
        }
        Ok(Updated::Done(blk.begin))
    }
}

impl Buffer {
    /// A writer that left no newline before `#+END:` gets one.
    pub fn org_dblock_mend_end(&mut self, begin: usize) {
        for l in begin + 1..self.line_count() {
            let text = self.line(l);
            let t = text.trim_end();
            if t.eq_ignore_ascii_case("#+END:") {
                return;
            }
            if t.len() > 6
                && t.get(t.len() - 6..)
                    .is_some_and(|e| e.eq_ignore_ascii_case("#+END:"))
            {
                let at = self.line_start_offset(l) + t.chars().count() - 6;
                self.replace_offsets(at, at, "\n");
                return;
            }
        }
    }

    /// An empty block of kind `name` on `line` if that line is blank,
    /// otherwise on the line below. Returns its begin line.
    pub fn org_dblock_insert(&mut self, line: usize, name: &str) -> usize {
        let header = match name {
            "clocktable" => "#+BEGIN: clocktable :scope file :maxlevel 2".to_string(),
            "columnview" => "#+BEGIN: columnview :hlines 1 :id global".to_string(),
            n => format!("#+BEGIN: {n}"),
        };
        let line = line.min(self.line_count().saturating_sub(1));
        let at_line = if self.line(line).trim().is_empty() {
            line
        } else {
            line + 1
        };
        if at_line >= self.line_count() {
            let end = self.line_end_offset(line);
            self.replace_offsets(end, end, &format!("\n{header}\n#+END:"));
        } else {
            let at = self.line_start_offset(at_line);
            self.replace_offsets(at, at, &format!("{header}\n#+END:\n"));
        }
        at_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: Date = Date {
        y: 2026,
        m: 10,
        d: 14,
    }; // a Wednesday

    fn org(s: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(s);
        b
    }

    #[test]
    fn finds_blocks_and_params() {
        let b = org(
            "* A\n#+BEGIN: clocktable :maxlevel 3 :scope file\n#+END:\n#+BEGIN_SRC x\n#+END_SRC\n",
        );
        let all = all_blocks(&b);
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].begin, all[0].end), (1, 2));
        assert_eq!(all[0].name, "clocktable");
        let ps = parse_params(r#":format "%ITEM %Effort" :hlines t :x"#);
        assert_eq!(get(&ps, "format"), Some("%ITEM %Effort"));
        assert_eq!(get(&ps, "hlines"), Some("t"));
        assert_eq!(get(&ps, "x"), None);
        assert!(block_at(&b, 2).is_some() && block_at(&b, 0).is_none());
    }

    #[test]
    fn block_ranges() {
        let r = |s| block_range(s, D).map(|(a, z)| (a.iso(), z.iso()));
        assert_eq!(r("today"), Some(("2026-10-14".into(), "2026-10-15".into())));
        assert_eq!(r("yesterday").unwrap().0, "2026-10-13");
        assert_eq!(
            r("thisweek").unwrap(),
            ("2026-10-12".into(), "2026-10-19".into())
        );
        assert_eq!(r("lastweek").unwrap().0, "2026-10-05");
        assert_eq!(r("thisweek-2").unwrap().0, "2026-09-28");
        assert_eq!(
            r("thismonth").unwrap(),
            ("2026-10-01".into(), "2026-11-01".into())
        );
        assert_eq!(r("lastmonth").unwrap().0, "2026-09-01");
        assert_eq!(r("thisyear").unwrap().1, "2027-01-01");
        assert_eq!(
            r("2025").unwrap(),
            ("2025-01-01".into(), "2026-01-01".into())
        );
        assert_eq!(r("2026-02").unwrap().1, "2026-03-01");
        assert_eq!(r("2026-W41").unwrap().0, "2026-10-05");
        assert_eq!(r("2026-10-12").unwrap().1, "2026-10-13");
        assert_eq!(r("nonsense"), None);
    }

    const CLOCKS: &str = "* A :work:\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 10:00] =>  1:00\nCLOCK: [2026-10-14 Wed 09:00]--[2026-10-14 Wed 09:30] =>  0:30\n:END:\n** B\n:LOGBOOK:\nCLOCK: [2026-10-14 Wed 10:00]--[2026-10-14 Wed 10:45] =>  0:45\n:END:\n* C\n:LOGBOOK:\nCLOCK: [2026-10-13 Tue 23:30]--[2026-10-14 Wed 00:30] =>  1:00\n:END:\n";

    fn table_of(b: &Buffer, header: &str, today: Date) -> Vec<String> {
        let mut b2 = org(&format!("{CLOCKS}{header}\n#+END:\n"));
        let _ = b;
        let line = b2.line_count() - 3;
        b2.org_dblock_update(line, today, (12, 0)).unwrap();
        (0..b2.line_count()).map(|l| b2.line(l)).collect()
    }

    #[test]
    fn clocktable_with_a_block_clips_to_the_range() {
        let all = table_of(&org(""), "#+BEGIN: clocktable :maxlevel 2 :block today", D);
        let t = all.join("\n");
        // Wed: A 0:30 + B 0:45 under A, C only the half hour after midnight.
        assert!(t.contains("*Total time*") && t.contains("*1:45*"), "{t}");
        assert!(t.contains("| A") && t.contains("1:15"), "{t}");
        assert!(t.contains("| C") && t.contains("0:30"), "{t}");
        let w = table_of(&org(""), "#+BEGIN: clocktable :block thisweek", D).join("\n");
        assert!(w.contains("*3:15*"), "{w}");
        let r = table_of(
            &org(""),
            "#+BEGIN: clocktable :tstart \"2026-10-13\" :tend \"2026-10-14\"",
            D,
        )
        .join("\n");
        assert!(r.contains("*0:30*") || r.contains("*1:00*"), "{r}");
    }

    #[test]
    fn clocktable_steps_and_match() {
        let t = table_of(
            &org(""),
            "#+BEGIN: clocktable :block thisweek :step day :stepskip0 t",
            D,
        )
        .join("\n");
        assert!(t.contains("Daily report: [2026-10-12 Mon]"), "{t}");
        assert!(t.contains("Daily report: [2026-10-14 Wed]"), "{t}");
        assert!(!t.contains("2026-10-15"), "{t}");
        let m = table_of(&org(""), "#+BEGIN: clocktable :match \"work\"", D).join("\n");
        assert!(m.contains("*2:15*"), "{m}");
        let n = table_of(&org(""), "#+BEGIN: clocktable :match \"-work\"", D).join("\n");
        assert!(n.contains("*1:00*"), "{n}");
    }

    #[test]
    fn clocktable_subtree_scope() {
        let mut b = org(&format!(
            "{CLOCKS}** Sub\n#+BEGIN: clocktable :scope subtree\n#+END:\n"
        ));
        // The block sits under "** Sub" (no clocks): empty total.
        let at = b.line_count() - 3;
        b.org_dblock_update(at, D, (12, 0)).unwrap();
        let t = b.get_contents();
        assert!(t.contains("*0:00*"), "{t}");
    }

    #[test]
    fn columnview_block() {
        let mut b = org(
            "* A\n:PROPERTIES:\n:Effort: 1:00\n:END:\n** B\n:PROPERTIES:\n:Effort: 0:30\n:END:\n#+BEGIN: columnview :id global :format \"%ITEM %Effort\" :hlines 1\n#+END:\n",
        );
        let at = b.line_count() - 3;
        let u = b.org_dblock_update(at, D, (12, 0)).unwrap();
        assert!(matches!(u, Updated::Done(_)));
        let t = b.get_contents();
        assert!(t.contains("| ITEM") && t.contains("| Effort"), "{t}");
        assert!(t.contains("| A") && t.contains("1:00"), "{t}");
        assert!(t.contains("\\_") && t.contains("0:30"), "{t}");
        // Running it again replaces the table.
        let at = t.lines().position(|l| l.starts_with("#+BEGIN:")).unwrap();
        b.org_dblock_update(at, D, (12, 0)).unwrap();
        assert_eq!(b.get_contents(), t);
        let mut c = org("* A\n#+BEGIN: columnview :id nope\n#+END:\n");
        assert!(c.org_dblock_update(1, D, (0, 0)).is_err());
        let mut d = org("* A\ntext");
        let at = d.org_dblock_insert(1, "clocktable");
        assert_eq!(at, 2);
        assert_eq!(
            d.get_contents(),
            "* A\ntext\n#+BEGIN: clocktable :scope file :maxlevel 2\n#+END:"
        );
        let mut e = org("x\n#+BEGIN: mine\nfoo#+END:\n");
        e.org_dblock_mend_end(1);
        assert_eq!(e.get_contents(), "x\n#+BEGIN: mine\nfoo\n#+END:\n");
    }

    #[test]
    fn unknown_blocks_are_left_empty_for_a_writer() {
        let mut b = org("#+BEGIN: mine :x 1\nold\n#+END:\nafter\n");
        let u = b.org_dblock_update(1, D, (0, 0)).unwrap();
        let Updated::Custom(k) = u else { panic!() };
        assert_eq!((k.name.as_str(), k.params.as_str()), ("mine", ":x 1"));
        assert_eq!(b.get_contents(), "#+BEGIN: mine :x 1\n#+END:\nafter\n");
        assert_eq!(k.end, 1);
        assert!(org("text\n").org_dblock_update(0, D, (0, 0)).is_err());
    }
}
