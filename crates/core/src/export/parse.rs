// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parsing Org text into the export tree.

use super::ast::*;
use super::cite;
use crate::org::{self as o, Keywords};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([-+*]|\d+[.)])(?:\s+(.*)|$)").unwrap());
static KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#\+([A-Za-z0-9_-]+)(\[[^\]]*\])?:\s?(.*)$").unwrap());
static BEGIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*#\+begin_(\S+)\s*(.*)$").unwrap());
static PLANNING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(SCHEDULED|DEADLINE|CLOSED):\s*([<\[][^>\]]*[>\]])").unwrap());
static DRAWER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*:([\w-]+):\s*$").unwrap());
static FN_DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[fn:([^\]\s]+)\]\s?(.*)$").unwrap());
static MACRO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\{([A-Za-z][\w-]*)(?:\(((?s:.*?))\))?\}\}\}").unwrap());
static TS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:<\d{4}-\d{2}-\d{2}[^>\n]*>(?:--<\d{4}-\d{2}-\d{2}[^>\n]*>)?|\[\d{4}-\d{2}-\d{2}[^\]\n]*\](?:--\[\d{4}-\d{2}-\d{2}[^\]\n]*\])?)").unwrap()
});
static COOKIE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[(?:\d*/\d*|\d*%)\]").unwrap());
static NUMERIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[-+]?[0-9][0-9.,]*(?:[eE][-+]?\d+)?%?$|^[-+]?\.[0-9]+$").unwrap()
});
static INCLUDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^\s*#\+INCLUDE:\s*"?([^"\s]+)"?\s*(.*)$"#).unwrap());
static LINES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#":lines\s+"(\d*)-(\d*)""#).unwrap());
static MINLEVEL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":minlevel\s+(\d+)").unwrap());

/// Parse `text`. `read` loads a file named by `#+INCLUDE`, `#+BIBLIOGRAPHY`
/// and such, relative to the document.
pub fn parse(text: &str, read: &dyn Fn(&str) -> Option<String>) -> Doc {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines = expand_includes(lines, read, 0);
    let mut meta = read_meta(&lines);
    lines = expand_macros(lines, &meta);
    // Footnote definitions come out of the line stream first.
    let mut fndefs: HashMap<String, Vec<String>> = HashMap::new();
    let mut rest: Vec<String> = Vec::new();
    let mut in_src = false;
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        let low = l.trim_start().to_ascii_lowercase();
        if low.starts_with("#+begin_src")
            || low.starts_with("#+begin_example")
            || low.starts_with("#+begin_export")
            || low.starts_with("#+begin_comment")
        {
            in_src = true;
        } else if low.starts_with("#+end_") {
            in_src = false;
        }
        if !in_src && let Some(c) = FN_DEF_RE.captures(l) {
            let mut body = vec![c[2].to_string()];
            i += 1;
            let mut blanks = 0;
            while i < lines.len() {
                let n = &lines[i];
                if o::heading_level(n).is_some() || FN_DEF_RE.is_match(n) {
                    break;
                }
                if n.trim().is_empty() {
                    blanks += 1;
                    if blanks >= 2 {
                        break;
                    }
                } else {
                    blanks = 0;
                }
                body.push(n.clone());
                i += 1;
            }
            fndefs.insert(c[1].to_string(), body);
            continue;
        }
        rest.push(l.clone());
        i += 1;
    }
    let kw = Keywords::from_text(rest.iter().map(String::as_str));
    let mut p = P {
        opts: meta.options.clone(),
        kw,
        fndefs,
        fn_pending: Vec::new(),
        fn_index: HashMap::new(),
        cited: Vec::new(),
        exclude: meta.exclude_tags.clone(),
        select: meta.select_tags.clone(),
        base_line: 0,
    };
    if !p.exclude.iter().any(|t| t == "noexport") {
        p.exclude.push("noexport".into());
    }
    let nodes = p.blocks(&rest, true);
    // Footnote bodies, which may cite or reference more footnotes.
    let mut footnotes = Vec::new();
    let mut k = 0;
    while k < p.fn_pending.len() {
        let (label, inline_def) = p.fn_pending[k].clone();
        let content = match inline_def {
            Some(d) => vec![Node::new(Block::Para(p.inline(&d)))],
            None => match p.fndefs.get(&label).cloned() {
                Some(body) => p.blocks(&body, false),
                None => vec![Node::new(Block::Para(vec![Inline::Text(
                    "(Footnote definition missing)".into(),
                )]))],
            },
        };
        footnotes.push(Footnote {
            number: k + 1,
            label,
            content,
        });
        k += 1;
    }
    meta.options = p.opts.clone();
    let mut bib = Vec::new();
    for f in &meta.bibliography {
        if let Some(t) = read(f) {
            bib.extend(cite::parse_bib(&t));
        }
    }
    Doc {
        meta,
        nodes,
        footnotes,
        bib,
        cited: p.cited,
        warnings: Vec::new(),
    }
}

