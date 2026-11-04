// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exporting Org documents: a parser to a tree (`parse`) and the
//! backends that write it out.
//!
//! [`export`] is the entry point: give it the text, the name of a
//! backend (see [`BACKENDS`]) and how to read the files the text names.

pub mod ascii;
pub mod ast;
pub mod cite;
pub mod common;
pub mod entities;
pub mod html;
pub mod ical;
pub mod latex;
pub mod markdown;
pub mod odt;
pub mod org;
pub mod parse;
pub mod texinfo;

pub use ast::*;
pub use common::Settings;

/// A way to export.
#[derive(Debug, Clone, Copy)]
pub struct Backend {
    pub name: &'static str,
    /// The key in the export dispatcher.
    pub key: char,
    /// File name extension of the result.
    pub ext: &'static str,
    pub label: &'static str,
}

pub const BACKENDS: &[Backend] = &[
    Backend {
        name: "ascii",
        key: 't',
        ext: "txt",
        label: "plain text (ASCII)",
    },
    Backend {
        name: "utf8",
        key: 'u',
        ext: "txt",
        label: "plain text (UTF-8)",
    },
    Backend {
        name: "html",
        key: 'h',
        ext: "html",
        label: "HTML",
    },
    Backend {
        name: "icalendar",
        key: 'c',
        ext: "ics",
        label: "iCalendar",
    },
    Backend {
        name: "latex",
        key: 'l',
        ext: "tex",
        label: "LaTeX",
    },
    Backend {
        name: "beamer",
        key: 'P',
        ext: "tex",
        label: "Beamer slides (LaTeX)",
    },
    Backend {
        name: "md",
        key: 'm',
        ext: "md",
        label: "Markdown",
    },
    Backend {
        name: "odt",
        key: 'o',
        ext: "odt",
        label: "OpenDocument text",
    },
    Backend {
        name: "org",
        key: 'O',
        ext: "org",
        label: "Org (expanded)",
    },
    Backend {
        name: "texinfo",
        key: 'i',
        ext: "texi",
        label: "Texinfo",
    },
];

pub fn backend(name: &str) -> Option<&'static Backend> {
    BACKENDS.iter().find(|b| {
        b.name == name
            || (name == "markdown" && b.name == "md")
            || (name == "ical" && b.name == "icalendar")
    })
}

/// What an export produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Text(String),
    Bytes(Vec<u8>),
}

impl Output {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Output::Text(t) => t.as_bytes(),
            Output::Bytes(b) => b,
        }
    }

    pub fn text(&self) -> Option<&str> {
        if let Output::Text(t) = self {
            Some(t)
        } else {
            None
        }
    }
}

/// Export `text` with `backend`.
pub fn export(
    backend_name: &str,
    text: &str,
    read: &dyn Fn(&str) -> Option<String>,
    st: &Settings,
) -> Result<Output, String> {
    let b =
        backend(backend_name).ok_or_else(|| format!("No export backend called {backend_name}"))?;
    if b.name == "org" {
        return Ok(Output::Text(org::export(text, read, st)));
    }
    let doc = parse::parse(text, read);
    Ok(match b.name {
        "ascii" => Output::Text(ascii::export(&doc, st, false)),
        "utf8" => Output::Text(ascii::export(&doc, st, true)),
        "html" => Output::Text(html::export(&doc, st)),
        "icalendar" => Output::Text(ical::export(&doc, st)),
        "latex" => Output::Text(latex::export(&doc, st, false)),
        "beamer" => Output::Text(latex::export(&doc, st, true)),
        "md" => Output::Text(markdown::export(&doc, st)),
        "odt" => Output::Bytes(odt::export(&doc, st)),
        "texinfo" => Output::Text(texinfo::export(&doc, st)),
        _ => return Err(format!("No export backend called {backend_name}")),
    })
}

