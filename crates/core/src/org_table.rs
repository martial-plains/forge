// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org tables (Phase 11, stage 3): parsing, alignment, editing,
//! movement, import/export, sorting and the region commands. Formulas
//! live in `org_calc.rs`; this file hands them the rows.
//!
//! A table is a run of lines whose first non-blank character is `|`.
//! A line starting `|-` is a horizontal rule. Everything is a method on
//! [`Buffer`], in the same style as `org.rs`; the editor binds keys.

use crate::buffer::{Buffer, Pos};
use crate::width::line_width;
use regex::Regex;
use std::sync::LazyLock;

/// One table row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Rule,
    Data(Vec<String>),
}

impl Row {
    pub fn is_rule(&self) -> bool {
        matches!(self, Row::Rule)
    }
}

/// Does `line` belong to a table?
pub fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// `#+TBLFM:` line?
pub fn is_tblfm(line: &str) -> bool {
    let t = line.trim_start();
    t.get(..8)
        .is_some_and(|head| head.eq_ignore_ascii_case("#+TBLFM:"))
}

/// Parse one table line.
pub fn parse_row(line: &str) -> Row {
    let t = line.trim();
    if t.starts_with("|-") {
        return Row::Rule;
    }
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    Row::Data(inner.split('|').map(|c| c.trim().to_string()).collect())
}

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-+]?(\d+(\.\d*)?|\.\d+)([eE][-+]?\d+)?$").unwrap());
static COOKIE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<([lrc])?(\d+)?>$").unwrap());

pub fn is_number(s: &str) -> bool {
    NUMBER.is_match(s.trim())
}

/// A row made only of `<l>`, `<r>`, `<c>`, `<N>` cookies.
fn is_cookie_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| c.is_empty() || COOKIE.is_match(c))
        && cells.iter().any(|c| !c.is_empty())
}

/// Align a set of rows: `indent` goes before every line.
pub fn format_rows(rows: &[Row], indent: &str) -> Vec<String> {
    format_rows_by(rows, indent, &line_width)
}

/// The width a cell takes on screen when descriptive links are shown:
/// `[[target][desc]]` counts as just `desc` measures it.
pub fn shown_width(cell: &str) -> usize {
    if !cell.contains("[[") {
        return line_width(cell);
    }
    let chars: Vec<char> = cell.chars().collect();
    let mut shown = String::new();
    let mut at = 0;
    for l in crate::org_ext::links_in(cell) {
        if l.start >= at && (l.start < l.text_start || l.text_end < l.end) {
            shown.extend(&chars[at..l.start]);
            shown.extend(&chars[l.text_start..l.text_end]);
            at = l.end;
        }
    }
    shown.extend(&chars[at.min(chars.len())..]);
    line_width(&shown)
}