/// The text with includes and macros expanded and the subtrees that
/// don't export (`:noexport:`, COMMENT) removed: what Org-to-Org export
/// writes.
pub fn preprocess(text: &str, read: &dyn Fn(&str) -> Option<String>) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines = expand_includes(lines, read, 0);
    let meta = read_meta(&lines);
    lines = expand_macros(lines, &meta);
    let kw = Keywords::from_text(lines.iter().map(String::as_str));
    let mut exclude = meta.exclude_tags.clone();
    if !exclude.iter().any(|t| t == "noexport") {
        exclude.push("noexport".into());
    }
    let mut out = Vec::new();
    let mut skip: Option<usize> = None;
    let mut in_block = false;
    for l in lines {
        let low = l.trim_start().to_ascii_lowercase();
        if low.starts_with("#+begin_") {
            in_block = true;
        } else if low.starts_with("#+end_") {
            in_block = false;
        }
        if !in_block && let Some(h) = o::parse_heading(&l, &kw) {
            if let Some(s) = skip
                && h.level > s
            {
                continue;
            }
            skip = None;
            let commented = h.title == "COMMENT" || h.title.starts_with("COMMENT ");
            if commented || h.tags.iter().any(|t| exclude.contains(t)) {
                skip = Some(h.level);
                continue;
            }
        } else if skip.is_some() {
            continue;
        }
        out.push(l);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Parse a string of inline markup on its own (a title, say).
pub fn inline_of(s: &str, opts: &Options) -> Vec<Inline> {
    let mut p = P {
        opts: opts.clone(),
        kw: Keywords::default(),
        fndefs: HashMap::new(),
        fn_pending: Vec::new(),
        fn_index: HashMap::new(),
        cited: Vec::new(),
        exclude: Vec::new(),
        select: Vec::new(),
        base_line: 0,
    };
    p.inline(s)
}

fn expand_includes(
    lines: Vec<String>,
    read: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for l in lines {
        let Some(c) = INCLUDE_RE.captures(&l) else {
            out.push(l);
            continue;
        };
        if depth >= 3 {
            continue;
        }
        let file = c[1].to_string();
        let args = c[2].to_string();
        let Some(text) = read(&file) else {
            out.push(format!("# include failed: {file}"));
            continue;
        };
        let mut body: Vec<String> = text.lines().map(String::from).collect();
        if let Some(m) = LINES_RE.captures(&args) {
            let a = m[1].parse::<usize>().unwrap_or(1).max(1) - 1;
            let b = m[2].parse::<usize>().unwrap_or(body.len()).min(body.len());
            body = body.get(a..b.max(a)).unwrap_or(&[]).to_vec();
        }
        let words: Vec<&str> = args.split_whitespace().collect();
        match words.first().copied() {
            Some("src") => {
                out.push(format!("#+BEGIN_SRC {}", words.get(1).unwrap_or(&"")));
                out.extend(body);
                out.push("#+END_SRC".into());
            }
            Some("example") => {
                out.push("#+BEGIN_EXAMPLE".into());
                out.extend(body);
                out.push("#+END_EXAMPLE".into());
            }
            Some("export") => {
                out.push(format!("#+BEGIN_EXPORT {}", words.get(1).unwrap_or(&"")));
                out.extend(body);
                out.push("#+END_EXPORT".into());
            }
            Some("quote") => {
                out.push("#+BEGIN_QUOTE".into());
                out.extend(body);
                out.push("#+END_QUOTE".into());
            }
            _ => {
                let body = expand_includes(body, read, depth + 1);
                if let Some(m) = MINLEVEL_RE.captures(&args) {
                    let min: usize = m[1].parse().unwrap_or(1);
                    let cur = body.iter().filter_map(|l| o::heading_level(l)).min();
                    if let Some(cur) = cur
                        && min > cur
                    {
                        let add = "*".repeat(min - cur);
                        for l in body {
                            if o::heading_level(&l).is_some() {
                                out.push(format!("{add}{l}"));
                            } else {
                                out.push(l);
                            }
                        }
                        continue;
                    }
                }
                out.extend(body);
            }
        }
    }
    out
}

fn read_meta(lines: &[String]) -> Meta {
    let mut m = Meta::default();
    let mut in_block = false;
    for l in lines {
        let low = l.trim_start().to_ascii_lowercase();
        if low.starts_with("#+begin_") {
            in_block = true;
        } else if low.starts_with("#+end_") {
            in_block = false;
        }
        if in_block {
            continue;
        }
        let Some(c) = KEYWORD_RE.captures(l) else {
            continue;
        };
        let key = c[1].to_ascii_uppercase();
        let val = c[3].trim().to_string();
        let join = |dst: &mut String, v: &str| {
            if !dst.is_empty() {
                dst.push(' ');
            }
            dst.push_str(v);
        };
        match key.as_str() {
            "TITLE" => join(&mut m.title, &val),
            "SUBTITLE" => join(&mut m.subtitle, &val),
            "AUTHOR" => m.author = val,
            "EMAIL" => m.email = val,
            "DATE" => m.date = val,
            "LANGUAGE" => m.language = val,
            "DESCRIPTION" => join(&mut m.description, &val),
            "KEYWORDS" => join(&mut m.keywords, &val),
            "OPTIONS" => m.options.apply(&val),
            "LATEX_CLASS" => m.latex_class = val,
            "LATEX_CLASS_OPTIONS" => m.latex_class_options = val,
            "LATEX_HEADER" | "LATEX_HEADER_EXTRA" => m.latex_headers.push(val),
            "HTML_HEAD" | "HTML_HEAD_EXTRA" => m.html_head.push(val),
            "HTML_LINK_HOME" => m.html_link_home = val,
            "HTML_LINK_UP" => m.html_link_up = val,
            "BEAMER_THEME" => m.beamer_theme = val,
            "BEAMER_COLOR_THEME" => m.beamer_color_theme = val,
            "BEAMER_FONT_THEME" => m.beamer_font_theme = val,
            "BEAMER_HEADER" => m.beamer_header.push(val),
            "BIBLIOGRAPHY" => m.bibliography.push(val),
            "CITE_EXPORT" => m.cite_export = val.split_whitespace().map(String::from).collect(),
            "EXCLUDE_TAGS" => m.exclude_tags = val.split_whitespace().map(String::from).collect(),
            "SELECT_TAGS" => m.select_tags = val.split_whitespace().map(String::from).collect(),
            "TEXINFO_FILENAME" => m.texinfo_filename = val,
            "TEXINFO_CLASS" => m.texinfo_class = val,
            "TEXINFO_DIR_CATEGORY" => m.texinfo_dir_category = val,
            "TEXINFO_DIR_TITLE" => m.texinfo_dir_title = val,
            "TEXINFO_DIR_DESC" => m.texinfo_dir_desc = val,
            "ODT_STYLES_FILE" => m.odt_styles_file = val,
            "ICALENDAR_CALENDAR" => m.ical_calendar = val,
            "EXPORT_FILE_NAME" => m.export_file_name = val,
            "MACRO" => {
                if let Some((n, b)) = val.split_once(char::is_whitespace) {
                    m.macros.push((n.to_string(), b.trim().to_string()));
                }
            }
            _ => {}
        }
    }
    m
}

fn expand_macros(lines: Vec<String>, meta: &Meta) -> Vec<String> {
    if !lines.iter().any(|l| l.contains("{{{")) {
        return lines;
    }
    lines
        .into_iter()
        .map(|l| {
            if KEYWORD_RE.is_match(&l) && l.to_ascii_uppercase().contains("#+MACRO:") {
                return l;
            }
            MACRO_RE
                .replace_all(&l, |c: &regex::Captures| {
                    let name = &c[1];
                    let args: Vec<String> = c
                        .get(2)
                        .map(|a| {
                            a.as_str()
                                .split(',')
                                .map(|x| x.trim().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some((_, body)) = meta.macros.iter().find(|(n, _)| n == name) {
                        let mut r = body.clone();
                        for (k, a) in args.iter().enumerate().rev() {
                            r = r.replace(&format!("${}", k + 1), a);
                        }
                        return r;
                    }
                    match name {
                        "title" => meta.title.clone(),
                        "author" => meta.author.clone(),
                        "email" => meta.email.clone(),
                        "date" => meta.date.clone(),
                        "kbd" => format!(
                            "@@html:<kbd>{}</kbd>@@",
                            args.first().cloned().unwrap_or_default()
                        ),
                        _ => c[0].to_string(),
                    }
                })
                .into_owned()
        })
        .collect()
}

struct P {
    opts: Options,
    kw: Keywords,
    fndefs: HashMap<String, Vec<String>>,
    /// (label, inline definition) in order of first reference.
    fn_pending: Vec<(String, Option<String>)>,
    fn_index: HashMap<String, usize>,
    cited: Vec<String>,
    exclude: Vec<String>,
    select: Vec<String>,
    base_line: usize,
}

fn indent_of(l: &str) -> usize {
    l.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 8 } else { 1 })
        .sum()
}

fn strip_indent(l: &str, n: usize) -> String {
    let mut k = 0;
    let mut cols = 0;
    for ch in l.chars() {
        if cols >= n || !(ch == ' ' || ch == '\t') {
            break;
        }
        cols += if ch == '\t' { 8 } else { 1 };
        k += ch.len_utf8();
    }
    l[k..].to_string()
}

/// Remove the indentation common to all non-blank lines.
fn dedent(lines: &[String]) -> Vec<String> {
    let n = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    lines.iter().map(|l| strip_indent(l, n)).collect()
}

fn unescape_code(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with(",*") || t.starts_with(",#+") {
                let ind = l.len() - t.len();
                format!("{}{}", &l[..ind], &t[1..])
            } else {
                l
            }
        })
        .collect()
}

