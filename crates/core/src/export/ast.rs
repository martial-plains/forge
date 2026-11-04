// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tree an Org buffer is parsed into before export.

use super::cite::{BibEntry, Cite};

/// Settings from `#+OPTIONS:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Table of contents depth (`toc:3`); None for none.
    pub toc: Option<usize>,
    /// Number the headings (`num:t`).
    pub num: bool,
    /// Headlines deeper than this become list items (`H:3`).
    pub headline_levels: usize,
    /// `H:` was given.
    pub headline_levels_set: bool,
    pub author: bool,
    pub date: bool,
    pub email: bool,
    pub title: bool,
    pub tags: bool,
    pub todo: bool,
    pub priority: bool,
    pub footnotes: bool,
    pub timestamps: bool,
    pub special_strings: bool,
    pub entities: bool,
    pub emphasis: bool,
    pub latex: bool,
    /// `^:t` (any `_x` is a subscript), `^:{}` (only `_{x}`), `^:nil`.
    pub sub_super: SubSuper,
    pub drawers: bool,
    pub inline_tasks: bool,
    pub stat: bool,
    pub creator: bool,
    pub broken_links: bool,
    pub section_numbers_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubSuper {
    Off,
    Braces,
    On,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            toc: Some(usize::MAX),
            num: true,
            headline_levels: 3,
            headline_levels_set: false,
            author: true,
            date: true,
            email: false,
            title: true,
            tags: true,
            todo: true,
            priority: false,
            footnotes: true,
            timestamps: true,
            special_strings: true,
            entities: true,
            emphasis: true,
            latex: true,
            sub_super: SubSuper::On,
            drawers: true,
            inline_tasks: true,
            stat: true,
            creator: false,
            broken_links: false,
            section_numbers_depth: usize::MAX,
        }
    }
}

impl Options {
    /// Apply the words of an `#+OPTIONS:` line.
    pub fn apply(&mut self, line: &str) {
        for w in split_options(line) {
            let Some((k, v)) = w.split_once(':') else {
                continue;
            };
            let on = !(v == "nil" || v == "no");
            let num = v.parse::<usize>().ok();
            match k {
                "toc" => {
                    self.toc = if v == "nil" {
                        None
                    } else {
                        Some(num.unwrap_or(usize::MAX))
                    }
                }
                "num" => {
                    self.num = on;
                    if let Some(n) = num {
                        self.section_numbers_depth = n;
                        self.num = n > 0;
                    }
                }
                "H" => {
                    self.headline_levels = num.unwrap_or(3);
                    self.headline_levels_set = num.is_some();
                }
                "author" => self.author = on,
                "date" => self.date = on,
                "email" => self.email = on,
                "title" => self.title = on,
                "tags" => self.tags = on,
                "todo" => self.todo = on,
                "pri" => self.priority = on,
                "f" => self.footnotes = on,
                "timestamp" => self.timestamps = on,
                "-" => self.special_strings = on,
                "e" => self.entities = on,
                "*" => self.emphasis = on,
                "tex" => self.latex = on,
                "^" => {
                    self.sub_super = match v {
                        "nil" | "no" => SubSuper::Off,
                        "{}" => SubSuper::Braces,
                        _ => SubSuper::On,
                    }
                }
                "d" => self.drawers = on,
                "inline" => self.inline_tasks = on,
                "stat" => self.stat = on,
                "creator" => self.creator = on && v != "comment",
                "broken-links" => self.broken_links = on,
                _ => {}
            }
        }
    }
}

