// SPDX-License-Identifier: MIT OR Apache-2.0
//! few: a small text web browser's page engine. It turns HTML into
//! wrapped plain text with link positions, resolves URLs and decides what
//! a typed address means. No network here; the editor fetches.

use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub line: usize,
    /// Character columns, end exclusive.
    pub start: usize,
    pub end: usize,
    pub href: String,
}

/// A heading for the table of contents: 1-6, the buffer line it starts
/// on (page text, before the few header), and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: usize,
    pub line: usize,
    pub text: String,
}

/// What a form control is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Password,
    Hidden,
    Checkbox,
    Radio,
    Submit,
    Button,
    TextArea,
    Select,
}

/// One form control. Spans live in [`Page::field_spans`]: a wrapped
/// label can cover several visual lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The form it submits with, if any.
    pub form: Option<usize>,
    pub kind: FieldKind,
    pub name: String,
    /// Text fields and areas: the current value. Submit/buttons: the
    /// label. Selects: unused (see `options`/`selected`).
    pub value: String,
    /// Checkboxes and radios.
    pub checked: bool,
    /// Selects: (shown label, submitted value).
    pub options: Vec<(String, String)>,
    /// Selects: which option is chosen.
    pub selected: usize,
}

/// One `<form>`: where it goes, how, and which fields it owns.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    /// Resolved against the page.
    pub action: String,
    /// `get` or `post` (anything else behaves as get).
    pub method: String,
    /// Indexes into [`Page::fields`].
    pub fields: Vec<usize>,
}

/// One visual line of a field's label: which field, in character columns.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpan {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub field: usize,
}

/// One `<img>`: alt text and resolved address. Spans live in
/// [`Page::image_spans`].
#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    pub alt: String,
    pub src: String,
}

/// One visual line of an image's `[alt]` label.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSpan {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub image: usize,
}

/// Editable values overlaid on a fresh render: field order is
/// deterministic per source, so indexes survive re-renders (resizes,
/// readable toggles).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldState {
    pub values: Vec<String>,
    pub checked: Vec<bool>,
    pub selected: Vec<usize>,
}

impl FieldState {
    /// Remember every field's current value.
    pub fn capture(page: &Page) -> Self {
        FieldState {
            values: page.fields.iter().map(|f| f.value.clone()).collect(),
            checked: page.fields.iter().map(|f| f.checked).collect(),
            selected: page.fields.iter().map(|f| f.selected).collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub title: String,
    pub lines: Vec<String>,
    pub links: Vec<Link>,
    pub headings: Vec<Heading>,
    pub fields: Vec<Field>,
    pub forms: Vec<Form>,
    pub field_spans: Vec<FieldSpan>,
    pub images: Vec<PageImage>,
    pub image_spans: Vec<ImageSpan>,
}

// ------------------------------------------------------------------ URLs

/// What the user typed: a URL, a file path, or search words.
pub fn normalize_input(s: &str) -> String {
    normalize_input_with(s, &search_url)
}

/// The search engines Forge knows by name: name, the address for a
/// browser that runs scripts, the address for few (which doesn't).
/// `%s` is where the words go.
pub const ENGINES: &[(&str, &str, &str)] = &[
    (
        "duckduckgo",
        "https://duckduckgo.com/?q=%s",
        "https://html.duckduckgo.com/html/?q=%s",
    ),
    (
        "google",
        "https://www.google.com/search?q=%s",
        "https://www.google.com/search?q=%s",
    ),
    (
        "bing",
        "https://www.bing.com/search?q=%s",
        "https://www.bing.com/search?q=%s",
    ),
    (
        "brave",
        "https://search.brave.com/search?q=%s",
        "https://search.brave.com/search?q=%s",
    ),
    (
        "startpage",
        "https://www.startpage.com/do/search?q=%s",
        "https://www.startpage.com/do/search?q=%s",
    ),
    (
        "mojeek",
        "https://www.mojeek.com/search?q=%s",
        "https://www.mojeek.com/search?q=%s",
    ),
    (
        "ecosia",
        "https://www.ecosia.org/search?q=%s",
        "https://www.ecosia.org/search?q=%s",
    ),
    (
        "kagi",
        "https://kagi.com/search?q=%s",
        "https://kagi.com/search?q=%s",
    ),
    (
        "wikipedia",
        "https://en.wikipedia.org/w/index.php?search=%s",
        "https://en.wikipedia.org/w/index.php?search=%s",
    ),
    (
        "github",
        "https://github.com/search?q=%s",
        "https://github.com/search?q=%s",
    ),
];

/// The address template for an engine called `spec` (a name above, or a
/// template of your own with `%s`, or an address the words are added to).
/// None if it is neither.
pub fn engine_template(spec: &str, text_browser: bool) -> Option<String> {
    let spec = spec.trim();
    if let Some((_, web, text)) = ENGINES.iter().find(|(n, ..)| n.eq_ignore_ascii_case(spec)) {
        return Some((if text_browser { text } else { web }).to_string());
    }
    (spec.starts_with("http://") || spec.starts_with("https://")).then(|| spec.to_string())
}

/// `template` with the words in it: at `%s`, or on the end.
pub fn search_with(template: &str, q: &str) -> String {
    if template.contains("%s") {
        template.replacen("%s", &percent_encode(q), 1)
    } else {
        format!("{template}{}", percent_encode(q))
    }
}

/// `normalize_input`, searching with `search` when it's words.
pub fn normalize_input_with(s: &str, search: &dyn Fn(&str) -> String) -> String {
    let s = s.trim();
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("file://") {
        return s.to_string();
    }
    if s.starts_with('/') || s.starts_with("~/") || s.starts_with("./") {
        return s.to_string();
    }
    let looks_like_host = !s.contains(char::is_whitespace)
        && (s.contains('.') || s.starts_with("localhost"))
        && !s.ends_with('.');
    if looks_like_host {
        format!("https://{s}")
    } else {
        search(s)
    }
}

pub fn search_url(q: &str) -> String {
    format!("https://html.duckduckgo.com/html/?q={}", percent_encode(q))
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

struct Parts<'a> {
    scheme: &'a str,
    host: &'a str,
    path: &'a str,
}

fn split_url(u: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = u.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(end);
    Some(Parts {
        scheme,
        host,
        path: tail,
    })
}

/// `href` as seen from the page at `base`.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.contains("://")
        || href.starts_with("mailto:")
        || href.starts_with("javascript:")
        || href.starts_with("data:")
    {
        return href.to_string();
    }
    let Some(p) = split_url(base) else {
        return href.to_string();
    };
    if let Some(r) = href.strip_prefix("//") {
        return format!("{}://{r}", p.scheme);
    }
    let origin = format!("{}://{}", p.scheme, p.host);
    let path_only = p.path.split(['?', '#']).next().unwrap_or("");
    if href.starts_with('#') {
        return format!(
            "{origin}{path_only}{}{href}",
            p.path.find('?').map_or(String::new(), |i| p.path[i..]
                .split('#')
                .next()
                .unwrap_or("")
                .to_string())
        );
    }
    if href.starts_with('?') {
        return format!("{origin}{path_only}{href}");
    }
    let (rel, suffix) = match href.find(['?', '#']) {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    };
    let mut segs: Vec<&str> = if rel.starts_with('/') {
        Vec::new()
    } else {
        let dir = path_only.rsplit_once('/').map_or("", |(d, _)| d);
        dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for s in rel.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            x => segs.push(x),
        }
    }
    let trailing = rel.ends_with('/') || rel.ends_with("/.") || rel.ends_with("/..");
    let mut path = format!("/{}", segs.join("/"));
    if trailing && !path.ends_with('/') {
        path.push('/');
    }
    format!("{origin}{path}{suffix}")
}

// -------------------------------------------------------------- entities