impl P {
    fn is_boundary(l: &str) -> bool {
        o::heading_level(l).is_some()
            || l.trim().is_empty()
            || BEGIN_RE.is_match(l)
            || KEYWORD_RE.is_match(l)
            || ITEM_RE.is_match(l) && !l.starts_with('*')
            || l.trim_start().starts_with('|')
            || l.trim_start().starts_with("\\begin{")
            || l.trim_start().starts_with(": ")
            || (l.trim().len() >= 5 && l.trim().chars().all(|c| c == '-'))
            || DRAWER_RE.is_match(l)
    }

    fn blocks(&mut self, lines: &[String], top: bool) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        let mut aff = Aff::default();
        let mut i = 0;
        let mut skip_below: Option<usize> = None;
        while i < lines.len() {
            let l = &lines[i];
            if l.trim().is_empty() {
                i += 1;
                continue;
            }
            if let Some(lv) = skip_below {
                match o::heading_level(l) {
                    Some(n) if n <= lv => skip_below = None,
                    _ => {
                        i += 1;
                        continue;
                    }
                }
            }
            if top && let Some(h) = o::parse_heading(l, &self.kw) {
                let tagged = |set: &[String]| h.tags.iter().any(|t| set.contains(t));
                if tagged(&self.exclude) && !(self.select.iter().any(|t| h.tags.contains(t))) {
                    skip_below = Some(h.level);
                    i += 1;
                    continue;
                }
                i = self.heading(lines, i, h, &mut out, &mut aff);
                continue;
            }
            // Affiliated keywords and other keyword lines.
            if let Some(c) = KEYWORD_RE.captures(l) {
                let key = c[1].to_ascii_uppercase();
                let val = c[3].trim().to_string();
                let low = BEGIN_RE.is_match(l);
                if !low {
                    match key.as_str() {
                        "CAPTION" => {
                            aff.caption = Some(match aff.caption.take() {
                                Some(p) => format!("{p} {val}"),
                                None => val,
                            });
                            if let Some(s) = c.get(2) {
                                aff.short_caption =
                                    Some(s.as_str().trim_matches(['[', ']']).to_string());
                            }
                        }
                        "NAME" | "LABEL" | "SRCNAME" | "TBLNAME" | "RESNAME" => {
                            aff.name = Some(val)
                        }
                        k if k.starts_with("ATTR_") => aff.attrs.push((k[5..].to_lowercase(), val)),
                        "HEADER" | "HEADERS" => aff.attrs.push(("header".into(), val)),
                        "HTML" => out.push(Node::new(Block::Export {
                            backend: "html".into(),
                            text: val,
                        })),
                        "LATEX" => out.push(Node::new(Block::Export {
                            backend: "latex".into(),
                            text: val,
                        })),
                        "BEAMER" => out.push(Node::new(Block::Export {
                            backend: "beamer".into(),
                            text: val,
                        })),
                        "ASCII" => out.push(Node::new(Block::Export {
                            backend: "ascii".into(),
                            text: val,
                        })),
                        "TEXINFO" => out.push(Node::new(Block::Export {
                            backend: "texinfo".into(),
                            text: val,
                        })),
                        "ODT" => out.push(Node::new(Block::Export {
                            backend: "odt".into(),
                            text: val,
                        })),
                        "TOC" | "PRINT_BIBLIOGRAPHY" | "INDEX" | "CINDEX" | "FINDEX" | "KINDEX"
                        | "PINDEX" | "TINDEX" | "VINDEX" => {
                            let mut n = Node::new(Block::Keyword { key, value: val });
                            n.aff = std::mem::take(&mut aff);
                            out.push(n);
                        }
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
            }
            if l.trim_start() == "#" || l.trim_start().starts_with("# ") {
                i += 1;
                continue;
            }
            if let Some(c) = BEGIN_RE.captures(l) {
                let name = c[1].to_string();
                let params = c[2].trim().to_string();
                let end_pat = format!("#+end_{}", name.to_ascii_lowercase());
                let mut j = i + 1;
                while j < lines.len() && !lines[j].trim().to_ascii_lowercase().starts_with(&end_pat)
                {
                    j += 1;
                }
                let raw: Vec<String> = lines[i + 1..j.min(lines.len())].to_vec();
                i = (j + 1).min(lines.len() + 1);
                let block = self.block(&name.to_ascii_lowercase(), &params, raw);
                if let Some(b) = block {
                    let mut n = Node::new(b);
                    n.aff = std::mem::take(&mut aff);
                    out.push(n);
                } else {
                    aff = Aff::default();
                }
                continue;
            }
            if let Some(c) = DRAWER_RE.captures(l)
                && !c[1].eq_ignore_ascii_case("end")
            {
                let name = c[1].to_string();
                let mut j = i + 1;
                while j < lines.len() && !lines[j].trim().eq_ignore_ascii_case(":end:") {
                    j += 1;
                }
                let raw = dedent(&lines[i + 1..j.min(lines.len())]);
                i = j + 1;
                if !name.eq_ignore_ascii_case("logbook")
                    && !name.eq_ignore_ascii_case("properties")
                    && self.opts.drawers
                {
                    let content = self.blocks(&raw, false);
                    out.push(Node::new(Block::Drawer { name, content }));
                }
                continue;
            }
            if l.trim_start().starts_with(": ") || l.trim() == ":" {
                let mut text = Vec::new();
                while i < lines.len()
                    && (lines[i].trim_start().starts_with(": ") || lines[i].trim() == ":")
                {
                    let t = lines[i].trim_start();
                    text.push(t.get(2..).unwrap_or("").to_string());
                    i += 1;
                }
                let mut n = Node::new(Block::Example(text.join("\n")));
                n.aff = std::mem::take(&mut aff);
                out.push(n);
                continue;
            }
            if l.trim_start().starts_with('|') {
                let mut rows = Vec::new();
                while i < lines.len() && lines[i].trim_start().starts_with('|') {
                    rows.push(lines[i].trim().to_string());
                    i += 1;
                }
                while i < lines.len()
                    && lines[i]
                        .trim_start()
                        .to_ascii_uppercase()
                        .starts_with("#+TBLFM")
                {
                    i += 1;
                }
                let mut n = Node::new(Block::Table(self.table(&rows)));
                n.aff = std::mem::take(&mut aff);
                out.push(n);
                continue;
            }
            if l.trim().len() >= 5 && l.trim().chars().all(|c| c == '-') {
                out.push(Node::new(Block::Rule));
                i += 1;
                continue;
            }
            if let Some(rest) = l.trim_start().strip_prefix("\\begin{")
                && let Some(end) = rest.find('}')
            {
                let name = &rest[..end];
                let close = format!("\\end{{{name}}}");
                let mut j = i;
                while j < lines.len() && !lines[j].contains(&close) {
                    j += 1;
                }
                if j < lines.len() {
                    let text = lines[i..=j].join("\n");
                    i = j + 1;
                    let mut n = Node::new(Block::LatexEnv(text));
                    n.aff = std::mem::take(&mut aff);
                    out.push(n);
                    continue;
                }
            }
            if ITEM_RE.is_match(l) && (indent_of(l) > 0 || !l.starts_with('*')) {
                let (list, next) = self.list(lines, i);
                i = next;
                let mut n = Node::new(Block::List(list));
                n.aff = std::mem::take(&mut aff);
                out.push(n);
                continue;
            }
            let mut para = vec![l.trim().to_string()];
            i += 1;
            while i < lines.len() && !Self::is_boundary(&lines[i]) {
                para.push(lines[i].trim().to_string());
                i += 1;
            }
            let text = para.join("\n");
            let mut n = Node::new(Block::Para(self.inline(&text)));
            n.aff = std::mem::take(&mut aff);
            out.push(n);
        }
        out
    }

    fn heading(
        &mut self,
        lines: &[String],
        i: usize,
        h: o::Heading,
        out: &mut Vec<Node>,
        aff: &mut Aff,
    ) -> usize {
        let mut title = h.title.clone();
        let mut commented = false;
        if let Some(r) = title.strip_prefix("COMMENT")
            && (r.is_empty() || r.starts_with(' '))
        {
            commented = true;
            title = r.trim_start().to_string();
        }
        let mut priority = None;
        if title.starts_with("[#") && title.len() >= 4 && title.as_bytes()[3] == b']' {
            priority = title.chars().nth(2);
            title = title[4..].trim_start().to_string();
        }
        let mut j = i + 1;
        let mut planning = Vec::new();
        if j < lines.len()
            && PLANNING_RE.is_match(&lines[j])
            && !lines[j].trim_start().starts_with('*')
            && lines[j].trim_start().starts_with(['S', 'D', 'C'])
        {
            for c in PLANNING_RE.captures_iter(&lines[j]) {
                planning.push((c[1].to_string(), c[2].to_string()));
            }
            j += 1;
        }
        let mut props = Vec::new();
        if j < lines.len() && lines[j].trim().eq_ignore_ascii_case(":PROPERTIES:") {
            j += 1;
            while j < lines.len() && !lines[j].trim().eq_ignore_ascii_case(":END:") {
                let t = lines[j].trim();
                if let Some(rest) = t.strip_prefix(':')
                    && let Some((k, v)) = rest.split_once(':')
                {
                    props.push((
                        k.trim().trim_end_matches('+').to_string(),
                        v.trim().to_string(),
                    ));
                }
                j += 1;
            }
            j += 1;
        }
        let done = h.keyword.as_ref().is_some_and(|k| self.kw.done.contains(k));
        let node = Node {
            aff: std::mem::take(aff),
            block: Block::Heading(Heading {
                level: h.level,
                todo: h.keyword,
                done,
                priority,
                commented,
                raw_title: title.clone(),
                title: self.inline(&title),
                tags: h.tags,
                props,
                planning,
                line: i + self.base_line,
            }),
        };
        // A COMMENT heading drops with its subtree.
        if commented {
            let mut k = j;
            while k < lines.len() && !o::heading_level(&lines[k]).is_some_and(|n| n <= h.level) {
                k += 1;
            }
            return k;
        }
        out.push(node);
        j
    }

    fn block(&mut self, name: &str, params: &str, raw: Vec<String>) -> Option<Block> {
        Some(match name {
            "src" => {
                let mut w = params.splitn(2, char::is_whitespace);
                let lang = w.next().unwrap_or("").to_string();
                let params = w.next().unwrap_or("").trim().to_string();
                Block::Src {
                    lang,
                    params,
                    text: dedent(&unescape_code(raw)).join("\n"),
                }
            }
            "example" => Block::Example(dedent(&unescape_code(raw)).join("\n")),
            "export" => Block::Export {
                backend: params
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase(),
                text: raw.join("\n"),
            },
            "comment" => return None,
            "verse" => Block::Verse(dedent(&raw).join("\n")),
            "quote" => Block::Quote(self.blocks(&dedent(&raw), false)),
            "center" => Block::Center(self.blocks(&dedent(&raw), false)),
            other => Block::Special {
                name: other.to_string(),
                params: params.to_string(),
                content: self.blocks(&dedent(&raw), false),
            },
        })
    }

    fn list(&mut self, lines: &[String], start: usize) -> (List, usize) {
        let base = indent_of(&lines[start]);
        let first = ITEM_RE
            .captures(&lines[start])
            .map(|c| c[2].to_string())
            .unwrap_or_default();
        let ordered = first.chars().next().is_some_and(|c| c.is_ascii_digit());
        let mut items = Vec::new();
        let mut i = start;
        let mut descriptive = false;
        while i < lines.len() {
            let Some(c) = ITEM_RE.captures(&lines[i]) else {
                break;
            };
            if indent_of(&lines[i]) != base || (c[2] == *"*" && base == 0) {
                break;
            }
            let bullet = c[2].to_string();
            if bullet.chars().next().is_some_and(|b| b.is_ascii_digit()) != ordered {
                break;
            }
            let mut first_text = c.get(3).map(|m| m.as_str().to_string()).unwrap_or_default();
            let content_indent = base + bullet.chars().count() + 1;
            let mut body: Vec<String> = Vec::new();
            let mut j = i + 1;
            let mut blanks = 0;
            while j < lines.len() {
                let n = &lines[j];
                if n.trim().is_empty() {
                    blanks += 1;
                    if blanks >= 2 {
                        break;
                    }
                    body.push(String::new());
                    j += 1;
                    continue;
                }
                let ind = indent_of(n);
                if ind > base {
                    blanks = 0;
                    body.push(strip_indent(n, content_indent.min(ind)));
                    j += 1;
                } else {
                    break;
                }
            }
            // Trailing blank lines belong to the list, not the item.
            while body.last().is_some_and(|l| l.is_empty()) {
                body.pop();
            }
            let mut counter = None;
            let mut checkbox = None;
            let mut t = first_text.trim_start().to_string();
            if let Some(r) = t.strip_prefix("[@")
                && let Some(e) = r.find(']')
            {
                counter = r[..e].parse::<usize>().ok();
                t = r[e + 1..].trim_start().to_string();
            }
            if t.len() >= 3
                && t.starts_with('[')
                && matches!(t.as_bytes()[1], b' ' | b'x' | b'X' | b'-')
                && t.as_bytes()[2] == b']'
                && (t.len() == 3 || t.as_bytes()[3] == b' ')
            {
                checkbox = Some(if t.as_bytes()[1] == b'x' {
                    'X'
                } else {
                    t.as_bytes()[1] as char
                });
                t = t[3..].trim_start().to_string();
            }
            let mut tag = None;
            if !ordered && let Some(p) = find_tag_sep(&t) {
                let (a, b) = t.split_at(p);
                tag = Some(self.inline(a.trim()));
                t = b[2..].trim_start().to_string();
                descriptive = true;
            }
            first_text = t;
            let mut content_lines = vec![first_text];
            content_lines.extend(body);
            let content = self.blocks(&content_lines, false);
            items.push(Item {
                bullet,
                counter,
                checkbox,
                tag,
                content,
            });
            i = j;
        }
        (
            List {
                ordered,
                descriptive,
                items,
            },
            i,
        )
    }

    fn table(&mut self, rows: &[String]) -> Table {
        let mut out: Vec<Row> = Vec::new();
        let mut align: Vec<char> = Vec::new();
        let mut cookie_align: Vec<Option<char>> = Vec::new();
        for r in rows {
            let t = r.trim();
            if t.starts_with("|-") || t.starts_with("|+") {
                out.push(Row::Rule);
                continue;
            }
            let inner = t.trim_start_matches('|');
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
            // A row of alignment cookies.
            let is_cookie = |c: &str| {
                c.is_empty()
                    || (c.starts_with('<')
                        && c.ends_with('>')
                        && c[1..c.len() - 1]
                            .chars()
                            .all(|x| matches!(x, 'l' | 'r' | 'c') || x.is_ascii_digit()))
            };
            if cells.iter().all(|c| is_cookie(c)) && cells.iter().any(|c| !c.is_empty()) {
                for (k, c) in cells.iter().enumerate() {
                    let a = c.chars().find(|x| matches!(x, 'l' | 'r' | 'c'));
                    if cookie_align.len() <= k {
                        cookie_align.resize(k + 1, None);
                    }
                    if a.is_some() {
                        cookie_align[k] = a;
                    }
                }
                continue;
            }
            out.push(Row::Cells(cells.iter().map(|c| self.inline(c)).collect()));
        }
        let cols = out
            .iter()
            .map(|r| if let Row::Cells(c) = r { c.len() } else { 0 })
            .max()
            .unwrap_or(0);
        // Header: rows before the first rule, when rows follow it.
        let first_rule = out.iter().position(|r| matches!(r, Row::Rule));
        let has_header = first_rule
            .is_some_and(|p| p > 0 && out[p + 1..].iter().any(|r| matches!(r, Row::Cells(_))));
        let body_start = if has_header {
            first_rule.unwrap() + 1
        } else {
            0
        };
        for k in 0..cols {
            if let Some(Some(a)) = cookie_align.get(k) {
                align.push(*a);
                continue;
            }
            let mut n = 0;
            let mut num = 0;
            for r in &out[body_start..] {
                if let Row::Cells(c) = r
                    && let Some(cell) = c.get(k)
                {
                    let t = plain(cell);
                    if !t.trim().is_empty() {
                        n += 1;
                        if NUMERIC_RE.is_match(t.trim()) {
                            num += 1;
                        }
                    }
                }
            }
            align.push(if n > 0 && num * 10 >= n * 9 { 'r' } else { 'l' });
        }
        Table {
            rows: out,
            has_header,
            align,
        }
    }

    // --------------------------------------------------------- inline

    fn footnote_number(&mut self, label: &str, def: Option<String>) -> usize {
        let key = if label.is_empty() {
            format!("\u{0}anon{}", self.fn_pending.len())
        } else {
            label.to_string()
        };
        if let Some(&n) = self.fn_index.get(&key) {
            return n + 1;
        }
        let n = self.fn_pending.len();
        self.fn_index.insert(key.clone(), n);
        self.fn_pending.push((
            if label.is_empty() {
                key
            } else {
                label.to_string()
            },
            def,
        ));
        n + 1
    }

    pub(crate) fn inline(&mut self, s: &str) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::new();
        let mut text = String::new();
        let mut i = 0;
        macro_rules! flush {
            () => {
                if !text.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut text)));
                }
            };
        }
        while i < s.len() {
            let rest = &s[i..];
            let ch = rest.chars().next().unwrap();
            let prev = s[..i].chars().next_back();
            // Line break: `\\` at the end of a line.
            if let Some(after) = rest.strip_prefix("\\\\") {
                let after = after.trim_start_matches([' ', '\t']);
                if after.is_empty() || after.starts_with('\n') {
                    flush!();
                    out.push(Inline::Break);
                    i = s.len() - after.len();
                    if after.starts_with('\n') {
                        i += 1;
                    }
                    continue;
                }
            }
            match ch {
                '[' => {
                    if rest.starts_with("[[")
                        && let Some((end, target, desc)) = parse_link(rest)
                    {
                        flush!();
                        let d = desc.map(|d| self.inline(&d));
                        out.push(Inline::Link { target, desc: d });
                        i += end;
                        continue;
                    }
                    if rest.starts_with("[fn:")
                        && let Some(end) = balanced(rest, '[', ']')
                    {
                        let inner = &rest[4..end - 1];
                        let (label, def) = match inner.split_once(':') {
                            Some((l, d)) => (l, Some(d.to_string())),
                            None => (inner, None),
                        };
                        if label
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        {
                            flush!();
                            let number = self.footnote_number(label, def);
                            let lab = self.fn_pending[number - 1].0.clone();
                            out.push(Inline::FnRef { number, label: lab });
                            i += end;
                            continue;
                        }
                    }
                    if rest.starts_with("[cite")
                        && let Some(end) = balanced(rest, '[', ']')
                        && let Some(c) = cite::parse_cite(&rest[5..end - 1])
                    {
                        flush!();
                        for r in &c.refs {
                            if !self.cited.contains(&r.key) {
                                self.cited.push(r.key.clone());
                            }
                        }
                        out.push(Inline::Cite(c));
                        i += end;
                        continue;
                    }
                    if let Some(m) = TS_RE.find(rest) {
                        flush!();
                        out.push(Inline::Timestamp {
                            text: m.as_str().to_string(),
                            active: false,
                        });
                        i += m.end();
                        continue;
                    }
                    if let Some(m) = COOKIE_RE.find(rest) {
                        flush!();
                        out.push(Inline::Cookie(m.as_str().to_string()));
                        i += m.end();
                        continue;
                    }
                }
                '<' => {
                    if let Some(m) = TS_RE.find(rest) {
                        flush!();
                        out.push(Inline::Timestamp {
                            text: m.as_str().to_string(),
                            active: true,
                        });
                        i += m.end();
                        continue;
                    }
                    if rest.starts_with("<<")
                        && let Some(e) = rest.find(">>")
                        && !rest[2..e].contains(['<', '>', '\n'])
                        && e > 2
                    {
                        flush!();
                        let name = rest[2..e].to_string();
                        if rest.starts_with("<<<") {}
                        out.push(Inline::Target(name));
                        i += e + 2;
                        continue;
                    }
                    if let Some(e) = rest.find('>')
                        && ["http://", "https://", "ftp://", "mailto:", "file:", "id:"]
                            .iter()
                            .any(|p| rest[1..].starts_with(p))
                        && !rest[1..e].contains(char::is_whitespace)
                    {
                        flush!();
                        out.push(Inline::Link {
                            target: rest[1..e].to_string(),
                            desc: None,
                        });
                        i += e + 1;
                        continue;
                    }
                }
                '\\' => {
                    if self.opts.latex
                        && let Some((end, display)) = latex_delim(rest)
                    {
                        flush!();
                        out.push(Inline::Latex {
                            text: rest[..end].to_string(),
                            display,
                        });
                        i += end;
                        continue;
                    }
                    let name_len = rest[1..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .count();
                    if name_len > 0 {
                        let name = &rest[1..1 + name_len];
                        let after = &rest[1 + name_len..];
                        let entity = super::entities::find(name);
                        if let Some(e) = entity
                            && self.opts.entities
                            && !after.starts_with(|c: char| c.is_ascii_alphabetic())
                        {
                            flush!();
                            out.push(Inline::Entity(e.0.to_string()));
                            i += 1 + name_len;
                            if after.starts_with("{}") {
                                i += 2;
                            }
                            continue;
                        }
                        if self.opts.latex && (after.starts_with('{') || after.starts_with('[')) {
                            let mut end = 1 + name_len;
                            let mut ok = false;
                            while let Some(r) = rest.get(end..) {
                                let (o, c) = match r.chars().next() {
                                    Some('{') => ('{', '}'),
                                    Some('[') => ('[', ']'),
                                    _ => break,
                                };
                                match balanced(r, o, c) {
                                    Some(n) => {
                                        end += n;
                                        ok = true;
                                    }
                                    None => break,
                                }
                            }
                            if ok {
                                flush!();
                                out.push(Inline::Latex {
                                    text: rest[..end].to_string(),
                                    display: false,
                                });
                                i += end;
                                continue;
                            }
                        }
                    }
                    // `\_` and friends: the next character, literally.
                    if let Some(n) = rest[1..].chars().next()
                        && matches!(
                            n,
                            '_' | '^' | '*' | '/' | '+' | '=' | '~' | '[' | ']' | '\\' | '$'
                        )
                        && !rest.starts_with("\\\\")
                    {
                        text.push(n);
                        i += 1 + n.len_utf8();
                        continue;
                    }
                }
                '$' if self.opts.latex => {
                    if let Some((end, display)) = dollar_math(s, i) {
                        flush!();
                        out.push(Inline::Latex {
                            text: s[i..i + end].to_string(),
                            display,
                        });
                        i += end;
                        continue;
                    }
                }
                '*' | '/' | '_' | '+' | '=' | '~' if self.opts.emphasis => {
                    if let Some((end, inner)) = markup(s, i, ch) {
                        flush!();
                        out.push(match ch {
                            '*' => Inline::Bold(self.inline(inner)),
                            '/' => Inline::Italic(self.inline(inner)),
                            '_' => Inline::Underline(self.inline(inner)),
                            '+' => Inline::Strike(self.inline(inner)),
                            '=' => Inline::Verbatim(inner.to_string()),
                            _ => Inline::Code(inner.to_string()),
                        });
                        i = end;
                        continue;
                    }
                }
                _ => {}
            }
            if (ch == '_' || ch == '^')
                && self.opts.sub_super != SubSuper::Off
                && prev.is_some_and(|p| !p.is_whitespace())
                && let Some((end, inner)) = script(rest, self.opts.sub_super == SubSuper::Braces)
            {
                flush!();
                let v = self.inline(inner);
                out.push(if ch == '_' {
                    Inline::Sub(v)
                } else {
                    Inline::Sup(v)
                });
                i += end;
                continue;
            }
            if matches!(ch, 'h' | 'f' | 'm') && prev.is_none_or(|p| !p.is_alphanumeric()) {
                let mut found = None;
                for pre in ["https://", "http://", "ftp://", "mailto:"] {
                    if rest.starts_with(pre) {
                        let mut e = rest
                            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
                            .unwrap_or(rest.len());
                        while e > pre.len()
                            && rest[..e].ends_with(['.', ',', ';', ':', '!', '?', ')', '\''])
                        {
                            e -= 1;
                        }
                        if e > pre.len() {
                            found = Some(e);
                        }
                        break;
                    }
                }
                if let Some(e) = found {
                    flush!();
                    out.push(Inline::Link {
                        target: rest[..e].to_string(),
                        desc: None,
                    });
                    i += e;
                    continue;
                }
            }
            text.push(ch);
            i += ch.len_utf8();
        }
        flush!();
        out
    }
}