/// `format_rows`, measuring each cell with `measure`.
pub fn format_rows_by(rows: &[Row], indent: &str, measure: &dyn Fn(&str) -> usize) -> Vec<String> {
    let ncols = rows
        .iter()
        .map(|r| match r {
            Row::Data(c) => c.len(),
            Row::Rule => 0,
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let mut width = vec![1usize; ncols];
    let mut numeric = vec![0usize; ncols];
    let mut filled = vec![0usize; ncols];
    let mut align: Vec<Option<char>> = vec![None; ncols];
    for r in rows {
        if let Row::Data(cells) = r {
            if is_cookie_row(cells) {
                for (i, c) in cells.iter().enumerate() {
                    if let Some(m) = COOKIE.captures(c)
                        && let Some(a) = m.get(1)
                    {
                        align[i] = a.as_str().chars().next();
                    }
                }
                // The cookie row's own width counts too.
            }
            for (i, c) in cells.iter().enumerate() {
                width[i] = width[i].max(measure(c));
                if !c.is_empty() {
                    filled[i] += 1;
                    if is_number(c) {
                        numeric[i] += 1;
                    }
                }
            }
        }
    }
    let right: Vec<bool> = (0..ncols)
        .map(|i| match align[i] {
            Some('r') => true,
            Some(_) => false,
            None => filled[i] > 0 && numeric[i] * 2 >= filled[i],
        })
        .collect();
    rows.iter()
        .map(|r| match r {
            Row::Rule => {
                let parts: Vec<String> = width.iter().map(|w| "-".repeat(w + 2)).collect();
                format!("{indent}|{}|", parts.join("+"))
            }
            Row::Data(cells) => {
                let mut out = String::from(indent);
                out.push('|');
                for i in 0..ncols {
                    let c = cells.get(i).map(String::as_str).unwrap_or("");
                    let pad = width[i].saturating_sub(measure(c));
                    let (l, r) = match align[i] {
                        Some('c') => (pad / 2, pad - pad / 2),
                        _ if right[i] && !is_cookie_cell(c) => (pad, 0),
                        _ => (0, pad),
                    };
                    // Cookie cells in a right-aligned column still line up
                    // with the data.
                    let (l, r) = if right[i] && is_cookie_cell(c) {
                        (pad, 0)
                    } else {
                        (l, r)
                    };
                    out.push(' ');
                    out.push_str(&" ".repeat(l));
                    out.push_str(c);
                    out.push_str(&" ".repeat(r));
                    out.push_str(" |");
                }
                out
            }
        })
        .collect()
}

fn is_cookie_cell(c: &str) -> bool {
    COOKIE.is_match(c)
}

/// Where the fields of an aligned line begin: char columns of the `|`s.
fn bars(line: &str) -> Vec<usize> {
    line.chars()
        .enumerate()
        .filter(|(_, c)| *c == '|')
        .map(|(i, _)| i)
        .collect()
}

/// The 0-based field index at char column `col` of a table line.
pub fn field_index(line: &str, col: usize) -> usize {
    let n = line.chars().take(col).filter(|c| *c == '|').count();
    n.saturating_sub(1)
}

/// Start (first character of content) and end of field `idx` in an
/// aligned line, in char columns. Empty fields start and end at the same
/// column.
pub fn field_span(line: &str, idx: usize) -> Option<(usize, usize)> {
    let b = bars(line);
    let (open, close) = (
        *b.get(idx)?,
        b.get(idx + 1)
            .copied()
            .unwrap_or_else(|| line.chars().count()),
    );
    let chars: Vec<char> = line.chars().collect();
    let mut s = open + 1;
    while s < close && chars.get(s) == Some(&' ') {
        s += 1;
    }
    let mut e = close;
    while e > s && chars.get(e - 1) == Some(&' ') {
        e -= 1;
    }
    if s == e && close > open + 1 {
        // Empty: point goes one space in, where typing lands.
        s = open + 2.min(close - open - 1).max(1);
        s = s.min(close);
        e = s;
    }
    Some((s, e))
}

/// Compare as the sort kinds do.
fn time_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(ts) = crate::org_time::timestamps_in(s).first() {
        return Some(ts.minutes() as f64 * 60.0);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for p in &parts {
        total = total * 60.0 + p.trim().parse::<f64>().ok()?;
    }
    Some(total)
}

/// How a column can be sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKind {
    Alpha,
    AlphaCaseInsensitive,
    Numeric,
    Time,
}

impl SortKind {
    /// From Org's characters: `a A n N t T` (upper case is reverse),
    /// `x X` for case-sensitive alpha. Returns the kind and whether the
    /// order is reversed.
    pub fn from_char(c: char) -> Option<(Self, bool)> {
        Some(match c {
            'a' => (SortKind::AlphaCaseInsensitive, false),
            'A' => (SortKind::AlphaCaseInsensitive, true),
            'x' => (SortKind::Alpha, false),
            'X' => (SortKind::Alpha, true),
            'n' => (SortKind::Numeric, false),
            'N' => (SortKind::Numeric, true),
            't' => (SortKind::Time, false),
            'T' => (SortKind::Time, true),
            _ => return None,
        })
    }
}

fn sort_cmp(kind: SortKind, a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match kind {
        SortKind::Alpha => a.cmp(b),
        SortKind::AlphaCaseInsensitive => a.to_lowercase().cmp(&b.to_lowercase()),
        SortKind::Numeric => {
            let (x, y) = (a.trim().parse::<f64>().ok(), b.trim().parse::<f64>().ok());
            match (x, y) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Equal),
                (Some(_), None) => Less,
                (None, Some(_)) => Greater,
                (None, None) => Equal,
            }
        }
        SortKind::Time => {
            let (x, y) = (time_value(a), time_value(b));
            match (x, y) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Equal),
                (Some(_), None) => Less,
                (None, Some(_)) => Greater,
                (None, None) => Equal,
            }
        }
    }
}

/// Sort the data rows of `rows` by column `col` (0-based). Sections between
/// rules sort separately; rows above the first rule (the header) stay.
pub fn sort_rows(rows: &mut [Row], col: usize, kind: SortKind, reverse: bool) {
    let first_rule = rows.iter().position(Row::is_rule);
    let start = first_rule.map_or(0, |i| i + 1);
    let mut i = start;
    while i < rows.len() {
        if rows[i].is_rule() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < rows.len() && !rows[j].is_rule() {
            j += 1;
        }
        rows[i..j].sort_by(|a, b| {
            let get = |r: &Row| match r {
                Row::Data(c) => c.get(col).cloned().unwrap_or_default(),
                Row::Rule => String::new(),
            };
            let o = sort_cmp(kind, &get(a), &get(b));
            if reverse { o.reverse() } else { o }
        });
        i = j;
    }
}

/// A new table of `cols` x `rows` fields, with a header rule.
pub fn create_table(cols: usize, rows: usize) -> String {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let mut body = vec![Row::Data(vec![String::new(); cols]), Row::Rule];
    for _ in 1..rows {
        body.push(Row::Data(vec![String::new(); cols]));
    }
    format_rows(&body, "").join("\n")
}

/// The separator `org-table-convert-region` guesses.
pub fn guess_separator(text: &str) -> Separator {
    if text.contains('\t') {
        Separator::Tab
    } else if text.contains(',') {
        Separator::Comma
    } else {
        Separator::Spaces(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Separator {
    Tab,
    Comma,
    /// Runs of at least this many blanks.
    Spaces(usize),
    Custom(String),
}

/// Split one line of delimited text, honouring CSV quotes for commas.
pub fn split_fields(line: &str, sep: &Separator) -> Vec<String> {
    match sep {
        Separator::Tab => line.split('\t').map(|s| s.trim().to_string()).collect(),
        Separator::Custom(s) if !s.is_empty() => line
            .split(s.as_str())
            .map(|s| s.trim().to_string())
            .collect(),
        Separator::Custom(_) => vec![line.trim().to_string()],
        Separator::Comma => {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut quoted = false;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '"' if quoted && chars.peek() == Some(&'"') => {
                        cur.push('"');
                        chars.next();
                    }
                    '"' => quoted = !quoted,
                    ',' if !quoted => {
                        out.push(cur.trim().to_string());
                        cur.clear();
                    }
                    _ => cur.push(c),
                }
            }
            out.push(cur.trim().to_string());
            out
        }
        Separator::Spaces(n) => {
            let n = (*n).max(1);
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut run = 0usize;
            for c in line.trim().chars() {
                if c == ' ' || c == '\t' {
                    run += 1;
                    cur.push(c);
                } else {
                    if run >= n && !cur.trim().is_empty() {
                        let t = cur.trim_end().to_string();
                        out.push(t);
                        cur.clear();
                    }
                    run = 0;
                    cur.push(c);
                }
            }
            out.push(cur.trim().to_string());
            out
        }
    }
}

/// Convert delimited text to an aligned table. Lines starting with `|` are
/// kept as rows already.
pub fn convert_text(text: &str, sep: &Separator, indent: &str) -> String {
    let rows: Vec<Row> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            if is_table_line(l) {
                parse_row(l)
            } else {
                Row::Data(split_fields(l, sep))
            }
        })
        .collect();
    format_rows(&rows, indent).join("\n")
}

