// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the backends share: the section tree, link classification,
//! numbering, and a few text and archive helpers.

use super::ast::*;
use std::collections::HashMap;

/// Settings of one export.
#[derive(Clone, Default)]
pub struct Settings {
    /// Only the body: no preamble, no postamble.
    pub body_only: bool,
    /// Name of the source file (for default output names and the date).
    pub source: String,
    /// Today as `YYYY-MM-DD`.
    pub today: String,
    /// Image bytes by path, for backends that embed pictures.
    pub images: Option<std::rc::Rc<dyn Fn(&str) -> Option<Vec<u8>>>>,
}

/// How deep the section tree is walked. Headings nest without limit,
/// so a file with thousands of levels would overflow the stack;
/// sections below this depth are left out of the export.
pub(crate) const MAX_SECTION_DEPTH: usize = 64;

/// A heading with what's under it.
pub struct Section<'a> {
    pub heading: Option<&'a Heading>,
    /// "1.2.3"; empty for the root and for unnumbered ones.
    pub number: Vec<usize>,
    pub aff: Option<&'a Aff>,
    /// Content before the first subheading.
    pub body: Vec<&'a Node>,
    pub children: Vec<Section<'a>>,
    /// Depth below the root: 1 for the top headings.
    pub depth: usize,
    /// Unique id usable as an anchor.
    pub id: String,
}

/// Nest `doc.nodes` by heading level. Depth rather than level decides
/// numbers, so a file that starts at `***` still numbers 1, 2, ...
pub fn sections(doc: &Doc) -> Section<'_> {
    let mut root = Section {
        heading: None,
        number: Vec::new(),
        aff: None,
        body: Vec::new(),
        children: Vec::new(),
        depth: 0,
        id: String::new(),
    };
    let mut path: Vec<(usize, usize)> = Vec::new(); // (level, index in parent)
    fn walk<'b, 'a>(root: &'b mut Section<'a>, path: &[(usize, usize)]) -> &'b mut Section<'a> {
        let mut cur = root;
        for &(_, ix) in path {
            cur = &mut cur.children[ix];
        }
        cur
    }
    for n in &doc.nodes {
        match &n.block {
            Block::Heading(h) => {
                while path.last().is_some_and(|&(lv, _)| lv >= h.level) {
                    path.pop();
                }
                let depth = path.len() + 1;
                let parent = walk(&mut root, &path);
                let mut number = parent.number.clone();
                let nth = parent.children.len() + 1;
                number.push(nth);
                let id = match h.prop("CUSTOM_ID") {
                    Some(c) => c.to_string(),
                    None => format!(
                        "sec-{}",
                        number
                            .iter()
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join("-")
                    ),
                };
                parent.children.push(Section {
                    heading: Some(h),
                    number,
                    aff: Some(&n.aff),
                    body: Vec::new(),
                    children: Vec::new(),
                    depth,
                    id,
                });
                let ix = parent.children.len() - 1;
                path.push((h.level, ix));
            }
            _ => walk(&mut root, &path).body.push(n),
        }
    }
    root
}

impl<'a> Section<'a> {
    pub fn title_plain(&self) -> String {
        self.heading.map(|h| plain(&h.title)).unwrap_or_default()
    }

    /// Whether the heading is numbered (UNNUMBERED property, `num:` option).
    pub fn numbered(&self, o: &Options) -> bool {
        match self.heading.and_then(|h| h.prop("UNNUMBERED")) {
            Some(v) if v != "nil" => false,
            _ => o.num && self.depth <= o.section_numbers_depth,
        }
    }

    pub fn number_string(&self) -> String {
        self.number
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".")
    }

    pub fn all<'s>(&'s self, out: &mut Vec<&'s Section<'a>>) {
        // An explicit stack: a deeply nested tree would overflow the
        // call stack.
        let mut stack: Vec<&'s Section<'a>> = self.children.iter().rev().collect();
        while let Some(s) = stack.pop() {
            out.push(s);
            stack.extend(s.children.iter().rev());
        }
    }
}

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    External(String),
    /// An id of a heading, target or named element in this document.
    Internal(String),
    /// A local file (maybe `::search`).
    File {
        path: String,
        search: Option<String>,
    },
    Image(String),
    /// Nothing exportable (shell:, elisp:, an unknown heading ...).
    Unresolved(String),
}

pub fn is_image_path(p: &str) -> bool {
    crate::org_display::is_image_file_name(p)
}

/// Anchors in a document: lookup text to id.
pub struct Anchors {
    map: HashMap<String, String>,
}

