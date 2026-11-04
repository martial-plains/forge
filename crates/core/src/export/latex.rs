// SPDX-License-Identifier: MIT OR Apache-2.0
//! LaTeX export, and Beamer slides built on it.

use super::ast::*;
use super::cite::{self, Cite};
use super::common::*;
use super::entities;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Proc {
    Basic,
    Biblatex,
    Natbib,
}

struct L<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
    beamer: bool,
    proc: Proc,
    seen_fn: RefCell<HashSet<usize>>,
    class: String,
}

/// The characters `\verb` can use as its delimiter, in order. The
/// delimiter must not occur in the text the environment holds.
const VERB_DELIMITERS: [char; 12] = ['|', '+', '!', '@', '#', '=', '?', ';', ':', ',', '.', '/'];

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\textbackslash{}"),
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            '~' => o.push_str("\\textasciitilde{}"),
            '^' => o.push_str("\\^{}"),
            '<' => o.push_str("\\textless{}"),
            '>' => o.push_str("\\textgreater{}"),
            '|' => o.push_str("\\textbar{}"),
            c => o.push(c),
        }
    }
    o
}

fn lst_language(l: &str) -> Option<&'static str> {
    Some(match l.to_lowercase().as_str() {
        "python" | "py" => "Python",
        "sh" | "bash" | "shell" | "zsh" => "bash",
        "c" => "C",
        "cpp" | "c++" => "C++",
        "java" => "Java",
        "sql" => "SQL",
        "html" => "HTML",
        "xml" => "XML",
        "ruby" => "Ruby",
        "perl" => "Perl",
        "lua" => "Lua",
        "javascript" | "js" => "JavaScript",
        "haskell" => "Haskell",
        "make" | "makefile" => "make",
        "tex" | "latex" => "TeX",
        "r" => "R",
        "go" => "Go",
        "rust" => "Rust",
        "emacs-lisp" | "elisp" | "lisp" => "Lisp",
        _ => return None,
    })
}

