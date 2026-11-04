// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain text export (ASCII, or UTF-8 with real arrows and boxes).

use super::ast::*;
use super::cite;
use super::common::*;
use super::entities;

const WIDTH: usize = 72;

struct A<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
    utf8: bool,
}

pub fn export(doc: &Doc, st: &Settings, utf8: bool) -> String {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let a = A {
        doc,
        anchors,
        o: o.clone(),
        utf8,
    };
    let mut out = String::new();
    if !st.body_only {
        let center = |s: &str| {
            let w = s.chars().count();
            format!("{}{}", " ".repeat(WIDTH.saturating_sub(w) / 2), s)
        };
        if o.title && !doc.meta.title.is_empty() {
            let t = plain(&super::parse::inline_of(&doc.meta.title, &o));
            out.push_str(&format!(
                "{}\n{}\n",
                center(&t),
                center(&"=".repeat(t.chars().count()))
            ));
            if !doc.meta.subtitle.is_empty() {
                out.push_str(&format!("{}\n", center(&doc.meta.subtitle)));
            }
            out.push('\n');
        }
        if o.author && !doc.meta.author.is_empty() {
            out.push_str(&format!("{}\n", center(&doc.meta.author)));
        }
        if o.email && !doc.meta.email.is_empty() {
            out.push_str(&format!("{}\n", center(&format!("<{}>", doc.meta.email))));
        }
        let d = date_of(doc);
        if o.date && !d.is_empty() {
            out.push_str(&format!("{}\n", center(&d)));
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    let toc = match (&o.toc, toc_keyword(doc)) {
        (_, Some((d, _))) => Some(d),
        (Some(d), None) => Some(*d),
        _ => None,
    };
    if let Some(d) = toc
        && !st.body_only
        && !root.children.is_empty()
    {
        out.push_str("Table of Contents\n");
        out.push_str(&"=".repeat(17));
        out.push('\n');
        let mut all = Vec::new();
        root.all(&mut all);
        for s in all {
            if s.depth > d.min(o.headline_levels) {
                continue;
            }
            let num = if s.numbered(&o) {
                format!("{} ", s.number_string())
            } else {
                String::new()
            };
            out.push_str(&format!(
                "{}{}{}\n",
                "  ".repeat(s.depth - 1),
                num,
                s.title_plain()
            ));
        }
        out.push('\n');
    }
    let body: Vec<Node> = root.body.iter().map(|n| (*n).clone()).collect();
    out.push_str(&a.nodes(&body, 0, WIDTH));
    for c in &root.children {
        out.push_str(&a.section(c));
    }
    out.push_str(&a.footnotes());
    let mut s = out.trim_end().to_string();
    s.push('\n');
    s
}

impl<'a> A<'a> {
    fn section(&self, s: &Section) -> String {
        let h = s.heading.unwrap();
        let mut out = String::new();
        let mut title = String::new();
        if s.numbered(&self.o) {
            title.push_str(&format!("{} ", s.number_string()));
        }
        if let Some(k) = &h.todo
            && self.o.todo
        {
            title.push_str(&format!("{k} "));
        }
        title.push_str(&plain(&h.title));
        if self.o.tags && !h.tags.is_empty() {
            title.push_str(&format!(" :{}:", h.tags.join(":")));
        }
        if s.depth > self.o.headline_levels {
            let ind = "  ".repeat(s.depth - self.o.headline_levels - 1);
            out.push_str(&format!("{ind}- {title}\n"));
            let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
            // The indent can be wider than the page for deep headings.
            let w = WIDTH.saturating_sub(ind.len()).saturating_sub(2);
            for l in self.nodes(&body, 0, w).trim_end().lines() {
                out.push_str(&format!("{ind}  {l}\n"));
            }
            for c in &s.children {
                if c.depth > MAX_SECTION_DEPTH {
                    continue;
                }
                out.push_str(&self.section(c));
            }
            out.push('\n');
            return out;
        }
        let ch = ['=', '-', '~', '^'][(s.depth - 1).min(3)];
        out.push_str(&format!(
            "{title}\n{}\n\n",
            ch.to_string().repeat(title.chars().count())
        ));
        if !h.planning.is_empty() && self.o.timestamps {
            let p: Vec<String> = h
                .planning
                .iter()
                .map(|(k, t)| format!("{k}: {t}"))
                .collect();
            out.push_str(&format!("{}\n\n", p.join(" ")));
        }
        let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
        out.push_str(&self.nodes(&body, 0, WIDTH));
        for c in &s.children {
            if c.depth > MAX_SECTION_DEPTH {
                continue;
            }
            out.push_str(&self.section(c));
        }
        out
    }

    fn nodes(&self, ns: &[Node], ind: usize, width: usize) -> String {
        ns.iter().map(|n| self.node(n, ind, width)).collect()
    }

    fn pad(s: &str, n: usize) -> String {
        s.lines()
            .map(|l| {
                if l.is_empty() {
                    "\n".to_string()
                } else {
                    format!("{}{l}\n", " ".repeat(n))
                }
            })
            .collect()
    }

    fn node(&self, n: &Node, ind: usize, width: usize) -> String {
        let w = width.saturating_sub(ind).max(20);
        let body = match &n.block {
            Block::Heading(_) => String::new(),
            Block::Para(p) => {
                let t = self.inline(p);
                let mut s = String::new();
                if t.contains("\n\n") || p.iter().any(|i| matches!(i, Inline::Break)) {
                    for l in t.split('\n') {
                        s.push_str(&wrap(l, w).join("\n"));
                        s.push('\n');
                    }
                } else {
                    s.push_str(&wrap(&t.replace('\n', " "), w).join("\n"));
                    s.push('\n');
                }
                s.push('\n');
                if let Some(c) = &n.aff.caption {
                    s.push_str(&format!(
                        "Figure: {}\n\n",
                        plain(&super::parse::inline_of(c, &self.o))
                    ));
                }
                s
            }
            Block::List(l) => format!("{}\n", self.list(l, w)),
            Block::Table(t) => self.table(n, t),
            Block::Src { params, text, .. } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return String::new();
                }
                format!("{}\n", Self::pad(text, 2))
            }
            Block::Example(t) => format!("{}\n", Self::pad(t, 2)),
            Block::Verse(t) => format!(
                "{}\n",
                Self::pad(
                    &t.lines()
                        .map(|l| self.inline(&super::parse::inline_of(l, &self.o)))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    2
                )
            ),
            Block::Quote(c) => format!(
                "{}\n",
                Self::pad(self.nodes(c, 0, w.saturating_sub(5)).trim_end(), 5)
            ),
            Block::Center(c) => {
                let inner = self.nodes(c, 0, w.saturating_sub(10));
                let mut s = String::new();
                for l in inner.trim_end().lines() {
                    let lw = l.chars().count();
                    s.push_str(&format!("{}{l}\n", " ".repeat(w.saturating_sub(lw) / 2)));
                }
                format!("{s}\n")
            }
            Block::Special { content: c, .. } | Block::Drawer { content: c, .. } => {
                self.nodes(c, 0, w)
            }
            Block::Export { backend, text } => {
                if backend == "ascii" {
                    format!("{text}\n\n")
                } else {
                    String::new()
                }
            }
            Block::Rule => format!("{}\n\n", "-".repeat(w.min(WIDTH))),
            Block::LatexEnv(t) => format!(
                "{}\n\n",
                Self::pad(&entities::latex_to_unicode(t), 0).trim_end()
            ),
            Block::Keyword { key, .. } => {
                if key == "PRINT_BIBLIOGRAPHY" {
                    let list = cite::bibliography(&self.doc.cited, &self.doc.bib);
                    if list.is_empty() {
                        return String::new();
                    }
                    let mut s = String::from("Bibliography\n============\n\n");
                    for (_, r) in list {
                        s.push_str(&wrap(&r, w).join("\n"));
                        s.push_str("\n\n");
                    }
                    s
                } else {
                    String::new()
                }
            }
        };
        if ind > 0 {
            Self::pad(body.trim_end(), ind) + "\n"
        } else {
            body
        }
    }

    fn list(&self, l: &List, w: usize) -> String {
        let mut out = String::new();
        for (i, it) in l.items.iter().enumerate() {
            let bullet = if l.ordered {
                format!("{}.", it.counter.unwrap_or(i + 1))
            } else if self.utf8 {
                "•".to_string()
            } else {
                "-".to_string()
            };
            let cb = match it.checkbox {
                Some('X') => "[X] ",
                Some('-') => "[-] ",
                Some(_) => "[ ] ",
                None => "",
            };
            let tag = it
                .tag
                .as_ref()
                .map(|t| format!("{} :: ", self.inline(t)))
                .unwrap_or_default();
            let pad = bullet.chars().count() + 1;
            let mut first = true;
            for n in &it.content {
                if first {
                    if let Block::Para(p) = &n.block {
                        let text = format!("{cb}{tag}{}", self.inline(p).replace('\n', " "));
                        let lines = wrap(&text, w.saturating_sub(pad));
                        for (k, line) in lines.iter().enumerate() {
                            if k == 0 {
                                out.push_str(&format!("{bullet} {line}\n"));
                            } else {
                                out.push_str(&format!("{}{line}\n", " ".repeat(pad)));
                            }
                        }
                    } else {
                        out.push_str(&format!("{bullet} {cb}{tag}\n"));
                        out.push_str(&Self::pad(self.node(n, 0, w - pad).trim_end(), pad));
                    }
                } else if let Block::List(sub) = &n.block {
                    out.push_str(&Self::pad(
                        self.list(sub, w.saturating_sub(pad)).trim_end(),
                        pad,
                    ));
                } else {
                    out.push('\n');
                    out.push_str(&Self::pad(
                        self.node(n, 0, w.saturating_sub(pad)).trim_end(),
                        pad,
                    ));
                }
                first = false;
            }
            if it.content.is_empty() {
                out.push_str(&format!("{bullet} {cb}{tag}\n"));
            }
        }
        out
    }

    fn table(&self, n: &Node, t: &Table) -> String {
        let mut rows: Vec<Option<Vec<String>>> = Vec::new();
        for r in &t.rows {
            rows.push(match r {
                Row::Rule => None,
                Row::Cells(c) => Some(
                    c.iter()
                        .map(|x| self.inline(x).replace('\n', " "))
                        .collect(),
                ),
            });
        }
        let cols = t
            .align
            .len()
            .max(rows.iter().flatten().map(|r| r.len()).max().unwrap_or(0));
        let mut widths = vec![1usize; cols];
        for r in rows.iter().flatten() {
            for (k, c) in r.iter().enumerate() {
                widths[k] = widths[k].max(c.chars().count());
            }
        }
        let (v, h, x) = if self.utf8 {
            ('│', '─', '┼')
        } else {
            ('|', '-', '+')
        };
        let rule = |l: char, m: char, r: char| {
            let mut s = String::new();
            s.push(l);
            for (k, wd) in widths.iter().enumerate() {
                s.push_str(&h.to_string().repeat(wd + 2));
                s.push(if k + 1 == widths.len() { r } else { m });
            }
            s
        };
        let mut out = String::new();
        if let Some(c) = &n.aff.caption {
            out.push_str(&format!(
                "Table: {}\n",
                plain(&super::parse::inline_of(c, &self.o))
            ));
        }
        let (tl, tm, tr, ml, mr, bl, bm, br) = if self.utf8 {
            ('┌', '┬', '┐', '├', '┤', '└', '┴', '┘')
        } else {
            ('+', '+', '+', '|', '|', '+', '+', '+')
        };
        let _ = x;
        out.push_str(&rule(tl, tm, tr));
        out.push('\n');
        for r in &rows {
            match r {
                None => out.push_str(&rule(ml, if self.utf8 { '┼' } else { '+' }, mr)),
                Some(c) => {
                    let mut s = String::new();
                    s.push(v);
                    for (k, &w) in widths.iter().enumerate() {
                        let cell = c.get(k).cloned().unwrap_or_default();
                        let pad = w - cell.chars().count();
                        let text = match t.align.get(k) {
                            Some('r') => format!("{}{cell}", " ".repeat(pad)),
                            Some('c') => format!(
                                "{}{cell}{}",
                                " ".repeat(pad / 2),
                                " ".repeat(pad - pad / 2)
                            ),
                            _ => format!("{cell}{}", " ".repeat(pad)),
                        };
                        s.push_str(&format!(" {text} {v}"));
                    }
                    out.push_str(&s);
                }
            }
            out.push('\n');
        }
        out.push_str(&rule(bl, bm, br));
        out.push_str("\n\n");
        out
    }

    fn footnotes(&self) -> String {
        if !self.o.footnotes || self.doc.footnotes.is_empty() {
            return String::new();
        }
        let mut out = String::from("\nFootnotes\n---------\n\n");
        for f in &self.doc.footnotes {
            let body = self.nodes(&f.content, 0, WIDTH - 5);
            let mut lines = body.trim_end().lines();
            out.push_str(&format!("[{}] {}\n", f.number, lines.next().unwrap_or("")));
            for l in lines {
                out.push_str(&format!("    {l}\n"));
            }
            out.push('\n');
        }
        out
    }

    fn inline(&self, v: &[Inline]) -> String {
        let mut s = String::new();
        for i in v {
            match i {
                Inline::Text(t) => s.push_str(&if self.o.special_strings && self.utf8 {
                    special_strings(t)
                } else {
                    t.clone()
                }),
                Inline::Bold(x) => s.push_str(&format!("*{}*", self.inline(x))),
                Inline::Italic(x) => s.push_str(&format!("/{}/", self.inline(x))),
                Inline::Underline(x) => s.push_str(&format!("_{}_", self.inline(x))),
                Inline::Strike(x) => s.push_str(&format!("+{}+", self.inline(x))),
                Inline::Code(t) => s.push_str(&format!("`{t}'")),
                Inline::Verbatim(t) => s.push_str(&format!("\"{t}\"")),
                Inline::Link { target, desc } => match (self.anchors.classify(target), desc) {
                    (LinkKind::External(u), Some(d)) => {
                        s.push_str(&format!("{} ({u})", self.inline(d)))
                    }
                    (LinkKind::External(u), None) => s.push_str(&format!("<{u}>")),
                    (LinkKind::Image(p), None) => s.push_str(&format!("[image: {p}]")),
                    (LinkKind::File { path, .. }, d) => s.push_str(
                        &d.as_ref()
                            .map(|d| format!("{} ({path})", self.inline(d)))
                            .unwrap_or(path),
                    ),
                    (LinkKind::Image(p), Some(d)) => {
                        s.push_str(&format!("{} ({p})", self.inline(d)))
                    }
                    (_, Some(d)) => s.push_str(&self.inline(d)),
                    (LinkKind::Internal(_), None) => {
                        s.push_str(target.trim_start_matches(['*', '#']))
                    }
                    (LinkKind::Unresolved(t), None) => s.push_str(&t),
                },
                Inline::FnRef { number, .. } => {
                    if self.o.footnotes {
                        s.push_str(&format!("[{number}]"));
                    }
                }
                Inline::Cite(c) => s.push_str(&cite::render(c, &self.doc.bib)),
                Inline::Latex { text, .. } => s.push_str(&entities::latex_to_unicode(text)),
                Inline::Entity(n) => {
                    if let Some(e) = entities::find(n) {
                        s.push_str(if self.utf8 { e.4 } else { e.3 });
                    }
                }
                Inline::Timestamp { text, .. } => s.push_str(text),
                Inline::Sub(x) => s.push_str(&format!("_{{{}}}", self.inline(x))),
                Inline::Sup(x) => s.push_str(&format!("^{{{}}}", self.inline(x))),
                Inline::Break => s.push('\n'),
                Inline::Target(_) => {}
                Inline::Cookie(c) => s.push_str(c),
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn txt(src: &str) -> String {
        export(
            &parse(src, &|_| None),
            &Settings {
                body_only: true,
                ..Default::default()
            },
            false,
        )
    }

    #[test]
    fn headings_are_underlined_and_numbered() {
        let t = txt("#+OPTIONS: toc:nil\n* One\ntext\n** Two\nmore\n");
        assert!(t.contains("1 One\n=====\n"), "{t}");
        assert!(t.contains("1.1 Two\n-------\n"));
    }

    #[test]
    fn paragraphs_wrap_and_links_show_urls() {
        let t = txt(&format!("{} [[https://x.org][site]]\n", "word ".repeat(30)));
        assert!(t.lines().all(|l| l.chars().count() <= 72));
        assert!(t.contains("site (https://x.org)"));
    }

    #[test]
    fn lists_and_tables() {
        let t = txt("- one\n- two\n  - nested\n\n1. a\n2. b\n\n| A | B |\n|---+---|\n| 1 | xx |\n");
        assert!(t.contains("- one\n- two\n  - nested"), "{t}");
        assert!(t.contains("1. a\n2. b"));
        assert!(
            t.contains("+---+----+\n| A | B  |\n|---+----|\n| 1 | xx |\n+---+----+"),
            "{t}"
        );
    }

    #[test]
    fn utf8_flavor_uses_real_characters() {
        let d = parse("a \\rarr b \\alpha\n\n| A |\n", &|_| None);
        let t = export(
            &d,
            &Settings {
                body_only: true,
                ..Default::default()
            },
            true,
        );
        assert!(t.contains("a → b α"));
        assert!(t.contains('┌'));
        let t = export(
            &d,
            &Settings {
                body_only: true,
                ..Default::default()
            },
            false,
        );
        assert!(t.contains("a -> b alpha"));
    }

    #[test]
    fn title_block_and_footnotes() {
        let d = parse(
            "#+TITLE: Doc\n#+AUTHOR: Me\n#+OPTIONS: toc:nil\nx[fn:1]\n\n[fn:1] The note\n",
            &|_| None,
        );
        let t = export(&d, &Settings::default(), false);
        assert!(t.trim_start().starts_with("Doc\n"));
        assert!(t.contains("Me"));
        assert!(t.contains("x[1]"));
        assert!(t.contains("Footnotes\n---------\n\n[1] The note"));
    }

    #[test]
    fn blocks_are_indented() {
        let t = txt("#+BEGIN_SRC sh\nls\n#+END_SRC\n\n#+BEGIN_QUOTE\nsaid\n#+END_QUOTE\n");
        assert!(t.contains("  ls\n"));
        assert!(t.contains("     said"));
    }

    /// A document with `n` nested headings.
    fn nested(n: usize) -> String {
        let mut src = String::new();
        for i in 1..=n {
            src.push_str(&"*".repeat(i));
            src.push_str(&format!(" h{i}\n"));
        }
        src
    }

    #[test]
    fn deep_headings_do_not_underflow_the_width() {
        // The indent of a deep heading is wider than the page: the
        // width must not go negative.
        let t = txt(&nested(70));
        assert!(t.contains("h40"), "{t}");
    }

    #[test]
    fn sections_below_the_depth_limit_are_left_out() {
        let t = txt(&nested(70));
        assert!(t.contains("h64"), "{t}");
        assert!(!t.contains("h65"), "{t}");
    }
}