fn entity(name: &str) -> Option<String> {
    let s = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "laquo" => "«",
        "raquo" => "»",
        "ldquo" => "“",
        "rdquo" => "”",
        "lsquo" => "‘",
        "rsquo" => "’",
        "bull" => "•",
        "middot" => "·",
        "times" => "×",
        "deg" => "°",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "rarr" => "→",
        "larr" => "←",
        "uarr" => "↑",
        "darr" => "↓",
        "sect" => "§",
        "para" => "¶",
        "frac12" => "½",
        _ => {
            let n = name.strip_prefix('#')?;
            let code = if let Some(h) = n.strip_prefix(['x', 'X']) {
                u32::from_str_radix(h, 16).ok()?
            } else {
                n.parse().ok()?
            };
            return char::from_u32(code).map(String::from);
        }
    };
    Some(s.to_string())
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|e| e + 1)
            .unwrap_or(rest.len());
        let name = &rest[1..end];
        match entity(name) {
            Some(r) if !name.is_empty() => {
                out.push_str(&r);
                rest = if rest[end..].starts_with(';') {
                    &rest[end + 1..]
                } else {
                    &rest[end..]
                };
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// --------------------------------------------------------------- tokens

enum Tok {
    Text(String),
    Open(String, BTreeMap<String, String>, bool),
    Close(String),
}

fn attrs(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_whitespace() || b[i] == '/') {
            i += 1;
        }
        let ks = i;
        while i < b.len() && !b[i].is_whitespace() && b[i] != '=' && b[i] != '/' {
            i += 1;
        }
        if ks == i {
            i += 1;
            continue;
        }
        let key: String = b[ks..i].iter().collect::<String>().to_lowercase();
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == '=' {
            i += 1;
            while i < b.len() && b[i].is_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == '"' || b[i] == '\'') {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    val.push(b[i]);
                    i += 1;
                }
                i += 1;
            } else {
                while i < b.len() && !b[i].is_whitespace() {
                    val.push(b[i]);
                    i += 1;
                }
            }
        }
        out.insert(key, decode_entities(&val));
    }
    out
}

const SKIP: &[&str] = &[
    "script", "style", "noscript", "svg", "template", "iframe", "object", "canvas",
];

fn tokenize(html: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            toks.push(Tok::Text(decode_entities(rest)));
            break;
        };
        if lt > 0 {
            toks.push(Tok::Text(decode_entities(&rest[..lt])));
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        let next = rest[1..].chars().next();
        if !matches!(next, Some(c) if c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?') {
            toks.push(Tok::Text("<".into()));
            rest = &rest[1..];
            continue;
        }
        // Find the closing '>' outside quotes.
        let mut q: Option<char> = None;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            match (q, c) {
                (Some(x), c) if c == x => q = None,
                (None, '"' | '\'') => q = Some(c),
                (None, '>') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            toks.push(Tok::Close(name.trim().to_lowercase()));
            continue;
        }
        let name_end = inner
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_lowercase();
        let selfclose = inner.ends_with('/');
        toks.push(Tok::Open(
            name.clone(),
            attrs(&inner[name_end..]),
            selfclose,
        ));
        // Raw-text elements: skip to the matching close tag, scanning
        // case-insensitively without copying the rest of the page.
        if SKIP.contains(&name.as_str()) && !selfclose {
            match find_close_tag(rest, &name) {
                Some(p) => rest = &rest[p..],
                None => rest = "",
            }
        }
    }
    toks
}

