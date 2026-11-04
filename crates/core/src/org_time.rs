// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dates, times and Org timestamps, without a date crate.
//!
//! - [`Date`]: a civil date with day/month/year arithmetic and weekdays.
//! - [`Timestamp`]: `<2026-10-12 Mon 14:00-15:30 +1w -2d>` and its inactive
//!   `[...]` form, parsed out of a line with character columns so the
//!   buffer can replace it in place.
//! - [`parse_date_input`]: what you type at an "org-read-date" prompt
//!   ("tomorrow", "+2w", "fri 9:00", "10-31", "2026-12-24 18:30").
//! - [`now_local`]: the local date and time (libc on unix, UTC elsewhere).

use regex::Regex;
use std::sync::LazyLock;

pub const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const LONG_DAY_NAMES: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The name of month `m`, clamped to the months that exist: a `Date` can
/// be built with any month number.
pub(crate) fn month_name(m: i64) -> &'static str {
    MONTH_NAMES[(m.clamp(1, 12) - 1) as usize]
}

/// A civil (proleptic Gregorian) date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub y: i64,
    pub m: i64,
    pub d: i64,
}

impl Date {
    pub fn new(y: i64, m: i64, d: i64) -> Self {
        Self { y, m, d }
    }

    /// Days since 1970-01-01.
    pub fn days(self) -> i64 {
        let y = self.y - i64::from(self.m <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (self.m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + self.d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_days(z: i64) -> Self {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Self {
            y: y + i64::from(m <= 2),
            m,
            d,
        }
    }

    /// Monday is 0.
    pub fn weekday(self) -> usize {
        (self.days() + 3).rem_euclid(7) as usize
    }

    pub fn day_name(self) -> &'static str {
        DAY_NAMES[self.weekday()]
    }

    pub fn add_days(self, n: i64) -> Self {
        Self::from_days(self.days() + n)
    }

    pub fn days_in_month(y: i64, m: i64) -> i64 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            _ => 28,
        }
    }

    /// Months later, clamping the day (Jan 31 + 1 month = Feb 28).
    pub fn add_months(self, n: i64) -> Self {
        let total = self.y * 12 + (self.m - 1) + n;
        let (y, m) = (total.div_euclid(12), total.rem_euclid(12) + 1);
        Self {
            y,
            m,
            d: self.d.min(Self::days_in_month(y, m)),
        }
    }

    pub fn add_years(self, n: i64) -> Self {
        self.add_months(n * 12)
    }

    /// The Monday of this date's week.
    pub fn week_start(self) -> Self {
        self.add_days(-(self.weekday() as i64))
    }

    /// ISO week number.
    pub fn iso_week(self) -> i64 {
        let thursday = self.add_days(3 - self.weekday() as i64);
        let jan1 = Date::new(thursday.y, 1, 1);
        (thursday.days() - jan1.days()) / 7 + 1
    }

    pub fn valid(self) -> bool {
        (1..=12).contains(&self.m) && self.d >= 1 && self.d <= Self::days_in_month(self.y, self.m)
    }

    /// "2026-10-12".
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    pub fn parse_iso(s: &str) -> Option<Self> {
        let mut it = s.trim().splitn(3, '-');
        let d = Self {
            y: it.next()?.parse().ok()?,
            m: it.next()?.parse().ok()?,
            d: it.next()?.parse().ok()?,
        };
        d.valid().then_some(d)
    }
}

/// Hours and minutes.
pub type Hm = (i64, i64);

/// The unit of a repeater or warning period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Unit {
    fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'h' => Self::Hour,
            'd' => Self::Day,
            'w' => Self::Week,
            'm' => Self::Month,
            'y' => Self::Year,
            _ => return None,
        })
    }

    pub fn ch(self) -> char {
        match self {
            Self::Hour => 'h',
            Self::Day => 'd',
            Self::Week => 'w',
            Self::Month => 'm',
            Self::Year => 'y',
        }
    }
}

/// `+1w`, `++1w`, `.+1w`; or a warning `-3d` / `--3d`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    /// "+", "++", ".+", "-" or "--".
    pub mark: String,
    pub n: i64,
    pub unit: Unit,
}

impl Period {
    pub fn text(&self) -> String {
        format!("{}{}{}", self.mark, self.n, self.unit.ch())
    }

    /// Days a warning period covers (months and years are 30 and 365).
    pub fn days(&self) -> i64 {
        match self.unit {
            Unit::Hour => 0,
            Unit::Day => self.n,
            Unit::Week => self.n * 7,
            Unit::Month => self.n * 30,
            Unit::Year => self.n * 365,
        }
    }
}

