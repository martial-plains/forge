// SPDX-License-Identifier: MIT OR Apache-2.0
//! Markdown export (GitHub-flavoured: pipe tables, fenced code).

use super::ast::*;
use super::cite;
use super::common::*;
use super::entities;

struct M<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
}

fn esc(t: &str) -> String {
    let mut s = String::new();
    for c in t.chars() {
        if matches!(c, '\\' | '`' | '*' | '_') {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// A link target that cannot end the link early: the characters
/// Markdown reserves are percent-encoded.
fn link_target(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    for c in t.chars() {
        if matches!(c, ' ' | '(' | ')' | '<' | '>' | '"' | '\\') {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).as_bytes() {
                s.push('%');
                s.push_str(&format!("{b:02X}"));
            }
        } else {
            s.push(c);
        }
    }
    s
}

pub fn export(doc: &Doc, st: &Settings) -> String {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let m = M {
        doc,
        anchors,
        o: o.clone(),
    };
    let mut out = String::new();
    if !st.body_only && o.title && !doc.meta.title.is_empty() {
        out.push_str(&format!(
            "# {}\n\n",
            m.inline(&super::parse::inline_of(&doc.meta.title, &o))
        ));
        let mut by = Vec::new();
        if o.author && !doc.meta.author.is_empty() {
            by.push(format!("*{}*", esc(&doc.meta.author)));
        }
        let d = date_of(doc);
        if o.date && !d.is_empty() {
            by.push(format!("*{}*", esc(&d)));
        }
        if !by.is_empty() {
            out.push_str(&by.join(" — "));
            out.push_str("\n\n");
        }
    }
    let title_offset = usize::from(!st.body_only && o.title && !doc.meta.title.is_empty());
    if let Some(d) = match (&o.toc, toc_keyword(doc)) {
        (_, Some((d, _))) => Some(d),
        (Some(d), None) => Some(*d),
        _ => None,
    } && !root.children.is_empty()
        && !st.body_only
    {
        out.push_str(&m.toc(&root, d.min(o.headline_levels)));
    }
    out.push_str(&m.nodes(
        &root.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>(),
        0,
    ));
    for c in &root.children {
        out.push_str(&m.section(c, title_offset));
    }
    out.push_str(&m.footnotes());
    let mut s = out.trim_end().to_string();
    s.push('\n');
    s
}

impl<'a> M<'a> {
    fn toc(&self, root: &Section, depth: usize) -> String {
        fn go(m: &M, secs: &[Section], depth: usize, ind: usize, out: &mut String) {
            for s in secs {
                let num = if s.numbered(&m.o) {
                    format!("{} ", s.number_string())
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "{}- [{}{}](#{})\n",
                    "  ".repeat(ind),
                    num,
                    esc(&s.title_plain()),
                    github_slug(&s.title_plain())
                ));
                if s.depth < depth && s.depth < MAX_SECTION_DEPTH {
                    go(m, &s.children, depth, ind + 1, out);
                }
            }
        }
        let mut out = String::from("## Table of Contents\n\n");
        go(self, &root.children, depth, 0, &mut out);
        out.push('\n');
        out
    }

    fn section(&self, s: &Section, offset: usize) -> String {
        let h = s.heading.unwrap();
        let mut out = String::new();
        if s.depth > self.o.headline_levels {
            let ind = "  ".repeat(s.depth - self.o.headline_levels - 1);
            out.push_str(&format!("{ind}- {}\n", self.heading_text(s, h)));
            let body = self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>(), 0);
            for l in body.trim_end().lines() {
                out.push_str(&format!("{ind}  {l}\n"));
            }
            for c in &s.children {
                if c.depth > MAX_SECTION_DEPTH {
                    continue;
                }
                out.push_str(&self.section(c, offset));
            }
            out.push('\n');
            return out;
        }
        let level = (s.depth + offset).min(6);
        out.push_str(&format!(
            "{} {}\n\n",
            "#".repeat(level),
            self.heading_text(s, h)
        ));
        let body = self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>(), 0);
        out.push_str(&body);
        for c in &s.children {
            if c.depth > MAX_SECTION_DEPTH {
                continue;
            }
            out.push_str(&self.section(c, offset));
        }
        out
    }

    fn heading_text(&self, s: &Section, h: &Heading) -> String {
        let mut t = String::new();
        if s.numbered(&self.o) && s.depth <= self.o.headline_levels {
            t.push_str(&format!("{} ", s.number_string()));
        }
        if let Some(k) = &h.todo
            && self.o.todo
        {
            t.push_str(&format!("{k} "));
        }
        t.push_str(&self.inline(&h.title));
        if self.o.tags && !h.tags.is_empty() {
            t.push_str(&format!(" :{}:", h.tags.join(":")));
        }
        t
    }

    fn nodes(&self, ns: &[Node], _depth: usize) -> String {
        let mut out = String::new();
        for n in ns {
            out.push_str(&self.node(n));
        }
        out
    }

    fn node(&self, n: &Node) -> String {
        match &n.block {
            Block::Heading(_) => String::new(),
            Block::Para(p) => {
                let mut s = String::new();
                if let Some(cap) = &n.aff.caption {
                    let only_img = p.len() == 1
                        && matches!(&p[0], Inline::Link { target, .. } if matches!(self.anchors.classify(target), LinkKind::Image(_)));
                    if !only_img {
                        s.push_str(&format!(
                            "*{}*\n\n",
                            self.inline(&super::parse::inline_of(cap, &self.o))
                        ));
                    }
                }
                s.push_str(self.inline(p).trim_end());
                s.push_str("\n\n");
                if let Some(cap) = &n.aff.caption
                    && p.len() == 1
                    && matches!(&p[0], Inline::Link { target, .. } if matches!(self.anchors.classify(target), LinkKind::Image(_)))
                {
                    s.push_str(&format!(
                        "*{}*\n\n",
                        self.inline(&super::parse::inline_of(cap, &self.o))
                    ));
                }
                s
            }
            Block::List(l) => format!("{}\n", self.list(l, 0)),
            Block::Table(t) => self.table(n, t),
            Block::Src { lang, params, text } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return String::new();
                }
                format!("```{lang}\n{text}\n```\n\n")
            }
            Block::Example(t) => format!("```\n{t}\n```\n\n"),
            Block::Verse(t) => {
                let mut s = String::new();
                for l in t.lines() {
                    s.push_str(&format!(
                        "{}  \n",
                        self.inline(&super::parse::inline_of(l, &self.o))
                    ));
                }
                s.push('\n');
                s
            }
            Block::Quote(c) => {
                let inner = self.nodes(c, 0);
                let mut s = String::new();
                for l in inner.trim_end().lines() {
                    s.push_str(&format!("> {l}\n").replace("> \n", ">\n"));
                }
                s.push('\n');
                s
            }
            Block::Center(c)
            | Block::Special { content: c, .. }
            | Block::Drawer { content: c, .. } => self.nodes(c, 0),
            Block::Export { backend, text } => {
                if backend == "md" || backend == "markdown" || backend == "html" {
                    format!("{text}\n\n")
                } else {
                    String::new()
                }
            }
            Block::Rule => "---\n\n".into(),
            Block::LatexEnv(t) => format!("{t}\n\n"),
            Block::Keyword { key, .. } => {
                if key == "PRINT_BIBLIOGRAPHY" {
                    let list = cite::bibliography(&self.doc.cited, &self.doc.bib);
                    if list.is_empty() {
                        return String::new();
                    }
                    let mut s = String::from("## Bibliography\n\n");
                    for (_, r) in list {
                        s.push_str(&format!("- {}\n", esc(&r)));
                    }
                    s.push('\n');
                    s
                } else {
                    String::new()
                }
            }
        }
    }

    fn list(&self, l: &List, depth: usize) -> String {
        let mut out = String::new();
        let ind = "    ".repeat(depth);
        for (i, it) in l.items.iter().enumerate() {
            let bullet = if l.ordered {
                format!("{}.", it.counter.unwrap_or(i + 1))
            } else {
                "-".to_string()
            };
            let cb = match it.checkbox {
                Some('X') => "[x] ",
                Some(_) => "[ ] ",
                None => "",
            };
            let tag = it
                .tag
                .as_ref()
                .map(|t| format!("**{}**: ", self.inline(t)))
                .unwrap_or_default();
            let mut first = true;
            let pad = " ".repeat(bullet.len() + 1);
            for n in &it.content {
                if first {
                    if let Block::Para(p) = &n.block {
                        out.push_str(&format!(
                            "{ind}{bullet} {cb}{tag}{}\n",
                            self.inline(p)
                                .trim_end()
                                .replace('\n', &format!("\n{ind}{pad}"))
                        ));
                    } else {
                        out.push_str(&format!("{ind}{bullet} {cb}{tag}\n"));
                        out.push_str(&self.nested(n, &format!("{ind}{pad}")));
                    }
                } else if let Block::List(sub) = &n.block {
                    out.push_str(&self.list(sub, depth + 1));
                } else {
                    out.push('\n');
                    out.push_str(&self.nested(n, &format!("{ind}{pad}")));
                }
                first = false;
            }
            if it.content.is_empty() {
                out.push_str(&format!("{ind}{bullet} {cb}{tag}\n"));
            }
        }
        out
    }

    fn nested(&self, n: &Node, pad: &str) -> String {
        let t = self.node(n);
        t.trim_end()
            .lines()
            .map(|l| {
                if l.is_empty() {
                    "\n".to_string()
                } else {
                    format!("{pad}{l}\n")
                }
            })
            .collect()
    }

    fn table(&self, n: &Node, t: &Table) -> String {
        let mut out = String::new();
        if let Some(c) = &n.aff.caption {
            out.push_str(&format!(
                "*{}*\n\n",
                self.inline(&super::parse::inline_of(c, &self.o))
            ));
        }
        let rows: Vec<&Vec<Vec<Inline>>> = t
            .rows
            .iter()
            .filter_map(|r| if let Row::Cells(c) = r { Some(c) } else { None })
            .collect();
        if rows.is_empty() {
            return out;
        }
        let cols = t
            .align
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        let line = |cells: &Vec<Vec<Inline>>| {
            let v: Vec<String> = (0..cols)
                .map(|k| {
                    cells
                        .get(k)
                        .map(|c| self.inline(c).replace('|', "\\|").replace('\n', " "))
                        .unwrap_or_default()
                })
                .collect();
            format!("| {} |\n", v.join(" | "))
        };
        // Markdown tables need a header row: use the first when there's none.
        out.push_str(&line(rows[0]));
        let sep: Vec<String> = (0..cols)
            .map(|k| {
                match t.align.get(k) {
                    Some('r') => "---:",
                    Some('c') => ":---:",
                    _ => "---",
                }
                .to_string()
            })
            .collect();
        out.push_str(&format!("| {} |\n", sep.join(" | ")));
        for r in &rows[1..] {
            out.push_str(&line(r));
        }
        out.push('\n');
        out
    }

    fn footnotes(&self) -> String {
        if !self.o.footnotes || self.doc.footnotes.is_empty() {
            return String::new();
        }
        let mut out = String::from("\n");
        for f in &self.doc.footnotes {
            let body = self.nodes(&f.content, 0);
            let mut lines = body.trim_end().lines();
            out.push_str(&format!(
                "[^{}]: {}\n",
                f.number,
                lines.next().unwrap_or("")
            ));
            for l in lines {
                out.push_str(&format!("    {l}\n"));
            }
        }
        out
    }

    fn inline(&self, v: &[Inline]) -> String {
        let mut s = String::new();
        for i in v {
            match i {
                Inline::Text(t) => s.push_str(&esc(&if self.o.special_strings {
                    special_strings(t)
                } else {
                    t.clone()
                })),
                Inline::Bold(x) => s.push_str(&format!("**{}**", self.inline(x))),
                Inline::Italic(x) => s.push_str(&format!("*{}*", self.inline(x))),
                Inline::Underline(x) => s.push_str(&format!("<u>{}</u>", self.inline(x))),
                Inline::Strike(x) => s.push_str(&format!("~~{}~~", self.inline(x))),
                Inline::Code(t) | Inline::Verbatim(t) => {
                    if t.contains('`') {
                        s.push_str(&format!("`` {t} ``"));
                    } else {
                        s.push_str(&format!("`{t}`"));
                    }
                }
                Inline::Link { target, desc } => s.push_str(&self.link(target, desc.as_deref())),
                Inline::FnRef { number, .. } => {
                    if self.o.footnotes {
                        s.push_str(&format!("[^{number}]"));
                    }
                }
                Inline::Cite(c) => s.push_str(&esc(&cite::render(c, &self.doc.bib))),
                Inline::Latex { text, .. } => s.push_str(text),
                Inline::Entity(n) => {
                    if let Some(e) = entities::find(n) {
                        s.push_str(e.4);
                    }
                }
                Inline::Timestamp { text, .. } => s.push_str(&esc(text)),
                Inline::Sub(x) => s.push_str(&format!("<sub>{}</sub>", self.inline(x))),
                Inline::Sup(x) => s.push_str(&format!("<sup>{}</sup>", self.inline(x))),
                Inline::Break => s.push_str("  \n"),
                Inline::Target(_) => {}
                Inline::Cookie(c) => s.push_str(&format!("`{c}`")),
            }
        }
        s
    }

    fn link(&self, target: &str, desc: Option<&[Inline]>) -> String {
        match self.anchors.classify(target) {
            LinkKind::External(u) => match desc {
                Some(d) => format!("[{}]({})", self.inline(d), link_target(&u)),
                None => format!("<{}>", link_target(&u)),
            },
            LinkKind::Internal(id) => {
                let d = desc
                    .map(|d| self.inline(d))
                    .unwrap_or_else(|| esc(target.trim_start_matches(['*', '#'])));
                format!("[{d}](#{id})")
            }
            LinkKind::File { path, search } => {
                let mut href = path.clone();
                if href.ends_with(".org") {
                    href.truncate(href.len() - 4);
                    href.push_str(".md");
                }
                let _ = search;
                let d = desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&path));
                format!("[{d}]({})", link_target(&href))
            }
            LinkKind::Image(p) => match desc {
                None => format!(
                    "![{}]({})",
                    p.rsplit('/').next().unwrap_or(&p),
                    link_target(&p)
                ),
                Some(d) => format!("[{}]({})", self.inline(d), link_target(&p)),
            },
            LinkKind::Unresolved(t) => desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&t)),
        }
    }
}