/// Byte offset of `</name` in `hay`, matching ASCII case-insensitively.
/// A `to_lowercase` of the tail would copy megabytes on script-heavy
/// pages; this scans in place.
fn find_close_tag(hay: &str, name: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = name.as_bytes();
    if n.is_empty() {
        return None;
    }
    let mut i = 0;
    while i + 2 + n.len() <= h.len() {
        if h[i] == b'<' && h[i + 1] == b'/' {
            let mut ok = true;
            for (k, &b) in n.iter().enumerate() {
                if h[i + 2 + k].to_ascii_lowercase() != b {
                    ok = false;
                    break;
                }
            }
            if ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

// -------------------------------------------------------------- renderer

/// What a word belongs to: at most one of each, so links, fields and
/// image labels wrap across lines without losing their spans.
#[derive(Clone, Copy, Default, PartialEq)]
struct Ann {
    link: Option<usize>,
    field: Option<usize>,
    image: Option<usize>,
}

struct R<'a> {
    base: &'a str,
    width: usize,
    /// Edited values overlaid on this render (field order is
    /// deterministic, so indexes line up).
    init: &'a FieldState,
    lines: Vec<String>,
    links: Vec<Link>,
    hrefs: Vec<String>,
    words: Vec<(String, Ann)>,
    space: bool,
    pre: bool,
    /// Preformatted chunks with the annotation active for each, so links
    /// (and controls) in `<pre>` stay live.
    pre_runs: Vec<(String, Ann)>,
    quote: usize,
    indent: usize,
    bullet: Option<String>,
    lists: Vec<Option<usize>>,
    need_blank: bool,
    cur_link: Option<usize>,
    cur_field: Option<usize>,
    cur_image: Option<usize>,
    title: String,
    in_title: bool,
    skip_depth: usize,
    /// Emitted spans and records.
    field_spans: Vec<FieldSpan>,
    image_spans: Vec<ImageSpan>,
    fields: Vec<Field>,
    forms: Vec<Form>,
    images: Vec<PageImage>,
    /// The form new controls join, if any (nested forms join the outer).
    cur_form: Option<usize>,
    /// A `<textarea>` gathering its content: field index and text.
    textarea: Option<(usize, String)>,
    /// A `<button>` gathering its content: field index and text.
    button: Option<(usize, String)>,
    /// A `<select>` gathering options: field index, (label, value) pairs,
    /// which are marked selected, and the open option's (value, selected,
    /// text).
    select: Option<(
        usize,
        Vec<(String, String)>,
        Vec<bool>,
        Option<(Option<String>, bool, String)>,
    )>,
    /// Collected headings (level, page line, text) for the table of contents.
    headings: Vec<Heading>,
    /// An open heading: its level, the line count when it started, and
    /// its text so far.
    h_open: Option<(usize, usize)>,
    h_text: String,
    /// A table being buffered for column alignment; nested tables are
    /// flattened into the outer one.
    table: Option<Table>,
    /// Open nested `<table>` tags inside the buffered one: their closes
    /// must not emit the outer table.
    nested_tables: usize,
}

/// One table cell: a single unwrapped line, link/field/image spans in
/// character columns, and how many columns it spans.
struct TableCell {
    text: String,
    spans: Vec<(usize, usize, usize)>,
    field_spans: Vec<(usize, usize, usize)>,
    image_spans: Vec<(usize, usize, usize)>,
    header: bool,
    colspan: usize,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<TableCell>>,
    row: Vec<TableCell>,
    in_cell: bool,
    cell_header: bool,
    cell_colspan: usize,
}

fn block_tag(t: &str) -> bool {
    matches!(
        t,
        "p" | "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "aside"
            | "form"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "tr"
            | "blockquote"
            | "figure"
            | "figcaption"
            | "details"
            | "summary"
            | "fieldset"
            | "address"
            | "center"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "pre"
            | "hr"
            | "body"
            | "html"
            | "tbody"
            | "thead"
            | "tfoot"
            | "caption"
    )
}

/// Join wrapped words into one table-cell line: single spaces, no wrap.
/// Returns the text and link/field/image spans as (char start, char end,
/// index).
fn join_cell_words(
    words: Vec<(String, Ann)>,
) -> (
    String,
    Vec<(usize, usize, usize)>,
    Vec<(usize, usize, usize)>,
    Vec<(usize, usize, usize)>,
) {
    let mut text = String::new();
    let mut links: Vec<(usize, usize, usize)> = Vec::new();
    let mut fields: Vec<(usize, usize, usize)> = Vec::new();
    let mut images: Vec<(usize, usize, usize)> = Vec::new();
    let mut open_link: Option<(usize, usize)> = None;
    let mut open_field: Option<(usize, usize)> = None;
    let mut open_image: Option<(usize, usize)> = None;
    let mut col = 0usize;
    for (w, a) in words {
        if w == " " {
            continue;
        }
        if open_link.map(|(i, _)| i) != a.link
            && let Some((idx, s)) = open_link.take()
        {
            links.push((s, col, idx));
        }
        if open_field.map(|(i, _)| i) != a.field
            && let Some((idx, s)) = open_field.take()
        {
            fields.push((s, col, idx));
        }
        if open_image.map(|(i, _)| i) != a.image
            && let Some((idx, s)) = open_image.take()
        {
            images.push((s, col, idx));
        }
        if !text.is_empty() {
            text.push(' ');
            col += 1;
        }
        if open_link.is_none() {
            open_link = a.link.map(|n| (n, col));
        }
        if open_field.is_none() {
            open_field = a.field.map(|n| (n, col));
        }
        if open_image.is_none() {
            open_image = a.image.map(|n| (n, col));
        }
        text.push_str(&w);
        col += w.chars().count();
    }
    if let Some((idx, s)) = open_link.take() {
        links.push((s, col, idx));
    }
    if let Some((idx, s)) = open_field.take() {
        fields.push((s, col, idx));
    }
    if let Some((idx, s)) = open_image.take() {
        images.push((s, col, idx));
    }
    (text, links, fields, images)
}

impl<'a> R<'a> {
    fn prefix(&self) -> String {
        format!("{}{}", "│ ".repeat(self.quote), " ".repeat(self.indent))
    }

    fn push_line(&mut self, s: String) {
        if self.need_blank
            && !self.lines.is_empty()
            && !self.lines.last().is_some_and(|l| l.is_empty())
        {
            self.lines.push(String::new());
        }
        self.need_blank = false;
        self.lines.push(s);
    }

    fn cur_ann(&self) -> Ann {
        Ann {
            link: self.cur_link,
            field: self.cur_field,
            image: self.cur_image,
        }
    }

    fn add_word(&mut self, w: &str) {
        self.words.push((w.to_string(), self.cur_ann()));
    }

    fn text(&mut self, t: &str) {
        if self.skip_depth > 0 {
            return;
        }
        if self.in_title {
            self.title.push_str(t);
            return;
        }
        if self.h_open.is_some() {
            self.h_text.push_str(t);
        }
        // Textarea, button and option contents gather for their labels;
        // they don't flow into the page (except inside <pre>, the label
        // does when the element closes).
        if let Some((_, buf)) = self.textarea.as_mut() {
            buf.push_str(t);
            return;
        }
        if let Some((_, buf)) = self.button.as_mut() {
            buf.push_str(t);
            return;
        }
        if let Some((_, _, _, open)) = self.select.as_mut() {
            if let Some((_, _, text)) = open {
                text.push_str(t);
            }
            return;
        }
        if self.pre {
            self.pre_runs.push((t.to_string(), self.cur_ann()));
            return;
        }
        let t = t.replace('\u{a0}', " ");
        let starts = t.starts_with(char::is_whitespace);
        let ends = t.ends_with(char::is_whitespace);
        let mut first = true;
        for w in t.split_whitespace() {
            if first && starts && !self.words.is_empty() {
                self.space = true;
            }
            if !first {
                self.space = true;
            }
            if self.space && !self.words.is_empty() {
                let cur = self.cur_ann();
                let shared = self.words.last().map(|(_, a)| *a).filter(|a| *a == cur);
                self.words.push((" ".into(), shared.unwrap_or_default()));
            }
            self.space = false;
            self.add_word(w);
            first = false;
        }
        if first && starts && !self.words.is_empty() {
            self.space = true;
        }
        if ends && !first {
            self.space = true;
        }
        if self.space && !self.words.is_empty() && first {
            // whitespace-only text between inline elements
        }
    }

    fn flush(&mut self) {
        if self.pre && !self.pre_runs.is_empty() {
            if self.table.as_ref().is_some_and(|t| t.in_cell) {
                // Preformatted text in a cell degrades to words; the cell
                // is a single line when it closes.
                let runs = std::mem::take(&mut self.pre_runs);
                for (t, a) in runs {
                    for w in t.split_whitespace() {
                        if self.space && !self.words.is_empty() {
                            self.words.push((" ".into(), Ann::default()));
                        }
                        self.space = false;
                        self.words.push((w.to_string(), a));
                        self.space = true;
                    }
                }
                self.space = true;
            } else {
                self.flush_pre();
            }
        }
        if self.table.as_ref().is_some_and(|t| t.in_cell) {
            // Block breaks inside a cell are word spaces; the cell joins
            // everything when it closes.
            self.space = true;
            return;
        }
        // Drop a trailing space word.
        while self.words.last().is_some_and(|(w, _)| w == " ") {
            self.words.pop();
        }
        while self.words.first().is_some_and(|(w, _)| w == " ") {
            self.words.remove(0);
        }
        self.space = false;
        if self.words.is_empty() {
            return;
        }
        let words = std::mem::take(&mut self.words);
        let pfx = self.prefix();
        let first_pfx = match self.bullet.take() {
            Some(b) => {
                let used = b.chars().count();
                let base = format!(
                    "{}{}",
                    "│ ".repeat(self.quote),
                    " ".repeat(self.indent.saturating_sub(used))
                );
                format!("{base}{b}")
            }
            None => pfx.clone(),
        };
        let avail = self.width.saturating_sub(pfx.width()).max(20);
        // Greedy wrap over words (spaces are words of their own). `len`
        // is display cells (wide characters take two); link columns stay
        // character-based, matching buffer positions.
        let mut line: Vec<(String, Ann)> = Vec::new();
        let mut len = 0;
        let mut rows: Vec<Vec<(String, Ann)>> = Vec::new();
        for (w, l) in words {
            let wl = w.width();
            if w == " " {
                if !line.is_empty() {
                    line.push((w, l));
                    len += 1;
                }
                continue;
            }
            if len + wl > avail && !line.is_empty() {
                while line.last().is_some_and(|(x, _)| x == " ") {
                    line.pop();
                }
                rows.push(std::mem::take(&mut line));
                len = 0;
            }
            len += wl;
            line.push((w, l));
        }
        while line.last().is_some_and(|(x, _)| x == " ") {
            line.pop();
        }
        if !line.is_empty() {
            rows.push(line);
        }
        for (i, row) in rows.into_iter().enumerate() {
            let p = if i == 0 {
                first_pfx.clone()
            } else {
                pfx.clone()
            };
            let mut text = p.clone();
            let mut col = p.chars().count();
            let line_no = self.lines.len()
                + usize::from(
                    self.need_blank
                        && !self.lines.is_empty()
                        && !self.lines.last().is_some_and(|l| l.is_empty()),
                );
            let mut open_link: Option<(usize, usize)> = None;
            let mut open_field: Option<(usize, usize)> = None;
            let mut open_image: Option<(usize, usize)> = None;
            for (w, a) in &row {
                let wl = w.chars().count();
                match (open_link, a.link) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.links.push(Link {
                            line: line_no,
                            start: s,
                            end: col,
                            href: self.hrefs[idx].clone(),
                        });
                        open_link = a.link.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_link = Some((n, col)),
                    (None, None) => {}
                }
                match (open_field, a.field) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.field_spans.push(FieldSpan {
                            line: line_no,
                            start: s,
                            end: col,
                            field: idx,
                        });
                        open_field = a.field.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_field = Some((n, col)),
                    (None, None) => {}
                }
                match (open_image, a.image) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.image_spans.push(ImageSpan {
                            line: line_no,
                            start: s,
                            end: col,
                            image: idx,
                        });
                        open_image = a.image.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_image = Some((n, col)),
                    (None, None) => {}
                }
                text.push_str(w);
                col += wl;
            }
            if let Some((idx, s)) = open_link {
                self.links.push(Link {
                    line: line_no,
                    start: s,
                    end: col,
                    href: self.hrefs[idx].clone(),
                });
            }
            if let Some((idx, s)) = open_field {
                self.field_spans.push(FieldSpan {
                    line: line_no,
                    start: s,
                    end: col,
                    field: idx,
                });
            }
            if let Some((idx, s)) = open_image {
                self.image_spans.push(ImageSpan {
                    line: line_no,
                    start: s,
                    end: col,
                    image: idx,
                });
            }
            self.push_line(text);
        }
    }

    /// Emit preformatted runs as literal lines, keeping links (and form
    /// controls) live. Newlines break spans (one entry per visual line);
    /// tabs expand to four spaces first so columns stay character-based.
    fn flush_pre(&mut self) {
        let runs = std::mem::take(&mut self.pre_runs);
        let mut visual: Vec<Vec<(String, Ann)>> = vec![Vec::new()];
        for (t, a) in runs {
            let t = t.replace('\t', "    ");
            let mut parts = t.split('\n').peekable();
            loop {
                let part = parts.next().unwrap_or("");
                let part = part.trim_end_matches('\r');
                if !part.is_empty() {
                    visual
                        .last_mut()
                        .expect("a line")
                        .push((part.to_string(), a));
                }
                if parts.peek().is_none() {
                    break;
                }
                visual.push(Vec::new());
            }
        }
        // Like the page's own trimming: drop blank lines at both ends.
        while visual.len() > 1 && visual.first().is_some_and(|v| v.is_empty()) {
            visual.remove(0);
        }
        while visual.len() > 1 && visual.last().is_some_and(|v| v.is_empty()) {
            visual.pop();
        }
        self.space = false;
        let pfx = self.prefix();
        let pfx_chars = pfx.chars().count();
        for segs in visual {
            let line_no = self.lines.len()
                + usize::from(
                    self.need_blank
                        && !self.lines.is_empty()
                        && !self.lines.last().is_some_and(|l| l.is_empty()),
                );
            let mut text = pfx.clone();
            let mut col = pfx_chars;
            let mut open_link: Option<(usize, usize)> = None;
            let mut open_field: Option<(usize, usize)> = None;
            let mut open_image: Option<(usize, usize)> = None;
            for (seg, a) in &segs {
                match (open_link, a.link) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.links.push(Link {
                            line: line_no,
                            start: s,
                            end: col,
                            href: self.hrefs[idx].clone(),
                        });
                        open_link = a.link.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_link = Some((n, col)),
                    (None, None) => {}
                }
                match (open_field, a.field) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.field_spans.push(FieldSpan {
                            line: line_no,
                            start: s,
                            end: col,
                            field: idx,
                        });
                        open_field = a.field.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_field = Some((n, col)),
                    (None, None) => {}
                }
                match (open_image, a.image) {
                    (Some((idx, _)), Some(n)) if idx == n => {}
                    (Some((idx, s)), _) => {
                        self.image_spans.push(ImageSpan {
                            line: line_no,
                            start: s,
                            end: col,
                            image: idx,
                        });
                        open_image = a.image.map(|n| (n, col));
                    }
                    (None, Some(n)) => open_image = Some((n, col)),
                    (None, None) => {}
                }
                text.push_str(seg);
                col += seg.chars().count();
            }
            if let Some((idx, s)) = open_link {
                self.links.push(Link {
                    line: line_no,
                    start: s,
                    end: col,
                    href: self.hrefs[idx].clone(),
                });
            }
            if let Some((idx, s)) = open_field {
                self.field_spans.push(FieldSpan {
                    line: line_no,
                    start: s,
                    end: col,
                    field: idx,
                });
            }
            if let Some((idx, s)) = open_image {
                self.image_spans.push(ImageSpan {
                    line: line_no,
                    start: s,
                    end: col,
                    image: idx,
                });
            }
            self.push_line(text);
        }
    }

    /// Buffer the words gathered for one table cell as a single line.
    /// Outside any table this keeps the words where they are.
    fn finish_cell(&mut self) {
        if self.table.is_none() {
            return;
        }
        let words = std::mem::take(&mut self.words);
        self.space = false;
        let (text, spans, field_spans, image_spans) = join_cell_words(words);
        let t = self.table.as_mut().expect("a table");
        if !t.in_cell {
            // Stray text directly under a row still shows, as its own cell.
            if !text.is_empty() {
                t.row.push(TableCell {
                    text,
                    spans,
                    field_spans,
                    image_spans,
                    header: false,
                    colspan: 1,
                });
            }
            return;
        }
        t.in_cell = false;
        let (header, colspan) = (t.cell_header, t.cell_colspan.max(1));
        t.row.push(TableCell {
            text,
            spans,
            field_spans,
            image_spans,
            header,
            colspan,
        });
    }

    /// Emit the buffered table with aligned columns. Widths are display
    /// cells (capped so one long cell can't blow out the page); link,
    /// field and image spans shift with their cell.
    fn emit_table(&mut self) {
        let Some(mut t) = self.table.take() else {
            return;
        };
        // Flush an unclosed cell/row into the buffer first.
        if t.in_cell {
            t.in_cell = false;
            let words = std::mem::take(&mut self.words);
            self.space = false;
            let (text, spans, field_spans, image_spans) = join_cell_words(words);
            let (header, colspan) = (t.cell_header, t.cell_colspan.max(1));
            t.row.push(TableCell {
                text,
                spans,
                field_spans,
                image_spans,
                header,
                colspan,
            });
        } else if !self.words.is_empty() {
            let words = std::mem::take(&mut self.words);
            self.space = false;
            let (text, spans, field_spans, image_spans) = join_cell_words(words);
            if !text.is_empty() {
                t.row.push(TableCell {
                    text,
                    spans,
                    field_spans,
                    image_spans,
                    header: false,
                    colspan: 1,
                });
            }
        }
        if !t.row.is_empty() {
            t.rows.push(std::mem::take(&mut t.row));
        }
        if t.rows.is_empty() {
            return;
        }
        const MAX_COL: usize = 40;
        let ncols = t
            .rows
            .iter()
            .map(|r| r.iter().map(|c| c.colspan).sum::<usize>())
            .max()
            .unwrap_or(0)
            .max(1);
        let mut widths = vec![0usize; ncols];
        for row in &t.rows {
            let mut c = 0;
            for cell in row {
                if cell.colspan == 1 && c < ncols {
                    widths[c] = widths[c].max(cell.text.width().min(MAX_COL));
                }
                c += cell.colspan;
            }
        }
        let pfx = self.prefix();
        let pfx_chars = pfx.chars().count();
        let mut first = true;
        for row in &t.rows {
            let line_no = self.lines.len()
                + usize::from(
                    self.need_blank
                        && !self.lines.is_empty()
                        && !self.lines.last().is_some_and(|l| l.is_empty()),
                );
            let mut text = pfx.clone();
            let mut col = pfx_chars;
            let mut c = 0;
            for (i, cell) in row.iter().enumerate() {
                if i > 0 {
                    text.push_str(" │ ");
                    col += 3;
                }
                let span = cell.colspan.min(ncols.saturating_sub(c).max(1));
                let w: usize =
                    (c..c + span).map(|k| widths[k]).sum::<usize>() + 3 * span.saturating_sub(1);
                let pad = w.saturating_sub(cell.text.width());
                for (s, e, idx) in &cell.spans {
                    self.links.push(Link {
                        line: line_no,
                        start: col + s,
                        end: col + e,
                        href: self.hrefs[*idx].clone(),
                    });
                }
                for (s, e, idx) in &cell.field_spans {
                    self.field_spans.push(FieldSpan {
                        line: line_no,
                        start: col + s,
                        end: col + e,
                        field: *idx,
                    });
                }
                for (s, e, idx) in &cell.image_spans {
                    self.image_spans.push(ImageSpan {
                        line: line_no,
                        start: col + s,
                        end: col + e,
                        image: *idx,
                    });
                }
                text.push_str(&cell.text);
                col += cell.text.chars().count();
                for _ in 0..pad {
                    text.push(' ');
                    col += 1;
                }
                c += span;
            }
            self.push_line(text);
            // A header first row gets a rule under it.
            if first && row.iter().any(|c| c.header) {
                let rule_len = row
                    .iter()
                    .map(|c| c.text.width())
                    .sum::<usize>()
                    .saturating_add(3 * row.len().saturating_sub(1))
                    .max(1);
                self.push_line(format!("{pfx}{}", "─".repeat(rule_len)));
            }
            first = false;
        }
        self.need_blank = true;
    }

    fn brk(&mut self, gap: bool) {
        self.flush();
        if gap {
            self.need_blank = true;
        }
    }

    /// Edited value/checked/selected for the field about to be pushed
    /// (field order is deterministic, so the next index lines up).
    fn init_value(&self) -> Option<String> {
        self.init.values.get(self.fields.len()).cloned()
    }

    fn init_checked(&self) -> Option<bool> {
        self.init.checked.get(self.fields.len()).copied()
    }

    fn init_selected_for(&self, idx: usize) -> Option<usize> {
        self.init.selected.get(idx).copied()
    }

    /// Record a form control on the current form, if any. Returns its index.
    fn add_field(&mut self, kind: FieldKind, name: String, value: String, checked: bool) -> usize {
        let form = self.cur_form;
        let idx = self.fields.len();
        self.fields.push(Field {
            form,
            kind,
            name,
            value,
            checked,
            options: Vec::new(),
            selected: 0,
        });
        if let Some(f) = form.and_then(|f| self.forms.get_mut(f)) {
            f.fields.push(idx);
        }
        idx
    }

    /// Show a control's label, clickable to its field.
    fn field_label(&mut self, idx: usize, label: &str) {
        self.cur_field = Some(idx);
        self.text(label);
        self.cur_field = None;
    }

    /// Close an open `<option>`, keeping its text and value.
    fn finish_option(&mut self) {
        let Some((_, options, flags, open)) = self.select.as_mut() else {
            return;
        };
        let Some((value, selected, text)) = open.take() else {
            return;
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let value = value.unwrap_or_else(|| text.clone());
        options.push((if text.is_empty() { value.clone() } else { text }, value));
        flags.push(selected);
    }

    fn open(&mut self, tag: &str, a: &BTreeMap<String, String>) {
        if SKIP.contains(&tag) {
            // The tokenizer already skipped the contents; the close tag follows.
            self.skip_depth += 1;
            return;
        }
        match tag {
            "title" => {
                self.in_title = true;
                return;
            }
            "br" => {
                if self.pre {
                    self.pre_runs.push(("\n".to_string(), Ann::default()));
                } else {
                    self.flush();
                }
                return;
            }
            "hr" => {
                self.brk(true);
                let w = self
                    .width
                    .saturating_sub(self.prefix().chars().count())
                    .min(60);
                let pfx = self.prefix();
                self.push_line(format!("{pfx}{}", "─".repeat(w)));
                self.need_blank = true;
                return;
            }
            "img" => {
                let alt = a.get("alt").map(String::as_str).unwrap_or("").trim();
                let src = a.get("src").map(String::as_str).unwrap_or("").trim();
                if src.is_empty() {
                    if !alt.is_empty() {
                        self.text(&format!("[{alt}]"));
                    }
                    return;
                }
                let r = resolve(self.base, src);
                self.images.push(PageImage {
                    alt: alt.to_string(),
                    src: r,
                });
                let idx = self.images.len() - 1;
                let label = if alt.is_empty() {
                    "[image]".to_string()
                } else {
                    format!("[{alt}]")
                };
                self.cur_image = Some(idx);
                self.text(&label);
                self.cur_image = None;
                return;
            }
            "form" => {
                self.brk(true);
                if self.cur_form.is_some() {
                    // Nested forms join the outer one.
                    return;
                }
                let action = a
                    .get("action")
                    .map(|s| resolve(self.base, s))
                    .unwrap_or_else(|| self.base.to_string());
                let method = a
                    .get("method")
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_else(|| "get".to_string());
                self.forms.push(Form {
                    action,
                    method,
                    fields: Vec::new(),
                });
                self.cur_form = Some(self.forms.len() - 1);
                return;
            }
            "input" => {
                let ty = a
                    .get("type")
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_else(|| "text".to_string());
                if ty == "hidden" {
                    // Sent on submit, never shown.
                    let value = self
                        .init_value()
                        .unwrap_or_else(|| a.get("value").cloned().unwrap_or_default());
                    self.add_field(
                        FieldKind::Hidden,
                        a.get("name").cloned().unwrap_or_default(),
                        value,
                        false,
                    );
                    return;
                }
                let name = a.get("name").cloned().unwrap_or_default();
                let value = self
                    .init_value()
                    .unwrap_or_else(|| a.get("value").cloned().unwrap_or_default());
                let checked = self
                    .init_checked()
                    .unwrap_or_else(|| a.contains_key("checked"));
                let kind = match ty.as_str() {
                    "submit" | "image" => FieldKind::Submit,
                    "button" | "reset" => FieldKind::Button,
                    "checkbox" => FieldKind::Checkbox,
                    "radio" => FieldKind::Radio,
                    "password" => FieldKind::Password,
                    _ => FieldKind::Text,
                };
                let idx = self.add_field(kind, name, value, checked);
                let label = match kind {
                    FieldKind::Submit | FieldKind::Button => {
                        let v = &self.fields[idx].value;
                        format!("[{}]", if v.is_empty() { "Submit" } else { v })
                    }
                    FieldKind::Checkbox => (if checked { "[x]" } else { "[ ]" }).to_string(),
                    FieldKind::Radio => (if checked { "(*)" } else { "( )" }).to_string(),
                    _ => {
                        let v = &self.fields[idx].value;
                        let shown = if v.is_empty() {
                            a.get("placeholder")
                                .cloned()
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "________".to_string())
                        } else {
                            v.clone()
                        };
                        format!("[{shown}]")
                    }
                };
                self.field_label(idx, &label);
                return;
            }
            "button" => {
                // The content (or value) is the label; gathered till close.
                let ty = a
                    .get("type")
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_else(|| "submit".to_string());
                let kind = if ty == "button" || ty == "reset" {
                    FieldKind::Button
                } else {
                    FieldKind::Submit
                };
                let idx = self.add_field(
                    kind,
                    a.get("name").cloned().unwrap_or_default(),
                    a.get("value").cloned().unwrap_or_default(),
                    false,
                );
                self.button = Some((idx, String::new()));
                return;
            }
            "textarea" => {
                let idx = self.add_field(
                    FieldKind::TextArea,
                    a.get("name").cloned().unwrap_or_default(),
                    String::new(),
                    false,
                );
                self.textarea = Some((idx, String::new()));
                return;
            }
            "select" => {
                let idx = self.add_field(
                    FieldKind::Select,
                    a.get("name").cloned().unwrap_or_default(),
                    String::new(),
                    false,
                );
                self.select = Some((idx, Vec::new(), Vec::new(), None));
                return;
            }
            "option" => {
                // An unclosed option ends where the next begins.
                if self.select.is_some() {
                    self.finish_option();
                    if let Some((_, _, _, open)) = self.select.as_mut() {
                        *open = Some((
                            a.get("value").cloned(),
                            a.contains_key("selected"),
                            String::new(),
                        ));
                    }
                }
                return;
            }
            "a" => {
                if let Some(h) = a.get("href") {
                    let r = resolve(self.base, h);
                    if !r.starts_with("javascript:") && !h.trim().is_empty() {
                        self.hrefs.push(r);
                        self.cur_link = Some(self.hrefs.len() - 1);
                    }
                }
                return;
            }
            "li" => {
                self.brk(false);
                let b = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "• ".to_string(),
                };
                self.indent += b.chars().count();
                self.bullet = Some(b);
                return;
            }
            "ul" | "ol" => {
                self.brk(self.lists.is_empty());
                self.lists.push(if tag == "ol" {
                    Some(
                        a.get("start")
                            .and_then(|s| s.parse::<usize>().ok())
                            .map_or(0, |s| s.saturating_sub(1)),
                    )
                } else {
                    None
                });
                return;
            }
            "blockquote" => {
                self.brk(true);
                self.quote += 1;
                return;
            }
            "pre" => {
                self.brk(true);
                self.pre = true;
                return;
            }
            "td" | "th" => {
                if self.table.is_none() {
                    self.brk(false);
                    return;
                }
                // Close an unclosed cell first (<td>a<td>b).
                self.finish_cell();
                let t = self.table.as_mut().expect("a table");
                t.in_cell = true;
                t.cell_header = tag == "th";
                t.cell_colspan = a
                    .get("colspan")
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|c| (1..=8).contains(c))
                    .unwrap_or(1);
                return;
            }
            "tr" => {
                if self.table.is_none() {
                    self.brk(false);
                    return;
                }
                self.finish_cell();
                let t = self.table.as_mut().expect("a table");
                if !t.row.is_empty() {
                    t.rows.push(std::mem::take(&mut t.row));
                }
                return;
            }
            "table" => {
                if self.table.is_some() {
                    // Nested tables flatten: a break, then content joins
                    // the outer table's current cell or row.
                    self.nested_tables += 1;
                    self.brk(false);
                    return;
                }
                self.flush();
                self.table = Some(Table::default());
                return;
            }
            "dd" => {
                self.brk(false);
                self.indent += 4;
                return;
            }
            "dt" => {
                self.brk(false);
                return;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.brk(true);
                let n = tag[1..].parse::<usize>().unwrap_or(1);
                self.add_word(&"#".repeat(n));
                self.space = true;
                self.h_open = Some((n.clamp(1, 6), self.lines.len()));
                self.h_text.clear();
                return;
            }
            _ => {}
        }
        if block_tag(tag) {
            self.brk(
                matches!(
                    tag,
                    "p" | "div"
                        | "table"
                        | "figure"
                        | "details"
                        | "form"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "nav"
                        | "aside"
                        | "main"
                ) && tag != "div",
            );
        }
    }

    fn close(&mut self, tag: &str) {
        if SKIP.contains(&tag) {
            self.skip_depth = self.skip_depth.saturating_sub(1);
            return;
        }
        match tag {
            "title" => self.in_title = false,
            "a" => self.cur_link = None,
            "form" => {
                self.cur_form = None;
                self.brk(false);
            }
            "textarea" => {
                if let Some((idx, content)) = self.textarea.take() {
                    let content = self.init.values.get(idx).cloned().unwrap_or(content);
                    if let Some(f) = self.fields.get_mut(idx) {
                        f.value = content.clone();
                    }
                    let first: String = content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(24)
                        .collect();
                    let label = format!("[{}]", if first.is_empty() { "________" } else { &first });
                    self.field_label(idx, &label);
                }
            }
            "button" => {
                if let Some((idx, content)) = self.button.take() {
                    let content = content.split_whitespace().collect::<Vec<_>>().join(" ");
                    let label = if content.is_empty() {
                        let v = self
                            .fields
                            .get(idx)
                            .map(|f| f.value.clone())
                            .unwrap_or_default();
                        format!("[{}]", if v.is_empty() { "Submit" } else { &v })
                    } else {
                        format!("[{content}]")
                    };
                    self.field_label(idx, &label);
                }
            }
            "option" => self.finish_option(),
            "select" => {
                self.finish_option();
                if let Some((idx, options, flags, _)) = self.select.take() {
                    let mut selected = flags.iter().position(|s| *s).unwrap_or(0);
                    if let Some(o) = self.init_selected_for(idx) {
                        selected = o;
                    }
                    selected = selected.min(options.len().saturating_sub(1));
                    let label = options
                        .get(selected)
                        .map(|(l, _)| l.clone())
                        .unwrap_or_default();
                    if let Some(f) = self.fields.get_mut(idx) {
                        f.options = options;
                        f.selected = selected;
                        f.value = f
                            .options
                            .get(selected)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default();
                    }
                    self.field_label(idx, &format!("[{label}]"));
                }
            }
            "li" => {
                self.flush();
                self.indent =
                    self.indent
                        .saturating_sub(if matches!(self.lists.last(), Some(Some(_))) {
                            3
                        } else {
                            2
                        });
            }
            "ul" | "ol" => {
                self.brk(self.lists.len() == 1);
                self.lists.pop();
            }
            "blockquote" => {
                self.brk(true);
                self.quote = self.quote.saturating_sub(1);
            }
            "pre" => {
                self.flush();
                self.pre = false;
                self.need_blank = true;
            }
            "dd" => {
                self.brk(false);
                self.indent = self.indent.saturating_sub(4);
            }
            "td" | "th" => self.finish_cell(),
            "tr" => {
                if self.table.is_some() {
                    self.finish_cell();
                    let t = self.table.as_mut().expect("a table");
                    if !t.row.is_empty() {
                        t.rows.push(std::mem::take(&mut t.row));
                    }
                } else {
                    self.brk(false);
                }
            }
            "table" => {
                if self.nested_tables > 0 {
                    self.nested_tables -= 1;
                    self.brk(false);
                } else if self.table.is_some() {
                    self.emit_table();
                    self.need_blank = true;
                } else {
                    self.brk(true);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.brk(true);
                if let Some((level, start)) = self.h_open.take() {
                    let text = self.h_text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() && self.lines.len() > start {
                        // The break may have left a separator blank first.
                        let mut hl = start;
                        if hl < self.lines.len() && self.lines[hl].is_empty() {
                            hl += 1;
                        }
                        if hl < self.lines.len() {
                            self.headings.push(Heading {
                                level,
                                line: hl,
                                text,
                            });
                        }
                    }
                }
                self.h_text.clear();
            }
            "p" => self.brk(true),
            t if block_tag(t) => self.brk(false),
            _ => {}
        }
    }
}