/// Export rows in a format (`tsv`, `csv`, `orgtbl`, `html`, `latex`,
/// `ascii`). Rules are dropped except in the Org and ASCII forms.
pub fn export_rows(rows: &[Row], format: &str) -> Result<String, String> {
    let data: Vec<&Vec<String>> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Data(c) => Some(c),
            Row::Rule => None,
        })
        .collect();
    let f = format.trim().trim_start_matches("orgtbl-to-");
    Ok(match f {
        "" | "tsv" => data
            .iter()
            .map(|r| r.join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
        "csv" => data
            .iter()
            .map(|r| {
                r.iter()
                    .map(|c| {
                        if c.contains([',', '"', '\n']) {
                            format!("\"{}\"", c.replace('"', "\"\""))
                        } else {
                            c.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "org" | "orgtbl" => format_rows(rows, "").join("\n"),
        "ascii" | "text" => format_rows(rows, "").join("\n"),
        "html" => {
            let esc = |s: &str| {
                s.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            };
            let mut s = String::from("<table>\n");
            for r in &data {
                s.push_str("<tr>");
                for c in r.iter() {
                    s.push_str(&format!("<td>{}</td>", esc(c)));
                }
                s.push_str("</tr>\n");
            }
            s.push_str("</table>");
            s
        }
        "latex" => {
            let esc = |s: &str| {
                s.replace('&', "\\&")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
                    .replace('#', "\\#")
            };
            let cols = data.iter().map(|r| r.len()).max().unwrap_or(1);
            let mut s = format!("\\begin{{tabular}}{{{}}}\n", "l".repeat(cols));
            for r in &data {
                s.push_str(&r.iter().map(|c| esc(c)).collect::<Vec<_>>().join(" & "));
                s.push_str(" \\\\\n");
            }
            s.push_str("\\end{tabular}");
            s
        }
        other => return Err(format!("Unknown table export format: {other}")),
    })
}

/// Swap rows and columns. Rules are dropped.
pub fn transpose(rows: &[Row]) -> Vec<Row> {
    let data: Vec<&Vec<String>> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Data(c) => Some(c),
            Row::Rule => None,
        })
        .collect();
    let cols = data.iter().map(|r| r.len()).max().unwrap_or(0);
    (0..cols)
        .map(|c| {
            Row::Data(
                data.iter()
                    .map(|r| r.get(c).cloned().unwrap_or_default())
                    .collect(),
            )
        })
        .collect()
}

// ----------------------------------------------------------- buffer side

/// What a table command did to point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableMove {
    pub pos: Pos,
}

impl Buffer {
    /// The first and last lines of the table `line` is in.
    pub fn org_table_bounds(&self, line: usize) -> Option<(usize, usize)> {
        let n = self.line_count();
        if line >= n || !is_table_line(&self.line(line)) {
            return None;
        }
        let mut a = line;
        while a > 0 && is_table_line(&self.line(a - 1)) {
            a -= 1;
        }
        let mut b = line;
        while b + 1 < n && is_table_line(&self.line(b + 1)) {
            b += 1;
        }
        Some((a, b))
    }

    /// The `#+TBLFM:` lines straight after a table.
    pub fn org_table_tblfm_lines(&self, last: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut l = last + 1;
        while l < self.line_count() && is_tblfm(&self.line(l)) {
            out.push(l);
            l += 1;
        }
        out
    }

    /// The rows of the table at `line`, its first line number, and its
    /// indent.
    pub fn org_table_rows(&self, line: usize) -> Option<(usize, Vec<Row>, String)> {
        let (a, b) = self.org_table_bounds(line)?;
        let indent: String = self
            .line(a)
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let rows = (a..=b).map(|l| parse_row(&self.line(l))).collect();
        Some((a, rows, indent))
    }

    /// Replace the table at `first..=last` with `rows`, aligned.
    fn org_table_write(&mut self, first: usize, last: usize, rows: &[Row], indent: &str) {
        let text = if self.org_display.link_descriptive {
            format_rows_by(rows, indent, &shown_width)
        } else {
            format_rows(rows, indent)
        }
        .join("\n");
        let (a, z) = (self.line_start_offset(first), self.line_end_offset(last));
        if self.text_between(a, z) != text {
            self.replace_offsets(a, z, &text);
        }
    }

    fn text_between(&self, a: usize, z: usize) -> String {
        let (pa, pz) = (self.offset_to_pos(a), self.offset_to_pos(z));
        (pa.line..=pz.line)
            .map(|l| {
                let s = self.line(l);
                let from = if l == pa.line { pa.col } else { 0 };
                let to = if l == pz.line {
                    pz.col
                } else {
                    s.chars().count()
                };
                s.chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn field_pos(&self, line: usize, idx: usize) -> Pos {
        let text = self.line(line);
        let (s, _) = field_span(&text, idx).unwrap_or((0, 0));
        Pos::new(line, s)
    }

    /// `org-table-align`: realign the table at `line`; returns the new
    /// point for a point that was at `col` of `line`.
    pub fn org_table_align(&mut self, line: usize, col: usize) -> Option<Pos> {
        let (a, rows, indent) = self.org_table_rows(line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(line), col);
        self.org_table_write(a, b, &rows, &indent);
        Some(self.field_pos(line, idx))
    }

    /// `org-table-create`.
    pub fn org_table_insert_new(&mut self, line: usize, cols: usize, rows: usize) {
        let text = create_table(cols, rows);
        let at = self.line_start_offset(line);
        let here = self.line(line);
        let prefix = if here.trim().is_empty() {
            String::new()
        } else {
            String::from("\n")
        };
        if here.trim().is_empty() {
            let (a, z) = (self.line_start_offset(line), self.line_end_offset(line));
            self.replace_offsets(a, z, &text);
        } else {
            let end = self.line_end_offset(line);
            self.replace_offsets(end, end, &format!("{prefix}{text}"));
        }
        let _ = at;
        let first = if here.trim().is_empty() {
            line
        } else {
            line + 1
        };
        self.set_point(self.field_pos(first, 0));
    }

    /// `org-table-create-or-convert-from-region`: turn lines `first..=last`
    /// into a table.
    pub fn org_table_convert_lines(
        &mut self,
        first: usize,
        last: usize,
        sep: Option<Separator>,
    ) -> usize {
        let text: String = (first..=last)
            .map(|l| self.line(l))
            .collect::<Vec<_>>()
            .join("\n");
        let sep = sep.unwrap_or_else(|| guess_separator(&text));
        let indent: String = self
            .line(first)
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let body = convert_text(text.trim_start_matches(char::is_whitespace), &sep, &indent);
        let (a, z) = (self.line_start_offset(first), self.line_end_offset(last));
        self.replace_offsets(a, z, &body);
        self.set_point(self.field_pos(first, 0));
        body.lines().count()
    }

    /// `org-table-transpose-table-at-point`.
    pub fn org_table_transpose(&mut self, line: usize) -> bool {
        let Some((a, rows, indent)) = self.org_table_rows(line) else {
            return false;
        };
        let b = a + rows.len() - 1;
        let new = transpose(&rows);
        self.org_table_write(a, b, &new, &indent);
        self.set_point(self.field_pos(a, 0));
        true
    }

    /// `org-table-goto-line`: the `n`th data line of the table (1-based).
    pub fn org_table_goto_row(&self, line: usize, n: usize) -> Option<Pos> {
        let (a, rows, _) = self.org_table_rows(line)?;
        let idx = field_index(&self.line(line), self.point().col);
        let mut count = 0;
        for (i, r) in rows.iter().enumerate() {
            if !r.is_rule() {
                count += 1;
                if count == n {
                    return Some(self.field_pos(a + i, idx));
                }
            }
        }
        None
    }

    /// `org-table-beginning-of-field` / `end-of-field` with a count.
    pub fn org_table_field_edge(&self, p: Pos, end: bool) -> Option<Pos> {
        let text = self.line(p.line);
        if !is_table_line(&text) {
            return None;
        }
        let idx = field_index(&text, p.col);
        let (s, e) = field_span(&text, idx)?;
        Some(Pos::new(p.line, if end { e } else { s }))
    }

    /// `org-table-next-field` (TAB): realign, then move to the next field;
    /// past the last field of the last row a new row is added. Skips
    /// rules.
    pub fn org_table_next_field(&mut self, p: Pos) -> Option<Pos> {
        let (a, rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        self.org_table_write(a, b, &rows, &indent);
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(1);
        if idx + 1 < ncols && !rows[p.line - a].is_rule() {
            return Some(self.field_pos(p.line, idx + 1));
        }
        Some(self.org_table_row_below(p.line, 0, a, b, ncols, &indent))
    }

    /// `org-table-previous-field` (S-TAB).
    pub fn org_table_previous_field(&mut self, p: Pos) -> Option<Pos> {
        let (a, rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        self.org_table_write(a, b, &rows, &indent);
        if idx > 0 && !rows[p.line - a].is_rule() {
            return Some(self.field_pos(p.line, idx - 1));
        }
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(1);
        let mut l = p.line;
        while l > a {
            l -= 1;
            if !rows[l - a].is_rule() {
                return Some(self.field_pos(l, ncols - 1));
            }
        }
        Some(self.field_pos(p.line, 0))
    }

    /// Point in field `idx` of the first data row below `from` (adding a
    /// row when there is none).
    fn org_table_row_below(
        &mut self,
        from: usize,
        idx: usize,
        a: usize,
        b: usize,
        ncols: usize,
        indent: &str,
    ) -> Pos {
        let mut l = from + 1;
        while l <= b && parse_row(&self.line(l)).is_rule() {
            l += 1;
        }
        if l <= b {
            return self.field_pos(l, idx);
        }
        // Add an empty row at the end.
        let _ = a;
        let empty = Row::Data(vec![String::new(); ncols]);
        let mut rows = self
            .org_table_rows(b)
            .map(|(_, r, _)| r)
            .unwrap_or_default();
        rows.push(empty);
        let first = self.org_table_bounds(b).map_or(a, |t| t.0);
        self.org_table_write(first, b, &rows, indent);
        self.field_pos(b + 1, idx)
    }

    /// `org-table-next-row` (RET).
    pub fn org_table_next_row(&mut self, p: Pos) -> Option<Pos> {
        let (a, rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        self.org_table_write(a, b, &rows, &indent);
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(1);
        Some(self.org_table_row_below(p.line, idx, a, b, ncols, &indent))
    }

    /// `org-table-insert-row`: an empty row above (or below) the current
    /// line. Point goes to the new row.
    pub fn org_table_insert_row(&mut self, p: Pos, below: bool) -> Option<Pos> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(1);
        let at = p.line - a + usize::from(below);
        rows.insert(at, Row::Data(vec![String::new(); ncols]));
        self.org_table_write(a, b, &rows, &indent);
        Some(self.field_pos(a + at, idx))
    }

    /// `org-table-insert-hline`: a rule above (or below) the current line.
    pub fn org_table_insert_hline(&mut self, p: Pos, below: bool) -> Option<Pos> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let at = p.line - a + usize::from(below);
        rows.insert(at, Row::Rule);
        self.org_table_write(a, b, &rows, &indent);
        Some(self.field_pos(a + at + usize::from(!below), idx))
    }

    /// `org-table-hline-and-move` (C-c RET): hline below, then the next
    /// row.
    pub fn org_table_hline_and_move(&mut self, p: Pos) -> Option<Pos> {
        let q = self.org_table_insert_hline(p, true)?;
        self.org_table_next_row(Pos::new(q.line - 1, q.col))
    }

    /// `org-table-kill-row`: remove the current row; returns its text.
    pub fn org_table_kill_row(&mut self, p: Pos) -> Option<String> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let gone = format_rows(&[rows.remove(p.line - a)], &indent).remove(0);
        if rows.is_empty() {
            let (s, e) = (self.line_start_offset(a), self.line_end_offset(b));
            let e = if e < self.char_count() { e + 1 } else { e };
            self.replace_offsets(s, e, "");
            return Some(gone);
        }
        // The write replaces the whole table, so nothing is left over.
        self.org_table_write(a, b, &rows[..], &indent);
        let new_last = a + rows.len() - 1;
        let l = p.line.min(new_last);
        self.set_point(self.field_pos(l, idx));
        Some(gone)
    }

    /// `org-table-move-row-up` / `-down`.
    pub fn org_table_move_row(&mut self, p: Pos, down: bool) -> bool {
        let Some((a, mut rows, indent)) = self.org_table_rows(p.line) else {
            return false;
        };
        let b = a + rows.len() - 1;
        let i = p.line - a;
        let j = if down {
            if i + 1 >= rows.len() {
                return false;
            } else {
                i + 1
            }
        } else if i == 0 {
            return false;
        } else {
            i - 1
        };
        let idx = field_index(&self.line(p.line), p.col);
        rows.swap(i, j);
        self.org_table_write(a, b, &rows, &indent);
        self.set_point(self.field_pos(a + j, idx));
        true
    }

    /// `org-table-insert-column`: an empty column before the current one.
    pub fn org_table_insert_column(&mut self, p: Pos) -> Option<Pos> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        for r in &mut rows {
            if let Row::Data(c) = r {
                while c.len() < idx {
                    c.push(String::new());
                }
                c.insert(idx, String::new());
            }
        }
        self.org_table_write(a, b, &rows, &indent);
        Some(self.field_pos(p.line, idx))
    }

    /// `org-table-delete-column`.
    pub fn org_table_delete_column(&mut self, p: Pos) -> Option<Pos> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let mut ncols = 0;
        for r in &mut rows {
            if let Row::Data(c) = r {
                if idx < c.len() {
                    c.remove(idx);
                }
                ncols = ncols.max(c.len());
            }
        }
        if ncols == 0 {
            let (s, e) = (self.line_start_offset(a), self.line_end_offset(b));
            self.replace_offsets(s, e, "");
            return Some(Pos::new(a.min(self.line_count().saturating_sub(1)), 0));
        }
        self.org_table_write(a, b, &rows, &indent);
        Some(self.field_pos(p.line, idx.min(ncols - 1)))
    }

    /// `org-table-move-column-left` / `-right`.
    pub fn org_table_move_column(&mut self, p: Pos, right: bool) -> bool {
        let Some((a, mut rows, indent)) = self.org_table_rows(p.line) else {
            return false;
        };
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        let j = if right {
            if idx + 1 >= ncols {
                return false;
            } else {
                idx + 1
            }
        } else if idx == 0 {
            return false;
        } else {
            idx - 1
        };
        for r in &mut rows {
            if let Row::Data(c) = r {
                while c.len() < ncols {
                    c.push(String::new());
                }
                c.swap(idx, j);
            }
        }
        self.org_table_write(a, b, &rows, &indent);
        self.set_point(self.field_pos(p.line, j));
        true
    }

    /// Replace the text of the field at point (`org-table-blank-field`
    /// with an empty string). Returns the old text.
    pub fn org_table_set_field(&mut self, p: Pos, text: &str) -> Option<String> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let Row::Data(cells) = &mut rows[p.line - a] else {
            return None;
        };
        while cells.len() <= idx {
            cells.push(String::new());
        }
        let old = std::mem::replace(&mut cells[idx], text.to_string());
        self.org_table_write(a, b, &rows, &indent);
        self.set_point(self.field_pos(p.line, idx));
        Some(old)
    }

    /// The text of the field at point.
    pub fn org_table_field_text(&self, p: Pos) -> Option<String> {
        let (a, rows, _) = self.org_table_rows(p.line)?;
        let idx = field_index(&self.line(p.line), p.col);
        match rows.get(p.line - a)? {
            Row::Data(c) => Some(c.get(idx).cloned().unwrap_or_default()),
            Row::Rule => None,
        }
    }

    /// `org-table-copy-down` (S-RET): copy the field above into this one
    /// and move down; numbers increment.
    pub fn org_table_copy_down(&mut self, p: Pos) -> Option<Pos> {
        let (a, rows, _) = self.org_table_rows(p.line)?;
        let idx = field_index(&self.line(p.line), p.col);
        let mut l = p.line - a;
        let above = loop {
            if l == 0 {
                return None;
            }
            l -= 1;
            if let Row::Data(c) = &rows[l] {
                break c.get(idx).cloned().unwrap_or_default();
            }
        };
        let new = match above.trim().parse::<i64>() {
            Ok(n) if self.org_table_field_text(p).is_some_and(|t| t.is_empty()) => {
                (n + 1).to_string()
            }
            _ => above,
        };
        self.org_table_set_field(p, &new)?;
        self.org_table_next_row(p)
    }

    // ------------------------------------------------------- regions

    /// The rectangle of fields between two points as rows of text.
    pub fn org_table_region_fields(&self, from: Pos, to: Pos) -> Option<Vec<Vec<String>>> {
        let (a, rows, _) = self.org_table_rows(from.line)?;
        let (l1, l2) = (from.line.min(to.line), from.line.max(to.line));
        let (c1, c2) = {
            let x = field_index(&self.line(from.line), from.col);
            let y = field_index(&self.line(to.line), to.col);
            (x.min(y), x.max(y))
        };
        let mut out = Vec::new();
        for l in l1..=l2 {
            if let Some(Row::Data(c)) = rows.get(l.checked_sub(a)?) {
                out.push(
                    (c1..=c2)
                        .map(|i| c.get(i).cloned().unwrap_or_default())
                        .collect(),
                );
            }
        }
        Some(out)
    }

    /// `org-table-copy-region` / `org-table-cut-region`. Cutting blanks
    /// the fields.
    pub fn org_table_copy_region(&mut self, from: Pos, to: Pos, cut: bool) -> Option<String> {
        let fields = self.org_table_region_fields(from, to)?;
        if cut {
            let (a, mut rows, indent) = self.org_table_rows(from.line)?;
            let b = a + rows.len() - 1;
            let (l1, l2) = (from.line.min(to.line), from.line.max(to.line));
            let x = field_index(&self.line(from.line), from.col);
            let y = field_index(&self.line(to.line), to.col);
            for l in l1..=l2 {
                // The region may reach past the last line of the table.
                let Some(idx) = l.checked_sub(a) else {
                    continue;
                };
                if let Some(Row::Data(c)) = rows.get_mut(idx) {
                    for i in x.min(y)..=x.max(y) {
                        if i < c.len() {
                            c[i].clear();
                        }
                    }
                }
            }
            self.org_table_write(a, b, &rows, &indent);
        }
        Some(
            fields
                .iter()
                .map(|r| r.join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// `org-table-paste-rectangle`: put copied fields at point, extending
    /// the table. Returns the number of fields pasted.
    pub fn org_table_paste_rectangle(&mut self, p: Pos, text: &str) -> Option<usize> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let mut n = 0;
        let mut r = p.line - a;
        for line in text.lines() {
            while r < rows.len() && rows[r].is_rule() {
                r += 1;
            }
            if r >= rows.len() {
                rows.push(Row::Data(Vec::new()));
            }
            if let Row::Data(c) = &mut rows[r] {
                for (k, f) in line.split('\t').enumerate() {
                    while c.len() <= idx + k {
                        c.push(String::new());
                    }
                    c[idx + k] = f.to_string();
                    n += 1;
                }
            }
            r += 1;
        }
        let rowsn = rows.len();
        let new_last = a + rowsn - 1;
        if new_last > b {
            // Make room first so the write covers the new rows.
            let end = self.line_end_offset(b);
            self.replace_offsets(end, end, &"\n|".repeat(new_last - b));
        }
        self.org_table_write(a, new_last, &rows, &indent);
        self.set_point(self.field_pos(p.line, idx));
        Some(n)
    }

    /// `org-table-wrap-region`: wrap the text in `from..=to` (lines) into
    /// a one-field-per-line column table: each line's words are split at
    /// `width` characters into rows.
    pub fn org_table_wrap_region(&mut self, p: Pos, width: usize) -> Option<usize> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let Row::Data(cells) = rows.get(p.line - a)?.clone() else {
            return None;
        };
        let text = cells.get(idx).cloned().unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for w in text.split_whitespace() {
            if !cur.is_empty() && line_width(&cur) + 1 + line_width(w) > width.max(1) {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        if lines.len() <= 1 {
            return Some(1);
        }
        let n = lines.len();
        let row_i = p.line - a;
        let ncols = cells.len();
        // The first piece stays; the others become the same field of new rows.
        if let Row::Data(c) = &mut rows[row_i] {
            c[idx] = lines[0].clone();
        }
        for (k, piece) in lines.iter().enumerate().skip(1) {
            let mut c = vec![String::new(); ncols];
            c[idx] = piece.clone();
            rows.insert(row_i + k, Row::Data(c));
        }
        let new_last = a + rows.len() - 1;
        if new_last > b {
            let end = self.line_end_offset(b);
            self.replace_offsets(end, end, &"\n|".repeat(new_last - b));
        }
        self.org_table_write(a, new_last, &rows, &indent);
        self.set_point(self.field_pos(p.line, idx));
        Some(n)
    }

    /// `org-table-sort-lines`: sort by the column at point.
    pub fn org_table_sort(&mut self, p: Pos, kind: SortKind, reverse: bool) -> Option<usize> {
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        sort_rows(&mut rows, idx, kind, reverse);
        self.org_table_write(a, b, &rows, &indent);
        self.set_point(self.field_pos(p.line, idx));
        Some(rows.iter().filter(|r| !r.is_rule()).count())
    }

    /// `org-table-sum` (C-c +): total of the numbers in the column above
    /// point (up to a rule or the table's start).
    pub fn org_table_column_sum(&self, p: Pos) -> Option<f64> {
        let (a, rows, _) = self.org_table_rows(p.line)?;
        let idx = field_index(&self.line(p.line), p.col);
        let mut sum = 0.0;
        let mut l = p.line - a;
        while l > 0 {
            l -= 1;
            match &rows[l] {
                Row::Rule => break,
                Row::Data(c) => {
                    if let Some(v) = c.get(idx).and_then(|s| s.trim().parse::<f64>().ok()) {
                        sum += v;
                    }
                }
            }
        }
        Some(sum)
    }

    /// Rows of the table at `line`, for export.
    pub fn org_table_export(&self, line: usize, format: &str) -> Result<String, String> {
        let (_, rows, _) = self.org_table_rows(line).ok_or("Not in a table")?;
        export_rows(&rows, format)
    }

    /// Replace the table at `line` with the table made from delimited
    /// `text` (`org-table-import`). Inserts after `line` when not in one.
    pub fn org_table_import(&mut self, line: usize, text: &str) -> usize {
        let sep = guess_separator(text);
        let body = convert_text(text, &sep, "");
        let n = body.lines().count();
        let end = self.line_end_offset(line.min(self.line_count().saturating_sub(1)));
        self.replace_offsets(end, end, &format!("\n{body}"));
        self.set_point(self.field_pos(line + 1, 0));
        n
    }

    /// `org-table-rotate-recalc-marks`: cycle the first-column mark of the
    /// current row.
    pub fn org_table_rotate_mark(&mut self, p: Pos) -> Option<String> {
        const MARKS: [&str; 7] = ["", "#", "*", "!", "$", "^", "_"];
        let (a, mut rows, indent) = self.org_table_rows(p.line)?;
        let b = a + rows.len() - 1;
        let idx = field_index(&self.line(p.line), p.col);
        let Row::Data(c) = &mut rows[p.line - a] else {
            return None;
        };
        if c.is_empty() {
            c.push(String::new());
        }
        let i = MARKS.iter().position(|m| *m == c[0]).unwrap_or(0);
        let next = MARKS[(i + 1) % MARKS.len()];
        c[0] = next.to_string();
        self.org_table_write(a, b, &rows, &indent);
        self.set_point(self.field_pos(p.line, idx));
        Some(next.to_string())
    }

    /// Recalculate the table at `line` from its `#+TBLFM:` lines. Returns
    /// the number of fields that changed.
    pub fn org_table_recalculate(
        &mut self,
        line: usize,
        iterate: bool,
        only_current: Option<(usize, usize)>,
    ) -> Result<usize, String> {
        let (a, mut rows, indent) = self.org_table_rows(line).ok_or("Not in a table")?;
        let b = a + rows.len() - 1;
        let formulas: Vec<String> = self
            .org_table_tblfm_lines(b)
            .into_iter()
            .map(|l| self.line(l).trim_start()[8..].trim().to_string())
            .collect();
        if formulas.is_empty() {
            return Err("No #+TBLFM: line below this table".into());
        }
        let mut changed = 0;
        let passes = if iterate { 10 } else { 1 };
        for _ in 0..passes {
            let n = crate::org_calc::recalculate(&mut rows, &formulas, only_current)?;
            changed += n;
            if n == 0 {
                break;
            }
        }
        self.org_table_write(a, b, &rows, &indent);
        Ok(changed)
    }

    /// `org-table-recalculate-buffer-tables`.
    pub fn org_table_recalculate_all(&mut self, iterate: bool) -> (usize, Vec<String>) {
        let mut tables = 0;
        let mut errors = Vec::new();
        let mut l = 0;
        while l < self.line_count() {
            if is_table_line(&self.line(l)) {
                let (_, b) = self.org_table_bounds(l).unwrap_or((l, l));
                if !self.org_table_tblfm_lines(b).is_empty() {
                    match self.org_table_recalculate(l, iterate, None) {
                        Ok(_) => tables += 1,
                        Err(e) => errors.push(format!("line {}: {e}", l + 1)),
                    }
                }
                l = b + 1;
            } else {
                l += 1;
            }
        }
        (tables, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b
    }

    fn lines(b: &Buffer) -> Vec<String> {
        (0..b.line_count()).map(|l| b.line(l)).collect()
    }

    #[test]
    fn parse_and_align() {
        assert_eq!(parse_row("|a|b |"), Row::Data(vec!["a".into(), "b".into()]));
        assert_eq!(parse_row("|---+---|"), Row::Rule);
        let rows = vec![
            Row::Data(vec!["name".into(), "qty".into()]),
            Row::Rule,
            Row::Data(vec!["apple".into(), "3".into()]),
            Row::Data(vec!["fig".into(), "12".into()]),
        ];
        assert_eq!(
            format_rows(&rows, ""),
            [
                "| name  | qty |",
                "|-------+-----|",
                "| apple |   3 |",
                "| fig   |  12 |"
            ]
        );
    }

    #[test]
    fn tab_moves_and_adds_rows() {
        let mut b = buf("| a | b |\n|---+---|\n| 1 | 2 |\n");
        b.set_point(Pos::new(0, 2));
        let p = b.org_table_next_field(b.point()).unwrap();
        assert_eq!(p.line, 0);
        assert_eq!(field_index(&b.line(0), p.col), 1);
        let p = b.org_table_next_field(p).unwrap();
        assert_eq!(p.line, 2, "rules are skipped");
        let p = b
            .org_table_next_field(Pos::new(2, field_span(&b.line(2), 1).unwrap().0))
            .unwrap();
        assert_eq!(p.line, 3);
        assert_eq!(b.line(3), "|   |   |");
    }

    #[test]
    fn row_and_column_editing() {
        let mut b = buf("| a | b |\n| 1 | 2 |\n");
        let p = Pos::new(1, 2);
        b.org_table_insert_row(p, false).unwrap();
        assert_eq!(lines(&b)[..3], ["| a | b |", "|   |   |", "| 1 | 2 |"]);
        b.org_table_move_row(Pos::new(1, 2), true);
        assert_eq!(b.line(2), "|   |   |");
        b.org_table_insert_column(Pos::new(0, 2)).unwrap();
        assert_eq!(b.line(0), "|   | a | b |");
        b.org_table_move_column(Pos::new(0, 6), false);
        assert_eq!(b.line(0), "| a |   | b |");
        b.org_table_delete_column(Pos::new(0, 6)).unwrap();
        assert_eq!(b.line(0), "| a | b |");
        let gone = b.org_table_kill_row(Pos::new(0, 2)).unwrap();
        assert_eq!(gone, "| a | b |");
        assert_eq!(b.line(0), "| 1 | 2 |");
    }

    #[test]
    fn sorting_keeps_the_header() {
        let mut b = buf("| n | v |\n|---+---|\n| b | 10 |\n| a | 9 |\n| c | 100 |\n");
        b.org_table_sort(Pos::new(2, 2), SortKind::AlphaCaseInsensitive, false)
            .unwrap();
        assert_eq!(b.line(2).split('|').nth(1).unwrap().trim(), "a");
        b.org_table_sort(Pos::new(2, 8), SortKind::Numeric, true)
            .unwrap();
        assert_eq!(b.line(2).split('|').nth(1).unwrap().trim(), "c");
        assert_eq!(b.line(0).split('|').nth(1).unwrap().trim(), "n");
    }

    #[test]
    fn convert_import_export_transpose() {
        let mut b = buf("x,y\n1,2\n");
        b.org_table_convert_lines(0, 1, None);
        assert_eq!(lines(&b)[..2], ["| x | y |", "| 1 | 2 |"]);
        let csv = b.org_table_export(0, "csv").unwrap();
        assert_eq!(csv, "x,y\n1,2");
        assert_eq!(b.org_table_export(0, "tsv").unwrap(), "x\ty\n1\t2");
        assert!(b.org_table_transpose(0));
        assert_eq!(lines(&b)[..2], ["| x | 1 |", "| y | 2 |"]);
        let spaces = convert_text("a  b c\nd  e", &Separator::Spaces(2), "");
        assert_eq!(spaces, "| a | b c |\n| d | e   |");
    }

    #[test]
    fn cut_region_past_the_table_is_not_a_panic() {
        let mut b = buf("| a | b |\n|---+---|\n| 1 | 2 |\ntext\n");
        let text = b
            .org_table_copy_region(Pos::new(0, 0), Pos::new(3, 0), true)
            .unwrap();
        assert_eq!(text, "a\n1");
        assert_eq!(b.line(2), "|   | 2 |");
    }

    #[test]
    fn multibyte_tblfm_line_is_not_a_panic() {
        assert!(!is_tblfm("#+TBLFMé"));
        assert!(is_tblfm("#+TBLFM: $1=1"));
    }

    #[test]
    fn region_copy_cut_paste() {
        let mut b = buf("| a | b | c |\n| d | e | f |\n");
        let text = b
            .org_table_copy_region(Pos::new(0, 2), Pos::new(1, 6), true)
            .unwrap();
        assert_eq!(text, "a\tb\nd\te");
        assert_eq!(b.line(0), "|   |   | c |");
        let n = b.org_table_paste_rectangle(Pos::new(1, 10), &text).unwrap();
        assert_eq!(n, 4);
        assert_eq!(b.line(1), "|   |   | a | b |");
        assert!(b.line_count() >= 3);
        assert_eq!(b.line(2).trim_end(), "|   |   | d | e |");
    }

    #[test]
    fn column_sum_and_marks() {
        let mut b = buf("| 1 |\n| 2.5 |\n|   |\n");
        assert_eq!(b.org_table_column_sum(Pos::new(2, 2)), Some(3.5));
        let mut m = buf("|   | x |\n|   | y |\n");
        assert_eq!(
            m.org_table_rotate_mark(Pos::new(0, 6)).as_deref(),
            Some("#")
        );
        assert_eq!(m.line(0), "| # | x |");
        assert_eq!(
            m.org_table_rotate_mark(Pos::new(0, 6)).as_deref(),
            Some("*")
        );
        let _ = &mut b;
    }
}
