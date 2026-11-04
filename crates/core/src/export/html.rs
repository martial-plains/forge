// SPDX-License-Identifier: MIT OR Apache-2.0
//! HTML export.

use super::ast::*;
use super::cite;
use super::common::*;
use super::entities;
use std::fmt::Write;

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

const CSS: &str = r#"<style>
  .title { text-align: center; margin-bottom: .2em; }
  .subtitle { text-align: center; font-size: medium; font-weight: bold; margin-top: 0; }
  .todo { font-family: monospace; color: red; }
  .done { font-family: monospace; color: green; }
  .priority { font-family: monospace; color: orange; }
  .tag { background-color: #eee; font-family: monospace; padding: 2px; font-size: 80%; font-weight: normal; }
  .timestamp { color: #bebebe; }
  .timestamp-kwd { color: #5f9ea0; }
  .org-right { margin-left: auto; margin-right: 0px; text-align: right; }
  .org-left { margin-left: 0px; margin-right: auto; text-align: left; }
  .org-center { margin-left: auto; margin-right: auto; text-align: center; }
  .underline { text-decoration: underline; }
  #postamble p, #preamble p { font-size: 90%; margin: .2em; }
  p.verse { margin-left: 3%; }
  pre { border: 1px solid #e6e6e6; border-radius: 3px; background-color: #f2f2f2; padding: 8pt; font-family: monospace; overflow: auto; margin: 1.2em; }
  pre.src { position: relative; overflow: auto; }
  pre.src:before { display: none; position: absolute; top: -8px; right: 12px; padding: 3px; color: #555; background-color: #f2f2f299; }
  pre.src:hover:before { display: inline; }
  pre.src-sh:before { content: 'sh'; }
  pre.src-python:before { content: 'Python'; }
  pre.src-rust:before { content: 'Rust'; }
  pre.src-emacs-lisp:before { content: 'Elisp'; }
  table { border-collapse: collapse; }
  caption.t-above { caption-side: top; }
  caption.t-bottom { caption-side: bottom; }
  td, th { vertical-align: top; }
  th.org-right { text-align: center; }
  th.org-left { text-align: center; }
  th.org-center { text-align: center; }
  td.org-right { text-align: right; }
  td.org-left { text-align: left; }
  td.org-center { text-align: center; }
  dt { font-weight: bold; }
  .footpara { display: inline; }
  .footdef { margin-bottom: 1em; }
  .figure { padding: 1em; }
  .figure p { text-align: center; }
  .equation-container { display: table; text-align: center; width: 100%; }
  .inlinetask { padding: 10px; border: 2px solid gray; margin: 10px; background: #ffffcc; }
  #org-div-home-and-up { text-align: right; font-size: 70%; white-space: nowrap; }
  textarea { overflow-x: auto; }
  .linenr { font-size: smaller; }
  .code-highlighted { background-color: #ffff00; }
  .org-info-js_info-navigation { border-style: none; }
  #content { max-width: 60em; margin: auto; }
</style>"#;

const MATHJAX: &str = r#"<script>
  window.MathJax = { tex: { ams: { multlineWidth: '85%' }, tags: 'ams', tagSide: 'right', tagIndent: '.8em' }, chtml: { scale: 1.0, displayAlign: 'center', displayIndent: '0em' }, svg: { scale: 1.0, displayAlign: 'center', displayIndent: '0em' }, output: { font: 'mathjax-modern', displayOverflow: 'overflow' } };
</script>
<script id="MathJax-script" async src="https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js"></script>"#;

struct H<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
    figs: usize,
    tables: usize,
    listings: usize,
    out: String,
    cited: Vec<String>,
}

pub fn export(doc: &Doc, s: &Settings) -> String {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let mut h = H {
        doc,
        anchors,
        o: o.clone(),
        figs: 0,
        tables: 0,
        listings: 0,
        out: String::new(),
        cited: doc.cited.clone(),
    };
    let mut body = String::new();
    let toc_at = toc_keyword(doc);
    let toc_depth = match (&o.toc, toc_at) {
        (_, Some((d, _))) => Some(d),
        (Some(d), None) => Some(*d),
        _ => None,
    };
    let toc_html = match toc_depth {
        Some(d) if !root.children.is_empty() => h.toc(&root, d.min(o.headline_levels)),
        _ => String::new(),
    };
    h.nodes(&root.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>());
    body.push_str(&std::mem::take(&mut h.out));
    if toc_at.is_some() {
        // The keyword node itself rendered nothing; put the TOC at its place by
        // finding the marker.
        body = body.replace("<!--toc-->", &toc_html);
    }
    for c in &root.children {
        h.section(c);
    }
    body.push_str(&std::mem::take(&mut h.out));
    let mut text = String::new();
    if toc_at.is_none() && !toc_html.is_empty() {
        // Before the first section, after any intro: Org puts it before the content.
        text.push_str(&toc_html);
    }
    text.push_str(&body);
    let foot = h.footnotes();
    text.push_str(&foot);
    if s.body_only {
        return text;
    }
    let mut out = String::new();
    let lang = if doc.meta.language.is_empty() {
        "en"
    } else {
        &doc.meta.language
    };
    let _ = writeln!(
        out,
        "<!DOCTYPE html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\" />",
        esc(lang)
    );
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n");
    let title = if doc.meta.title.is_empty() {
        "&lrm;".to_string()
    } else {
        esc(&plain(&super::parse::inline_of(&doc.meta.title, &o)))
    };
    let _ = writeln!(out, "<title>{title}</title>");
    out.push_str("<meta name=\"generator\" content=\"Forge Org Mode\" />\n");
    if !doc.meta.author.is_empty() && o.author {
        let _ = writeln!(
            out,
            "<meta name=\"author\" content=\"{}\" />",
            esc(&doc.meta.author)
        );
    }
    if !doc.meta.description.is_empty() {
        let _ = writeln!(
            out,
            "<meta name=\"description\" content=\"{}\" />",
            esc(&doc.meta.description)
        );
    }
    if !doc.meta.keywords.is_empty() {
        let _ = writeln!(
            out,
            "<meta name=\"keywords\" content=\"{}\" />",
            esc(&doc.meta.keywords)
        );
    }
    for hd in &doc.meta.html_head {
        out.push_str(hd);
        out.push('\n');
    }
    if !doc.meta.html_head.iter().any(|x| x.contains("<style")) {
        out.push_str(CSS);
        out.push('\n');
    }
    if has_math(doc) {
        out.push_str(MATHJAX);
        out.push('\n');
    }
    out.push_str("</head>\n<body>\n");
    if !doc.meta.html_link_up.is_empty() || !doc.meta.html_link_home.is_empty() {
        let _ = writeln!(
            out,
            "<div id=\"org-div-home-and-up\">\n <a accesskey=\"h\" href=\"{}\"> UP </a>\n |\n <a accesskey=\"H\" href=\"{}\"> HOME </a>\n</div>",
            esc(&doc.meta.html_link_up),
            esc(&doc.meta.html_link_home)
        );
    }
    out.push_str("<div id=\"content\" class=\"content\">\n");
    if o.title && !doc.meta.title.is_empty() {
        let _ = writeln!(
            out,
            "<h1 class=\"title\">{}</h1>",
            h.inline(&super::parse::inline_of(&doc.meta.title, &o))
        );
        if !doc.meta.subtitle.is_empty() {
            let _ = writeln!(
                out,
                "<p class=\"subtitle\" role=\"doc-subtitle\">{}</p>",
                h.inline(&super::parse::inline_of(&doc.meta.subtitle, &o))
            );
        }
    }
    out.push_str(&text);
    out.push_str("</div>\n");
    let date = date_of(doc);
    let mut post = String::new();
    if o.author && !doc.meta.author.is_empty() {
        let _ = writeln!(
            post,
            "<p class=\"author\">Author: {}</p>",
            esc(&doc.meta.author)
        );
    }
    if o.date && !date.is_empty() {
        let _ = writeln!(post, "<p class=\"date\">Date: {}</p>", esc(&date));
    }
    if o.email && !doc.meta.email.is_empty() {
        let _ = writeln!(
            post,
            "<p class=\"email\">Email: <a href=\"mailto:{0}\">{0}</a></p>",
            esc(&doc.meta.email)
        );
    }
    if o.creator {
        post.push_str("<p class=\"creator\"><a href=\"https://orgmode.org\">Org</a> mode export by Forge</p>\n");
    }
    if !post.is_empty() {
        let _ = writeln!(out, "<div id=\"postamble\" class=\"status\">\n{post}</div>");
    }
    out.push_str("</body>\n</html>\n");
    out
}

impl<'a> H<'a> {
    // ------------------------------------------------------- sections

    fn toc(&self, root: &Section, depth: usize) -> String {
        fn list(h: &H, secs: &[Section], depth: usize, out: &mut String) {
            if secs.is_empty() {
                return;
            }
            out.push_str("<ul>\n");
            for s in secs {
                let numbered = s.numbered(&h.o);
                let mut label = String::new();
                if numbered {
                    let _ = write!(
                        label,
                        "<span class=\"section-number-{}\">{}</span> ",
                        s.depth + 1,
                        s.number_string()
                    );
                }
                let head = s.heading.unwrap();
                let _ = write!(label, "{}", h.heading_label(head));
                let _ = write!(out, "<li><a href=\"#{}\">{}</a>", esc(&s.id), label);
                if s.depth < depth && s.depth < MAX_SECTION_DEPTH {
                    let kids: Vec<&Section> = s.children.iter().collect();
                    if !kids.is_empty() {
                        out.push('\n');
                        let owned: Vec<Section> = Vec::new();
                        let _ = owned;
                        sublist(h, &kids, depth, out);
                    }
                }
                out.push_str("</li>\n");
            }
            out.push_str("</ul>\n");
        }
        fn sublist(h: &H, secs: &[&Section], depth: usize, out: &mut String) {
            out.push_str("<ul>\n");
            for s in secs {
                let mut label = String::new();
                if s.numbered(&h.o) {
                    let _ = write!(
                        label,
                        "<span class=\"section-number-{}\">{}</span> ",
                        s.depth + 1,
                        s.number_string()
                    );
                }
                label.push_str(&h.heading_label(s.heading.unwrap()));
                let _ = write!(out, "<li><a href=\"#{}\">{}</a>", esc(&s.id), label);
                if s.depth < depth && s.depth < MAX_SECTION_DEPTH && !s.children.is_empty() {
                    out.push('\n');
                    let kids: Vec<&Section> = s.children.iter().collect();
                    sublist(h, &kids, depth, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str("</ul>\n");
        }
        let mut out = String::new();
        let _ = root;
        out.push_str("<div id=\"table-of-contents\" role=\"doc-toc\">\n<h2>Table of Contents</h2>\n<div id=\"text-table-of-contents\" role=\"doc-toc\">\n");
        let _ = list;
        let kids: Vec<&Section> = root.children.iter().collect();
        sublist(self, &kids, depth, &mut out);
        out.push_str("</div>\n</div>\n");
        out
    }

    fn heading_label(&self, h: &Heading) -> String {
        let mut s = String::new();
        if let Some(t) = &h.todo
            && self.o.todo
        {
            let _ = write!(
                s,
                "<span class=\"{} {}\">{}</span> ",
                if h.done { "done" } else { "todo" },
                esc(t),
                esc(t)
            );
        }
        if let Some(p) = h.priority
            && self.o.priority
        {
            let _ = write!(s, "<span class=\"priority\">[#{p}]</span> ");
        }
        s.push_str(&self.inline(&h.title));
        if self.o.tags && !h.tags.is_empty() {
            s.push_str("&#xa0;&#xa0;&#xa0;<span class=\"tag\">");
            for t in &h.tags {
                let _ = write!(s, "<span class=\"{}\">{}</span>&#xa0;", esc(t), esc(t));
            }
            s.truncate(s.len() - "&#xa0;".len());
            s.push_str("</span>");
        }
        s
    }

    fn section(&mut self, s: &Section) {
        let h = s.heading.unwrap();
        if s.depth > self.o.headline_levels {
            self.item_section(s);
            return;
        }
        let level = (s.depth + 1).min(6);
        let num = if s.numbered(&self.o) {
            format!(
                "<span class=\"section-number-{level}\">{}</span> ",
                s.number_string()
            )
        } else {
            String::new()
        };
        let id = esc(&s.id);
        let _ = writeln!(
            self.out,
            "<div id=\"outline-container-{id}\" class=\"outline-{level}\">"
        );
        let _ = writeln!(
            self.out,
            "<h{level} id=\"{id}\">{num}{}</h{level}>",
            self.heading_label(h)
        );
        let _ = writeln!(
            self.out,
            "<div class=\"outline-text-{level}\" id=\"text-{id}\">"
        );
        self.planning(h);
        let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
        self.nodes(&body);
        // Deeper headings become a list inside.
        let mut items = Vec::new();
        for c in &s.children {
            if c.depth > MAX_SECTION_DEPTH {
                continue;
            }
            if c.depth > self.o.headline_levels {
                items.push(c);
            } else {
                self.out.push_str("</div>\n");
                // The text div closes before children in Org; reopen not needed.
                self.section(c);
                let _ = writeln!(self.out, "<div class=\"outline-text-{level}\">");
            }
        }
        if !items.is_empty() {
            self.out.push_str("<ul class=\"org-ul\">\n");
            for c in items {
                self.item_section(c);
            }
            self.out.push_str("</ul>\n");
        }
        self.out.push_str("</div>\n</div>\n");
    }

    /// A heading too deep for a title: a list item.
    fn item_section(&mut self, s: &Section) {
        let h = s.heading.unwrap();
        let _ = write!(
            self.out,
            "<li><a id=\"{}\"></a>{}",
            esc(&s.id),
            self.heading_label(h)
        );
        self.out.push('\n');
        let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
        self.nodes(&body);
        if !s.children.is_empty() {
            self.out.push_str("<ul class=\"org-ul\">\n");
            for c in &s.children {
                if c.depth > MAX_SECTION_DEPTH {
                    continue;
                }
                self.item_section(c);
            }
            self.out.push_str("</ul>\n");
        }
        self.out.push_str("</li>\n");
    }

    fn planning(&mut self, h: &Heading) {
        if !self.o.timestamps || h.planning.is_empty() {
            return;
        }
        self.out.push_str("<p>\n");
        for (i, (k, t)) in h.planning.iter().enumerate() {
            if i > 0 {
                self.out.push('\n');
            }
            let _ = write!(
                self.out,
                "<span class=\"timestamp-wrapper\"><span class=\"timestamp-kwd\">{k}:</span> <span class=\"timestamp\">{}</span></span>",
                esc(t)
            );
        }
        self.out.push_str("\n</p>\n");
    }

    // --------------------------------------------------------- blocks

    fn nodes(&mut self, ns: &[Node]) {
        for n in ns {
            self.node(n);
        }
    }

    fn caption_html(&self, n: &Node) -> Option<String> {
        n.aff
            .caption
            .as_ref()
            .map(|c| self.inline(&super::parse::inline_of(c, &self.o)))
    }

    fn anchor_attr(&self, n: &Node) -> String {
        n.aff
            .name
            .as_ref()
            .map(|nm| format!(" id=\"org-{}\"", slug(nm)))
            .unwrap_or_default()
    }

    fn node(&mut self, n: &Node) {
        match &n.block {
            Block::Heading(_) => {}
            Block::Para(p) => self.para(n, p),
            Block::List(l) => self.list(l),
            Block::Table(t) => self.table(n, t),
            Block::Src { lang, params, text } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return;
                }
                let cap = self.caption_html(n);
                if let Some(c) = &cap {
                    self.listings += 1;
                    let _ = writeln!(
                        self.out,
                        "<div class=\"org-src-container\">\n<label class=\"org-src-name\"><span class=\"listing-number\">Listing {}: </span>{c}</label>",
                        self.listings
                    );
                } else {
                    self.out.push_str("<div class=\"org-src-container\">\n");
                }
                let cls = if lang.is_empty() {
                    String::new()
                } else {
                    format!(" src-{}", esc(lang))
                };
                let _ = writeln!(
                    self.out,
                    "<pre class=\"src{cls}\"{}>{}</pre>\n</div>",
                    self.anchor_attr(n),
                    esc(text)
                );
            }
            Block::Example(t) => {
                let _ = writeln!(
                    self.out,
                    "<pre class=\"example\"{}>\n{}\n</pre>",
                    self.anchor_attr(n),
                    esc(t)
                );
            }
            Block::Verse(t) => {
                let body: Vec<String> = t
                    .split('\n')
                    .map(|l| {
                        let lead = l.chars().take_while(|c| *c == ' ').count();
                        format!(
                            "{}{}",
                            "&#xa0;".repeat(lead),
                            self.inline(&super::parse::inline_of(l.trim_start(), &self.o))
                        )
                    })
                    .collect();
                let _ = writeln!(
                    self.out,
                    "<p class=\"verse\">\n{}\n</p>",
                    body.join("<br />\n")
                );
            }
            Block::Quote(c) => {
                self.out.push_str("<blockquote>\n");
                self.nodes(c);
                self.out.push_str("</blockquote>\n");
            }
            Block::Center(c) => {
                self.out.push_str("<div class=\"org-center\">\n");
                self.nodes(c);
                self.out.push_str("</div>\n");
            }
            Block::Special {
                name,
                content,
                params,
            } => {
                let cls = esc(name);
                let tag = match name.as_str() {
                    "aside" | "article" | "section" | "header" | "footer" | "nav" | "main"
                    | "details" | "summary" | "figure" => name.as_str(),
                    _ => "div",
                };
                if tag == "div" {
                    let _ = writeln!(
                        self.out,
                        "<div class=\"{cls}\"{}>",
                        if params.is_empty() {
                            String::new()
                        } else {
                            // The parameters are attributes: a quote in
                            // them would end the tag.
                            format!(" {}", esc(params))
                        }
                    );
                } else {
                    let _ = writeln!(self.out, "<{tag}>");
                }
                self.nodes(content);
                let _ = writeln!(self.out, "</{tag}>");
            }
            Block::Export { backend, text } => {
                if backend == "html" {
                    self.out.push_str(text);
                    self.out.push('\n');
                }
            }
            Block::Rule => self.out.push_str("<hr />\n"),
            Block::LatexEnv(t) => {
                let _ = writeln!(
                    self.out,
                    "<div class=\"equation-container\">\n<span class=\"equation\">\n{}\n</span>\n</div>",
                    esc(t)
                );
            }
            Block::Drawer { name, content } => {
                let _ = writeln!(self.out, "<div class=\"drawer {}\">", esc(name));
                self.nodes(content);
                self.out.push_str("</div>\n");
            }
            Block::Keyword { key, value } => match key.as_str() {
                "TOC" => {
                    if value.trim_start().starts_with("headlines") {
                        self.out.push_str("<!--toc-->");
                    }
                }
                "PRINT_BIBLIOGRAPHY" => self.bibliography(),
                _ => {}
            },
        }
    }

    fn bibliography(&mut self) {
        let list = cite::bibliography(&self.cited, &self.doc.bib);
        if list.is_empty() {
            return;
        }
        self.out.push_str(
            "<h2 class=\"bibliography\">Bibliography</h2>\n<div class=\"csl-bib-body\">\n",
        );
        for (k, r) in list {
            let _ = writeln!(
                self.out,
                "<div class=\"csl-entry\" id=\"citeproc_bib_item_{}\">{}</div>",
                esc(&k),
                esc(&r)
            );
        }
        self.out.push_str("</div>\n");
    }

    fn para(&mut self, n: &Node, p: &[Inline]) {
        // A paragraph that is just an image link is a figure.
        let only: Vec<&Inline> = p
            .iter()
            .filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()))
            .collect();
        if only.len() == 1
            && let Inline::Link { target, desc } = only[0]
            && let LinkKind::Image(path) = self.anchors.classify(target)
            && (desc.is_none() || n.aff.caption.is_some() || true)
        {
            let img = self.image(&path, &n.aff, desc.as_ref().map(|d| plain(d)));
            if let Some(cap) = self.caption_html(n) {
                self.figs += 1;
                let _ = writeln!(
                    self.out,
                    "<figure{}>\n<p>{img}</p>\n<figcaption><span class=\"figure-number\">Figure {}: </span>{cap}</figcaption>\n</figure>",
                    self.anchor_attr(n),
                    self.figs
                );
            } else {
                let _ = writeln!(self.out, "<div class=\"figure\"><p>{img}</p></div>");
            }
            return;
        }
        let mut body = self.inline(p);
        body = body.trim_end().to_string();
        let _ = writeln!(self.out, "<p{}>\n{body}\n</p>", self.anchor_attr(n));
    }

    fn image(&self, path: &str, aff: &Aff, alt: Option<String>) -> String {
        let mut attrs = String::new();
        for k in ["width", "height", "align"] {
            if let Some(v) = aff.attr("html", k) {
                if k == "align" {
                    continue;
                }
                let _ = write!(attrs, " {k}=\"{}\"", esc(&v));
            }
        }
        if let Some(a) = aff.attr("html", "alt") {
            let _ = write!(attrs, " alt=\"{}\"", esc(&a));
        } else {
            let _ = write!(
                attrs,
                " alt=\"{}\"",
                esc(&alt.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string()))
            );
        }
        format!("<img src=\"{}\"{attrs} />", esc(path))
    }

    fn list(&mut self, l: &List) {
        let (open, close) = if l.descriptive {
            ("<dl class=\"org-dl\">".to_string(), "</dl>")
        } else if l.ordered {
            let start = l.items.first().and_then(|i| i.counter).filter(|c| *c != 1);
            (
                format!(
                    "<ol class=\"org-ol\"{}>",
                    start.map(|s| format!(" start=\"{s}\"")).unwrap_or_default()
                ),
                "</ol>",
            )
        } else {
            ("<ul class=\"org-ul\">".to_string(), "</ul>")
        };
        let _ = writeln!(self.out, "{open}");
        for it in &l.items {
            let cb = match it.checkbox {
                Some('X') => "<code>[X]</code>&#xa0;",
                Some('-') => "<code>[-]</code>&#xa0;",
                Some(_) => "<code>[&#xa0;]</code>&#xa0;",
                None => "",
            };
            if l.descriptive {
                let tag = it.tag.as_ref().map(|t| self.inline(t)).unwrap_or_default();
                let _ = writeln!(self.out, "<dt>{cb}{tag}</dt>");
                self.out.push_str("<dd>\n");
                self.item_content(&it.content);
                self.out.push_str("</dd>\n");
            } else {
                let val = it
                    .counter
                    .filter(|_| l.ordered)
                    .map(|c| format!(" value=\"{c}\""))
                    .unwrap_or_default();
                let _ = write!(self.out, "<li{val}>{cb}");
                self.item_content(&it.content);
                self.out.push_str("</li>\n");
            }
        }
        let _ = writeln!(self.out, "{close}");
    }

    fn item_content(&mut self, c: &[Node]) {
        let mut first = true;
        for n in c {
            if first && let Block::Para(p) = &n.block {
                let t = self.inline(p);
                self.out.push_str(t.trim_end());
                self.out.push('\n');
            } else {
                self.node(n);
            }
            first = false;
        }
    }

    fn table(&mut self, n: &Node, t: &Table) {
        let cap = self.caption_html(n);
        let _ = writeln!(
            self.out,
            "<table border=\"2\" cellspacing=\"0\" cellpadding=\"6\" rules=\"groups\" frame=\"hsides\"{}>",
            self.anchor_attr(n)
        );
        if let Some(c) = cap {
            self.tables += 1;
            let _ = writeln!(
                self.out,
                "<caption class=\"t-above\"><span class=\"table-number\">Table {}:</span> {c}</caption>",
                self.tables
            );
        }
        let cols = t.align.len();
        self.out.push('\n');
        self.out.push_str("<colgroup>\n");
        for a in &t.align {
            let c = match a {
                'r' => "right",
                'c' => "center",
                _ => "left",
            };
            let _ = writeln!(self.out, "<col class=\"org-{c}\" />");
        }
        self.out.push_str("</colgroup>\n");
        let mut group_open = false;
        let mut header_done = !t.has_header;
        let mut in_head = t.has_header;
        let mut rows_in_group = 0;
        if t.has_header {
            self.out.push_str("<thead>\n");
            group_open = true;
        } else if !t.rows.is_empty() && !matches!(t.rows[0], Row::Rule) {
            self.out.push_str("<tbody>\n");
            group_open = true;
        }
        for r in &t.rows {
            match r {
                Row::Rule => {
                    if group_open {
                        self.out
                            .push_str(if in_head { "</thead>\n" } else { "</tbody>\n" });
                        group_open = false;
                    }
                    if in_head {
                        in_head = false;
                        header_done = true;
                    }
                    rows_in_group = 0;
                }
                Row::Cells(c) => {
                    if !group_open {
                        self.out.push_str("<tbody>\n");
                        group_open = true;
                    }
                    let _ = header_done;
                    self.out.push_str("<tr>\n");
                    for k in 0..cols.max(c.len()) {
                        let a = match t.align.get(k) {
                            Some('r') => "right",
                            Some('c') => "center",
                            _ => "left",
                        };
                        let content = c.get(k).map(|x| self.inline(x)).unwrap_or_default();
                        if in_head {
                            let _ = writeln!(
                                self.out,
                                "<th scope=\"col\" class=\"org-{a}\">{content}</th>"
                            );
                        } else {
                            let _ = writeln!(self.out, "<td class=\"org-{a}\">{content}</td>");
                        }
                    }
                    self.out.push_str("</tr>\n");
                    rows_in_group += 1;
                }
            }
        }
        let _ = rows_in_group;
        if group_open {
            self.out
                .push_str(if in_head { "</thead>\n" } else { "</tbody>\n" });
        }
        self.out.push_str("</table>\n");
    }

    fn footnotes(&mut self) -> String {
        if !self.o.footnotes || self.doc.footnotes.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str("<div id=\"footnotes\">\n<h2 class=\"footnotes\">Footnotes: </h2>\n<div id=\"text-footnotes\">\n\n");
        let notes = self.doc.footnotes.clone();
        for f in notes {
            self.out.clear();
            let n = f.number;
            let _ = write!(
                out,
                "<div class=\"footdef\"><sup><a id=\"fn.{n}\" class=\"footnum\" href=\"#fnr.{n}\" role=\"doc-backlink\">{n}</a></sup> <div class=\"footpara\" role=\"doc-footnote\">"
            );
            let mut first = true;
            for node in &f.content {
                if first && let Block::Para(p) = &node.block {
                    let _ = write!(
                        out,
                        "<p class=\"footpara\">\n{}\n</p>",
                        self.inline(p).trim_end()
                    );
                } else {
                    self.node(node);
                    out.push_str(&std::mem::take(&mut self.out));
                }
                first = false;
            }
            out.push_str("</div></div>\n\n");
        }
        out.push_str("</div>\n</div>\n");
        out
    }

    // --------------------------------------------------------- inline

    fn text(&self, t: &str) -> String {
        if self.o.special_strings {
            esc(&special_strings(t))
        } else {
            esc(t)
        }
    }

    fn inline(&self, v: &[Inline]) -> String {
        let mut s = String::new();
        for i in v {
            match i {
                Inline::Text(t) => s.push_str(&self.text(t)),
                Inline::Bold(x) => {
                    let _ = write!(s, "<b>{}</b>", self.inline(x));
                }
                Inline::Italic(x) => {
                    let _ = write!(s, "<i>{}</i>", self.inline(x));
                }
                Inline::Underline(x) => {
                    let _ = write!(s, "<span class=\"underline\">{}</span>", self.inline(x));
                }
                Inline::Strike(x) => {
                    let _ = write!(s, "<del>{}</del>", self.inline(x));
                }
                Inline::Code(t) | Inline::Verbatim(t) => {
                    let _ = write!(s, "<code>{}</code>", esc(t));
                }
                Inline::Link { target, desc } => s.push_str(&self.link(target, desc.as_deref())),
                Inline::FnRef { number, .. } => {
                    if self.o.footnotes {
                        let _ = write!(
                            s,
                            "<sup><a id=\"fnr.{number}\" class=\"footref\" href=\"#fn.{number}\" role=\"doc-backlink\">{number}</a></sup>"
                        );
                    }
                }
                Inline::Cite(c) => {
                    let keys: Vec<&str> = c.refs.iter().map(|r| r.key.as_str()).collect();
                    let _ = write!(
                        s,
                        "<span class=\"citation\" data-cites=\"{}\">{}</span>",
                        esc(&keys.join(" ")),
                        esc(&cite::render(c, &self.doc.bib))
                    );
                }
                Inline::Latex { text, display } => {
                    let t = if text.starts_with("$$") {
                        format!("\\[{}\\]", &text[2..text.len() - 2])
                    } else if text.starts_with('$') {
                        format!("\\({}\\)", &text[1..text.len() - 1])
                    } else {
                        text.clone()
                    };
                    let _ = display;
                    s.push_str(&esc(&t));
                }
                Inline::Entity(n) => {
                    if let Some(e) = entities::find(n) {
                        s.push_str(e.2);
                    }
                }
                Inline::Timestamp { text, .. } => {
                    if self.o.timestamps {
                        let _ = write!(
                            s,
                            "<span class=\"timestamp-wrapper\"><span class=\"timestamp\">{}</span></span>",
                            esc(text)
                        );
                    }
                }
                Inline::Sub(x) => {
                    let _ = write!(s, "<sub>{}</sub>", self.inline(x));
                }
                Inline::Sup(x) => {
                    let _ = write!(s, "<sup>{}</sup>", self.inline(x));
                }
                Inline::Break => s.push_str("<br />\n"),
                Inline::Target(t) => {
                    let _ = write!(s, "<a id=\"org-target-{}\"></a>", slug(t));
                }
                Inline::Cookie(c) => {
                    let _ = write!(s, "<code>{}</code>", esc(c));
                }
            }
        }
        s
    }

    fn link(&self, target: &str, desc: Option<&[Inline]>) -> String {
        match self.anchors.classify(target) {
            LinkKind::External(u) => {
                let d = desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&u));
                format!("<a href=\"{}\">{d}</a>", esc(&u))
            }
            LinkKind::Internal(id) => {
                let d = desc
                    .map(|d| self.inline(d))
                    .unwrap_or_else(|| esc(target.trim_start_matches(['*', '#'])));
                format!("<a href=\"#{}\">{d}</a>", esc(&id))
            }
            LinkKind::File { path, search } => {
                let mut href = path.clone();
                if href.ends_with(".org") {
                    href.truncate(href.len() - 4);
                    href.push_str(".html");
                }
                if let Some(s) = search
                    && let Some(c) = s.strip_prefix('#')
                {
                    href.push('#');
                    href.push_str(c);
                }
                let d = desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&path));
                format!("<a href=\"{}\">{d}</a>", esc(&href))
            }
            LinkKind::Image(p) => match desc {
                // An image as the description is shown by the caller's inline
                // pass; here the link is the image itself.
                None => self.image(&p, &Aff::default(), None),
                Some(d) => format!("<a href=\"{}\">{}</a>", esc(&p), self.inline(d)),
            },
            LinkKind::Unresolved(t) => desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn html(src: &str) -> String {
        export(
            &parse(src, &|_| None),
            &Settings {
                body_only: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn full_page_has_head_title_and_postamble() {
        let d = parse(
            "#+TITLE: My *Doc*\n#+AUTHOR: Ann\n#+DATE: 2024-01-02\n\n* One\ntext\n",
            &|_| None,
        );
        let h = export(&d, &Settings::default());
        assert!(h.starts_with("<!DOCTYPE html>"));
        assert!(h.contains("<title>My Doc</title>"));
        assert!(h.contains("<h1 class=\"title\">My <b>Doc</b></h1>"));
        assert!(h.contains("<p class=\"author\">Author: Ann</p>"));
        assert!(h.contains("<p class=\"date\">Date: 2024-01-02</p>"));
        assert!(h.contains("table-of-contents"));
        assert!(h.contains("<h2 id=\"sec-1\"><span class=\"section-number-2\">1</span> One</h2>"));
    }

    #[test]
    fn inline_markup_and_escaping() {
        let h = html("a <b> & *bold* /it/ =v= ~c~ +del+ _u_ x^2 H_{2}O --- ok");
        assert!(h.contains(
            "a &lt;b&gt; &amp; <b>bold</b> <i>it</i> <code>v</code> <code>c</code> <del>del</del>"
        ));
        assert!(h.contains("<span class=\"underline\">u</span>"));
        assert!(h.contains("x<sup>2</sup> H<sub>2</sub>O \u{2014} ok"));
    }

    #[test]
    fn lists_tables_blocks() {
        let h = html(
            "- a\n- [X] b\n\n1. one\n\n- t :: d\n\n| H1 | H2 |\n|----+----|\n| x  | 10 |\n\n#+BEGIN_SRC python\nprint('<hi>')\n#+END_SRC\n\n#+BEGIN_QUOTE\nQ\n#+END_QUOTE",
        );
        assert!(h.contains("<ul class=\"org-ul\">\n<li>a\n</li>"));
        assert!(h.contains("<code>[X]</code>&#xa0;b"));
        assert!(h.contains("<ol class=\"org-ol\">"));
        assert!(h.contains("<dt>t</dt>"));
        assert!(
            h.contains("<thead>") && h.contains("<th scope=\"col\" class=\"org-left\">H1</th>")
        );
        assert!(h.contains("<td class=\"org-right\">10</td>"));
        assert!(h.contains("<pre class=\"src src-python\">print('&lt;hi&gt;')</pre>"));
        assert!(h.contains("<blockquote>"));
    }

    #[test]
    fn links_images_and_figures() {
        let h = html(
            "[[https://x.org][site]] [[#cid][in]]\n\n#+CAPTION: A pic\n#+ATTR_HTML: :width 300\n[[file:p.png]]\n",
        );
        assert!(h.contains("<a href=\"https://x.org\">site</a>"));
        assert!(h.contains("<figure>"));
        assert!(h.contains("<img src=\"p.png\" width=\"300\" alt=\"p.png\" />"));
        assert!(h.contains("<span class=\"figure-number\">Figure 1: </span>A pic"));
        let h = html("[[file:other.org][other]]");
        assert!(h.contains("href=\"other.html\""));
    }

    #[test]
    fn footnotes_and_citations() {
        let d = parse(
            "#+BIBLIOGRAPHY: r.bib\nText[fn:1] and [cite:@k].\n\n[fn:1] The note.\n\n#+PRINT_BIBLIOGRAPHY:\n",
            &|f| {
                (f == "r.bib").then(|| {
                    "@book{k, author={Doe, J}, title={Book}, year={2001}, publisher={P}}"
                        .to_string()
                })
            },
        );
        let h = export(
            &d,
            &Settings {
                body_only: true,
                ..Default::default()
            },
        );
        assert!(h.contains("<sup><a id=\"fnr.1\" class=\"footref\" href=\"#fn.1\""));
        assert!(h.contains("<p class=\"footpara\">\nThe note.\n</p>"));
        assert!(h.contains("<span class=\"citation\" data-cites=\"k\">(Doe 2001)</span>"));
        assert!(h.contains("csl-entry"));
        assert!(h.contains("Doe, J. (2001). Book. P."));
    }

    #[test]
    fn todo_tags_planning_and_deep_levels() {
        let h = html(
            "#+OPTIONS: H:1 toc:nil\n* TODO Task :work:\nDEADLINE: <2024-05-06 Mon>\nbody\n** sub item\nmore\n",
        );
        assert!(h.contains("<span class=\"todo TODO\">TODO</span>"));
        assert!(h.contains("<span class=\"tag\"><span class=\"work\">work</span></span>"));
        assert!(h.contains("<span class=\"timestamp-kwd\">DEADLINE:</span>"));
        // Level 2 is past H:1, so a list item.
        assert!(h.contains("<ul class=\"org-ul\">\n<li><a id=\"sec-1-1\"></a>sub item"));
    }

    #[test]
    fn latex_fragments_use_mathjax_delimiters() {
        let d = parse("Math $x^2$ and $$y$$ and \\(z\\).\n", &|_| None);
        let h = export(&d, &Settings::default());
        assert!(h.contains("MathJax"));
        assert!(h.contains("\\(x^2\\)") && h.contains("\\[y\\]") && h.contains("\\(z\\)"));
    }

    #[test]
    fn src_exports_option_and_export_blocks() {
        let h = html(
            "#+BEGIN_SRC sh :exports none\nsecret\n#+END_SRC\n#+BEGIN_EXPORT html\n<i>raw</i>\n#+END_EXPORT\n#+BEGIN_EXPORT latex\nnope\n#+END_EXPORT",
        );
        assert!(!h.contains("secret") && !h.contains("nope"));
        assert!(h.contains("<i>raw</i>"));
    }

    #[test]
    fn toc_keyword_places_the_toc() {
        let h = html("intro\n\n#+TOC: headlines 1\n\n* A\n** A1\n* B\n");
        let toc = h.find("table-of-contents").unwrap();
        let a = h.find("outline-container-sec-1").unwrap();
        assert!(toc < a);
        assert!(!h.contains("A1</a>"), "{h}");
    }

    #[test]
    fn special_block_parameters_do_not_break_the_tag() {
        let h = html("#+BEGIN_foo :attr \"quoted\" & more\ntext\n#+END_FOO\n");
        assert!(
            h.contains("<div class=\"foo\" :attr &quot;quoted&quot; &amp; more>"),
            "{h}"
        );
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
    fn deep_headings_do_not_overflow_the_stack() {
        let h = html(&nested(200));
        assert!(h.contains("h64"), "{h}");
        // Sections below the depth limit are left out.
        assert!(!h.contains("h65"), "{h}");
    }
}
