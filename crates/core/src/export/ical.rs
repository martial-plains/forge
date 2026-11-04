// SPDX-License-Identifier: MIT OR Apache-2.0
//! iCalendar export: an event for each SCHEDULED, DEADLINE and active
//! timestamp.

use super::ast::*;
use super::common::*;

fn text_esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Fold at 75 octets, as RFC 5545 says.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in line.chars() {
        let l = c.len_utf8();
        if n + l > 75 {
            out.push_str("\r\n ");
            n = 1;
        }
        out.push(c);
        n += l;
    }
    out.push_str("\r\n");
    out
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

fn next_day(y: i32, m: u32, d: u32) -> (i32, u32, u32) {
    if d < days_in_month(y, m) {
        (y, m, d + 1)
    } else if m < 12 {
        (y, m + 1, 1)
    } else {
        (y + 1, 1, 1)
    }
}

fn rrule(rep: &str) -> Option<String> {
    let body = rep.trim_start_matches(['+', '.']);
    let n: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    let unit = body.chars().find(|c| c.is_ascii_alphabetic())?;
    let f = match unit {
        'h' => "HOURLY",
        'd' => "DAILY",
        'w' => "WEEKLY",
        'm' => "MONTHLY",
        'y' => "YEARLY",
        _ => return None,
    };
    Some(format!(
        "FREQ={f};INTERVAL={}",
        if n.is_empty() { "1" } else { &n }
    ))
}

struct Event {
    prefix: &'static str,
    summary: String,
    ts: String,
    uid: String,
    desc: String,
    cats: Vec<String>,
    loc: String,
    done: bool,
}

fn body_text(body: &[&Node]) -> String {
    let mut s = String::new();
    for n in body {
        if let Block::Para(p) = &n.block {
            let t = plain(p);
            if !t.trim().is_empty() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(t.trim());
            }
        }
    }
    s.chars().take(100).collect()
}

fn active_stamps(v: &[Inline], out: &mut Vec<String>) {
    for i in v {
        match i {
            Inline::Timestamp { text, active: true } => out.push(text.clone()),
            Inline::Bold(x) | Inline::Italic(x) | Inline::Underline(x) | Inline::Strike(x) => {
                active_stamps(x, out)
            }
            _ => {}
        }
    }
}

fn collect(s: &Section, out: &mut Vec<Event>) {
    for c in &s.children {
        let h = c.heading.unwrap();
        let title = plain(&h.title);
        let desc = body_text(&c.body);
        let cats = h.tags.clone();
        let loc = h.prop("LOCATION").unwrap_or("").to_string();
        let id = h
            .prop("ID")
            .map(String::from)
            .unwrap_or_else(|| format!("org-{}", c.id));
        let done = h.done;
        for (k, ts) in &h.planning {
            let prefix = match k.as_str() {
                "SCHEDULED" => "S: ",
                "DEADLINE" => "DL: ",
                _ => continue,
            };
            out.push(Event {
                prefix,
                summary: title.clone(),
                ts: ts.clone(),
                uid: format!("{}-{}", id, k.to_lowercase()),
                desc: desc.clone(),
                cats: cats.clone(),
                loc: loc.clone(),
                done,
            });
        }
        let mut stamps = Vec::new();
        active_stamps(&h.title, &mut stamps);
        for n in &c.body {
            if let Block::Para(p) = &n.block {
                active_stamps(p, &mut stamps);
            }
        }
        for (i, ts) in stamps.iter().enumerate() {
            out.push(Event {
                prefix: "",
                summary: title.clone(),
                ts: ts.clone(),
                uid: format!("{id}-ts{i}"),
                desc: desc.clone(),
                cats: cats.clone(),
                loc: loc.clone(),
                done,
            });
        }
        collect(c, out);
    }
}

fn fmt_date(y: i32, m: u32, d: u32) -> String {
    format!("{y:04}{m:02}{d:02}")
}