impl Anchors {
    pub fn new(root: &Section) -> Self {
        let mut map = HashMap::new();
        let mut all = Vec::new();
        root.all(&mut all);
        for s in all {
            let Some(h) = s.heading else { continue };
            map.entry(format!("*{}", s.title_plain()))
                .or_insert_with(|| s.id.clone());
            map.entry(s.title_plain()).or_insert_with(|| s.id.clone());
            if let Some(c) = h.prop("CUSTOM_ID") {
                map.insert(format!("#{c}"), s.id.clone());
            }
            if let Some(i) = h.prop("ID") {
                map.insert(format!("id:{i}"), s.id.clone());
            }
        }
        Anchors { map }
    }

    /// Add the names and `<<targets>>` found in the body of the document.
    pub fn add_nodes(&mut self, nodes: &[Node]) {
        fn inl(a: &mut Anchors, v: &[Inline]) {
            for i in v {
                match i {
                    Inline::Target(t) => {
                        a.map
                            .entry(t.clone())
                            .or_insert_with(|| format!("org-target-{}", slug(t)));
                    }
                    Inline::Bold(x)
                    | Inline::Italic(x)
                    | Inline::Underline(x)
                    | Inline::Strike(x) => inl(a, x),
                    _ => {}
                }
            }
        }
        for n in nodes {
            if let Some(name) = &n.aff.name {
                self.map
                    .entry(name.clone())
                    .or_insert_with(|| format!("org-{}", slug(name)));
            }
            match &n.block {
                Block::Para(p) => inl(self, p),
                Block::List(l) => {
                    for it in &l.items {
                        self.add_nodes(&it.content);
                    }
                }
                Block::Quote(c)
                | Block::Center(c)
                | Block::Special { content: c, .. }
                | Block::Drawer { content: c, .. } => self.add_nodes(c),
                _ => {}
            }
        }
    }

    pub fn find(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(String::as_str)
    }

    pub fn classify(&self, target: &str) -> LinkKind {
        let t = target.trim();
        for p in [
            "http://", "https://", "ftp://", "mailto:", "news:", "tel:", "irc:",
        ] {
            if t.starts_with(p) {
                return LinkKind::External(t.to_string());
            }
        }
        if let Some(rest) = t
            .strip_prefix("file:")
            .or_else(|| t.strip_prefix("attachment:"))
        {
            let (path, search) = match rest.split_once("::") {
                Some((p, s)) => (p.to_string(), Some(s.to_string())),
                None => (rest.to_string(), None),
            };
            if is_image_path(&path) && search.is_none() {
                return LinkKind::Image(path);
            }
            return LinkKind::File { path, search };
        }
        if t.starts_with('/') || t.starts_with("./") || t.starts_with("../") || t.starts_with("~/")
        {
            let path = t.split("::").next().unwrap_or(t).to_string();
            if is_image_path(&path) {
                return LinkKind::Image(path);
            }
            return LinkKind::File { path, search: None };
        }
        for p in ["shell:", "elisp:", "info:", "help:", "doi:", "id:", "roam:"] {
            if t.starts_with(p) {
                if p == "doi:" {
                    return LinkKind::External(format!("https://doi.org/{}", &t[4..]));
                }
                if p == "id:" {
                    return self
                        .find(t)
                        .map(|i| LinkKind::Internal(i.to_string()))
                        .unwrap_or_else(|| LinkKind::Unresolved(t.to_string()));
                }
                return LinkKind::Unresolved(t.to_string());
            }
        }
        if let Some(i) = self.find(t) {
            return LinkKind::Internal(i.to_string());
        }
        if let Some(c) = t.strip_prefix('#')
            && let Some(i) = self.find(&format!("#{c}"))
        {
            return LinkKind::Internal(i.to_string());
        }
        if is_image_path(t) {
            return LinkKind::Image(t.to_string());
        }
        LinkKind::Unresolved(t.to_string())
    }
}

/// A lowercase id-safe form of `s`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The `:exports` header argument of a source block: "code" (default),
/// "results", "both" or "none".
pub fn exports_of(params: &str) -> &'static str {
    let w: Vec<&str> = params.split_whitespace().collect();
    for (i, x) in w.iter().enumerate() {
        if *x == ":exports"
            && let Some(v) = w.get(i + 1)
        {
            return match *v {
                "results" => "results",
                "both" => "both",
                "none" => "none",
                _ => "code",
            };
        }
    }
    "code"
}

/// The title of the document or of "".
pub fn title_of(doc: &Doc) -> String {
    doc.meta.title.clone()
}

/// Break `text` into lines of at most `width` characters at spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut len = 0;
        for w in para.split_whitespace() {
            let wl = w.chars().count();
            if len > 0 && len + 1 + wl > width {
                lines.push(std::mem::take(&mut cur));
                len = 0;
            }
            if len > 0 {
                cur.push(' ');
                len += 1;
            }
            cur.push_str(w);
            len += wl;
        }
        lines.push(cur);
    }
    lines
}