/// Where a ` ::` separating an item's tag from its text is.
fn find_tag_sep(t: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(p) = t[from..].find("::") {
        let at = from + p;
        let before_ok = at > 0 && t[..at].ends_with(char::is_whitespace);
        let after_ok = t[at + 2..].is_empty() || t[at + 2..].starts_with(char::is_whitespace);
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + 2;
    }
    None
}

/// The byte length of the balanced group starting at `s[0]`.
fn balanced(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + c.len_utf8());
            }
        }
    }
    None
}

/// `[[target][description]]`: (length, target, description).
fn parse_link(s: &str) -> Option<(usize, String, Option<String>)> {
    let t_end = s[2..].find(']')? + 2;
    let target = &s[2..t_end];
    if target.is_empty() || target.contains('[') {
        return None;
    }
    let rest = &s[t_end..];
    if rest.starts_with("]]") {
        return Some((t_end + 2, unescape_target(target), None));
    }
    if rest.starts_with("][") {
        // The description runs to the `]]` that closes this link; nested
        // `[[...]]` (an image in the description) are skipped.
        let d0 = t_end + 2;
        let mut depth = 0;
        let b = s.as_bytes();
        let mut k = d0;
        while k + 1 < b.len() {
            if b[k] == b'[' && b[k + 1] == b'[' {
                depth += 1;
                k += 2;
            } else if b[k] == b']' && b[k + 1] == b']' {
                if depth == 0 {
                    return Some((k + 2, unescape_target(target), Some(s[d0..k].to_string())));
                }
                depth -= 1;
                k += 2;
            } else {
                k += 1;
            }
        }
    }
    None
}