/// The file an export of `source` goes to: `#+EXPORT_FILE_NAME`, or the
/// source's name with the backend's extension.
pub fn output_name(source: &str, text: &str, b: &Backend) -> String {
    for l in text.lines() {
        let t = l.trim();
        if t.len() > 18 && t[..18].eq_ignore_ascii_case("#+EXPORT_FILE_NAME") {
            let v = t[18..].trim_start_matches(':').trim();
            if !v.is_empty() {
                let mut out = v.to_string();
                if !out.rsplit('/').next().unwrap_or("").contains('.') {
                    out.push('.');
                    out.push_str(b.ext);
                }
                return out;
            }
        }
    }
    let stem = source.strip_suffix(".org").unwrap_or(source);
    // `foo.org` exported to Org must not overwrite itself.
    if b.name == "org" {
        return format!("{stem}.org.org");
    }
    format!("{stem}.{}", b.ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backend_exports_the_same_document() {
        let src = "#+TITLE: Doc\n* TODO Head\nSCHEDULED: <2024-01-02 Tue>\nSome *text* with [[https://x.org][a link]] and a note[fn:1].\n\n- one\n- two\n\n| a | b |\n|---+---|\n| 1 | 2 |\n\n[fn:1] The note.\n";
        for b in BACKENDS {
            let o = export(
                b.name,
                src,
                &|_| None,
                &Settings {
                    source: "doc.org".into(),
                    today: "2024-01-01".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(!o.bytes().is_empty(), "{}", b.name);
            if let Some(t) = o.text() {
                assert!(
                    t.contains("Head") || b.name == "icalendar",
                    "{}: {t}",
                    b.name
                );
            }
        }
        assert!(export("nope", "", &|_| None, &Settings::default()).is_err());
    }

    #[test]
    fn output_names() {
        let h = backend("html").unwrap();
        assert_eq!(output_name("/a/b.org", "x", h), "/a/b.html");
        assert_eq!(
            output_name("/a/b.org", "#+EXPORT_FILE_NAME: out\n", h),
            "out.html"
        );
        assert_eq!(
            output_name("b.org", "#+EXPORT_FILE_NAME: x/o.htm\n", h),
            "x/o.htm"
        );
        assert_eq!(
            output_name("b.org", "", backend("org").unwrap()),
            "b.org.org"
        );
        assert_eq!(backend("markdown").unwrap().name, "md");
    }
}

/// The text to export for the subtree at `line`: the file's `#+KEY:`
/// header lines, the subtree's `:EXPORT_x:` properties as `#+X:` lines
/// (the heading's own title is the document title unless
/// `:EXPORT_TITLE:` says otherwise), the subtree's body, and the
/// footnote definitions from the rest of the file.
pub fn subtree_source(text: &str, line: usize) -> Option<String> {
    use crate::org::heading_level;
    let lines: Vec<&str> = text.lines().collect();
    // An empty file has no heading to export.
    if lines.is_empty() {
        return None;
    }
    let h = (0..=line.min(lines.len().saturating_sub(1)))
        .rev()
        .find(|&l| heading_level(lines[l]).is_some())?;
    let level = heading_level(lines[h])?;
    let end = (h + 1..lines.len())
        .find(|&l| heading_level(lines[l]).is_some_and(|n| n <= level))
        .unwrap_or(lines.len());
    let first_heading = lines
        .iter()
        .position(|l| heading_level(l).is_some())
        .unwrap_or(lines.len());
    let title = {
        let t = lines[h][level..].trim();
        let kw = crate::org::Keywords::from_text(lines.iter().copied());
        crate::org::parse_heading(lines[h], &kw)
            .map(|x| x.title)
            .unwrap_or_else(|| t.to_string())
    };
    let mut export_props: Vec<(String, String)> = Vec::new();
    let mut body_start = h + 1;
    // Planning and the property drawer follow the heading.
    while body_start < end
        && lines[body_start].trim_start().starts_with(['S', 'D', 'C'])
        && lines[body_start].contains(':')
        && lines[body_start].contains(['<', '['])
    {
        body_start += 1;
    }
    if body_start < end
        && lines[body_start]
            .trim()
            .eq_ignore_ascii_case(":PROPERTIES:")
    {
        body_start += 1;
        while body_start < end && !lines[body_start].trim().eq_ignore_ascii_case(":END:") {
            let t = lines[body_start].trim();
            if let Some(r) = t.strip_prefix(":EXPORT_")
                && let Some((k, v)) = r.split_once(':')
            {
                export_props.push((k.to_uppercase(), v.trim().to_string()));
            }
            body_start += 1;
        }
        body_start += 1;
    }
    let mut out = String::new();
    let has_title = export_props.iter().any(|(k, _)| k == "TITLE");
    for l in &lines[..first_heading] {
        let t = l.trim_start();
        if t.starts_with("#+")
            && !(has_title && t.to_ascii_uppercase().starts_with("#+TITLE:"))
            && !t.to_ascii_uppercase().starts_with("#+TITLE:")
        {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !has_title {
        out.push_str(&format!("#+TITLE: {title}\n"));
    }
    for (k, v) in &export_props {
        let k = if k == "FILE_NAME" {
            "EXPORT_FILE_NAME"
        } else {
            k.as_str()
        };
        out.push_str(&format!("#+{k}: {v}\n"));
    }
    out.push('\n');
    for l in lines.iter().take(end).skip(body_start.min(end)) {
        out.push_str(l);
        out.push('\n');
    }
    // Footnote definitions elsewhere in the file.
    for (i, l) in lines.iter().enumerate() {
        if (i < h || i >= end) && l.starts_with("[fn:") {
            out.push_str(l);
            out.push('\n');
        }
    }
    Some(out)
}

/// Merge several iCalendar texts into one calendar.
pub fn combine_calendars(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            out = p.trim_end_matches("END:VCALENDAR\r\n").to_string();
            continue;
        }
        if let (Some(a), Some(b)) = (p.find("BEGIN:VEVENT"), p.rfind("END:VEVENT")) {
            // The event ends at the line after `END:VEVENT`. The text
            // may end in bare newlines, so the line ending is ours.
            let end = (b + "END:VEVENT".len()).min(p.len());
            out.push_str(&p[a..end]);
            out.push_str("\r\n");
        }
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// A default `#+TITLE:` header for a new file (`org-export-insert-default-template`).
pub fn default_template(title: &str, author: &str, date: &str) -> String {
    format!(
        "#+TITLE: {title}\n#+DATE: {date}\n#+AUTHOR: {author}\n#+EMAIL:\n#+LANGUAGE: en\n#+OPTIONS: H:3 num:t toc:t \\n:nil ::t |:t ^:t -:t f:t *:t <:t\n#+OPTIONS: author:t broken-links:nil date:t email:nil title:t todo:t\n#+SELECT_TAGS: export\n#+EXCLUDE_TAGS: noexport\n"
    )
}

#[cfg(test)]
mod subtree_tests {
    use super::*;

    #[test]
    fn subtree_source_takes_header_properties_and_footnotes() {
        let t = "#+TITLE: File\n#+AUTHOR: Me\n\n* One\ntext[fn:1]\n* Two\n:PROPERTIES:\n:EXPORT_FILE_NAME: two\n:EXPORT_OPTIONS: toc:nil\n:END:\nbody\n** Sub\nx\n* Three\n[fn:1] Note.\n";
        let s = subtree_source(t, 8).unwrap();
        assert!(
            s.contains("#+AUTHOR: Me")
                && s.contains("#+TITLE: Two")
                && !s.contains("#+TITLE: File")
        );
        assert!(s.contains("#+EXPORT_FILE_NAME: two"));
        assert!(s.contains("#+OPTIONS: toc:nil"));
        assert!(s.contains("body\n** Sub\nx\n") && !s.contains("* Three"));
        assert!(s.contains("[fn:1] Note."));
        assert!(!s.contains("text[fn:1]"));
        assert!(subtree_source("no headings", 0).is_none());
    }

    #[test]
    fn subtree_source_of_an_empty_file_is_nothing() {
        assert!(subtree_source("", 0).is_none());
        assert!(subtree_source("\n", 0).is_none());
    }

    #[test]
    fn calendars_merge() {
        let a = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_string();
        let b = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_string();
        let m = combine_calendars(&[a, b]);
        assert_eq!(m.matches("BEGIN:VEVENT").count(), 2);
        assert_eq!(m.matches("BEGIN:VCALENDAR").count(), 1);
        assert!(m.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn calendars_merge_whatever_the_line_endings() {
        let a = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_string();
        // Bare newlines, and no newline after the last event.
        let b = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:2\nEND:VEVENT\nEND:VCALENDAR\n".to_string();
        let m = combine_calendars(&[a.clone(), b]);
        assert!(m.contains("UID:1\r\nEND:VEVENT\r\n"), "{m}");
        // The event is copied whole, and the line after it is not
        // swallowed.
        assert!(m.contains("BEGIN:VEVENT\nUID:2\nEND:VEVENT\r\n"), "{m}");
        assert!(!m.contains("END:VEVENT\nEN"), "{m}");
        assert!(m.ends_with("END:VCALENDAR\r\n"), "{m}");

        let c = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:3\nEND:VEVENT".to_string();
        let m = combine_calendars(&[a, c]);
        assert!(m.contains("UID:3\nEND:VEVENT\r\n"), "{m}");
    }
}