pub fn export(doc: &Doc, st: &Settings, beamer: bool) -> String {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let proc = match doc.meta.cite_export.first().map(String::as_str) {
        Some("biblatex") => Proc::Biblatex,
        Some("natbib") | Some("bibtex") => Proc::Natbib,
        _ => Proc::Basic,
    };
    let class = if beamer {
        "beamer".to_string()
    } else if doc.meta.latex_class.is_empty() {
        "article".into()
    } else {
        doc.meta.latex_class.clone()
    };
    let l = L {
        doc,
        anchors,
        o: o.clone(),
        beamer,
        proc,
        seen_fn: RefCell::new(HashSet::new()),
        class: class.clone(),
    };
    let toc_kw = toc_keyword(doc);
    let mut body = String::new();
    body.push_str(&l.nodes(&root.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
    for c in &root.children {
        body.push_str(&l.section(c, l.frame_level()));
    }
    if st.body_only {
        return body;
    }
    let mut out = String::new();
    let opts = if doc.meta.latex_class_options.is_empty() {
        if beamer {
            "[presentation]".to_string()
        } else {
            "[11pt]".to_string()
        }
    } else {
        let s = doc.meta.latex_class_options.trim();
        if s.starts_with('[') {
            s.to_string()
        } else {
            format!("[{s}]")
        }
    };
    let _ = writeln!(out, "\\documentclass{opts}{{{class}}}");
    out.push_str("\\usepackage[utf8]{inputenc}\n\\usepackage[T1]{fontenc}\n\\usepackage{graphicx}\n\\usepackage{longtable}\n\\usepackage{wrapfig}\n\\usepackage{rotating}\n\\usepackage[normalem]{ulem}\n\\usepackage{amsmath}\n\\usepackage{amssymb}\n\\usepackage{capt-of}\n\\usepackage{hyperref}\n");
    let lang = match doc.meta.language.as_str() {
        "de" => Some("ngerman"),
        "fr" => Some("french"),
        "es" => Some("spanish"),
        "it" => Some("italian"),
        "nl" => Some("dutch"),
        "pt" => Some("portuguese"),
        _ => None,
    };
    if let Some(b) = lang {
        let _ = writeln!(out, "\\usepackage[{b}]{{babel}}");
    }
    let has_src = any_node(
        doc,
        &|b| matches!(b, Block::Src { params, .. } if exports_of(params) != "none" && exports_of(params) != "results"),
    );
    if has_src {
        out.push_str("\\usepackage{listings}\n\\lstset{basicstyle=\\ttfamily\\small,breaklines=true,columns=fullflexible}\n");
    }
    match proc {
        Proc::Biblatex => {
            out.push_str("\\usepackage[backend=biber]{biblatex}\n");
            for b in &doc.meta.bibliography {
                let _ = writeln!(out, "\\addbibresource{{{b}}}");
            }
        }
        Proc::Natbib => out.push_str("\\usepackage{natbib}\n"),
        Proc::Basic => {}
    }
    if beamer {
        for (k, v) in [
            ("usetheme", &doc.meta.beamer_theme),
            ("usecolortheme", &doc.meta.beamer_color_theme),
            ("usefonttheme", &doc.meta.beamer_font_theme),
        ] {
            if !v.is_empty() {
                let _ = writeln!(out, "\\{k}{{{v}}}");
            }
        }
        for h in &doc.meta.beamer_header {
            let _ = writeln!(out, "{h}");
        }
    }
    for h in &doc.meta.latex_headers {
        let _ = writeln!(out, "{h}");
    }
    let title = esc(&plain(&super::parse::inline_of(&doc.meta.title, &o)));
    let author = if o.author {
        esc(&doc.meta.author)
    } else {
        String::new()
    };
    let date = date_of(doc);
    let _ = writeln!(out, "\\author{{{author}}}");
    let _ = writeln!(
        out,
        "\\date{{{}}}",
        if !o.date {
            String::new()
        } else if date.is_empty() {
            "\\today".into()
        } else {
            esc(&date)
        }
    );
    let _ = writeln!(out, "\\title{{{title}}}");
    if !doc.meta.subtitle.is_empty() && beamer {
        let _ = writeln!(out, "\\subtitle{{{}}}", esc(&doc.meta.subtitle));
    }
    let _ = writeln!(
        out,
        "\\hypersetup{{\n pdfauthor={{{author}}},\n pdftitle={{{title}}},\n pdfkeywords={{{}}},\n pdfsubject={{{}}},\n pdfcreator={{Forge Org Mode}},\n pdflang={{{}}}}}",
        esc(&doc.meta.keywords),
        esc(&doc.meta.description),
        if doc.meta.language.is_empty() {
            "English"
        } else {
            &doc.meta.language
        }
    );
    out.push_str("\\begin{document}\n\n");
    if o.title && !doc.meta.title.is_empty() {
        out.push_str("\\maketitle\n");
    }
    let toc = match (&o.toc, toc_kw) {
        (_, Some((d, _))) => Some(d),
        (Some(d), None) => Some(*d),
        _ => None,
    };
    if toc_kw.is_none()
        && let Some(d) = toc
        && !root.children.is_empty()
    {
        if d < usize::MAX && !beamer {
            let _ = writeln!(out, "\\setcounter{{tocdepth}}{{{}}}", d.min(5));
        }
        out.push_str(if beamer {
            "\\begin{frame}{Outline}\n\\tableofcontents\n\\end{frame}\n\n"
        } else {
            "\\tableofcontents\n\n"
        });
    }
    out.push_str(&body.replace(
        "%%TOC%%",
        if beamer {
            "\\begin{frame}{Outline}\n\\tableofcontents\n\\end{frame}"
        } else {
            "\\tableofcontents"
        },
    ));
    if proc == Proc::Natbib
        && !doc.meta.bibliography.is_empty()
        && !body.contains("\\bibliography{")
    {
        // The reference list only where PRINT_BIBLIOGRAPHY put it.
    }
    out.push_str("\n\\end{document}\n");
    out
}

impl<'a> L<'a> {
    fn frame_level(&self) -> usize {
        if self.o.headline_levels_set {
            self.o.headline_levels
        } else {
            1
        }
    }

    fn commands(&self) -> &'static [&'static str] {
        if self.beamer {
            &["section", "subsection", "subsubsection"]
        } else if matches!(self.class.as_str(), "report" | "book") {
            &[
                "chapter",
                "section",
                "subsection",
                "subsubsection",
                "paragraph",
                "subparagraph",
            ]
        } else {
            &[
                "section",
                "subsection",
                "subsubsection",
                "paragraph",
                "subparagraph",
            ]
        }
    }

    fn heading_title(&self, h: &Heading) -> String {
        let mut t = String::new();
        if let Some(k) = &h.todo
            && self.o.todo
        {
            let _ = write!(t, "\\textsc{{{}}} ", esc(k));
        }
        if let Some(p) = h.priority
            && self.o.priority
        {
            let _ = write!(t, "\\framebox{{\\#{p}}} ");
        }
        t.push_str(&self.inline(&h.title));
        if self.o.tags && !h.tags.is_empty() {
            let _ = write!(t, "\\hfill{{}}\\textsc{{{}}}", esc(&h.tags.join(":")));
        }
        t
    }

    fn label(&self, s: &Section) -> String {
        format!("\\label{{{}}}", s.id)
    }

    fn section(&self, s: &Section, frame_level: usize) -> String {
        if s.depth > MAX_SECTION_DEPTH {
            return String::new();
        }
        if self.beamer {
            return self.beamer_section(s, frame_level);
        }
        let h = s.heading.unwrap();
        let cmds = self.commands();
        let mut out = String::new();
        let limit = self.o.headline_levels.min(cmds.len());
        if s.depth > limit {
            let _ = writeln!(out, "\\begin{{itemize}}\n\\item {}", self.heading_title(h));
            out.push_str(&self.label(s));
            out.push('\n');
            out.push_str(&self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
            for c in &s.children {
                out.push_str(&self.section(c, frame_level));
            }
            out.push_str("\\end{itemize}\n");
            return out;
        }
        let star = if s.numbered(&self.o) { "" } else { "*" };
        let _ = writeln!(
            out,
            "\\{}{star}{{{}}}",
            cmds[s.depth - 1],
            self.heading_title(h)
        );
        out.push_str(&self.label(s));
        out.push('\n');
        if self.o.timestamps && !h.planning.is_empty() {
            let p: Vec<String> = h
                .planning
                .iter()
                .map(|(k, t)| format!("\\textbf{{{k}:}} \\textit{{{}}}", esc(t)))
                .collect();
            let _ = writeln!(out, "{}\n", p.join(" "));
        }
        let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
        out.push_str(&self.nodes(&body));
        // Children too deep for a heading go in one list.
        let mut deep = false;
        for c in &s.children {
            if c.depth > limit && !deep {
                // handled by section() itself, each becomes its own itemize
            }
            deep = true;
            out.push_str(&self.section(c, frame_level));
        }
        out
    }

    fn beamer_section(&self, s: &Section, frame_level: usize) -> String {
        if s.depth > MAX_SECTION_DEPTH {
            return String::new();
        }
        let h = s.heading.unwrap();
        let env = h.prop("BEAMER_ENV").unwrap_or("").to_string();
        let mut out = String::new();
        let body: Vec<Node> = s.body.iter().map(|n| (*n).clone()).collect();
        let kids = |me: &Self| {
            s.children
                .iter()
                .map(|c| me.beamer_section(c, frame_level))
                .collect::<String>()
        };
        if env == "ignoreheading" {
            out.push_str(&self.nodes(&body));
            out.push_str(&kids(self));
            return out;
        }
        if env == "appendix" {
            out.push_str("\\appendix\n");
            out.push_str(&self.nodes(&body));
            out.push_str(&kids(self));
            return out;
        }
        if env == "note" || env == "noteNH" {
            let _ = writeln!(out, "\\note{{{}}}", self.nodes(&body).trim());
            return out;
        }
        let title = self.heading_title(h);
        if s.depth < frame_level && env.is_empty() {
            let cmds = self.commands();
            let _ = writeln!(
                out,
                "\\{}{{{title}}}",
                cmds[(s.depth - 1).min(cmds.len() - 1)]
            );
            out.push_str(&self.nodes(&body));
            out.push_str(&kids(self));
            return out;
        }
        let opt = h
            .prop("BEAMER_OPT")
            .map(|o| o.trim_matches(['[', ']']).to_string());
        if s.depth == frame_level || matches!(env.as_str(), "frame" | "fullframe") {
            let fragile = {
                let tmp = Doc {
                    nodes: s.body.iter().map(|n| (*n).clone()).collect(),
                    ..Default::default()
                };
                any_node(&tmp, &|b| {
                    matches!(b, Block::Src { .. } | Block::Example(_))
                })
            };
            let mut opts: Vec<String> = Vec::new();
            if fragile {
                opts.push("fragile".into());
            }
            if env == "fullframe" {
                opts.push("plain".into());
            }
            if let Some(o) = opt {
                opts.push(o);
            }
            let o = if opts.is_empty() {
                String::new()
            } else {
                format!("[{}]", opts.join(","))
            };
            let t = if env == "fullframe" {
                String::new()
            } else {
                format!("{{{title}}}")
            };
            let _ = writeln!(out, "\\begin{{frame}}{o}{t}");
            out.push_str(&self.nodes(&body));
            // Columns group runs of `column` children.
            let mut in_cols = false;
            for c in &s.children {
                let cenv = c.heading.and_then(|h| h.prop("BEAMER_ENV")).unwrap_or("");
                if cenv == "columns" {
                    out.push_str("\\begin{columns}\n");
                    out.push_str(
                        &c.children
                            .iter()
                            .map(|k| self.beamer_section(k, frame_level))
                            .collect::<String>(),
                    );
                    out.push_str("\\end{columns}\n");
                    continue;
                }
                if cenv == "column" && !in_cols {
                    out.push_str("\\begin{columns}\n");
                    in_cols = true;
                } else if cenv != "column" && in_cols {
                    out.push_str("\\end{columns}\n");
                    in_cols = false;
                }
                out.push_str(&self.beamer_section(c, frame_level));
            }
            if in_cols {
                out.push_str("\\end{columns}\n");
            }
            out.push_str("\\end{frame}\n");
            return out;
        }
        let col = h.prop("BEAMER_COL");
        match env.as_str() {
            "column" => {
                let w = col.unwrap_or("0.5");
                let w = if w.ends_with("\\textwidth") || w.ends_with("\\linewidth") {
                    w.to_string()
                } else {
                    format!("{w}\\textwidth")
                };
                let _ = writeln!(out, "\\begin{{column}}{{{w}}}");
                out.push_str(&self.nodes(&body));
                out.push_str(&kids(self));
                out.push_str("\\end{column}\n");
            }
            "block" | "alertblock" | "exampleblock" | "" | "theorem" | "definition" | "example"
            | "proof" | "lemma" => {
                let e = if env.is_empty() {
                    "block"
                } else {
                    env.as_str()
                };
                let with_title = matches!(e, "block" | "alertblock" | "exampleblock");
                if with_title {
                    let _ = writeln!(out, "\\begin{{{e}}}{{{title}}}");
                } else {
                    let _ = writeln!(out, "\\begin{{{e}}}");
                }
                out.push_str(&self.nodes(&body));
                out.push_str(&kids(self));
                let _ = writeln!(out, "\\end{{{e}}}");
            }
            other => {
                let _ = writeln!(out, "\\begin{{{other}}}");
                out.push_str(&self.nodes(&body));
                out.push_str(&kids(self));
                let _ = writeln!(out, "\\end{{{other}}}");
            }
        }
        out
    }

    // --------------------------------------------------------- blocks

    fn nodes(&self, ns: &[Node]) -> String {
        ns.iter().map(|n| self.node(n)).collect()
    }

    fn caption(&self, n: &Node) -> Option<String> {
        n.aff
            .caption
            .as_ref()
            .map(|c| self.inline(&super::parse::inline_of(c, &self.o)))
    }

    fn node(&self, n: &Node) -> String {
        match &n.block {
            Block::Heading(_) => String::new(),
            Block::Para(p) => self.para(n, p),
            Block::List(l) => self.list(l),
            Block::Table(t) => self.table(n, t),
            Block::Src { lang, params, text } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return String::new();
                }
                let mut s = String::new();
                let cap = self.caption(n);
                let lg = lst_language(lang)
                    .map(|l| format!("language={l}"))
                    .unwrap_or_default();
                let mut opts = vec![lg];
                if let Some(c) = &cap {
                    opts.push(format!("caption={{{c}}}"));
                }
                opts.retain(|o| !o.is_empty());
                let o = if opts.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", opts.join(","))
                };
                let _ = write!(s, "\\begin{{lstlisting}}{o}\n{text}\n\\end{{lstlisting}}\n");
                s
            }
            Block::Example(t) => format!("\\begin{{verbatim}}\n{t}\n\\end{{verbatim}}\n"),
            Block::Verse(t) => {
                let lines: Vec<String> = t
                    .lines()
                    .map(|l| self.inline(&super::parse::inline_of(l, &self.o)))
                    .collect();
                format!(
                    "\\begin{{verse}}\n{}\n\\end{{verse}}\n",
                    lines.join("\\\\\n")
                )
            }
            Block::Quote(c) => format!("\\begin{{quote}}\n{}\\end{{quote}}\n", self.nodes(c)),
            Block::Center(c) => format!("\\begin{{center}}\n{}\\end{{center}}\n", self.nodes(c)),
            Block::Special { name, content, .. } => {
                if name == "notes" {
                    return format!("\\note{{{}}}\n", self.nodes(content).trim());
                }
                if name == "abstract" {
                    return format!(
                        "\\begin{{abstract}}\n{}\\end{{abstract}}\n",
                        self.nodes(content)
                    );
                }
                let env = name.clone();
                format!("\\begin{{{env}}}\n{}\\end{{{env}}}\n", self.nodes(content))
            }
            Block::Export { backend, text } => {
                if backend == "latex" || (self.beamer && backend == "beamer") {
                    format!("{text}\n")
                } else {
                    String::new()
                }
            }
            Block::Rule => "\\rule{\\linewidth}{0.8pt}\n\n".into(),
            Block::LatexEnv(t) => format!("{t}\n\n"),
            Block::Drawer { content, .. } => self.nodes(content),
            Block::Keyword { key, .. } => match key.as_str() {
                "TOC" => "%%TOC%%\n".into(),
                "PRINT_BIBLIOGRAPHY" => self.bibliography(),
                _ => String::new(),
            },
        }
    }

    fn bibliography(&self) -> String {
        match self.proc {
            Proc::Biblatex => "\\printbibliography\n".into(),
            Proc::Natbib => {
                let files: Vec<String> = self
                    .doc
                    .meta
                    .bibliography
                    .iter()
                    .map(|b| b.trim_end_matches(".bib").to_string())
                    .collect();
                format!(
                    "\\bibliographystyle{{plain}}\n\\bibliography{{{}}}\n",
                    files.join(",")
                )
            }
            Proc::Basic => {
                let list = cite::bibliography(&self.doc.cited, &self.doc.bib);
                if list.is_empty() {
                    return String::new();
                }
                let mut s = String::from("\\begin{thebibliography}{99}\n");
                for (k, r) in list {
                    let _ = writeln!(s, "\\bibitem{{{k}}} {}", esc(&r));
                }
                s.push_str("\\end{thebibliography}\n");
                s
            }
        }
    }

    fn para(&self, n: &Node, p: &[Inline]) -> String {
        let only: Vec<&Inline> = p
            .iter()
            .filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()))
            .collect();
        if only.len() == 1
            && let Inline::Link { target, .. } = only[0]
            && let LinkKind::Image(path) = self.anchors.classify(target)
        {
            let w = n
                .aff
                .attr("latex", "width")
                .unwrap_or_else(|| ".9\\linewidth".into());
            let extra = n
                .aff
                .attr("latex", "options")
                .map(|o| format!(",{o}"))
                .unwrap_or_default();
            let img = format!("\\includegraphics[width={w}{extra}]{{{}}}", esc(&path));
            return match self.caption(n) {
                Some(c) => {
                    let lbl = n
                        .aff
                        .name
                        .as_ref()
                        .map(|nm| format!("\\label{{org-{}}}\n", slug(nm)))
                        .unwrap_or_default();
                    format!(
                        "\\begin{{figure}}[htbp]\n\\centering\n{img}\n\\caption{{{c}}}\n{lbl}\\end{{figure}}\n\n"
                    )
                }
                None => format!("\\begin{{center}}\n{img}\n\\end{{center}}\n\n"),
            };
        }
        let lbl = n
            .aff
            .name
            .as_ref()
            .map(|nm| format!("\\label{{org-{}}}\n", slug(nm)))
            .unwrap_or_default();
        format!("{lbl}{}\n\n", self.inline(p).trim_end())
    }

    fn list(&self, l: &List) -> String {
        let env = if l.descriptive {
            "description"
        } else if l.ordered {
            "enumerate"
        } else {
            "itemize"
        };
        let mut s = format!("\\begin{{{env}}}\n");
        for it in &l.items {
            let cb = match it.checkbox {
                Some('X') => "$\\boxtimes$ ",
                Some('-') => "$\\boxminus$ ",
                Some(_) => "$\\square$ ",
                None => "",
            };
            let tag = it
                .tag
                .as_ref()
                .map(|t| format!("[{}] ", self.inline(t)))
                .unwrap_or_default();
            let ctr = it
                .counter
                .filter(|_| l.ordered)
                .map(|c| format!("[{c}.] "))
                .unwrap_or_default();
            let mut first = true;
            let mut text = String::new();
            for n in &it.content {
                if first && let Block::Para(p) = &n.block {
                    text.push_str(self.inline(p).trim_end());
                    text.push('\n');
                } else {
                    text.push_str(&self.node(n));
                }
                first = false;
            }
            let _ = write!(s, "\\item {ctr}{tag}{cb}{text}");
        }
        let _ = writeln!(s, "\\end{{{env}}}");
        s
    }

    fn table(&self, n: &Node, t: &Table) -> String {
        let cols = t.align.len();
        let spec: String = (0..cols)
            .map(|k| t.align.get(k).copied().unwrap_or('l'))
            .collect();
        let mut tab = format!("\\begin{{tabular}}{{{spec}}}\n");
        for r in &t.rows {
            match r {
                Row::Rule => tab.push_str("\\hline\n"),
                Row::Cells(c) => {
                    let cells: Vec<String> = (0..cols.max(c.len()))
                        .map(|k| c.get(k).map(|x| self.inline(x)).unwrap_or_default())
                        .collect();
                    let _ = writeln!(tab, "{}\\\\", cells.join(" & "));
                }
            }
        }
        tab.push_str("\\end{tabular}\n");
        match self.caption(n) {
            Some(c) => {
                let lbl = n
                    .aff
                    .name
                    .as_ref()
                    .map(|nm| format!("\\label{{org-{}}}\n", slug(nm)))
                    .unwrap_or_default();
                format!(
                    "\\begin{{table}}[htbp]\n\\caption{{{c}}}\n{lbl}\\centering\n{tab}\\end{{table}}\n\n"
                )
            }
            None => format!("\\begin{{center}}\n{tab}\\end{{center}}\n\n"),
        }
    }

    // --------------------------------------------------------- inline

    fn text(&self, t: &str) -> String {
        let t = esc(t);
        if self.o.special_strings {
            t.replace("...", "\\ldots{}")
        } else {
            t
        }
    }

    fn cite(&self, c: &Cite) -> String {
        if self.proc == Proc::Basic {
            return esc(&cite::render(c, &self.doc.bib));
        }
        let bare = c.variant.starts_with('b');
        let cmd = match (self.proc, c.style.as_str()) {
            (Proc::Biblatex, "t" | "text") => "textcite",
            (Proc::Biblatex, "a" | "author") => "citeauthor",
            (Proc::Biblatex, "na" | "noauthor") => "autocite*",
            (Proc::Biblatex, "n" | "nocite") => "nocite",
            (Proc::Biblatex, _) => {
                if bare {
                    "cite"
                } else {
                    "autocite"
                }
            }
            (_, "t" | "text") => "citet",
            (_, "a" | "author") => "citeauthor",
            (_, "na" | "noauthor") => "citeyearpar",
            (_, "n" | "nocite") => "nocite",
            (_, _) => {
                if bare {
                    "citealp"
                } else {
                    "citep"
                }
            }
        };
        let keys: Vec<&str> = c.refs.iter().map(|r| r.key.as_str()).collect();
        if cmd == "nocite" {
            return format!("\\nocite{{{}}}", keys.join(","));
        }
        if c.refs.len() == 1 || self.proc == Proc::Natbib {
            let r = &c.refs[0];
            let pre = if c.refs.len() == 1 {
                r.prefix.clone()
            } else {
                c.prefix.clone()
            };
            let post = if c.refs.len() == 1 {
                r.suffix.clone()
            } else {
                c.suffix.clone()
            };
            let args = match (pre.is_empty(), post.is_empty()) {
                (true, true) => String::new(),
                (_, false) => format!("[{}][{}]", esc(&pre), esc(&post)),
                (false, true) => format!("[{}][]", esc(&pre)),
            };
            return format!("\\{cmd}{args}{{{}}}", keys.join(","));
        }
        // Several keys with their own notes: biblatex's plural commands.
        let mut s = format!("\\{cmd}s");
        for r in &c.refs {
            let args = if r.prefix.is_empty() && r.suffix.is_empty() {
                String::new()
            } else {
                format!("[{}][{}]", esc(&r.prefix), esc(&r.suffix))
            };
            let _ = write!(s, "{args}{{{}}}", r.key);
        }
        s
    }

    fn inline(&self, v: &[Inline]) -> String {
        let mut s = String::new();
        for i in v {
            match i {
                Inline::Text(t) => s.push_str(&self.text(t)),
                Inline::Bold(x) => {
                    let _ = write!(s, "\\textbf{{{}}}", self.inline(x));
                }
                Inline::Italic(x) => {
                    let _ = write!(s, "\\emph{{{}}}", self.inline(x));
                }
                Inline::Underline(x) => {
                    let _ = write!(s, "\\uline{{{}}}", self.inline(x));
                }
                Inline::Strike(x) => {
                    let _ = write!(s, "\\sout{{{}}}", self.inline(x));
                }
                Inline::Code(t) => {
                    let _ = write!(s, "\\texttt{{{}}}", esc(t));
                }
                Inline::Verbatim(t) => {
                    // `\verb` needs a delimiter the text does not hold.
                    match VERB_DELIMITERS.iter().find(|d| !t.contains(**d)) {
                        Some(d) => {
                            let _ = write!(s, "\\verb{d}{t}{d}");
                        }
                        None => {
                            let _ = write!(s, "\\texttt{{{}}}", esc(t));
                        }
                    }
                }
                Inline::Link { target, desc } => s.push_str(&self.link(target, desc.as_deref())),
                Inline::FnRef { number, .. } => {
                    if !self.o.footnotes {
                        continue;
                    }
                    let first = self.seen_fn.borrow_mut().insert(*number);
                    if first {
                        let content = self
                            .doc
                            .footnotes
                            .iter()
                            .find(|f| f.number == *number)
                            .map(|f| self.nodes(&f.content).trim().to_string())
                            .unwrap_or_default();
                        let _ = write!(s, "\\footnote{{{content}}}");
                    } else {
                        let _ = write!(s, "\\footnotemark[{number}]");
                    }
                }
                Inline::Cite(c) => s.push_str(&self.cite(c)),
                Inline::Latex { text, .. } => {
                    if text.starts_with("$$") {
                        let _ = write!(s, "\\[{}\\]", &text[2..text.len() - 2]);
                    } else {
                        s.push_str(text);
                    }
                }
                Inline::Entity(n) => {
                    if let Some(e) = entities::find(n) {
                        s.push_str(e.1);
                    }
                }
                Inline::Timestamp { text, .. } => {
                    if self.o.timestamps {
                        let _ = write!(s, "\\textit{{{}}}", esc(text));
                    }
                }
                Inline::Sub(x) => {
                    let _ = write!(s, "\\textsubscript{{{}}}", self.inline(x));
                }
                Inline::Sup(x) => {
                    let _ = write!(s, "\\textsuperscript{{{}}}", self.inline(x));
                }
                Inline::Break => s.push_str("\\\\\n"),
                Inline::Target(t) => {
                    let _ = write!(s, "\\label{{org-target-{}}}", slug(t));
                }
                Inline::Cookie(c) => {
                    let _ = write!(s, "\\texttt{{{}}}", esc(c));
                }
            }
        }
        s
    }

    fn link(&self, target: &str, desc: Option<&[Inline]>) -> String {
        match self.anchors.classify(target) {
            // The targets go into LaTeX arguments: a `_` or a `#` in
            // one would end the argument early.
            LinkKind::External(u) => match desc {
                Some(d) => format!("\\href{{{}}}{{{}}}", esc(&u), self.inline(d)),
                None => format!("\\url{{{}}}", esc(&u)),
            },
            LinkKind::Internal(id) => match desc {
                Some(d) => format!("\\hyperref[{id}]{{{}}}", self.inline(d)),
                None => format!("\\ref{{{id}}}"),
            },
            LinkKind::File { path, .. } => match desc {
                Some(d) => format!("\\href{{{}}}{{{}}}", esc(&path), self.inline(d)),
                None => format!("\\texttt{{{}}}", esc(&path)),
            },
            LinkKind::Image(p) => match desc {
                None => format!("\\includegraphics[width=.9\\linewidth]{{{}}}", esc(&p)),
                Some(d) => format!("\\href{{{}}}{{{}}}", esc(&p), self.inline(d)),
            },
            LinkKind::Unresolved(t) => desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;

    fn tex(src: &str) -> String {
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
    fn preamble_and_title() {
        let d = parse(
            "#+TITLE: T & U\n#+AUTHOR: Me\n#+LATEX_CLASS: report\n#+LATEX_CLASS_OPTIONS: [a4paper]\n#+LATEX_HEADER: \\usepackage{foo}\n* A\n",
            &|_| None,
        );
        let t = export(&d, &Settings::default(), false);
        assert!(t.starts_with("\\documentclass[a4paper]{report}"));
        assert!(t.contains("\\usepackage{foo}"));
        assert!(t.contains("\\title{T \\& U}"));
        assert!(t.contains("\\author{Me}"));
        assert!(t.contains("\\date{\\today}"));
        assert!(t.contains("\\maketitle") && t.contains("\\tableofcontents"));
        assert!(t.contains("\\chapter{A}"));
        assert!(t.trim_end().ends_with("\\end{document}"));
    }

    #[test]
    fn headings_markup_and_escapes() {
        let t = tex(
            "* TODO One :x:\ntext *b* /i/ =v= ~c~ _u_ +s+ 50% $x^2$ H_{2}O a_b\n** Two\n*** Three\n**** Four\n***** Five\n",
        );
        assert!(
            t.contains("\\section{\\textsc{TODO} One\\hfill{}\\textsc{x}}"),
            "{t}"
        );
        assert!(t.contains("\\textbf{b} \\emph{i} \\verb|v| \\texttt{c} \\uline{u} \\sout{s} 50\\% $x^2$ H\\textsubscript{2}O"));
        assert!(t.contains("\\subsection{Two}") && t.contains("\\subsubsection{Three}"));
        // Past H:3 a heading becomes an item.
        assert!(t.contains("\\begin{itemize}\n\\item Four"), "{t}");
    }

    #[test]
    fn lists_tables_figures_blocks() {
        let t = tex(
            "- a\n- [X] b\n\n1. x\n\n- k :: v\n\n#+CAPTION: Cap\n| A | B |\n|---+---|\n| 1 | 2 |\n\n#+CAPTION: Pic\n[[file:p.png]]\n\n#+BEGIN_SRC python\nx = 1\n#+END_SRC\n\n#+BEGIN_QUOTE\nQ\n#+END_QUOTE\n",
        );
        assert!(
            t.contains("\\begin{itemize}\n\\item a\n\\item $\\boxtimes$ b"),
            "{t}"
        );
        assert!(
            t.contains("\\begin{enumerate}") && t.contains("\\begin{description}\n\\item [k] v")
        );
        assert!(
            t.contains("\\begin{table}[htbp]\n\\caption{Cap}")
                && t.contains("\\begin{tabular}{rr}")
                && t.contains("A & B\\\\\n\\hline\n1 & 2\\\\")
        );
        assert!(
            t.contains("\\begin{figure}[htbp]")
                && t.contains("\\includegraphics[width=.9\\linewidth]{p.png}")
                && t.contains("\\caption{Pic}")
        );
        assert!(t.contains("\\begin{lstlisting}[language=Python]\nx = 1\n\\end{lstlisting}"));
        assert!(t.contains("\\begin{quote}\nQ"));
    }

    #[test]
    fn links_footnotes_and_entities() {
        let t = tex(
            "[[https://x.org][site]] [[https://y.org]] \\alpha \\to see[fn:1] again[fn:1]\n\n[fn:1] Note *here*.\n",
        );
        assert!(
            t.contains("\\href{https://x.org}{site} \\url{https://y.org} $\\alpha$ $\\to$"),
            "{t}"
        );
        assert!(t.contains("\\footnote{Note \\textbf{here}.}") && t.contains("\\footnotemark[1]"));
    }

    #[test]
    fn link_targets_are_escaped() {
        let t = tex("[[https://x.org/a_b][site]] [[https://x.org/c_d]] [[file:a_b.png][pic]]\n");
        assert!(t.contains("\\href{https://x.org/a\\_b}{site}"), "{t}");
        assert!(t.contains("\\url{https://x.org/c\\_d}"), "{t}");
        assert!(t.contains("\\href{a\\_b.png}{pic}"), "{t}");
    }

    #[test]
    fn verbatim_picks_a_delimiter_the_text_has_not() {
        // Every delimiter is in the text: the environment would end at
        // the first one.
        let mut doc = Doc::default();
        doc.nodes.push(Node::new(Block::Para(vec![Inline::Verbatim(
            "|+!@#=?;:,./".to_string(),
        )])));
        let t = export(
            &doc,
            &Settings {
                body_only: true,
                ..Default::default()
            },
            false,
        );
        assert!(t.contains("\\texttt{\\textbar{}+!@\\#=?;:,./}"), "{t}");
        // A text with only one of them still gets a verb.
        let t = tex("=a|b=\n");
        assert!(t.contains("\\verb+a|b+"), "{t}");
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
        let t = tex(&nested(200));
        assert!(t.contains("h64"), "{t}");
        // Sections below the depth limit are left out.
        assert!(!t.contains("h65"), "{t}");
    }

    #[test]
    fn citation_processors() {
        let bib = |f: &str| {
            (f == "r.bib").then(|| "@book{k, author={Doe, J}, title={T}, year={2001}}".to_string())
        };
        let src = "#+BIBLIOGRAPHY: r.bib\nA [cite:@k] B [cite/t:see @k p. 3] C [cite/n:@k].\n#+PRINT_BIBLIOGRAPHY:\n";
        let basic = export(
            &parse(src, &bib),
            &Settings {
                body_only: true,
                ..Default::default()
            },
            false,
        );
        assert!(
            basic.contains("(Doe 2001)")
                && basic.contains("see Doe (2001, p. 3)")
                && basic.contains("\\begin{thebibliography}")
        );
        let bl = export(
            &parse(&format!("#+CITE_EXPORT: biblatex\n{src}"), &bib),
            &Settings::default(),
            false,
        );
        assert!(
            bl.contains("\\autocite{k}")
                && bl.contains("\\textcite[see][p. 3]{k}")
                && bl.contains("\\nocite{k}"),
            "{bl}"
        );
        assert!(
            bl.contains("\\usepackage[backend=biber]{biblatex}")
                && bl.contains("\\addbibresource{r.bib}")
                && bl.contains("\\printbibliography")
        );
        let nb = export(
            &parse(&format!("#+CITE_EXPORT: natbib\n{src}"), &bib),
            &Settings::default(),
            false,
        );
        assert!(
            nb.contains("\\citep{k}")
                && nb.contains("\\citet[see][p. 3]{k}")
                && nb.contains("\\bibliography{r}")
        );
    }

    #[test]
    fn beamer_frames_blocks_and_columns() {
        let src = "#+TITLE: Talk\n#+OPTIONS: H:2 toc:nil\n#+BEAMER_THEME: Madrid\n* Part\n** Frame one\nText\n*** A block\nInside\n** Frame two\n:PROPERTIES:\n:BEAMER_OPT: allowframebreaks\n:END:\n*** Left\n:PROPERTIES:\n:BEAMER_ENV: column\n:BEAMER_COL: 0.4\n:END:\nL\n*** Right\n:PROPERTIES:\n:BEAMER_ENV: column\n:BEAMER_COL: 0.6\n:END:\nR\n#+BEGIN_NOTES\nspeaker\n#+END_NOTES\n";
        let t = export(&parse(src, &|_| None), &Settings::default(), true);
        assert!(t.starts_with("\\documentclass[presentation]{beamer}"));
        assert!(t.contains("\\usetheme{Madrid}"));
        assert!(t.contains("\\section{Part}"));
        assert!(t.contains("\\begin{frame}{Frame one}\nText\n\n\\begin{block}{A block}\nInside\n\n\\end{block}\n\\end{frame}"), "{t}");
        assert!(t.contains("\\begin{frame}[allowframebreaks]{Frame two}"));
        assert!(
            t.contains("\\begin{columns}\n\\begin{column}{0.4\\textwidth}\nL\n")
                && t.contains("\\begin{column}{0.6\\textwidth}")
        );
        assert!(t.contains("\\end{columns}"));
        assert!(t.contains("\\note{speaker}"));
    }

    #[test]
    fn beamer_src_frames_are_fragile() {
        let t = export(
            &parse("* F\n#+BEGIN_SRC c\nint x;\n#+END_SRC\n", &|_| None),
            &Settings::default(),
            true,
        );
        assert!(t.contains("\\begin{frame}[fragile]{F}"));
    }
}