/// Words of an options line; a `(...)` or `"..."` value stays in one.
fn split_options(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut quote = false;
    for c in s.chars() {
        match c {
            '(' if !quote => depth += 1,
            ')' if !quote && depth > 0 => depth -= 1,
            '"' => quote = !quote,
            c if c.is_whitespace() && depth == 0 && !quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What the file says about itself.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub title: String,
    pub subtitle: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub language: String,
    pub description: String,
    pub keywords: String,
    pub options: Options,
    /// `#+LATEX_CLASS`, `#+LATEX_CLASS_OPTIONS`, `#+LATEX_HEADER` lines.
    pub latex_class: String,
    pub latex_class_options: String,
    pub latex_headers: Vec<String>,
    pub html_head: Vec<String>,
    pub html_link_home: String,
    pub html_link_up: String,
    pub beamer_theme: String,
    pub beamer_color_theme: String,
    pub beamer_font_theme: String,
    pub beamer_header: Vec<String>,
    pub beamer_frame_level: Option<usize>,
    pub bibliography: Vec<String>,
    pub cite_export: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub select_tags: Vec<String>,
    pub texinfo_filename: String,
    pub texinfo_class: String,
    pub texinfo_dir_category: String,
    pub texinfo_dir_title: String,
    pub texinfo_dir_desc: String,
    pub odt_styles_file: String,
    pub ical_calendar: String,
    pub export_file_name: String,
    pub macros: Vec<(String, String)>,
}

/// Affiliated keywords: `#+CAPTION`, `#+NAME`, `#+ATTR_x`, `#+RESULTS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aff {
    pub name: Option<String>,
    pub caption: Option<String>,
    pub short_caption: Option<String>,
    /// (backend, value).
    pub attrs: Vec<(String, String)>,
}

impl Aff {
    /// The `:key value` pairs of `#+ATTR_<backend>`.
    pub fn attr(&self, backend: &str, key: &str) -> Option<String> {
        for (b, v) in &self.attrs {
            if b.eq_ignore_ascii_case(backend) {
                let w: Vec<&str> = v.split_whitespace().collect();
                let k = format!(":{key}");
                if let Some(i) = w.iter().position(|x| *x == k) {
                    // The value runs to the next `:key`.
                    let mut val = Vec::new();
                    for x in &w[i + 1..] {
                        if x.starts_with(':') && x.len() > 1 {
                            break;
                        }
                        val.push(*x);
                    }
                    return Some(val.join(" "));
                }
            }
        }
        None
    }

    /// The raw `#+ATTR_<backend>` lines joined.
    pub fn attr_raw(&self, backend: &str) -> String {
        self.attrs
            .iter()
            .filter(|(b, _)| b.eq_ignore_ascii_case(backend))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Underline(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Verbatim(String),
    Link {
        target: String,
        desc: Option<Vec<Inline>>,
    },
    /// A footnote reference. `number` is its place among the document's
    /// footnotes, from 1.
    FnRef {
        number: usize,
        label: String,
    },
    Cite(Cite),
    Latex {
        text: String,
        display: bool,
    },
    Entity(String),
    Timestamp {
        text: String,
        active: bool,
    },
    Sub(Vec<Inline>),
    Sup(Vec<Inline>),
    Break,
    Target(String),
    Cookie(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    pub todo: Option<String>,
    pub done: bool,
    pub priority: Option<char>,
    pub commented: bool,
    pub raw_title: String,
    pub title: Vec<Inline>,
    pub tags: Vec<String>,
    pub props: Vec<(String, String)>,
    /// ("SCHEDULED" | "DEADLINE" | "CLOSED", timestamp text).
    pub planning: Vec<(String, String)>,
    pub line: usize,
}

impl Heading {
    pub fn prop(&self, k: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub bullet: String,
    pub counter: Option<usize>,
    /// ' ', 'X' or '-'.
    pub checkbox: Option<char>,
    pub tag: Option<Vec<Inline>>,
    pub content: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub ordered: bool,
    pub descriptive: bool,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Rule,
    Cells(Vec<Vec<Inline>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Row>,
    /// Rows before the first rule are the header (if rows follow).
    pub has_header: bool,
    /// 'l', 'r' or 'c' per column.
    pub align: Vec<char>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(Heading),
    Para(Vec<Inline>),
    List(List),
    Table(Table),
    Src {
        lang: String,
        params: String,
        text: String,
    },
    Example(String),
    Verse(String),
    Quote(Vec<Node>),
    Center(Vec<Node>),
    /// `#+BEGIN_name` with some other name.
    Special {
        name: String,
        params: String,
        content: Vec<Node>,
    },
    Export {
        backend: String,
        text: String,
    },
    Rule,
    LatexEnv(String),
    Drawer {
        name: String,
        content: Vec<Node>,
    },
    /// `#+RESULTS:`-style results are plain blocks; keyword lines that no
    /// backend cares about don't appear.
    Keyword {
        key: String,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub aff: Aff,
    pub block: Block,
}

impl Node {
    pub fn new(block: Block) -> Self {
        Node {
            aff: Aff::default(),
            block,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footnote {
    pub number: usize,
    pub label: String,
    pub content: Vec<Node>,
}

/// A parsed document.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    pub meta: Meta,
    pub nodes: Vec<Node>,
    pub footnotes: Vec<Footnote>,
    pub bib: Vec<BibEntry>,
    /// Keys cited, in order of first citation.
    pub cited: Vec<String>,
    /// Headings the export skipped are not in `nodes`.
    pub warnings: Vec<String>,
}

impl Doc {
    /// Section numbers for the headings in `nodes`, in order: `[1]`, `[1,1]`.
    pub fn heading_numbers(&self) -> Vec<Vec<usize>> {
        let mut cur: Vec<usize> = Vec::new();
        let mut out = Vec::new();
        let base = self.min_level();
        for n in &self.nodes {
            if let Block::Heading(h) = &n.block {
                let depth = h.level.saturating_sub(base) + 1;
                cur.truncate(depth);
                while cur.len() < depth {
                    cur.push(0);
                }
                cur[depth - 1] += 1;
                out.push(cur.clone());
            }
        }
        out
    }

    /// The shallowest heading level present.
    pub fn min_level(&self) -> usize {
        self.nodes
            .iter()
            .filter_map(|n| {
                if let Block::Heading(h) = &n.block {
                    Some(h.level)
                } else {
                    None
                }
            })
            .min()
            .unwrap_or(1)
    }
}

/// The plain text of inline objects (no markup).
pub fn plain(inl: &[Inline]) -> String {
    let mut s = String::new();
    for i in inl {
        match i {
            Inline::Text(t) | Inline::Code(t) | Inline::Verbatim(t) => s.push_str(t),
            Inline::Bold(v)
            | Inline::Italic(v)
            | Inline::Underline(v)
            | Inline::Strike(v)
            | Inline::Sub(v)
            | Inline::Sup(v) => s.push_str(&plain(v)),
            Inline::Link { target, desc } => match desc {
                Some(d) => s.push_str(&plain(d)),
                None => s.push_str(target),
            },
            Inline::Latex { text, .. } => s.push_str(text),
            Inline::Entity(n) => s.push_str(super::entities::find(n).map(|e| e.4).unwrap_or("")),
            Inline::Timestamp { text, .. } => s.push_str(text),
            Inline::Break => s.push('\n'),
            Inline::Cookie(c) => s.push_str(c),
            Inline::FnRef { .. } | Inline::Cite(_) | Inline::Target(_) => {}
        }
    }
    s
}