/// GitHub's anchor for a heading text.
pub fn github_slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn md(src: &str) -> String {
        export(
            &parse(src, &|_| None),
            &Settings {
                body_only: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn headings_and_inline() {
        let t = md(
            "#+OPTIONS: num:nil toc:nil\n* Head :tag:\ntext *b* /i/ =v= +s+ [[https://x.org][l]] snake\\_x\n** Sub\n",
        );
        assert!(t.contains("# Head :tag:\n"), "{t}");
        assert!(t.contains("text **b** *i* `v` ~~s~~ [l](https://x.org)"));
        assert!(t.contains("## Sub"));
    }

    #[test]
    fn lists_tables_code() {
        let t = md(
            "- a\n  - a1\n- [X] b\n\n1. one\n2. two\n\n| H | N |\n|---+---|\n| x | 1 |\n\n#+BEGIN_SRC rust\nlet x = 1;\n#+END_SRC\n\n#+BEGIN_QUOTE\nq1\n#+END_QUOTE\n",
        );
        assert!(t.contains("- a\n    - a1\n- [x] b\n"), "{t}");
        assert!(t.contains("1. one\n2. two"));
        assert!(t.contains("| H | N |\n| --- | ---: |\n| x | 1 |"));
        assert!(t.contains("```rust\nlet x = 1;\n```"));
        assert!(t.contains("> q1"));
    }

    #[test]
    fn footnotes_images_and_title() {
        let d = parse(
            "#+TITLE: T\n#+AUTHOR: A\n#+OPTIONS: toc:nil\nSee[fn:1] [[file:a.png]]\n\n[fn:1] Note.\n",
            &|_| None,
        );
        let t = export(&d, &Settings::default());
        assert!(t.starts_with("# T\n\n*A*"));
        assert!(t.contains("See[^1] ![a.png](a.png)"));
        assert!(t.contains("[^1]: Note."));
    }

    #[test]
    fn toc_links_headings() {
        let d = parse("#+TITLE: T\n* One Two\n* Three\n", &|_| None);
        let t = export(&d, &Settings::default());
        assert!(t.contains("- [1 One Two](#one-two)"), "{t}");
    }
}