/// A parsed timestamp and where it sits in its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp {
    pub active: bool,
    pub date: Date,
    pub time: Option<Hm>,
    pub end_time: Option<Hm>,
    pub repeater: Option<Period>,
    pub warning: Option<Period>,
    /// Character columns of the whole `<...>` in the line.
    pub start: usize,
    pub end: usize,
}

static TS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"([<\[])(\d{4})-(\d{2})-(\d{2})(?:\s+[A-Za-z]{2,}\.?)?(?:\s+(\d{1,2}):(\d{2})(?:-(\d{1,2}):(\d{2}))?)?((?:\s+(?:\+\+|\.\+|\+|--|-)\d+[hdwmy](?:/\d+[hdwmy])?)*)\s*([>\]])",
    )
    .unwrap()
});
static PERIOD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\+\+|\.\+|\+|--|-)(\d+)([hdwmy])").unwrap());

impl Timestamp {
    pub fn new(active: bool, date: Date, time: Option<Hm>) -> Self {
        Self {
            active,
            date,
            time,
            end_time: None,
            repeater: None,
            warning: None,
            start: 0,
            end: 0,
        }
    }

    /// The text: `<2026-10-12 Mon 14:00 +1w -2d>`.
    pub fn text(&self) -> String {
        let (o, c) = if self.active { ('<', '>') } else { ('[', ']') };
        let mut s = format!("{o}{} {}", self.date.iso(), self.date.day_name());
        if let Some((h, m)) = self.time {
            s.push_str(&format!(" {h:02}:{m:02}"));
            if let Some((h2, m2)) = self.end_time {
                s.push_str(&format!("-{h2:02}:{m2:02}"));
            }
        }
        if let Some(r) = &self.repeater {
            s.push(' ');
            s.push_str(&r.text());
        }
        if let Some(w) = &self.warning {
            s.push(' ');
            s.push_str(&w.text());
        }
        s.push(c);
        s
    }

    /// The same moment as an ordering key (minutes since the epoch).
    pub fn minutes(&self) -> i64 {
        let (h, m) = self.time.unwrap_or((0, 0));
        self.date.days() * 1440 + h * 60 + m
    }

    /// The repeated timestamp one step on, for a repeater. `today` and
    /// `now` matter for `++` (catch up past now) and `.+` (from now).
    pub fn next_repeat(&self, today: Date, now: Hm) -> Option<Timestamp> {
        let r = self.repeater.as_ref()?;
        let step = |d: Date, n: i64| match r.unit {
            Unit::Hour | Unit::Day => d.add_days(n * r.n),
            Unit::Week => d.add_days(n * r.n * 7),
            Unit::Month => d.add_months(n * r.n),
            Unit::Year => d.add_years(n * r.n),
        };
        let mut next = self.clone();
        match r.mark.as_str() {
            "+" => next.date = step(self.date, 1),
            "++" => {
                let mut d = step(self.date, 1);
                let mut guard = 0;
                while d <= today && guard < 100_000 {
                    d = step(d, 1);
                    guard += 1;
                }
                next.date = d;
            }
            _ => {
                // ".+": the interval counts from today.
                next.date = step(today, 1);
                if next.time.is_some() && r.unit == Unit::Hour {
                    let (h, m) = now;
                    let total = h * 60 + m + r.n * 60;
                    next.date = today.add_days(total.div_euclid(1440));
                    let t = total.rem_euclid(1440);
                    next.time = Some((t / 60, t % 60));
                }
            }
        }
        if r.unit == Unit::Hour
            && r.mark != ".+"
            && let Some((h, m)) = self.time
        {
            let total = h * 60 + m + r.n * 60;
            next.date = self.date.add_days(total.div_euclid(1440));
            let t = total.rem_euclid(1440);
            next.time = Some((t / 60, t % 60));
        }
        Some(next)
    }
}