/// Render `html`. `base` resolves links; `width` wraps; `readable` shows
/// only the article-like part of the page: an `<article>`/`<main>`, or
/// the `<div>`/`<section>` with the most paragraph text.
pub fn render_html(html: &str, base: &str, width: usize, readable: bool) -> Page {
    render_html_with(html, base, width, readable, &FieldState::default())
}

/// Like [`render_html`], with edited form values overlaid on the labels.
pub fn render_html_with(
    html: &str,
    base: &str,
    width: usize,
    readable: bool,
    init: &FieldState,
) -> Page {
    let mut toks = tokenize(html);
    if readable && let Some((s, e)) = readable_slice(&toks) {
        let mut kept = title_tokens(&toks, s).unwrap_or_default();
        kept.extend(toks.drain(s..=e).map(|t| clone_tok(&t)));
        toks = kept;
    }
    let mut r = R {
        base,
        width: width.max(30),
        init,
        lines: Vec::new(),
        links: Vec::new(),
        hrefs: Vec::new(),
        headings: Vec::new(),
        h_open: None,
        h_text: String::new(),
        words: Vec::new(),
        space: false,
        pre: false,
        pre_runs: Vec::new(),
        quote: 0,
        indent: 0,
        bullet: None,
        lists: Vec::new(),
        need_blank: false,
        cur_link: None,
        cur_field: None,
        cur_image: None,
        field_spans: Vec::new(),
        image_spans: Vec::new(),
        fields: Vec::new(),
        forms: Vec::new(),
        images: Vec::new(),
        cur_form: None,
        textarea: None,
        button: None,
        select: None,
        table: None,
        nested_tables: 0,
        title: String::new(),
        in_title: false,
        skip_depth: 0,
    };
    for t in toks {
        match t {
            Tok::Text(s) => r.text(&s),
            Tok::Open(n, a, selfclose) => {
                r.open(&n, &a);
                if selfclose && !matches!(n.as_str(), "br" | "hr" | "img" | "input") {
                    r.close(&n);
                }
            }
            Tok::Close(n) => r.close(&n),
        }
    }
    // Unclosed controls still show their labels.
    r.close("textarea");
    r.close("button");
    r.close("select");
    r.flush();
    // An unclosed table still shows.
    r.emit_table();
    while r.lines.last().is_some_and(|l| l.is_empty()) {
        r.lines.pop();
    }
    let title = r.title.split_whitespace().collect::<Vec<_>>().join(" ");
    Page {
        title: decode_entities(&title),
        lines: r.lines,
        links: r.links,
        headings: r.headings,
        fields: r.fields,
        forms: r.forms,
        field_spans: r.field_spans,
        images: r.images,
        image_spans: r.image_spans,
    }
}