fn unescape_target(t: &str) -> String {
    t.replace("\\[", "[").replace("\\]", "]")
}

/// `\( .. \)`, `\[ .. \]` or `\begin{x} .. \end{x}` at the start of `s`:
/// (length, display math).
fn latex_delim(s: &str) -> Option<(usize, bool)> {
    if s.starts_with("\\(") {
        let e = s.find("\\)")?;
        return Some((e + 2, false));
    }
    if s.starts_with("\\[") {
        let e = s.find("\\]")?;
        return Some((e + 2, true));
    }
    if let Some(r) = s.strip_prefix("\\begin{") {
        let n = r.find('}')?;
        let close = format!("\\end{{{}}}", &r[..n]);
        let e = s.find(&close)?;
        return Some((e + close.len(), true));
    }
    None
}

/// `$x$` or `$$x$$` at `s[i..]`.
fn dollar_math(s: &str, i: usize) -> Option<(usize, bool)> {
    let rest = &s[i..];
    let prev = s[..i].chars().next_back();
    if prev == Some('$') || prev == Some('\\') {
        return None;
    }
    if let Some(r) = rest.strip_prefix("$$") {
        let e = r.find("$$")?;
        return Some((e + 4, true));
    }
    let r = &rest[1..];
    let first = r.chars().next()?;
    if first.is_whitespace() || first == '$' || matches!(first, '.' | ',' | '?' | ';' | '"') {
        return None;
    }
    let mut prev_c = first;
    for (k, c) in r.char_indices() {
        if c == '\n' && r[..k].matches('\n').count() >= 2 {
            return None;
        }
        if c == '$' && k > 0 {
            let after = r[k + 1..].chars().next();
            let ok_after = after.is_none_or(|a| !a.is_alphanumeric() || false)
                && after.is_none_or(|a| !a.is_ascii_digit());
            if !prev_c.is_whitespace() && !matches!(prev_c, '.' | ',') && ok_after {
                return Some((k + 2, false));
            }
            if prev_c.is_whitespace() {
                return None;
            }
        }
        prev_c = c;
    }
    None
}

