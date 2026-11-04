// SPDX-License-Identifier: MIT OR Apache-2.0
//! Column view (`org-columns`): a table of one row per heading, one
//! column per property, from a `%25ITEM %TODO %Effort{:}` format.

use crate::Buffer;
use crate::org::{keywords, parse_heading};
use crate::org_ext::parse_effort;

pub const DEFAULT_FORMAT: &str = "%25ITEM %TODO %3PRIORITY %TAGS";

#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub prop: String,
    pub width: usize,
    pub title: String,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub line: usize,
    pub level: usize,
    pub cells: Vec<String>,
}

/// `%[width]NAME[(title)][{summary}]` items, separated by spaces.
pub fn parse_format(fmt: &str) -> Result<Vec<Col>, String> {
    let mut out = Vec::new();
    for tok in fmt.split_whitespace() {
        let t = tok
            .strip_prefix('%')
            .ok_or_else(|| format!("Bad column spec: {tok}"))?;
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let mut rest = &t[digits.len()..];
        let name_end = rest.find(['(', '{']).unwrap_or(rest.len());
        let prop = rest[..name_end].to_string();
        if prop.is_empty() {
            return Err(format!("Bad column spec: {tok}"));
        }
        rest = &rest[name_end..];
        let mut title = prop.clone();
        let mut summary = None;
        if let Some(r) = rest.strip_prefix('(') {
            let end = r
                .find(')')
                .ok_or_else(|| format!("Bad column spec: {tok}"))?;
            title = r[..end].to_string();
            rest = &r[end + 1..];
        }
        if let Some(r) = rest.strip_prefix('{') {
            let end = r
                .find('}')
                .ok_or_else(|| format!("Bad column spec: {tok}"))?;
            summary = Some(r[..end].to_string());
        }
        let width = digits
            .parse()
            .unwrap_or_else(|_| title.chars().count().max(6));
        out.push(Col {
            prop,
            width,
            title,
            summary,
        });
    }
    if out.is_empty() {
        return Err("Empty column format".into());
    }
    Ok(out)
}