/// Clone one token (tokens are drained out of the vector when slicing).
fn clone_tok(t: &Tok) -> Tok {
    match t {
        Tok::Text(s) => Tok::Text(s.clone()),
        Tok::Open(n, a, c) => Tok::Open(n.clone(), a.clone(), *c),
        Tok::Close(n) => Tok::Close(n.clone()),
    }
}

/// The `<title>` tokens before `limit`, if the page has a title.
fn title_tokens(toks: &[Tok], limit: usize) -> Option<Vec<Tok>> {
    let s = toks
        .iter()
        .position(|t| matches!(t, Tok::Open(n, _, _) if n == "title"))?;
    if s >= limit {
        return None;
    }
    let mut out = vec![clone_tok(&toks[s])];
    for t in &toks[s + 1..] {
        out.push(clone_tok(t));
        match t {
            Tok::Close(n) if n == "title" => return Some(out),
            Tok::Open(..) => return None, // malformed; give up
            _ => {}
        }
    }
    None
}

/// Class/id fragments marking page chrome, not content.
const CHROME: &[&str] = &[
    "nav", "menu", "sidebar", "footer", "header", "comment", "widget", "popup", "share", "related",
    "cookie",
];

/// The token range of the article-like part of the page: an
/// `<article>`/`<main>` when the page has one, else the
/// `<div>`/`<section>` with the most paragraph text (chrome-looking
/// blocks are penalized, and tiny blocks don't count).
fn readable_slice(toks: &[Tok]) -> Option<(usize, usize)> {
    // Matching close for the container at `s`, by depth.
    fn match_close(toks: &[Tok], s: usize, name: &str) -> Option<usize> {
        let mut depth = 0usize;
        for (i, t) in toks.iter().enumerate().skip(s) {
            match t {
                Tok::Open(n, _, sc) if n == name => {
                    depth += 1;
                    if *sc && depth == 1 {
                        return Some(i);
                    }
                }
                Tok::Close(n) if n == name => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        (depth > 0).then_some(toks.len().saturating_sub(1))
    }
    /// Paragraph-weighted text inside a range.
    fn score(toks: &[Tok], s: usize, e: usize) -> usize {
        let mut p_text = 0usize;
        let mut total = 0usize;
        let mut p_depth = 0usize;
        for t in &toks[s..=e] {
            match t {
                Tok::Open(n, _, _) if n == "p" => p_depth += 1,
                Tok::Close(n) if n == "p" => p_depth = p_depth.saturating_sub(1),
                Tok::Text(x) => {
                    let n = x.chars().count();
                    total += n;
                    if p_depth > 0 {
                        p_text += n;
                    }
                }
                _ => {}
            }
        }
        p_text.max(total / 4)
    }
    fn chrome(attrs: &BTreeMap<String, String>) -> bool {
        let class = attrs.get("class").map(String::as_str).unwrap_or("");
        let id = attrs.get("id").map(String::as_str).unwrap_or("");
        CHROME.iter().any(|c| class.contains(c) || id.contains(c))
    }
    // An explicit article or main always wins, largest first.
    let mut article: Option<(usize, usize, usize)> = None; // (score, start, end)
    for (s, t) in toks.iter().enumerate() {
        let name = match t {
            Tok::Open(n, _, _) if n == "article" || n == "main" => n,
            _ => continue,
        };
        let Some(e) = match_close(toks, s, name) else {
            continue;
        };
        let sc = score(toks, s, e);
        if sc > article.map_or(0, |(b, _, _)| b) {
            article = Some((sc, s, e));
        }
    }
    if let Some((_, s, e)) = article {
        return Some((s, e));
    }
    // Otherwise the meatiest generic block, if it reads like content.
    let mut best: Option<(usize, usize, usize)> = None;
    for (s, t) in toks.iter().enumerate() {
        let (name, attrs) = match t {
            Tok::Open(n, a, _) if n == "div" || n == "section" => (n, a),
            _ => continue,
        };
        let Some(e) = match_close(toks, s, name) else {
            continue;
        };
        let mut sc = score(toks, s, e);
        if chrome(attrs) {
            sc /= 4;
        }
        if sc > best.map_or(0, |(b, _, _)| b) {
            best = Some((sc, s, e));
        }
    }
    // Fewer than ~a paragraph isn't an article; show everything.
    const MIN_READABLE: usize = 150;
    best.filter(|(score, _, _)| *score >= MIN_READABLE)
        .map(|(_, s, e)| (s, e))
}

/// Image addresses on the page, in document order, for the background
/// downloader. Capped: inline pictures beyond this don't load.
pub const MAX_INLINE_IMAGES: usize = 20;

pub fn image_sources(html: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for t in tokenize(html) {
        if out.len() >= MAX_INLINE_IMAGES {
            break;
        }
        if let Tok::Open(n, a, _) = t
            && n == "img"
            && let Some(src) = a.get("src").map(|s| s.trim())
            && !src.is_empty()
        {
            let r = resolve(base, src);
            if (r.starts_with("http://") || r.starts_with("https://")) && !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// A plain text page: lines as they are, URLs in them made links.
pub fn render_text(text: &str) -> Page {
    /// Terminators: whitespace and closers that can't end a URL.
    const TRAIL: [char; 9] = ['.', ',', ';', ':', '!', '?', '\'', '"', ')'];
    fn is_term(c: char) -> bool {
        c.is_whitespace() || matches!(c, '<' | '>' | '"' | ')' | ']' | '}')
    }
    let mut links = Vec::new();
    let lines: Vec<String> = text.lines().map(|l| l.replace('\t', "    ")).collect();
    for (n, l) in lines.iter().enumerate() {
        let chars: Vec<char> = l.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let tail = &chars[i..];
            let bare = if tail.starts_with(&['h', 't', 't', 'p', ':', '/', '/'])
                || tail.starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/'])
            {
                Some(false)
            } else if tail.starts_with(&['w', 'w', 'w', '.'])
                && tail.get(4).is_some_and(|c| c.is_ascii_alphanumeric())
            {
                Some(true)
            } else {
                None
            };
            let Some(bare) = bare else {
                i += 1;
                continue;
            };
            let mut j = i;
            while j < chars.len() && !is_term(chars[j]) {
                j += 1;
            }
            let raw: String = chars[i..j].iter().collect();
            let raw = raw.trim_end_matches(TRAIL);
            if raw.chars().count() > 4 {
                let href = if bare {
                    format!("https://{raw}")
                } else {
                    raw.to_string()
                };
                let end = i + raw.chars().count();
                links.push(Link {
                    line: n,
                    start: i,
                    end,
                    href,
                });
                i = end.max(i + 1);
            } else {
                i += 1;
            }
        }
    }
    Page {
        title: String::new(),
        lines,
        links,
        headings: Vec::new(),
        fields: Vec::new(),
        forms: Vec::new(),
        field_spans: Vec::new(),
        images: Vec::new(),
        image_spans: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_becomes_url_or_search() {
        assert_eq!(normalize_input("example.com"), "https://example.com");
        assert_eq!(normalize_input("https://a.b/c"), "https://a.b/c");
        assert!(
            normalize_input("rust lifetimes")
                .starts_with("https://html.duckduckgo.com/html/?q=rust+lifetimes")
        );
        assert_eq!(normalize_input("/etc/hosts"), "/etc/hosts");
        assert!(normalize_input("hello").contains("q=hello"));
    }

    #[test]
    fn urls_resolve_like_a_browser() {
        let b = "https://ex.com/a/b/page.html?x=1";
        assert_eq!(resolve(b, "c.html"), "https://ex.com/a/b/c.html");
        assert_eq!(resolve(b, "../c"), "https://ex.com/a/c");
        assert_eq!(resolve(b, "/top"), "https://ex.com/top");
        assert_eq!(resolve(b, "//cdn.x/y"), "https://cdn.x/y");
        assert_eq!(resolve(b, "#frag"), "https://ex.com/a/b/page.html?x=1#frag");
        assert_eq!(resolve(b, "?q=2"), "https://ex.com/a/b/page.html?q=2");
        assert_eq!(resolve(b, "http://o.org"), "http://o.org");
        assert_eq!(resolve("https://ex.com", "x"), "https://ex.com/x");
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &bogus; &"),
            "a & b <c> AB &bogus; &"
        );
    }

    #[test]
    fn paragraphs_headings_lists_and_links() {
        let html = "<html><head><title> My  Page </title><style>p{}</style></head><body>\
            <h1>Hello</h1><p>Some <b>bold</b> text with a <a href=\"/x\">link here</a>.</p>\
            <ul><li>one</li><li>two <a href=\"y\">y</a></li></ul>\
            <ol><li>first</li><li>second</li></ol><script>alert(1)</script></body></html>";
        let p = render_html(html, "https://e.com/d/", 80, false);
        assert_eq!(p.title, "My Page");
        let text = p.lines.join("\n");
        assert!(text.contains("# Hello"), "{text}");
        assert!(text.contains("Some bold text with a link here."), "{text}");
        assert!(text.contains("• one") && text.contains("• two y"), "{text}");
        assert!(
            text.contains("1. first") && text.contains("2. second"),
            "{text}"
        );
        assert!(!text.contains("alert"));
        assert_eq!(p.links.len(), 2);
        let l = &p.links[0];
        assert_eq!(l.href, "https://e.com/x");
        let line: Vec<char> = p.lines[l.line].chars().collect();
        assert_eq!(line[l.start..l.end].iter().collect::<String>(), "link here");
        assert_eq!(p.links[1].href, "https://e.com/d/y");
    }

    #[test]
    fn wraps_and_keeps_link_spans_per_line() {
        let html = "<p>aaa bbb <a href=\"/l\">ccc ddd eee</a> fff</p>";
        let p = render_html(html, "https://e.com/", 30, false);
        // width.max(30) with avail >= 20
        let p2 = render_html(
            &format!("<p>{}</p>", "word ".repeat(30)),
            "https://e.com/",
            40,
            false,
        );
        assert!(
            p2.lines.iter().all(|l| l.chars().count() <= 40),
            "{:?}",
            p2.lines
        );
        assert!(p2.lines.len() > 2);
        assert_eq!(p.links.len(), 1);
    }

    #[test]
    fn pre_blockquote_table_and_readable() {
        let html = "<pre>  a\n    b</pre><blockquote>quoted</blockquote><table><tr><td>x</td><td>y</td></tr><tr><td>1</td><td>2</td></tr></table>";
        let p = render_html(html, "https://e.com/", 80, false);
        let t = p.lines.join("\n");
        assert!(t.contains("  a\n    b"), "{t}");
        assert!(t.contains("│ quoted"), "{t}");
        assert!(t.contains("x │ y") && t.contains("1 │ 2"), "{t}");
        let page = "<body><nav>menu stuff</nav><article><p>the story</p></article><footer>foot</footer></body>";
        let r = render_html(page, "https://e.com/", 80, true);
        assert_eq!(r.lines.join("\n"), "the story");
        let full = render_html(page, "https://e.com/", 80, false);
        assert!(full.lines.join("\n").contains("menu stuff"));
    }

    #[test]
    fn pre_links_stay_clickable() {
        let html = "<pre>run <a href=\"/man\">man ls</a> now\nplain</pre>";
        let p = render_html(html, "https://e.com/", 80, false);
        assert_eq!(p.links.len(), 1);
        let l = &p.links[0];
        assert_eq!(l.href, "https://e.com/man");
        let line: Vec<char> = p.lines[l.line].chars().collect();
        assert_eq!(line[l.start..l.end].iter().collect::<String>(), "man ls");
    }

    #[test]
    fn tables_align_columns_and_rule_headers() {
        let html = "<table><tr><th>name</th><th>age</th></tr><tr><td>al</td><td>30</td></tr><tr><td>bo</td><td>4</td></tr></table>";
        let p = render_html(html, "https://e.com/", 80, false);
        let t = p.lines.join("\n");
        assert!(t.contains("name │ age"), "{t}");
        assert!(t.contains("al   │ 30"), "{t}");
        assert!(t.contains("bo   │ 4"), "{t}");
        assert!(t.contains("────"), "{t}");
        // Links shift with their cell's padding.
        let html = "<table><tr><td>x</td><td><a href=\"/l\">go</a></td></tr><tr><td>longer</td><td>y</td></tr></table>";
        let p = render_html(html, "https://e.com/", 80, false);
        assert_eq!(p.links.len(), 1);
        let l = &p.links[0];
        let line: Vec<char> = p.lines[l.line].chars().collect();
        assert_eq!(line[l.start..l.end].iter().collect::<String>(), "go");
    }

    #[test]
    fn headings_are_recorded_for_the_toc() {
        let html = "<h1>Top</h1><p>text</p><h2>Sub <a href=\"/x\">link</a></h2><p>more</p>";
        let p = render_html(html, "https://e.com/", 80, false);
        assert_eq!(p.headings.len(), 2);
        assert_eq!(
            (p.headings[0].level, p.headings[0].text.as_str()),
            (1, "Top")
        );
        assert_eq!(
            (p.headings[1].level, p.headings[1].text.as_str()),
            (2, "Sub link")
        );
        let h1 = &p.headings[0];
        assert!(p.lines[h1.line].contains("Top"));
    }

    #[test]
    fn readable_falls_back_to_the_meatiest_div() {
        let story = "word ".repeat(60);
        let page = format!(
            "<body><div class=\"sidebar\">short</div><div id=\"content\"><p>{story}</p></div></body>"
        );
        let r = render_html(&page, "https://e.com/", 80, true);
        let t = r.lines.join("\n");
        assert!(t.contains("word word"), "{t}");
        assert!(!t.contains("short"), "{t}");
        // Nothing substantial: show everything.
        let thin = "<body><div>a</div><div>b</div></body>";
        let r = render_html(thin, "https://e.com/", 80, true);
        assert!(r.lines.join("\n").contains('a'));
    }

    #[test]
    fn wide_characters_wrap_by_display_cells() {
        let p = render_html(
            &format!("<p>{}</p>", "漢 ".repeat(30)),
            "https://e.com/",
            40,
            false,
        );
        assert!(
            p.lines
                .iter()
                .all(|l| unicode_width::UnicodeWidthStr::width(l.as_str()) <= 40),
            "{:?}",
            p.lines
        );
        assert!(p.lines.len() > 2);
    }

    #[test]
    fn forms_record_fields_and_labels() {
        let html = "<form action=\"/go\" method=\"post\">\
            <input name=\"q\" value=\"hi\"><input type=password name=pw>\
            <input type=checkbox name=c checked><input type=radio name=r value=1><input type=radio name=r value=2 checked>\
            <input type=hidden name=h value=s><input type=submit value=\"Go\">\
            <textarea name=t>multi</textarea>\
            <select name=s><option value=a>Aye</option><option selected>Bee</option></select>\
            <button name=b>Press</button></form>";
        let p = render_html(html, "https://e.com/", 80, false);
        let t = p.lines.join("\n");
        assert!(t.contains("[hi]") && t.contains("[________]"), "{t}");
        assert!(
            t.contains("[x]") && t.contains("( )") && t.contains("(*)"),
            "{t}"
        );
        assert!(!t.contains("[s]"), "hidden stays hidden: {t}");
        assert!(
            t.contains("[Go]")
                && t.contains("[multi]")
                && t.contains("[Bee]")
                && t.contains("[Press]"),
            "{t}"
        );
        assert_eq!(p.forms.len(), 1);
        assert_eq!(
            (p.forms[0].action.as_str(), p.forms[0].method.as_str()),
            ("https://e.com/go", "post")
        );
        assert_eq!(p.forms[0].fields.len(), p.fields.len());
        assert_eq!(p.fields.len(), 10);
        assert_eq!(p.field_spans.len(), 9, "hidden has no span");
        let sel = p
            .fields
            .iter()
            .find(|f| f.kind == FieldKind::Select)
            .unwrap();
        assert_eq!((sel.selected, sel.value.as_str()), (1, "Bee"));
        let ta = p
            .fields
            .iter()
            .find(|f| f.kind == FieldKind::TextArea)
            .unwrap();
        assert_eq!(ta.value, "multi");
        // Every span points at its control's label.
        for s in &p.field_spans {
            let line: Vec<char> = p.lines[s.line].chars().collect();
            let got: String = line[s.start..s.end.min(line.len())].iter().collect();
            assert!(
                (got.starts_with('[') && got.ends_with(']'))
                    || ["( )", "(*)", "[x]", "[ ]"].contains(&got.as_str()),
                "{got}"
            );
        }
    }

    #[test]
    fn overlay_toggles_checkbox_label() {
        let html = "<form><input type=checkbox name=c></form>";
        let p = render_html(html, "https://e.com/", 80, false);
        assert!(p.lines.join("").contains("[ ]"));
        let mut st = FieldState::capture(&p);
        st.checked[0] = true;
        let p2 = render_html_with(html, "https://e.com/", 80, false, &st);
        assert!(p2.lines.join("").contains("[x]"), "{:?}", p2.lines);
    }

    #[test]
    fn images_resolve_and_list() {
        let html = "<p><img src=\"/i.png\" alt=\"pic\"> and <img src=\"https://x.org/a.jpg\"></p>";
        let p = render_html(html, "https://e.com/d/", 80, false);
        assert_eq!(p.images.len(), 2);
        assert_eq!(p.images[0].src, "https://e.com/i.png");
        assert_eq!(p.images[1].alt, "");
        assert_eq!(p.image_spans.len(), 2);
        assert_eq!(
            image_sources(html, "https://e.com/d/"),
            vec![
                "https://e.com/i.png".to_string(),
                "https://x.org/a.jpg".to_string()
            ]
        );
    }

    #[test]
    fn text_pages_link_their_urls() {
        let p = render_text("see https://a.b/c, ok\nnone");
        assert_eq!(p.links.len(), 1);
        assert_eq!(p.links[0].href, "https://a.b/c");
        assert_eq!((p.links[0].start, p.links[0].end), (4, 17));
        // Bare www hosts and trailing punctuation.
        let p = render_text("go to www.example.com/path! (or https://b.example/x).");
        assert_eq!(p.links.len(), 2);
        assert_eq!(p.links[0].href, "https://www.example.com/path");
        assert_eq!(p.links[1].href, "https://b.example/x");
        let line = &p.lines[0];
        for l in &p.links {
            let chars: Vec<char> = line.chars().collect();
            let got: String = chars[l.start..l.end].iter().collect();
            assert!(l.href.ends_with(&got) || got.starts_with("www."), "{got}");
        }
    }

    #[test]
    fn bad_html_does_not_panic() {
        for s in [
            "<",
            "<a href=",
            "<<<>>>",
            "<p><b>unclosed",
            "&",
            "<!-- x",
            "<script>no end",
            "</p></p>",
            "<a href='x",
        ] {
            let _ = render_html(s, "https://e.com/", 40, false);
        }
    }
}