/// Every timestamp in `line`, with character columns.
pub fn timestamps_in(line: &str) -> Vec<Timestamp> {
    let mut out = Vec::new();
    for c in TS.captures_iter(line) {
        let whole = c.get(0).unwrap();
        let open = &c[1];
        let close = &c[10];
        if (open == "<") != (close == ">") {
            continue;
        }
        let num = |i: usize| c.get(i).and_then(|m| m.as_str().parse::<i64>().ok());
        let date = Date::new(
            num(2).unwrap_or(0),
            num(3).unwrap_or(0),
            num(4).unwrap_or(0),
        );
        if !date.valid() {
            continue;
        }
        let time = num(5).zip(num(6));
        let end_time = num(7).zip(num(8));
        let mut repeater = None;
        let mut warning = None;
        for p in PERIOD.captures_iter(c.get(9).map_or("", |m| m.as_str())) {
            let period = Period {
                mark: p[1].to_string(),
                n: p[2].parse().unwrap_or(1),
                unit: Unit::from_char(p[3].chars().next().unwrap_or('d')).unwrap_or(Unit::Day),
            };
            if period.mark.starts_with('-') {
                warning = Some(period);
            } else {
                repeater = Some(period);
            }
        }
        out.push(Timestamp {
            active: open == "<",
            date,
            time,
            end_time,
            repeater,
            warning,
            start: line[..whole.start()].chars().count(),
            end: line[..whole.end()].chars().count(),
        });
    }
    out
}

/// The timestamp whose text covers character column `col`.
pub fn timestamp_at(line: &str, col: usize) -> Option<Timestamp> {
    timestamps_in(line)
        .into_iter()
        .find(|t| col >= t.start && col <= t.end.saturating_sub(1).max(t.start))
}

/// Which part of a timestamp the cursor is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Repeater,
    Warning,
}

/// The field at `col` (a column inside the timestamp text).
pub fn field_at(ts: &Timestamp, col: usize) -> Field {
    let text = ts.text();
    let rel = col
        .saturating_sub(ts.start)
        .min(text.chars().count().saturating_sub(1));
    // "<2026-10-12 Mon 14:00 +1w -2d>": columns by layout.
    match rel {
        0..=5 => Field::Year,
        6..=8 => Field::Month,
        9..=11 => Field::Day,
        12..=15 => Field::Day, // the weekday name moves the day
        _ => {
            let chars: Vec<char> = text.chars().collect();
            let mut i = 16.min(chars.len());
            let mut start = i;
            let mut words: Vec<(usize, usize, String)> = Vec::new();
            while i <= chars.len() {
                if i == chars.len() || chars[i] == ' ' || chars[i] == '>' || chars[i] == ']' {
                    if i > start {
                        words.push((start, i, chars[start..i].iter().collect()));
                    }
                    start = i + 1;
                }
                i += 1;
            }
            for (a, z, w) in words {
                if rel >= a && rel < z {
                    if w.contains(':') {
                        let colon = a + w.find(':').unwrap_or(0);
                        return if rel < colon {
                            Field::Hour
                        } else {
                            Field::Minute
                        };
                    }
                    if w.starts_with('-') {
                        return Field::Warning;
                    }
                    return Field::Repeater;
                }
            }
            Field::Day
        }
    }
}

/// `ts` moved by `delta` in `field`.
pub fn adjust(ts: &Timestamp, field: Field, delta: i64) -> Timestamp {
    let mut t = ts.clone();
    match field {
        Field::Year => t.date = ts.date.add_years(delta),
        Field::Month => t.date = ts.date.add_months(delta),
        Field::Day => t.date = ts.date.add_days(delta),
        Field::Hour | Field::Minute => {
            let (h, m) = ts.time.unwrap_or((0, 0));
            let step = if field == Field::Hour { 60 } else { 5 };
            let total = h * 60 + m + delta * step;
            t.date = ts.date.add_days(total.div_euclid(1440));
            let r = total.rem_euclid(1440);
            // Minutes round to the step, like Org.
            let r = if field == Field::Minute { r / 5 * 5 } else { r };
            t.time = Some((r / 60, r % 60));
        }
        Field::Repeater => {
            if let Some(p) = &mut t.repeater {
                p.n = (p.n + delta).max(1);
            }
        }
        Field::Warning => {
            if let Some(p) = &mut t.warning {
                p.n = (p.n + delta).max(0);
            }
        }
    }
    t
}