pub fn format_to_string(cols: &[Col]) -> String {
    cols.iter()
        .map(|c| {
            let mut s = format!("%{}{}", c.width, c.prop);
            if c.title != c.prop {
                s.push_str(&format!("({})", c.title));
            }
            if let Some(f) = &c.summary {
                s.push_str(&format!("{{{f}}}"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn hmm(minutes: f64) -> String {
    let m = minutes.round() as i64;
    format!("{}:{:02}", m / 60, m % 60)
}

fn summarize(kind: &str, vals: &[String]) -> String {
    let nums: Vec<f64> = vals
        .iter()
        .filter_map(|v| {
            if kind == ":" {
                parse_effort(v).map(|h| h * 60.0)
            } else {
                v.trim().parse().ok()
            }
        })
        .collect();
    if nums.is_empty() {
        return String::new();
    }
    let num = |x: f64| {
        if x.fract() == 0.0 {
            format!("{}", x as i64)
        } else {
            format!("{x:.2}")
        }
    };
    match kind {
        "+" => num(nums.iter().sum()),
        ":" => hmm(nums.iter().sum()),
        "max" => num(nums.iter().cloned().fold(f64::MIN, f64::max)),
        "min" => num(nums.iter().cloned().fold(f64::MAX, f64::min)),
        "mean" => num(nums.iter().sum::<f64>() / nums.len() as f64),
        _ => String::new(),
    }
}

impl Buffer {
    /// The column format in force at `h`: `:COLUMNS:` (inherited), then
    /// `#+COLUMNS:`, then the default.
    pub fn org_columns_format(&self, h: Option<usize>) -> String {
        if let Some(h) = h
            && let Some(v) = self.org_get_property(h, "COLUMNS", true)
        {
            return v;
        }
        (0..self.line_count())
            .find_map(|l| {
                let line = self.line(l);
                let p = line.get(..10)?;
                p.eq_ignore_ascii_case("#+COLUMNS:")
                    .then(|| line[10..].trim().to_string())
            })
            .unwrap_or_else(|| DEFAULT_FORMAT.to_string())
    }

    fn org_column_value(&self, h: usize, col: &str) -> String {
        let kw = keywords(self);
        let hd = parse_heading(&self.line(h), &kw);
        match col {
            "ITEM" => hd.as_ref().map_or(String::new(), |d| d.title.clone()),
            "TODO" => hd.and_then(|d| d.keyword).unwrap_or_default(),
            "TAGS" => {
                let t = self.org_heading_tags(h);
                if t.is_empty() {
                    String::new()
                } else {
                    format!(":{}:", t.join(":"))
                }
            }
            "PRIORITY" => {
                let t = hd.map(|d| d.title).unwrap_or_default();
                t.strip_prefix("[#")
                    .and_then(|r| r.chars().next())
                    .map(String::from)
                    .unwrap_or_default()
            }
            p => self.org_get_property(h, p, false).unwrap_or_default(),
        }
    }

    /// Rows for the headings in `[from, to]` (lines), with summaries
    /// filled into parents that have no value of their own.
    pub fn org_columns_rows(&self, cols: &[Col], from: usize, to: usize) -> Vec<Row> {
        let heads: Vec<usize> = (from..=to.min(self.line_count().saturating_sub(1)))
            .filter(|&l| self.org_level(l).is_some())
            .collect();
        let mut rows: Vec<Row> = heads
            .iter()
            .map(|&l| Row {
                line: l,
                level: self.org_level(l).unwrap_or(1),
                cells: cols
                    .iter()
                    .map(|c| self.org_column_value(l, &c.prop))
                    .collect(),
            })
            .collect();
        for (ci, c) in cols.iter().enumerate() {
            let Some(kind) = &c.summary else { continue };
            // Deepest first so summaries roll up.
            for i in (0..rows.len()).rev() {
                let lvl = rows[i].level;
                let kids: Vec<String> = rows[i + 1..]
                    .iter()
                    .take_while(|r| r.level > lvl)
                    .filter(|r| r.level == lvl + 1)
                    .map(|r| r.cells[ci].clone())
                    .collect();
                if rows[i].cells[ci].is_empty() && !kids.is_empty() {
                    rows[i].cells[ci] = summarize(kind, &kids);
                }
            }
        }
        rows
    }
}

/// Render rows as aligned text: a title line, a rule, and one line per row.
pub fn render(cols: &[Col], rows: &[Row]) -> Vec<String> {
    let fit = |s: &str, w: usize| {
        let n = s.chars().count();
        if n > w {
            format!(
                "{}…",
                s.chars().take(w.saturating_sub(1)).collect::<String>()
            )
        } else {
            format!("{s}{}", " ".repeat(w - n))
        }
    };
    let mut out = Vec::new();
    out.push(
        cols.iter()
            .map(|c| fit(&c.title, c.width))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push(
        cols.iter()
            .map(|c| "-".repeat(c.width))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    for r in rows {
        out.push(
            cols.iter()
                .zip(&r.cells)
                .map(|(c, v)| {
                    let v = if c.prop == "ITEM" {
                        format!("{}{}", " ".repeat(r.level.saturating_sub(1)), v)
                    } else {
                        v.clone()
                    };
                    fit(&v, c.width)
                })
                .collect::<Vec<_>>()
                .join(" | "),
        );
    }
    out
}

/// Which column a display offset falls in.
pub fn column_at(cols: &[Col], col: usize) -> usize {
    let mut start = 0;
    for (i, c) in cols.iter().enumerate() {
        let end = start + c.width + 3;
        if col < end {
            return i;
        }
        start = end;
    }
    cols.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(t: &str) -> Buffer {
        let mut b = Buffer::new("c.org");
        b.set_contents(t);
        b
    }

    #[test]
    fn format_round_trip() {
        let c = parse_format("%25ITEM %TODO %Effort(Est){:} %5Cost{+}").unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].width, 25);
        assert_eq!(c[2].title, "Est");
        assert_eq!(c[2].summary.as_deref(), Some(":"));
        assert!(parse_format("ITEM").is_err());
        assert_eq!(parse_format(&format_to_string(&c)).unwrap(), c);
    }

    #[test]
    fn rows_summarize_into_parents() {
        let b = buf(
            "#+COLUMNS: %20ITEM %Cost{+} %Effort{:}\n* TODO Project\n** a\n:PROPERTIES:\n:Cost: 3\n:Effort: 1:30\n:END:\n** b\n:PROPERTIES:\n:Cost: 4\n:Effort: 0:45\n:END:\n",
        );
        let cols = parse_format(&b.org_columns_format(None)).unwrap();
        let rows = b.org_columns_rows(&cols, 0, b.line_count() - 1);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].cells[1], "7");
        assert_eq!(rows[0].cells[2], "2:15");
        assert_eq!(rows[0].cells[0], "Project");
        let text = render(&cols, &rows);
        assert_eq!(text.len(), 5);
        assert_eq!(column_at(&cols, 0), 0);
        assert_eq!(column_at(&cols, 25), 1);
    }

    #[test]
    fn allowed_values() {
        let b = buf("* H\n:PROPERTIES:\n:Prio_ALL: low mid high\n:END:\n");
        assert_eq!(b.org_allowed_values(0, "Prio"), vec!["low", "mid", "high"]);
    }
}
