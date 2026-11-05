// SPDX-License-Identifier: MIT OR Apache-2.0
//! The values properties and expressions have.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    /// Milliseconds since 1970-01-01 UTC.
    Date(i64),
    /// A length of time in milliseconds.
    Dur(i64),
    /// A link to a note, by the name it was written with.
    Link(String),
    Obj(BTreeMap<String, Value>),
}

pub const DAY: i64 = 86_400_000;

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` or `YYYY-MM-DD[T ]HH:MM[:SS]` (a trailing `Z` is allowed).
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut it = date.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut ms = days_from_civil(y, m, d) * DAY;
    if let Some(t) = time {
        let t = t.split(['+', '.']).next()?;
        let mut parts = t.split(':');
        let h: i64 = parts.next()?.parse().ok()?;
        let mi: i64 = parts.next()?.parse().ok()?;
        let sec: i64 = parts.next().map_or(Some(0), |x| x.parse().ok())?;
        if h > 23 || mi > 59 || sec > 60 {
            return None;
        }
        ms += ((h * 60 + mi) * 60 + sec) * 1000;
    }
    Some(ms)
}

/// `1d`, `2 weeks`, `3h 30m`: a duration in milliseconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let mut total = 0i64;
    let mut any = false;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .unwrap_or(rest.len());
        let n: f64 = rest[..end].parse().ok()?;
        rest = rest[end..].trim_start();
        let uend = rest
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(rest.len());
        let unit = &rest[..uend];
        rest = rest[uend..].trim_start();
        let ms = match unit {
            "ms" | "millisecond" | "milliseconds" => 1.0,
            "s" | "sec" | "second" | "seconds" => 1000.0,
            "m" | "min" | "minute" | "minutes" => 60_000.0,
            "h" | "hour" | "hours" => 3_600_000.0,
            "d" | "day" | "days" => DAY as f64,
            "w" | "week" | "weeks" => 7.0 * DAY as f64,
            "M" | "month" | "months" => 30.0 * DAY as f64,
            "y" | "year" | "years" => 365.0 * DAY as f64,
            _ => return None,
        };
        total += (n * ms).round() as i64;
        any = true;
    }
    any.then_some(total)
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Str(s) | Value::Link(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Date(_) | Value::Dur(_) => true,
            Value::Obj(o) => !o.is_empty(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Value::Null => true,
            Value::Str(s) | Value::Link(s) => s.is_empty(),
            Value::List(l) => l.is_empty(),
            Value::Obj(o) => o.is_empty(),
            _ => false,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Date(d) | Value::Dur(d) => Some(*d as f64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Date(_) => "date",
            Value::Dur(_) => "duration",
            Value::Link(_) => "link",
            Value::Obj(_) => "object",
        }
    }

    /// Order for sorting: nulls last, numbers and dates by size, text
    /// ignoring case.
    pub fn compare(&self, other: &Value) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self, other) {
            (Value::Null, Value::Null) => Equal,
            (Value::Null, _) => Greater,
            (_, Value::Null) => Less,
            (Value::Str(a) | Value::Link(a), Value::Str(b) | Value::Link(b)) => {
                a.to_lowercase().cmp(&b.to_lowercase())
            }
            (a, b) => match (a.as_num(), b.as_num()) {
                (Some(x), Some(y))
                    if !matches!(a, Value::Str(_)) || !matches!(b, Value::Str(_)) =>
                {
                    x.partial_cmp(&y).unwrap_or(Equal)
                }
                _ => a
                    .to_string()
                    .to_lowercase()
                    .cmp(&b.to_string().to_lowercase()),
            },
        }
    }

    pub fn from_yaml(y: &serde_yaml::Value) -> Value {
        use serde_yaml::Value as Y;
        match y {
            Y::Null => Value::Null,
            Y::Bool(b) => Value::Bool(*b),
            Y::Number(n) => Value::Num(n.as_f64().unwrap_or(0.0)),
            Y::String(s) => Value::from_text(s),
            Y::Sequence(l) => Value::List(l.iter().map(Value::from_yaml).collect()),
            Y::Mapping(m) => Value::Obj(
                m.iter()
                    .filter_map(|(k, v)| Some((k.as_str()?.to_string(), Value::from_yaml(v))))
                    .collect(),
            ),
            Y::Tagged(t) => Value::from_yaml(&t.value),
        }
    }

    /// A property's text: `[[Note]]` is a link, an ISO date is a date.
    pub fn from_text(s: &str) -> Value {
        let t = s.trim();
        if let Some(inner) = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) {
            let target = inner.split('|').next().unwrap_or(inner);
            return Value::Link(target.split('#').next().unwrap_or(target).to_string());
        }
        if t.len() >= 10
            && t.as_bytes()[4] == b'-'
            && let Some(d) = parse_date(t)
        {
            return Value::Date(d);
        }
        Value::Str(s.to_string())
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub fn format_date(ms: i64, fmt: &str) -> String {
    let days = ms.div_euclid(DAY);
    let (y, m, d) = civil_from_days(days);
    let rem = ms.rem_euclid(DAY) / 1000;
    let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let months = [
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
    let wd = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ][days.rem_euclid(7) as usize];
    let mut out = String::new();
    let mut rest = fmt;
    while !rest.is_empty() {
        let tokens: [(&str, String); 11] = [
            ("YYYY", format!("{y:04}")),
            ("MMMM", months[(m - 1) as usize].to_string()),
            ("MMM", months[(m - 1) as usize][..3].to_string()),
            ("MM", format!("{m:02}")),
            ("DD", format!("{d:02}")),
            ("dddd", wd.to_string()),
            ("HH", format!("{h:02}")),
            ("mm", format!("{mi:02}")),
            ("ss", format!("{s:02}")),
            ("YY", format!("{:02}", y % 100)),
            ("ddd", wd[..3].to_string()),
        ];
        if let Some((tok, val)) = tokens.iter().find(|(t, _)| rest.starts_with(t)) {
            out.push_str(val);
            rest = &rest[tok.len()..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

pub fn format_duration(ms: i64) -> String {
    let a = ms.abs();
    let sign = if ms < 0 { "-" } else { "" };
    let (v, u) = if a >= DAY {
        (a as f64 / DAY as f64, "d")
    } else if a >= 3_600_000 {
        (a as f64 / 3_600_000.0, "h")
    } else if a >= 60_000 {
        (a as f64 / 60_000.0, "m")
    } else {
        (a as f64 / 1000.0, "s")
    };
    format!("{sign}{}{u}", fmt_num((v * 10.0).round() / 10.0))
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => Ok(()),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{}", fmt_num(*n)),
            Value::Str(s) | Value::Link(s) => write!(f, "{s}"),
            Value::List(l) => {
                let parts: Vec<String> = l.iter().map(ToString::to_string).collect();
                write!(f, "{}", parts.join(", "))
            }
            Value::Date(ms) => {
                if ms.rem_euclid(DAY) == 0 {
                    write!(f, "{}", format_date(*ms, "YYYY-MM-DD"))
                } else {
                    write!(f, "{}", format_date(*ms, "YYYY-MM-DD HH:mm"))
                }
            }
            Value::Dur(ms) => write!(f, "{}", format_duration(*ms)),
            Value::Obj(o) => {
                let parts: Vec<String> = o.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                write!(f, "{}", parts.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        let d = parse_date("2024-03-15").unwrap();
        assert_eq!(d, 1_710_460_800_000);
        assert_eq!(format_date(d, "YYYY-MM-DD dddd"), "2024-03-15 Friday");
        assert_eq!(parse_date("2024-03-15T10:30:00Z"), Some(d + 37_800_000));
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date("hello"), None);
        assert_eq!(
            civil_from_days(days_from_civil(1999, 12, 31)),
            (1999, 12, 31)
        );
        assert_eq!(
            format_date(0, "MMM D"),
            "Jan D",
            "unknown tokens pass through"
        );
    }

    #[test]
    fn durations_parse_and_print() {
        assert_eq!(parse_duration("1d"), Some(DAY));
        assert_eq!(parse_duration("2 weeks"), Some(14 * DAY));
        assert_eq!(parse_duration("1h 30m"), Some(5_400_000));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(format_duration(3 * DAY), "3d");
        assert_eq!(format_duration(-90 * 60_000), "-1.5h");
    }

    #[test]
    fn text_becomes_links_and_dates() {
        assert_eq!(Value::from_text("[[Alice|A]]"), Value::Link("Alice".into()));
        assert_eq!(
            Value::from_text("[[Note#Heading]]"),
            Value::Link("Note".into())
        );
        assert!(matches!(Value::from_text("2024-01-02"), Value::Date(_)));
        assert_eq!(Value::from_text("plain"), Value::Str("plain".into()));
    }

    #[test]
    fn values_sort_and_print() {
        use std::cmp::Ordering::*;
        assert_eq!(Value::Num(2.0).compare(&Value::Num(10.0)), Less);
        assert_eq!(
            Value::Str("apple".into()).compare(&Value::Str("Banana".into())),
            Less
        );
        assert_eq!(
            Value::Null.compare(&Value::Num(1.0)),
            Greater,
            "empty sorts last"
        );
        assert_eq!(
            Value::List(vec![Value::Num(1.5), Value::Str("a".into())]).to_string(),
            "1.5, a"
        );
        assert_eq!(Value::Num(3.0).to_string(), "3");
        assert!(!Value::Str(String::new()).truthy() && Value::Num(0.1).truthy());
    }
}