/// Local date and time.
pub fn now_local() -> (Date, Hm) {
    #[cfg(unix)]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as libc::time_t);
        // SAFETY: localtime_r writes only into `tm`, which we own.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
        if ok {
            return (
                Date::new(
                    i64::from(tm.tm_year) + 1900,
                    i64::from(tm.tm_mon) + 1,
                    i64::from(tm.tm_mday),
                ),
                (i64::from(tm.tm_hour), i64::from(tm.tm_min)),
            );
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (
        Date::from_days(secs.div_euclid(86_400)),
        (secs.rem_euclid(86_400) / 3600, secs.rem_euclid(3600) / 60),
    )
}

fn parse_hm(s: &str) -> Option<Hm> {
    let (h, m) = s.split_once(':')?;
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

/// What someone types at a date prompt. `default` is the date used for
/// the parts left out ("10-31" uses the default's year, "25" its month).
/// Returns the date and a time if one was given.
pub fn parse_date_input(input: &str, today: Date, default: Date) -> Option<(Date, Option<Hm>)> {
    // "nov 20", "20 nov 2027": a month name turns the numbers around it
    // into a day and a year; they're rewritten as one ISO date.
    let month_of = |w: &str| {
        (w.len() >= 3)
            .then(|| {
                MONTH_NAMES
                    .iter()
                    .position(|m| m.to_lowercase().starts_with(w))
            })
            .flatten()
    };
    let words: Vec<String> = input.split_whitespace().map(str::to_lowercase).collect();
    if let Some(m) = words.iter().find_map(|w| month_of(w)) {
        let (mut day, mut year, mut rest) = (None, None, Vec::new());
        for w in &words {
            match w.parse::<i64>() {
                _ if month_of(w).is_some() => {}
                Ok(n) if n > 31 => year = Some(n),
                Ok(n) if day.is_none() => day = Some(n),
                Ok(n) => year = Some(2000 + n),
                Err(_) => rest.push(w.as_str()),
            }
        }
        let iso = format!(
            "{}-{}-{}",
            year.unwrap_or(default.y),
            m + 1,
            day.unwrap_or(1)
        );
        return parse_date_input(&format!("{iso} {}", rest.join(" ")), today, default);
    }
    let mut date: Option<Date> = None;
    let mut time: Option<Hm> = None;
    let mut rel = false;
    for w in input.split_whitespace() {
        let lw = w.to_lowercase();
        if let Some(t) = parse_hm(&lw) {
            time = Some(t);
        } else if (lw.ends_with("pm") || lw.ends_with("am")) && lw.len() >= 3 {
            let pm = lw.ends_with("pm");
            let body = &lw[..lw.len() - 2];
            let (h, m) = match body.split_once(':') {
                Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
                None => (body.parse::<i64>().ok()?, 0),
            };
            time = Some(((h % 12) + if pm { 12 } else { 0 }, m));
        } else if lw == "today" || lw == "now" || lw == "." {
            date = Some(today);
        } else if lw == "tomorrow" || lw == "tom" {
            date = Some(today.add_days(1));
        } else if lw == "yesterday" {
            date = Some(today.add_days(-1));
        } else if let Some(i) = DAY_NAMES.iter().position(|n| {
            lw == n.to_lowercase()
                || LONG_DAY_NAMES[DAY_NAMES.iter().position(|x| x == n).unwrap_or(0)] == lw
        }) {
            // The next such weekday after the base date. After a full date
            // that already is one ("2026-11-05 Thu") it just stays.
            let base = date.unwrap_or(default);
            let ahead = (i as i64 - base.weekday() as i64).rem_euclid(7);
            date = Some(if ahead == 0 && date.is_some() {
                base
            } else {
                base.add_days(if ahead == 0 { 7 } else { ahead })
            });
        } else if let Some(rest) = lw.strip_prefix(['+', '-']) {
            // "++1w" counts from today instead of the default date.
            let from_today = lw.starts_with("++");
            let rest = if from_today { &rest[1..] } else { rest };
            let sign = if lw.starts_with('-') { -1 } else { 1 };
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let n: i64 = if digits.is_empty() {
                1
            } else {
                digits.parse().ok()?
            };
            let unit = rest[digits.len()..].chars().next().unwrap_or('d');
            let base = if rel || date.is_some() {
                date.unwrap_or(default)
            } else if from_today {
                today
            } else {
                default
            };
            let n = sign * n;
            date = Some(match unit {
                'd' => base.add_days(n),
                'w' => base.add_days(n * 7),
                'm' => base.add_months(n),
                'y' => base.add_years(n),
                _ => return None,
            });
            rel = true;
        } else {
            let parts: Vec<&str> = lw.split(['-', '/', '.']).collect();
            let nums: Option<Vec<i64>> = parts.iter().map(|p| p.parse::<i64>().ok()).collect();
            let nums = nums?;
            let d = match nums.as_slice() {
                [y, m, d] => Date::new(if *y < 100 { 2000 + y } else { *y }, *m, *d),
                [m, d] => Date::new(default.y, *m, *d),
                [d] => Date::new(default.y, default.m, *d),
                _ => return None,
            };
            if !d.valid() {
                return None;
            }
            date = Some(d);
        }
    }
    if date.is_none() && time.is_none() {
        return input.trim().is_empty().then_some((default, None));
    }
    Some((date.unwrap_or(default), time))
}

/// `[2026-10-12 Mon 14:08]`, an inactive stamp for CLOSED and clocks.
pub fn inactive_now(date: Date, hm: Hm) -> String {
    Timestamp::new(false, date, Some(hm)).text()
}

/// Minutes as `H:MM`.
pub fn hmm(minutes: i64) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_math() {
        let d = Date::new(2026, 10, 12);
        assert_eq!(d.day_name(), "Mon");
        assert_eq!(Date::from_days(d.days()), d);
        assert_eq!(d.add_days(20).iso(), "2026-11-01");
        assert_eq!(Date::new(2026, 1, 31).add_months(1).iso(), "2026-02-28");
        assert_eq!(Date::new(2024, 2, 29).add_years(1).iso(), "2025-02-28");
        assert_eq!(Date::new(2026, 10, 12).iso_week(), 42);
        assert_eq!(Date::new(2026, 10, 14).week_start().iso(), "2026-10-12");
    }

    #[test]
    fn parse_and_format() {
        let line = "SCHEDULED: <2026-10-12 Mon 14:00-15:30 +1w -2d> and [2026-01-02 Fri]";
        let all = timestamps_in(line);
        assert_eq!(all.len(), 2);
        let t = &all[0];
        assert!(t.active);
        assert_eq!(t.time, Some((14, 0)));
        assert_eq!(t.end_time, Some((15, 30)));
        assert_eq!(t.repeater.as_ref().unwrap().text(), "+1w");
        assert_eq!(t.warning.as_ref().unwrap().days(), 2);
        assert_eq!(t.text(), "<2026-10-12 Mon 14:00-15:30 +1w -2d>");
        assert!(!all[1].active);
        assert_eq!(&line[all[1].start..all[1].end], "[2026-01-02 Fri]");
        assert!(timestamp_at(line, 15).is_some());
        assert!(timestamp_at(line, 2).is_none());
    }

    #[test]
    fn repeaters() {
        let t = &timestamps_in("<2026-10-12 Mon +1w>")[0];
        let today = Date::new(2026, 10, 30);
        assert_eq!(
            t.next_repeat(today, (0, 0)).unwrap().date.iso(),
            "2026-10-19"
        );
        let t = &timestamps_in("<2026-10-12 Mon ++1w>")[0];
        assert_eq!(
            t.next_repeat(today, (0, 0)).unwrap().date.iso(),
            "2026-11-02"
        );
        let t = &timestamps_in("<2026-10-12 Mon .+1w>")[0];
        assert_eq!(
            t.next_repeat(today, (0, 0)).unwrap().date.iso(),
            "2026-11-06"
        );
    }

    #[test]
    fn adjusting_fields() {
        let line = "<2026-10-12 Mon 14:00 +1w>";
        let t = timestamp_at(line, 3).unwrap();
        assert_eq!(field_at(&t, 3), Field::Year);
        assert_eq!(field_at(&t, 7), Field::Month);
        assert_eq!(field_at(&t, 10), Field::Day);
        assert_eq!(field_at(&t, 14), Field::Day);
        assert_eq!(field_at(&t, 17), Field::Hour);
        assert_eq!(field_at(&t, 20), Field::Minute);
        assert_eq!(field_at(&t, 23), Field::Repeater);
        assert_eq!(adjust(&t, Field::Month, 1).date.iso(), "2026-11-12");
        assert_eq!(adjust(&t, Field::Hour, 11).date.iso(), "2026-10-13");
        assert_eq!(adjust(&t, Field::Repeater, 1).repeater.unwrap().n, 2);
    }

    #[test]
    fn date_prompt() {
        let today = Date::new(2026, 10, 12);
        let p = |s: &str| parse_date_input(s, today, today);
        assert_eq!(p("tomorrow"), Some((Date::new(2026, 10, 13), None)));
        assert_eq!(p("+2w"), Some((Date::new(2026, 10, 26), None)));
        assert_eq!(
            p("fri 9:30"),
            Some((Date::new(2026, 10, 16), Some((9, 30))))
        );
        assert_eq!(p("10-31"), Some((Date::new(2026, 10, 31), None)));
        assert_eq!(p("25"), Some((Date::new(2026, 10, 25), None)));
        assert_eq!(
            p("2026-12-24 6pm"),
            Some((Date::new(2026, 12, 24), Some((18, 0))))
        );
        assert_eq!(p(""), Some((today, None)));
        assert_eq!(p("garbage"), None);
        assert_eq!(p("2026-02-30"), None);
    }
}