/// The date shown for the document: `#+DATE` or nothing.
pub fn date_of(doc: &Doc) -> String {
    let d = doc.meta.date.trim();
    if d.is_empty() {
        String::new()
    } else {
        d.trim_matches(['<', '>', '[', ']']).to_string()
    }
}

// ------------------------------------------------------------------ zip

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *t = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// A zip archive with the entries stored (not compressed), in order.
pub fn zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let nm = name.as_bytes();
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // utf-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(nm.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(nm);
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(nm.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(nm);
    }
    let cd_off = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Read the stored entries of a zip made by [`zip`].
pub fn unzip(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u16at = |p: usize| u16::from_le_bytes([data[p], data[p + 1]]) as usize;
    let u32at =
        |p: usize| u32::from_le_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]) as usize;
    let mut out = Vec::new();
    let mut p = 0;
    while p + 30 <= data.len() && u32at(p) == 0x0403_4b50 {
        let size = u32at(p + 18);
        let nl = u16at(p + 26);
        let el = u16at(p + 28);
        let name = String::from_utf8_lossy(&data[p + 30..p + 30 + nl]).to_string();
        let start = p + 30 + nl + el;
        out.push((name, data[start..start + size].to_vec()));
        p = start + size;
    }
    out
}

/// (width, height) of a PNG, GIF or JPEG.
pub fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() > 24 && &b[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some((
            u32::from_be_bytes([b[16], b[17], b[18], b[19]]),
            u32::from_be_bytes([b[20], b[21], b[22], b[23]]),
        ));
    }
    if b.len() > 10 && (&b[..6] == b"GIF87a" || &b[..6] == b"GIF89a") {
        return Some((
            u16::from_le_bytes([b[6], b[7]]) as u32,
            u16::from_le_bytes([b[8], b[9]]) as u32,
        ));
    }
    if b.len() > 4 && b[0] == 0xFF && b[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                return Some((
                    u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32,
                    u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32,
                ));
            }
            i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        }
    }
    None
}

/// `--` `---` `...` become dashes and an ellipsis.
pub fn special_strings(s: &str) -> String {
    if !s.contains("--") && !s.contains("...") && !s.contains("\\-") {
        return s.to_string();
    }
    s.replace("---", "\u{2014}")
        .replace("--", "\u{2013}")
        .replace("...", "\u{2026}")
        .replace("\\-", "")
}

/// The ids of the `#+TOC: headlines N` keyword, if any.
pub fn toc_keyword(doc: &Doc) -> Option<(usize, bool)> {
    fn scan(nodes: &[Node]) -> Option<(usize, bool)> {
        for n in nodes {
            if let Block::Keyword { key, value } = &n.block
                && key == "TOC"
            {
                let w: Vec<&str> = value.split_whitespace().collect();
                if w.first() == Some(&"headlines") {
                    let depth = w.get(1).and_then(|d| d.parse().ok()).unwrap_or(usize::MAX);
                    return Some((depth, w.contains(&"local")));
                }
            }
        }
        None
    }
    scan(&doc.nodes)
}

/// Whether `doc` has text for LaTeX math anywhere.
pub fn has_math(doc: &Doc) -> bool {
    fn inl(v: &[Inline]) -> bool {
        v.iter().any(|i| match i {
            Inline::Latex { .. } => true,
            Inline::Bold(x)
            | Inline::Italic(x)
            | Inline::Underline(x)
            | Inline::Strike(x)
            | Inline::Sub(x)
            | Inline::Sup(x) => inl(x),
            Inline::Link { desc: Some(d), .. } => inl(d),
            _ => false,
        })
    }
    fn nodes(ns: &[Node]) -> bool {
        ns.iter().any(|n| match &n.block {
            Block::Para(p) => inl(p),
            Block::LatexEnv(_) => true,
            Block::Heading(h) => inl(&h.title),
            Block::List(l) => l
                .items
                .iter()
                .any(|i| nodes(&i.content) || i.tag.as_deref().is_some_and(inl)),
            Block::Table(t) => t
                .rows
                .iter()
                .any(|r| matches!(r, Row::Cells(c) if c.iter().any(|x| inl(x)))),
            Block::Quote(c)
            | Block::Center(c)
            | Block::Special { content: c, .. }
            | Block::Drawer { content: c, .. } => nodes(c),
            _ => false,
        })
    }
    nodes(&doc.nodes) || doc.footnotes.iter().any(|f| nodes(&f.content))
}