pub fn export(doc: &Doc, st: &Settings) -> String {
    let root = sections(doc);
    let mut events = Vec::new();
    // Timestamps before the first heading belong to the file.
    let mut pre = Vec::new();
    for n in &root.body {
        if let Block::Para(p) = &n.block {
            active_stamps(p, &mut pre);
        }
    }
    for (i, ts) in pre.iter().enumerate() {
        events.push(Event {
            prefix: "",
            summary: doc.meta.title.clone(),
            ts: ts.clone(),
            uid: format!("org-file-ts{i}"),
            desc: String::new(),
            cats: vec![],
            loc: String::new(),
            done: false,
        });
    }
    collect(&root, &mut events);
    let stamp = {
        let t = if st.today.is_empty() {
            "1970-01-01"
        } else {
            &st.today
        };
        format!("{}T000000Z", t.replace('-', ""))
    };
    let mut out = String::new();
    out.push_str(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Forge//Org Mode//EN\r\nCALSCALE:GREGORIAN\r\n",
    );
    let name = if doc.meta.ical_calendar.is_empty() {
        doc.meta.title.clone()
    } else {
        doc.meta.ical_calendar.clone()
    };
    if !name.is_empty() {
        out.push_str(&fold(&format!("X-WR-CALNAME:{}", text_esc(&name))));
    }
    for e in events {
        // A range of two timestamps.
        let parts: Vec<&str> = e.ts.split("--").collect();
        let Some(a) = parse_ts(parts[0]) else {
            continue;
        };
        let b = parts.get(1).and_then(|p| parse_ts(p));
        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str(&fold(&format!("UID:{}@forge", e.uid)));
        out.push_str(&format!("DTSTAMP:{stamp}\r\n"));
        let (y, m, d, t, tend, rep) = a;
        match t {
            Some((h, mi)) => {
                out.push_str(&format!(
                    "DTSTART:{}T{h:02}{mi:02}00\r\n",
                    fmt_date(y, m, d)
                ));
                let end = match (&b, tend) {
                    (Some((y2, m2, d2, Some((h2, mi2)), _, _)), _) => {
                        format!("{}T{h2:02}{mi2:02}00", fmt_date(*y2, *m2, *d2))
                    }
                    (_, Some((h2, mi2))) => format!("{}T{h2:02}{mi2:02}00", fmt_date(y, m, d)),
                    _ => {
                        let (eh, ed) = if h + 1 >= 24 {
                            (0, next_day(y, m, d))
                        } else {
                            (h + 1, (y, m, d))
                        };
                        format!("{}T{eh:02}{mi:02}00", fmt_date(ed.0, ed.1, ed.2))
                    }
                };
                out.push_str(&format!("DTEND:{end}\r\n"));
            }
            None => {
                out.push_str(&format!("DTSTART;VALUE=DATE:{}\r\n", fmt_date(y, m, d)));
                let last = match &b {
                    Some((y2, m2, d2, _, _, _)) => (*y2, *m2, *d2),
                    None => (y, m, d),
                };
                let n = next_day(last.0, last.1, last.2);
                out.push_str(&format!("DTEND;VALUE=DATE:{}\r\n", fmt_date(n.0, n.1, n.2)));
            }
        }
        if let Some(r) = rep.as_deref().and_then(rrule) {
            out.push_str(&format!("RRULE:{r}\r\n"));
        }
        out.push_str(&fold(&format!(
            "SUMMARY:{}",
            text_esc(&format!("{}{}", e.prefix, e.summary))
        )));
        if !e.desc.is_empty() {
            out.push_str(&fold(&format!("DESCRIPTION:{}", text_esc(&e.desc))));
        }
        if !e.loc.is_empty() {
            out.push_str(&fold(&format!("LOCATION:{}", text_esc(&e.loc))));
        }
        if !e.cats.is_empty() {
            out.push_str(&fold(&format!(
                "CATEGORIES:{}",
                e.cats
                    .iter()
                    .map(|c| text_esc(c))
                    .collect::<Vec<_>>()
                    .join(",")
            )));
        }
        if e.done {
            out.push_str("STATUS:CONFIRMED\r\n");
        }
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn cal(src: &str) -> String {
        export(
            &parse(src, &|_| None),
            &Settings {
                today: "2024-03-01".into(),
                ..Default::default()
            },
        )
    }

    #[test]
    fn scheduled_and_deadline_become_events() {
        let c = cal(
            "#+TITLE: Plan\n* TODO Write :work:\nSCHEDULED: <2024-05-06 Mon 10:00-11:30> DEADLINE: <2024-05-10 Fri>\nNotes here\n",
        );
        assert!(c.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert_eq!(c.matches("BEGIN:VEVENT").count(), 2);
        assert!(
            c.contains("DTSTART:20240506T100000\r\nDTEND:20240506T113000\r\n"),
            "{c}"
        );
        assert!(c.contains("SUMMARY:S: Write") && c.contains("SUMMARY:DL: Write"));
        assert!(c.contains("DTSTART;VALUE=DATE:20240510\r\nDTEND;VALUE=DATE:20240511\r\n"));
        assert!(c.contains("CATEGORIES:work") && c.contains("DESCRIPTION:Notes here"));
        assert!(c.contains("DTSTAMP:20240301T000000Z"));
        assert!(c.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn repeaters_ranges_and_month_ends() {
        let c = cal(
            "* Weekly\nSCHEDULED: <2024-01-31 Wed +1w>\n* Trip\n<2024-12-30 Mon>--<2025-01-02 Thu>\n",
        );
        assert!(c.contains("RRULE:FREQ=WEEKLY;INTERVAL=1"));
        assert!(
            c.contains("DTSTART;VALUE=DATE:20241230\r\nDTEND;VALUE=DATE:20250103\r\n"),
            "{c}"
        );
        assert!(c.contains("DTEND;VALUE=DATE:20240201"));
    }

    #[test]
    fn long_lines_fold_and_text_is_escaped() {
        let c = cal(&format!(
            "* {}, a;b\nSCHEDULED: <2024-01-01 Mon>\n",
            "word ".repeat(30)
        ));
        assert!(c.contains("\r\n "));
        assert!(c.contains("\\, a\\;b") || c.replace("\r\n ", "").contains("\\, a\\;b"));
        for l in c.split("\r\n") {
            assert!(l.len() <= 75, "{l}");
        }
    }
}