/// Emphasis `*like this*` at `s[i..]`: (end offset in `s`, inner text).
fn markup(s: &str, i: usize, m: char) -> Option<(usize, &str)> {
    let prev = s[..i].chars().next_back();
    let pre_ok = match prev {
        None => true,
        Some(p) => {
            p.is_whitespace() || matches!(p, '-' | '(' | '{' | '\'' | '"' | '[' | '\u{2014}')
        }
    };
    if !pre_ok {
        return None;
    }
    let body = &s[i + 1..];
    let first = body.chars().next()?;
    if first.is_whitespace() || first == m {
        return None;
    }
    let mut last: Option<char> = None;
    for (k, c) in body.char_indices() {
        if c == m && k > 0 {
            let before = last?;
            let after = body[k + 1..].chars().next();
            let post_ok = after.is_none_or(|a| {
                a.is_whitespace()
                    || matches!(
                        a,
                        '-' | '.'
                            | ','
                            | ':'
                            | '!'
                            | '?'
                            | ';'
                            | '\''
                            | '"'
                            | ')'
                            | '}'
                            | '['
                            | ']'
                            | '\u{2014}'
                    )
            });
            if !before.is_whitespace() && post_ok {
                return Some((i + 1 + k + 1, &body[..k]));
            }
        }
        if c == '\n' && body[..k].matches('\n').count() >= 1 {
            return None;
        }
        last = Some(c);
    }
    None
}