/// Whether any node satisfies `f` (looking inside containers).
pub fn any_node(doc: &Doc, f: &dyn Fn(&Block) -> bool) -> bool {
    fn go(ns: &[Node], f: &dyn Fn(&Block) -> bool) -> bool {
        ns.iter().any(|n| {
            f(&n.block)
                || match &n.block {
                    Block::List(l) => l.items.iter().any(|i| go(&i.content, f)),
                    Block::Quote(c)
                    | Block::Center(c)
                    | Block::Special { content: c, .. }
                    | Block::Drawer { content: c, .. } => go(c, f),
                    _ => false,
                }
        })
    }
    go(&doc.nodes, f) || doc.footnotes.iter().any(|x| go(&x.content, f))
}

/// The bibliography file names without a directory.
pub fn bib_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A timestamp's parts: (year, month, day, optional (h, m), optional end (h, m), repeater like "+1w").
pub fn parse_ts(
    ts: &str,
) -> Option<(
    i32,
    u32,
    u32,
    Option<(u32, u32)>,
    Option<(u32, u32)>,
    Option<String>,
)> {
    let t = ts.trim_matches(['<', '>', '[', ']']);
    let mut w = t.split_whitespace();
    let date = w.next()?;
    let mut d = date.split('-');
    let y: i32 = d.next()?.parse().ok()?;
    let m: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let mut time = None;
    let mut end = None;
    let mut rep = None;
    for x in w {
        if x.contains(':') && x.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let (a, b) = match x.split_once('-') {
                Some((a, b)) => (a, Some(b)),
                None => (x, None),
            };
            fn hm(s: &str) -> Option<(u32, u32)> {
                let (h, mm) = s.split_once(':')?;
                Some((h.parse().ok()?, mm.get(..2)?.parse().ok()?))
            }
            time = hm(a);
            end = b.and_then(hm);
        } else if x.starts_with(['+', '.']) && x.len() > 2 {
            rep = Some(x.to_string());
        }
    }
    Some((y, m, day, time, end, rep))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    #[test]
    fn section_tree_and_numbers() {
        let d = parse("pre\n* A\na\n** A1\n*** deep\n** A2\n* B\n", &|_| None);
        let r = sections(&d);
        assert_eq!(r.body.len(), 1);
        assert_eq!(r.children.len(), 2);
        assert_eq!(r.children[0].children.len(), 2);
        assert_eq!(r.children[0].children[1].number_string(), "1.2");
        assert_eq!(r.children[0].children[0].children[0].depth, 3);
        assert_eq!(r.children[1].number_string(), "2");
        // Starting at level 3 still numbers from 1.
        let d = parse("*** X\n*** Y\n", &|_| None);
        assert_eq!(sections(&d).children[1].number_string(), "2");
    }

    #[test]
    fn links_are_classified() {
        let d = parse(
            "* Intro\n:PROPERTIES:\n:CUSTOM_ID: intro\n:END:\ntext <<tgt>>\n#+NAME: tbl\n| a |\n",
            &|_| None,
        );
        let r = sections(&d);
        let mut a = Anchors::new(&r);
        a.add_nodes(&d.nodes);
        assert_eq!(
            a.classify("https://x.org"),
            LinkKind::External("https://x.org".into())
        );
        assert_eq!(a.classify("#intro"), LinkKind::Internal("intro".into()));
        assert_eq!(a.classify("*Intro"), LinkKind::Internal("intro".into()));
        assert_eq!(
            a.classify("tgt"),
            LinkKind::Internal("org-target-tgt".into())
        );
        assert_eq!(a.classify("tbl"), LinkKind::Internal("org-tbl".into()));
        assert_eq!(a.classify("file:a.png"), LinkKind::Image("a.png".into()));
        assert_eq!(
            a.classify("file:b.org::*H"),
            LinkKind::File {
                path: "b.org".into(),
                search: Some("*H".into())
            }
        );
        assert_eq!(
            a.classify("shell:ls"),
            LinkKind::Unresolved("shell:ls".into())
        );
        assert_eq!(
            a.classify("doi:10.1/x"),
            LinkKind::External("https://doi.org/10.1/x".into())
        );
    }

    #[test]
    fn zip_round_trips() {
        let z = zip(&[
            ("a.txt".into(), b"hello".to_vec()),
            ("d/b".into(), vec![1, 2, 3]),
        ]);
        let back = unzip(&z);
        assert_eq!(
            back,
            vec![
                ("a.txt".to_string(), b"hello".to_vec()),
                ("d/b".to_string(), vec![1, 2, 3])
            ]
        );
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn wraps_text() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn export_option_of_src_blocks() {
        assert_eq!(exports_of(":exports results :tangle x"), "results");
        assert_eq!(exports_of("-n"), "code");
    }
}
