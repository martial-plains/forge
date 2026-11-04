// SPDX-License-Identifier: MIT OR Apache-2.0
//! Texinfo export (for `makeinfo`).

use super::ast::*;
use super::cite;
use super::common::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

struct T<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
    /// Section id to node name.
    nodes: HashMap<String, String>,
    seen_fn: RefCell<HashSet<usize>>,
}

pub fn esc(s: &str) -> String {
    s.replace('@', "@@").replace('{', "@{").replace('}', "@}")
}

fn node_name(title: &str, used: &mut HashSet<String>) -> String {
    let mut n: String = title
        .chars()
        .filter(|c| !matches!(c, ',' | ':' | '.' | '(' | ')' | '@' | '{' | '}' | '\n'))
        .collect();
    n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        n = "Section".into();
    }
    let base = n.clone();
    let mut k = 1;
    while !used.insert(n.to_lowercase()) {
        k += 1;
        n = format!("{base} {k}");
    }
    n
}

pub fn export(doc: &Doc, st: &Settings) -> String {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let mut used: HashSet<String> = HashSet::new();
    used.insert("top".into());
    let mut names = HashMap::new();
    let mut all = Vec::new();
    root.all(&mut all);
    for s in &all {
        names.insert(s.id.clone(), node_name(&s.title_plain(), &mut used));
    }
    let t = T {
        doc,
        anchors,
        o: o.clone(),
        nodes: names,
        seen_fn: RefCell::new(HashSet::new()),
    };
    let title_plain = plain(&super::parse::inline_of(&doc.meta.title, &o));
    let mut body = String::new();
    body.push_str(&t.nodes(&root.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
    for c in &root.children {
        body.push_str(&t.section(c));
    }
    if st.body_only {
        return body;
    }
    let file = if doc.meta.texinfo_filename.is_empty() {
        format!(
            "{}.info",
            st.source
                .rsplit('/')
                .next()
                .unwrap_or("document")
                .trim_end_matches(".org")
        )
    } else {
        doc.meta.texinfo_filename.clone()
    };
    let mut out = String::new();
    out.push_str("\\input texinfo    @c -*- texinfo -*-\n@c %**start of header\n");
    let _ = writeln!(out, "@setfilename {file}");
    out.push_str("@documentencoding UTF-8\n");
    let _ = writeln!(out, "@settitle {}", esc(&title_plain));
    out.push_str("@c %**end of header\n");
    if !doc.meta.texinfo_dir_category.is_empty() {
        let _ = writeln!(out, "@dircategory {}", doc.meta.texinfo_dir_category);
        let _ = writeln!(
            out,
            "@direntry\n* {}: ({}).    {}\n@end direntry",
            doc.meta.texinfo_dir_title,
            file.trim_end_matches(".info"),
            doc.meta.texinfo_dir_desc
        );
    }
    out.push_str("\n@finalout\n@titlepage\n");
    let _ = writeln!(out, "@title {}", esc(&title_plain));
    if o.author && !doc.meta.author.is_empty() {
        let _ = writeln!(out, "@author {}", esc(&doc.meta.author));
    }
    out.push_str("@end titlepage\n\n@contents\n\n");
    out.push_str("@ifnottex\n@node Top\n");
    let _ = writeln!(out, "@top {}", esc(&title_plain));
    out.push_str("@end ifnottex\n\n");
    if !root.children.is_empty() {
        out.push_str("@menu\n");
        for c in &root.children {
            let n = &t.nodes[&c.id];
            let _ = writeln!(out, "* {n}::");
        }
        out.push_str("@end menu\n\n");
    }
    out.push_str(&body);
    out.push_str("\n@bye\n");
    out
}

impl<'a> T<'a> {
    fn section(&self, s: &Section) -> String {
        let h = s.heading.unwrap();
        let mut out = String::new();
        let limit = self.o.headline_levels.min(4);
        let numbered = s.numbered(&self.o);
        let mut title = String::new();
        if let Some(k) = &h.todo
            && self.o.todo
        {
            let _ = write!(title, "{} ", esc(k));
        }
        title.push_str(&self.inline(&h.title));
        if s.depth > limit {
            let _ = writeln!(out, "@itemize @bullet\n@item\n{title}");
            out.push_str(&self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
            for c in &s.children {
                if c.depth > MAX_SECTION_DEPTH {
                    continue;
                }
                out.push_str(&self.section(c));
            }
            out.push_str("@end itemize\n\n");
            return out;
        }
        let cmd = match (s.depth, numbered) {
            (1, true) => "chapter",
            (2, true) => "section",
            (3, true) => "subsection",
            (_, true) => "subsubsection",
            (1, false) => "unnumbered",
            (2, false) => "unnumberedsec",
            (3, false) => "unnumberedsubsec",
            (_, false) => "unnumberedsubsubsec",
        };
        let _ = writeln!(out, "@node {}", self.nodes[&s.id]);
        let _ = writeln!(out, "@{cmd} {title}");
        let kids: Vec<&Section> = s.children.iter().filter(|c| c.depth <= limit).collect();
        if !kids.is_empty() {
            out.push_str("\n@menu\n");
            for c in &kids {
                let _ = writeln!(out, "* {}::", self.nodes[&c.id]);
            }
            out.push_str("@end menu\n");
        }
        out.push('\n');
        if self.o.timestamps && !h.planning.is_empty() {
            let p: Vec<String> = h
                .planning
                .iter()
                .map(|(k, t)| format!("@strong{{{k}:}} @emph{{{}}}", esc(t)))
                .collect();
            let _ = writeln!(out, "{}\n", p.join(" "));
        }
        out.push_str(&self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
        for c in &s.children {
            if c.depth > MAX_SECTION_DEPTH {
                continue;
            }
            out.push_str(&self.section(c));
        }
        out
    }

    fn nodes(&self, ns: &[Node]) -> String {
        ns.iter().map(|n| self.node(n)).collect()
    }

    fn node(&self, n: &Node) -> String {
        match &n.block {
            Block::Heading(_) => String::new(),
            Block::Para(p) => {
                let mut s = String::new();
                if let Some(c) = &n.aff.caption {
                    let _ = write!(
                        s,
                        "@strong{{{}}}\n\n",
                        self.inline(&super::parse::inline_of(c, &self.o))
                    );
                }
                let _ = write!(s, "{}\n\n", self.inline(p).trim_end());
                s
            }
            Block::List(l) => format!("{}\n", self.list(l)),
            Block::Table(t) => self.table(n, t),
            Block::Src { lang, params, text } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return String::new();
                }
                let env = if matches!(lang.as_str(), "emacs-lisp" | "elisp" | "lisp") {
                    "lisp"
                } else {
                    "example"
                };
                format!("@{env}\n{}\n@end {env}\n\n", esc(text))
            }
            Block::Example(t) => format!("@example\n{}\n@end example\n\n", esc(t)),
            Block::Verse(t) => format!("@display\n{}\n@end display\n\n", esc(t)),
            Block::Quote(c) => format!("@quotation\n{}@end quotation\n\n", self.nodes(c)),
            Block::Center(c) => {
                self.nodes(c)
                    .trim_end()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|l| format!("@center {l}\n"))
                    .collect::<String>()
                    + "\n"
            }
            Block::Special { content, .. } | Block::Drawer { content, .. } => self.nodes(content),
            Block::Export { backend, text } => {
                if backend == "texinfo" {
                    format!("{text}\n")
                } else {
                    String::new()
                }
            }
            Block::Rule => "@sp 1\n".into(),
            Block::LatexEnv(t) => format!(
                "@example\n{}\n@end example\n\n",
                esc(&super::entities::latex_to_unicode(t))
            ),
            Block::Keyword { key, .. } => {
                if key == "PRINT_BIBLIOGRAPHY" {
                    let list = cite::bibliography(&self.doc.cited, &self.doc.bib);
                    if list.is_empty() {
                        return String::new();
                    }
                    let mut s = String::from("@heading Bibliography\n\n@itemize @w{}\n");
                    for (_, r) in list {
                        let _ = writeln!(s, "@item\n{}", esc(&r));
                    }
                    s.push_str("@end itemize\n\n");
                    s
                } else {
                    String::new()
                }
            }
        }
    }

    fn list(&self, l: &List) -> String {
        let mut s = if l.descriptive {
            "@table @asis\n".to_string()
        } else if l.ordered {
            "@enumerate\n".to_string()
        } else {
            "@itemize @bullet\n".to_string()
        };
        for it in &l.items {
            let cb = match it.checkbox {
                Some('X') => "[X] ",
                Some('-') => "[-] ",
                Some(_) => "[ ] ",
                None => "",
            };
            if l.descriptive {
                let _ = writeln!(
                    s,
                    "@item {}",
                    it.tag.as_ref().map(|t| self.inline(t)).unwrap_or_default()
                );
            } else {
                s.push_str("@item\n");
            }
            let mut first = true;
            for n in &it.content {
                if first && let Block::Para(p) = &n.block {
                    let _ = writeln!(s, "{cb}{}", self.inline(p).trim_end());
                } else {
                    s.push_str(&self.node(n));
                }
                first = false;
            }
        }
        s.push_str(if l.descriptive {
            "@end table\n"
        } else if l.ordered {
            "@end enumerate\n"
        } else {
            "@end itemize\n"
        });
        s
    }

    fn table(&self, n: &Node, t: &Table) -> String {
        let cols = t.align.len().max(1);
        let frac = format!(".{}", 100 / cols.max(1)).replace(".100", ".99");
        let mut s = String::new();
        if let Some(c) = &n.aff.caption {
            let _ = write!(
                s,
                "@strong{{{}}}\n\n",
                self.inline(&super::parse::inline_of(c, &self.o))
            );
        }
        let _ = writeln!(
            s,
            "@multitable @columnfractions {}",
            vec![frac; cols].join(" ")
        );
        let mut head = t.has_header;
        for r in &t.rows {
            match r {
                Row::Rule => head = false,
                Row::Cells(c) => {
                    let cells: Vec<String> = c.iter().map(|x| self.inline(x)).collect();
                    let _ = writeln!(
                        s,
                        "@{} {}",
                        if head { "headitem" } else { "item" },
                        cells.join(" @tab ")
                    );
                }
            }
        }
        s.push_str("@end multitable\n\n");
        s
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
                Inline::Bold(x) => {
                    let _ = write!(s, "@strong{{{}}}", self.inline(x));
                }
                Inline::Italic(x) => {
                    let _ = write!(s, "@emph{{{}}}", self.inline(x));
                }
                Inline::Underline(x) => {
                    let _ = write!(s, "@underline{{{}}}", self.inline(x));
                }
                Inline::Strike(x) => s.push_str(&self.inline(x)),
                Inline::Code(t) => {
                    let _ = write!(s, "@code{{{}}}", esc(t));
                }
                Inline::Verbatim(t) => {
                    let _ = write!(s, "@samp{{{}}}", esc(t));
                }
                Inline::Link { target, desc } => match self.anchors.classify(target) {
                    LinkKind::External(u) => match desc {
                        Some(d) => {
                            let _ = write!(
                                s,
                                "@uref{{{},{}}}",
                                u.replace(',', "@comma{}"),
                                self.inline(d)
                            );
                        }
                        None => {
                            let _ = write!(s, "@uref{{{}}}", u.replace(',', "@comma{}"));
                        }
                    },
                    LinkKind::Internal(id) => match self.nodes.get(&id) {
                        Some(n) => match desc {
                            Some(d) => {
                                let _ = write!(s, "@ref{{{n},,{}}}", self.inline(d));
                            }
                            None => {
                                let _ = write!(s, "@ref{{{n}}}");
                            }
                        },
                        None => s.push_str(
                            &desc
                                .as_ref()
                                .map(|d| self.inline(d))
                                .unwrap_or_else(|| esc(target)),
                        ),
                    },
                    LinkKind::Image(p) => {
                        let stem = p
                            .rsplit_once('.')
                            .map(|(a, _)| a.to_string())
                            .unwrap_or(p.clone());
                        let _ = write!(s, "@image{{{stem}}}");
                    }
                    LinkKind::File { path, .. } => s.push_str(
                        &desc
                            .as_ref()
                            .map(|d| self.inline(d))
                            .unwrap_or_else(|| format!("@file{{{}}}", esc(&path))),
                    ),
                    LinkKind::Unresolved(t) => s.push_str(
                        &desc
                            .as_ref()
                            .map(|d| self.inline(d))
                            .unwrap_or_else(|| esc(&t)),
                    ),
                },
                Inline::FnRef { number, .. } => {
                    if !self.o.footnotes {
                        continue;
                    }
                    if self.seen_fn.borrow_mut().insert(*number) {
                        let content = self
                            .doc
                            .footnotes
                            .iter()
                            .find(|f| f.number == *number)
                            .map(|f| self.nodes(&f.content).trim().replace("\n\n", "\n"))
                            .unwrap_or_default();
                        let _ = write!(s, "@footnote{{{content}}}");
                    } else {
                        let _ = write!(s, "[{number}]");
                    }
                }
                Inline::Cite(c) => s.push_str(&esc(&cite::render(c, &self.doc.bib))),
                Inline::Latex { text, .. } => {
                    let u = esc(&super::entities::latex_to_unicode(text));
                    let _ = write!(s, "@math{{{u}}}");
                }
                Inline::Entity(n) => {
                    if let Some(e) = super::entities::find(n) {
                        s.push_str(&esc(e.4));
                    }
                }
                Inline::Timestamp { text, .. } => {
                    let _ = write!(s, "@emph{{{}}}", esc(text));
                }
                Inline::Sub(x) => {
                    let _ = write!(s, "_{{{}}}", self.inline(x));
                }
                Inline::Sup(x) => {
                    let _ = write!(s, "^{{{}}}", self.inline(x));
                }
                Inline::Break => s.push_str("@*\n"),
                Inline::Target(t) => {
                    let _ = write!(s, "@anchor{{{}}}", esc(t));
                }
                Inline::Cookie(c) => {
                    let _ = write!(s, "@code{{{}}}", esc(c));
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn info(src: &str) -> String {
        export(
            &parse(src, &|_| None),
            &Settings {
                source: "doc.org".into(),
                ..Default::default()
            },
        )
    }

    #[test]
    fn skeleton_nodes_and_menus() {
        let t = info("#+TITLE: Manual\n#+AUTHOR: Me\n* Intro\ntext\n** Details\nmore\n* Usage\n");
        assert!(t.starts_with("\\input texinfo"));
        assert!(t.contains("@setfilename doc.info"));
        assert!(t.contains("@settitle Manual"));
        assert!(t.contains("@title Manual") && t.contains("@author Me"));
        assert!(t.contains("@node Top\n@top Manual"));
        assert!(t.contains("@menu\n* Intro::\n* Usage::\n@end menu"));
        assert!(t.contains("@node Intro\n@chapter Intro"));
        assert!(t.contains("@node Details\n@section Details"));
        assert!(t.trim_end().ends_with("@bye"));
    }

    #[test]
    fn duplicate_titles_get_distinct_nodes() {
        let t = info("* A\n* A\n");
        assert!(t.contains("@node A\n") && t.contains("@node A 2\n"));
    }

    #[test]
    fn inline_lists_tables() {
        let t = info(
            "* S\n*b* /i/ =v= ~c~ [[https://x.org][site]] @ {x}\n\n- a\n- b\n\n1. x\n\n- t :: d\n\n| H | N |\n|---+---|\n| a | 1 |\n\n#+BEGIN_SRC sh\nls @\n#+END_SRC\n",
        );
        assert!(
            t.contains("@strong{b} @emph{i} @samp{v} @code{c} @uref{https://x.org,site} @@ @{x@}"),
            "{t}"
        );
        assert!(t.contains("@itemize @bullet\n@item\na\n@item\nb\n@end itemize"));
        assert!(t.contains("@enumerate") && t.contains("@table @asis\n@item t\nd\n@end table"));
        assert!(t.contains("@multitable @columnfractions .50 .50\n@headitem H @tab N\n@item a @tab 1\n@end multitable"), "{t}");
        assert!(t.contains("@example\nls @@\n@end example"));
    }

    #[test]
    fn footnotes_are_inline() {
        let t = info("* S\nWord[fn:1] end\n\n[fn:1] The note.\n");
        assert!(t.contains("Word@footnote{The note.} end"), "{t}");
    }
}