/// A sub/superscript at `s` (starting at `_` or `^`): (length, inner).
fn script(s: &str, braces_only: bool) -> Option<(usize, &str)> {
    let r = &s[1..];
    if r.starts_with('{') {
        let n = balanced(r, '{', '}')?;
        return Some((1 + n, &r[1..n - 1]));
    }
    if braces_only {
        return None;
    }
    if r.starts_with('*') {
        return Some((2, "*"));
    }
    let sign = if r.starts_with(['+', '-']) { 1 } else { 0 };
    let n = r[sign..]
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .map(|c| c.len_utf8())
        .sum::<usize>();
    // The last character before a dot or comma ends the run.
    (n > 0).then(|| (1 + sign + n, &r[..sign + n]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(t: &str) -> Doc {
        parse(t, &|_| None)
    }

    fn paras(d: &Doc) -> Vec<Vec<Inline>> {
        d.nodes
            .iter()
            .filter_map(|n| {
                if let Block::Para(p) = &n.block {
                    Some(p.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn meta_and_options() {
        let d = doc("#+TITLE: Hello\n#+AUTHOR: Me\n#+OPTIONS: toc:nil num:nil H:2 ^:{}\n\nText.");
        assert_eq!(d.meta.title, "Hello");
        assert_eq!(d.meta.author, "Me");
        assert_eq!(d.meta.options.toc, None);
        assert!(!d.meta.options.num);
        assert_eq!(d.meta.options.headline_levels, 2);
        assert_eq!(d.meta.options.sub_super, SubSuper::Braces);
        assert_eq!(d.nodes.len(), 1);
    }

    #[test]
    fn headings_with_everything() {
        let d = doc(
            "* TODO [#A] Title :a:b:\nSCHEDULED: <2024-01-02 Tue>\n:PROPERTIES:\n:ID: x1\n:END:\nBody",
        );
        let Block::Heading(h) = &d.nodes[0].block else {
            panic!()
        };
        assert_eq!(h.todo.as_deref(), Some("TODO"));
        assert_eq!(h.priority, Some('A'));
        assert_eq!(h.tags, vec!["a", "b"]);
        assert_eq!(h.prop("ID"), Some("x1"));
        assert_eq!(h.planning[0].0, "SCHEDULED");
        assert_eq!(plain(&h.title), "Title");
        assert!(matches!(d.nodes[1].block, Block::Para(_)));
    }

    #[test]
    fn noexport_and_comment_headings_drop() {
        let d = doc(
            "* Keep\ntext\n* Gone :noexport:\nsecret\n** inner\n* COMMENT Also gone\nx\n* Back\n",
        );
        let titles: Vec<String> = d
            .nodes
            .iter()
            .filter_map(|n| {
                if let Block::Heading(h) = &n.block {
                    Some(h.raw_title.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(titles, vec!["Keep", "Back"]);
        assert!(!format!("{:?}", d.nodes).contains("secret"));
    }

    #[test]
    fn inline_markup() {
        let d = doc("a *bold* /it/ _u_ +s+ =v= ~c~ end");
        let p = &paras(&d)[0];
        assert!(p.iter().any(|i| matches!(i, Inline::Bold(_))));
        assert!(p.iter().any(|i| matches!(i, Inline::Italic(_))));
        assert!(p.iter().any(|i| matches!(i, Inline::Underline(_))));
        assert!(p.iter().any(|i| matches!(i, Inline::Strike(_))));
        assert!(
            p.iter()
                .any(|i| matches!(i, Inline::Verbatim(v) if v == "v"))
        );
        assert!(p.iter().any(|i| matches!(i, Inline::Code(v) if v == "c")));
        // Not markup: stars in arithmetic, a path with slashes.
        let d = doc("2 * 3 * 4 and /usr/bin/ls");
        assert_eq!(plain(&paras(&d)[0]), "2 * 3 * 4 and /usr/bin/ls");
    }

    #[test]
    fn links_and_urls() {
        let d = doc(
            "[[https://x.org][site *x*]] and [[file:a.png]] and https://y.org/p. <https://z.org>",
        );
        let p = &paras(&d)[0];
        let links: Vec<&Inline> = p
            .iter()
            .filter(|i| matches!(i, Inline::Link { .. }))
            .collect();
        assert_eq!(links.len(), 4);
        assert!(
            matches!(links[0], Inline::Link { target, desc: Some(_) } if target == "https://x.org")
        );
        assert!(matches!(links[2], Inline::Link { target, .. } if target == "https://y.org/p"));
        // An image inside a link's description.
        let d = doc("[[https://x.org][[[file:i.png]]]]");
        assert!(matches!(
            &paras(&d)[0][0],
            Inline::Link { desc: Some(_), .. }
        ));
    }

    #[test]
    fn math_and_entities() {
        let d = doc(r"Cost $5 and $6, but $x^2$ and \(a+b\) and \alpha\to b");
        let p = &paras(&d)[0];
        let maths: Vec<&Inline> = p
            .iter()
            .filter(|i| matches!(i, Inline::Latex { .. }))
            .collect();
        assert_eq!(maths.len(), 2);
        assert_eq!(
            p.iter().filter(|i| matches!(i, Inline::Entity(_))).count(),
            2
        );
        let d = doc("\\begin{equation}\nx=1\n\\end{equation}");
        assert!(matches!(d.nodes[0].block, Block::LatexEnv(_)));
    }

    #[test]
    fn scripts() {
        let d = doc("H_2O and x^2 and a_{bc} and snake_case");
        let p = &paras(&d)[0];
        assert_eq!(p.iter().filter(|i| matches!(i, Inline::Sub(_))).count(), 3);
        assert_eq!(p.iter().filter(|i| matches!(i, Inline::Sup(_))).count(), 1);
        let d = doc("#+OPTIONS: ^:{}\nsnake_case a_{b}");
        let p = &paras(&d)[0];
        assert_eq!(p.iter().filter(|i| matches!(i, Inline::Sub(_))).count(), 1);
    }

    #[test]
    fn lists_nest_and_describe() {
        let d = doc("- a\n  - a1\n  - a2\n- b [X]\n\n1. one\n2. two\n\n- term :: desc\n- [ ] todo");
        let lists: Vec<&List> = d
            .nodes
            .iter()
            .filter_map(|n| {
                if let Block::List(l) = &n.block {
                    Some(l)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(lists.len(), 3, "{:#?}", d.nodes);
        assert_eq!(lists[0].items.len(), 2);
        assert!(
            lists[0].items[0]
                .content
                .iter()
                .any(|n| matches!(n.block, Block::List(_)))
        );
        assert!(lists[1].ordered);
        assert!(lists[2].descriptive);
        assert_eq!(lists[2].items[1].checkbox, Some(' '));
    }

    #[test]
    fn tables_get_header_and_alignment() {
        let d =
            doc("| Name | Qty |\n|------+-----|\n| a    |  10 |\n| b    |   2 |\n#+TBLFM: $2=1");
        let Block::Table(t) = &d.nodes[0].block else {
            panic!()
        };
        assert!(t.has_header);
        assert_eq!(t.align, vec!['l', 'r']);
        assert_eq!(t.rows.len(), 4);
        let d = doc("| <c> | <r> |\n| a | b |");
        let Block::Table(t) = &d.nodes[0].block else {
            panic!()
        };
        assert_eq!(t.align, vec!['c', 'r']);
        assert_eq!(t.rows.len(), 1);
    }

    #[test]
    fn blocks_and_affiliated() {
        let d = doc(
            "#+CAPTION: Cap\n#+NAME: n1\n#+ATTR_HTML: :width 50%\n#+BEGIN_SRC rust -n\nfn main() {}\n,* not heading\n#+END_SRC\n#+BEGIN_QUOTE\nQ\n#+END_QUOTE\n#+BEGIN_EXPORT html\n<b>x</b>\n#+END_EXPORT\n: fixed\n-----\n#+BEGIN_COMMENT\nno\n#+END_COMMENT",
        );
        let Block::Src { lang, params, text } = &d.nodes[0].block else {
            panic!("{:?}", d.nodes)
        };
        assert_eq!((lang.as_str(), params.as_str()), ("rust", "-n"));
        assert_eq!(text, "fn main() {}\n* not heading");
        assert_eq!(d.nodes[0].aff.caption.as_deref(), Some("Cap"));
        assert_eq!(d.nodes[0].aff.name.as_deref(), Some("n1"));
        assert_eq!(d.nodes[0].aff.attr("html", "width").as_deref(), Some("50%"));
        assert!(matches!(d.nodes[1].block, Block::Quote(_)));
        assert!(matches!(&d.nodes[2].block, Block::Export { backend, .. } if backend == "html"));
        assert!(matches!(d.nodes[3].block, Block::Example(_)));
        assert!(matches!(d.nodes[4].block, Block::Rule));
        assert_eq!(d.nodes.len(), 5);
    }

    #[test]
    fn footnotes_are_numbered_by_reference() {
        let d = doc(
            "Text[fn:b] more[fn:a] again[fn:b] inline[fn::anon def] and[fn:c:named inline].\n\n[fn:a] Def A.\n[fn:b] Def B.\n",
        );
        assert_eq!(d.footnotes.len(), 4);
        assert_eq!(d.footnotes[0].label, "b");
        assert_eq!(d.footnotes[1].label, "a");
        assert_eq!(d.footnotes[0].number, 1);
        let p = &paras(&d)[0];
        let nums: Vec<usize> = p
            .iter()
            .filter_map(|i| {
                if let Inline::FnRef { number, .. } = i {
                    Some(*number)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(nums, vec![1, 2, 1, 3, 4]);
        assert_eq!(
            plain(&match &d.footnotes[0].content[0].block {
                Block::Para(p) => p.clone(),
                _ => vec![],
            }),
            "Def B."
        );
        assert_eq!(
            plain(&match &d.footnotes[2].content[0].block {
                Block::Para(p) => p.clone(),
                _ => vec![],
            }),
            "anon def"
        );
    }

    #[test]
    fn citations_collect_keys() {
        let d = doc("#+BIBLIOGRAPHY: refs.bib\nSee [cite:@a;@b] and [cite/t:@a].");
        assert_eq!(d.cited, vec!["a", "b"]);
        assert_eq!(d.meta.bibliography, vec!["refs.bib"]);
        let d = parse("x [cite:@smith2020]\n#+BIBLIOGRAPHY: r.bib", &|f| {
            (f == "r.bib")
                .then(|| "@book{smith2020, author={Smith, A}, title={T}, year={2020}}".to_string())
        });
        assert_eq!(d.bib.len(), 1);
    }

    #[test]
    fn macros_and_includes() {
        let d = parse(
            "#+TITLE: T\n#+MACRO: hi Hello $1!\n{{{hi(World)}}} by {{{title}}}\n\n#+INCLUDE: \"inc.org\"\n\n#+INCLUDE: \"c.rs\" src rust",
            &|f| match f {
                "inc.org" => Some("included text".into()),
                "c.rs" => Some("let x = 1;".into()),
                _ => None,
            },
        );
        assert_eq!(plain(&paras(&d)[0]), "Hello World! by T");
        assert_eq!(plain(&paras(&d)[1]), "included text");
        assert!(matches!(&d.nodes[2].block, Block::Src { lang, .. } if lang == "rust"));
    }

    #[test]
    fn timestamps_cookies_breaks() {
        let d = doc("Due <2024-01-02 Tue> or [2024-01-03 Wed] [1/3] [50%]\\\\\nnext");
        let p = &paras(&d)[0];
        assert!(
            p.iter()
                .any(|i| matches!(i, Inline::Timestamp { active: true, .. }))
        );
        assert!(
            p.iter()
                .any(|i| matches!(i, Inline::Timestamp { active: false, .. }))
        );
        assert_eq!(
            p.iter().filter(|i| matches!(i, Inline::Cookie(_))).count(),
            2
        );
        assert!(p.iter().any(|i| matches!(i, Inline::Break)));
    }
}
