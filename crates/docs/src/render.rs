// SPDX-License-Identifier: MIT OR Apache-2.0
//! Renderers for the doc model into text, HTML, and later Org/Texinfo/man.

use crate::{Block, Doc, Kind, Span};

/// The first paragraph of a doc, as plain text.
pub fn summary_text(doc: &Doc) -> String {
    doc.body
        .first()
        .and_then(|b| match b {
            Block::Para(spans) => Some(spans),
            _ => None,
        })
        .map(|spans| spans_text(spans))
        .unwrap_or_else(|| doc.summary.clone())
}

/// The doc's whole body as plain text, one block after another, with every
/// link flattened to its label. Used for search and for callers that only
/// need the prose.
pub fn body_text(doc: &Doc) -> String {
    let mut out = String::new();
    for block in &doc.body {
        block_all_text(block, &mut out);
    }
    out
}

fn block_all_text(block: &Block, out: &mut String) {
    match block {
        Block::Para(spans) | Block::Heading(_, spans) => {
            out.push_str(&spans_text(spans));
            out.push(' ');
        }
        Block::List { items, .. } => {
            for item in items {
                for b in item {
                    block_all_text(b, out);
                }
            }
        }
        Block::Example(e) => {
            out.push_str(e);
            out.push(' ');
        }
        Block::Table { header, rows } => {
            for row in header.iter().chain(rows.iter().flatten()) {
                for cell in row {
                    out.push_str(&spans_text(std::slice::from_ref(cell)));
                    out.push(' ');
                }
            }
        }
        Block::Note(b) => {
            for b in b {
                block_all_text(b, out);
            }
        }
    }
}

pub(crate) fn spans_text(spans: &[Span]) -> String {
    let mut out = String::new();
    for sp in spans {
        match sp {
            Span::Text(t) => out.push_str(t),
            Span::Code(c) => out.push_str(c),
            Span::Key(k) => out.push_str(k),
            Span::Link { label, .. }
            | Span::NodeLink { label, .. }
            | Span::External { label, .. } => {
                out.push_str(&spans_text(label));
            }
            Span::Emph(v) | Span::Strong(v) => out.push_str(&spans_text(v)),
        }
    }
    out
}

/// Render a doc to plain text (help buffers, plain-text exports).
///
/// The summary is the first line; [`Doc::body`] follows it.
pub fn render_text(doc: &Doc) -> String {
    let mut out = String::new();
    if !doc.summary.is_empty() {
        out.push_str(&doc.summary);
        out.push('\n');
    }
    out.push_str(&render_body(&doc.body));
    out
}

/// Render a list of blocks to plain text, with the `Ref` spans left as their
/// display text. `render_text` uses this for a doc's body; callers with blocks
/// but no `Doc` (release notes, say) can use it directly.
pub fn render_body(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Heading(_, spans) => {
                out.push_str(&spans_text(spans));
                out.push('\n');
            }
            Block::Para(spans) => {
                out.push_str(&spans_text(spans));
                out.push('\n');
            }
            Block::List { items, ordered } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", i + 1)
                    } else {
                        "* ".to_string()
                    };
                    out.push_str(&marker);
                    out.push_str(&block_text(item));
                    out.push('\n');
                }
            }
            Block::Example(e) => {
                out.push_str(e);
                out.push('\n');
            }
            _ => {}
        }
    }
    out
}

fn block_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Para(spans) => out.push_str(&spans_text(spans)),
            Block::Example(e) => out.push_str(e),
            _ => {}
        }
    }
    out
}

/// Render a doc to a single HTML fragment.
pub fn render_html(doc: &Doc) -> String {
    let mut out = format!(
        "<article id=\"{}\"><h1>{}</h1><p class=\"summary\">{}</p>",
        html_anchor(doc),
        html_escape(&doc.name),
        html_escape(&doc.summary)
    );
    for block in &doc.body {
        block_to_html(block, &mut out);
    }
    out.push_str("</article>");
    out
}

fn block_to_html(block: &Block, out: &mut String) {
    match block {
        Block::Para(spans) => out.push_str(&format!("<p>{}</p>", spans_to_html(spans))),
        Block::Heading(level, spans) => {
            let level = (*level).clamp(2, 6);
            out.push_str(&format!("<h{level}>{}</h{level}>", spans_to_html(spans)));
        }
        Block::List { ordered, items } => {
            let (open, close) = if *ordered { ("ol", "ol") } else { ("ul", "ul") };
            out.push_str(&format!("<{open}>"));
            for item in items {
                out.push_str("<li>");
                for b in item {
                    block_to_html(b, out);
                }
                out.push_str("</li>");
            }
            out.push_str(&format!("</{close}>"));
        }
        Block::Example(e) => {
            out.push_str(&format!("<pre><code>{}</code></pre>", html_escape(e)));
        }
        Block::Table { header, rows } => {
            out.push_str("<table>");
            if !header.is_empty() {
                out.push_str(&format!("<thead><tr>{}</tr></thead>", table_row(header)));
            }
            out.push_str("<tbody>");
            for row in rows {
                out.push_str(&format!("<tr>{}</tr>", table_row(row)));
            }
            out.push_str("</tbody></table>");
        }
        Block::Note(blocks) => {
            out.push_str("<aside class=\"note\">");
            for b in blocks {
                block_to_html(b, out);
            }
            out.push_str("</aside>");
        }
    }
}

fn table_row(cells: &[Vec<Span>]) -> String {
    let mut out = String::new();
    for cell in cells {
        out.push_str(&format!("<td>{}</td>", spans_to_html(cell)));
    }
    out
}

fn spans_to_html(spans: &[Span]) -> String {
    let mut out = String::new();
    for sp in spans {
        match sp {
            Span::Text(t) => out.push_str(&html_escape(t)),
            Span::Code(c) => out.push_str(&format!("<code>{}</code>", html_escape(c))),
            Span::Key(k) => out.push_str(&format!("<kbd>{}</kbd>", html_escape(k))),
            Span::Emph(v) | Span::Strong(v) => {
                let inner = spans_to_html(v);
                if matches!(sp, Span::Emph(_)) {
                    out.push_str(&format!("<em>{inner}</em>"));
                } else {
                    out.push_str(&format!("<strong>{inner}</strong>"));
                }
            }
            Span::Link { target, label } => {
                out.push_str(&format!(
                    "<a href=\"#{}\">{}</a>",
                    anchor(target.kind, &target.name),
                    spans_to_html(label)
                ));
            }
            Span::NodeLink { node, label } => {
                out.push_str(&format!(
                    "<a href=\"#node-{}\">{}</a>",
                    slug(&node.0),
                    spans_to_html(label)
                ));
            }
            Span::External { url, label } => {
                out.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape(url),
                    spans_to_html(label)
                ));
            }
        }
    }
    out
}

/// The `id` a doc's anchor should have.
fn html_anchor(doc: &Doc) -> String {
    anchor(doc.kind, &doc.name)
}

/// The anchor for a `(kind, name)` pair, e.g. `command-save-buffer`.
fn anchor(kind: Kind, name: &str) -> String {
    format!("{}-{}", kind, slug(name))
}

/// Lowercase `s`, turning every run of non-alphanumerics into a single `-`.
/// The anchor fragment used by `#node-…` and `#…-<name>` links.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// --- The manual as HTML and Texinfo -----------------------------------------

use crate::manual::Manual;

/// The whole manual as one HTML page: a table of contents, then every node
/// with a navigation line, its body and a menu of its children. Anchors match
/// the `#node-…` links [`render_html`] emits for a [`Span::NodeLink`].
pub fn render_manual_html(manual: &Manual) -> String {
    let title = manual.title.as_deref().unwrap_or("The Forge Manual");
    let mut out = String::from("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str(&format!(
        "<title>{}</title></head><body>\n<h1>{}</h1>\n",
        html_escape(title),
        html_escape(title)
    ));
    if let Some(author) = &manual.author {
        out.push_str(&format!(
            "<p class=\"author\">{}</p>\n",
            html_escape(author)
        ));
    }

    out.push_str("<nav class=\"toc\">\n<ul>\n");
    for node in manual.iter() {
        out.push_str(&format!(
            "<li class=\"level-{}\"><a href=\"#node-{}\">{}</a></li>\n",
            node.level,
            slug(&node.title),
            html_escape(&node.title)
        ));
    }
    out.push_str("</ul>\n</nav>\n");

    for (i, node) in manual.iter().enumerate() {
        out.push_str(&format!(
            "<article class=\"node\" id=\"node-{}\">\n",
            slug(&node.title)
        ));
        out.push_str(&node_nav_html(manual, i));
        out.push_str(&format!(
            "<h2>{}</h2>\n<p class=\"summary\">{}</p>\n",
            html_escape(&node.title),
            html_escape(&node.summary())
        ));
        for block in &node.body {
            block_to_html(block, &mut out);
        }
        let children = manual.children(i);
        if !children.is_empty() {
            out.push_str("<ul class=\"menu\">\n");
            for child in children {
                let c = &manual.nodes[child];
                out.push_str(&format!(
                    "<li><a href=\"#node-{}\">{}</a> &mdash; {}</li>\n",
                    slug(&c.title),
                    html_escape(&c.title),
                    html_escape(&c.summary())
                ));
            }
            out.push_str("</ul>\n");
        }
        out.push_str("</article>\n");
    }
    out.push_str("</body></html>\n");
    out
}

fn node_nav_html(manual: &Manual, index: usize) -> String {
    let mut parts = Vec::new();
    let pos = |what: &str, target: Option<usize>| {
        if let Some(t) = target {
            format!(
                "{what}: <a href=\"#node-{}\">{}</a>",
                slug(&manual.nodes[t].title),
                html_escape(&manual.nodes[t].title)
            )
        } else {
            format!("{what}: (none)")
        }
    };
    parts.push(pos("Up", manual.up(index)));
    parts.push(pos("Next", manual.next_sibling(index)));
    parts.push(pos("Prev", manual.prev_sibling(index)));
    format!("<p class=\"nav\">{}</p>\n", parts.join(" &middot; "))
}

/// Render the whole manual to Texinfo, suitable for `makeinfo` to produce
/// Info and (via `texi2any`) other formats. Node names are the node titles
/// (they contain no `: . ,` by the manual's rules).
pub fn render_texinfo(manual: &Manual) -> String {
    let title = manual.title.as_deref().unwrap_or("The Forge Manual");
    let mut out = String::from("\\input texinfo\n");
    out.push_str("@c Generated from docs/manual by forge-docs. Do not edit.\n");
    out.push_str(&format!("@settitle {}\n", texinfo_escape(title)));
    if let Some(author) = &manual.author {
        out.push_str(&format!("@author {}\n", texinfo_escape(author)));
    }
    out.push('\n');

    for (i, node) in manual.iter().enumerate() {
        out.push_str(&format!("@node {}\n", texinfo_escape(&node.title)));
        out.push_str(&format!(
            "{} {}\n\n",
            texinfo_command(node.level),
            texinfo_escape(&node.title)
        ));
        for block in &node.body {
            block_to_texinfo(block, &mut out);
        }
        let children = manual.children(i);
        if !children.is_empty() {
            out.push_str("@menu\n");
            for child in children {
                let c = &manual.nodes[child];
                out.push_str(&format!(
                    "* {}:: {}\n",
                    texinfo_escape(&c.title),
                    texinfo_escape(&c.summary())
                ));
            }
            out.push_str("@end menu\n\n");
        }
    }
    out.push_str("@bye\n");
    out
}

fn texinfo_command(level: u8) -> &'static str {
    match level {
        1 => "@top",
        2 => "@chapter",
        3 => "@section",
        4 => "@subsection",
        5 => "@subsubsection",
        _ => "@heading",
    }
}

fn block_to_texinfo(block: &Block, out: &mut String) {
    match block {
        Block::Para(spans) => {
            out.push_str(&spans_to_texinfo(spans));
            out.push_str("\n\n");
        }
        Block::Heading(level, spans) => {
            let _ = level;
            out.push_str(&format!("@heading {}\n\n", spans_to_texinfo(spans)));
        }
        Block::List { ordered, items } => {
            let open = if *ordered {
                "@enumerate\n"
            } else {
                "@itemize @bullet\n"
            };
            out.push_str(open);
            for item in items {
                out.push_str("@item ");
                for b in item {
                    block_to_texinfo(b, out);
                }
                out.push('\n');
            }
            out.push_str(if *ordered {
                "@end enumerate\n\n"
            } else {
                "@end itemize\n\n"
            });
        }
        Block::Example(e) => {
            out.push_str("@example\n");
            out.push_str(e);
            out.push_str("\n@end example\n\n");
        }
        Block::Table { header, rows } => {
            out.push_str("@multitable @columnfractions .5 .5\n");
            if !header.is_empty() {
                out.push_str("@headitem ");
                out.push_str(&texinfo_row(header));
                out.push('\n');
            }
            for row in rows {
                out.push_str("@item ");
                out.push_str(&texinfo_row(row));
                out.push('\n');
            }
            out.push_str("@end multitable\n\n");
        }
        Block::Note(blocks) => {
            out.push_str("@quotation\n");
            for b in blocks {
                block_to_texinfo(b, out);
            }
            out.push_str("@end quotation\n\n");
        }
    }
}

fn texinfo_row(cells: &[Vec<Span>]) -> String {
    cells
        .iter()
        .map(|c| spans_to_texinfo(c))
        .collect::<Vec<_>>()
        .join(" @tab ")
}

fn spans_to_texinfo(spans: &[Span]) -> String {
    let mut out = String::new();
    for sp in spans {
        match sp {
            Span::Text(t) => out.push_str(&texinfo_escape(t)),
            Span::Code(c) => out.push_str(&format!("@code{{{}}}", texinfo_escape(c))),
            Span::Key(k) => out.push_str(&format!("@kbd{{{}}}", texinfo_escape(k))),
            Span::Emph(v) => out.push_str(&format!("@emph{{{}}}", spans_to_texinfo(v))),
            Span::Strong(v) => out.push_str(&format!("@strong{{{}}}", spans_to_texinfo(v))),
            Span::NodeLink { node, .. } => {
                out.push_str(&format!("@ref{{{}}}", texinfo_escape(&node.0)));
            }
            Span::Link { target, label } => {
                if spans_text(label) == target.name {
                    out.push_str(&format!("@ref{{{}}}", texinfo_escape(&target.name)));
                } else {
                    out.push_str(&format!(
                        "{} (@ref{{{}}})",
                        spans_to_texinfo(label),
                        texinfo_escape(&target.name)
                    ));
                }
            }
            Span::External { url, label } => {
                out.push_str(&format!(
                    "@uref{{{},{}}}",
                    texinfo_escape(url),
                    spans_to_texinfo(label)
                ));
            }
        }
    }
    out
}

fn texinfo_escape(s: &str) -> String {
    s.replace('@', "@@").replace('{', "@{").replace('}', "@}")
}

/// What a link in a rendered Info node points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoTarget {
    /// Another node of the manual, by its title.
    Node(String),
    /// A command, option, mode... in the registry, by name.
    Symbol(String),
}

/// A link in a rendered Info node: a character range of the rendered text
/// and what it points at. The editor turns these into the buffer's link
/// layer, so RET follows them and TAB moves between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoLink {
    /// The first character of the link text (char offset).
    pub start: usize,
    /// One past the last character (char offset).
    pub end: usize,
    /// What the link goes to.
    pub target: InfoTarget,
}

/// Render one node as an Info buffer: the `File: forge.org,  Node: ...`
/// header line, the underlined title, the body, and the menu of child
/// nodes. A link is emitted for every menu entry and every inline
/// node/symbol reference in the body.
pub fn render_info(manual: &crate::Manual, index: usize) -> (String, Vec<InfoLink>) {
    let node = &manual.nodes[index];
    let mut out = String::new();
    let mut links = Vec::new();

    out.push_str(&info_header(manual, index));
    out.push_str("\n\n");
    out.push_str(&node.title);
    out.push('\n');
    out.push_str(&"=".repeat(node.title.chars().count()));
    out.push_str("\n\n");
    info_blocks(&node.body, &mut out, &mut links);

    let children = manual.children(index);
    if !children.is_empty() {
        while out.ends_with('\n') {
            out.pop();
        }
        out.push_str("\n\n* Menu:\n\n");
        for child in children {
            let c = &manual.nodes[child];
            let start = out.chars().count();
            out.push_str(&c.title);
            let end = out.chars().count();
            links.push(InfoLink {
                start,
                end,
                target: InfoTarget::Node(c.title.clone()),
            });
            let summary = c.summary();
            if summary.is_empty() {
                out.push_str("::\n");
            } else {
                out.push_str(&format!("::  {summary}\n"));
            }
        }
    }
    (out, links)
}

/// The Info header line of a node: `File: forge.org,  Node: ...,  Next: ...,
///  Prev: ...,  Up: ...`, with the parts that do not exist left out.
fn info_header(manual: &crate::Manual, index: usize) -> String {
    let node = &manual.nodes[index];
    let mut h = format!("File: forge.org,  Node: {}", node.title);
    if let Some(n) = manual.next_sibling(index) {
        h.push_str(&format!(",  Next: {}", manual.nodes[n].title));
    }
    if let Some(p) = manual.prev_sibling(index) {
        h.push_str(&format!(",  Prev: {}", manual.nodes[p].title));
    }
    if let Some(u) = manual.up(index) {
        h.push_str(&format!(",  Up: {}", manual.nodes[u].title));
    }
    h
}

fn info_blocks(blocks: &[Block], out: &mut String, links: &mut Vec<InfoLink>) {
    for block in blocks {
        match block {
            Block::Heading(_, spans) | Block::Para(spans) => {
                info_spans(spans, out, links);
                out.push('\n');
            }
            Block::List { items, ordered } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        out.push_str(&format!("{}. ", i + 1));
                    } else {
                        out.push_str("* ");
                    }
                    info_blocks(item, out, links);
                }
            }
            Block::Example(e) => {
                out.push_str(e);
                out.push('\n');
            }
            Block::Note(inner) => {
                out.push_str("Note: ");
                info_blocks(inner, out, links);
            }
            Block::Table { header, rows } => {
                for row in header {
                    info_spans(row, out, links);
                    out.push('\n');
                }
                for row in rows {
                    info_row(row, out, links);
                }
            }
        }
    }
}

fn info_row(row: &[Vec<Span>], out: &mut String, links: &mut Vec<InfoLink>) {
    for (i, cell) in row.iter().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        info_spans(cell, out, links);
    }
    out.push('\n');
}

fn info_spans(spans: &[Span], out: &mut String, links: &mut Vec<InfoLink>) {
    for span in spans {
        match span {
            Span::Text(t) | Span::Code(t) | Span::Key(t) => out.push_str(t),
            Span::Link { target, label } => {
                let start = out.chars().count();
                info_spans(label, out, links);
                let end = out.chars().count();
                if end > start {
                    links.push(InfoLink {
                        start,
                        end,
                        target: InfoTarget::Symbol(target.name.clone()),
                    });
                }
            }
            Span::NodeLink { node, label } => {
                let start = out.chars().count();
                info_spans(label, out, links);
                let end = out.chars().count();
                if end > start {
                    links.push(InfoLink {
                        start,
                        end,
                        target: InfoTarget::Node(node.0.clone()),
                    });
                }
            }
            Span::External { label, .. } => info_spans(label, out, links),
            Span::Emph(inner) | Span::Strong(inner) => info_spans(inner, out, links),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Doc, Kind, NodeId, Ref, Source};

    fn doc(summary: &str, body: Vec<Block>) -> Doc {
        Doc {
            kind: Kind::Command,
            name: "frobnicate".to_string(),
            summary: summary.to_string(),
            body,
            args: Vec::new(),
            see_also: Vec::new(),
            examples: Vec::new(),
            since: None,
            deprecated: None,
            source: Source::Builtin,
            manual: None,
            category: None,
        }
    }

    #[test]
    fn render_text_puts_the_summary_first_then_body() {
        let d = doc(
            "Frob the widget.",
            vec![
                Block::Para(vec![Span::Text("First paragraph.".into())]),
                Block::Para(vec![Span::Text("Second.".into())]),
            ],
        );
        let text = render_text(&d);
        assert_eq!(text, "Frob the widget.\nFirst paragraph.\nSecond.\n");
    }

    #[test]
    fn summary_text_falls_back_to_the_summary_field() {
        let d = doc("A one-liner.", Vec::new());
        assert_eq!(summary_text(&d), "A one-liner.");
        let d = doc("", vec![Block::Para(vec![Span::Text("Body line.".into())])]);
        assert_eq!(summary_text(&d), "Body line.");
    }

    #[test]
    fn render_text_flattens_links_keys_and_emphasis() {
        let d = doc(
            "See it.",
            vec![Block::Para(vec![
                Span::Text("Press ".into()),
                Span::Key("C-x C-f".into()),
                Span::Text(" then ".into()),
                Span::Link {
                    target: Ref {
                        kind: Kind::Command,
                        name: "find-file".into(),
                        label: None,
                    },
                    label: vec![Span::Code("find-file".into())],
                },
                Span::Text(" (".into()),
                Span::Emph(vec![Span::Text("important".into())]),
                Span::Text(").".into()),
            ])],
        );
        assert_eq!(
            render_text(&d),
            "See it.\nPress C-x C-f then find-file (important).\n"
        );
    }

    #[test]
    fn render_text_renders_lists_examples_and_headings() {
        let d = doc(
            "",
            vec![
                Block::Heading(2, vec![Span::Text("Keys".into())]),
                Block::List {
                    ordered: false,
                    items: vec![
                        vec![Block::Para(vec![Span::Text("first".into())])],
                        vec![Block::Para(vec![Span::Text("second".into())])],
                    ],
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![Block::Para(vec![Span::Text("one".into())])]],
                },
                Block::Example("(message \"hi\")".into()),
            ],
        );
        assert_eq!(
            render_text(&d),
            "Keys\n* first\n* second\n1. one\n(message \"hi\")\n"
        );
    }

    #[test]
    fn render_html_escapes_and_marks_up() {
        let d = doc(
            "<Save> & quit",
            vec![Block::Para(vec![
                Span::Text("Use ".into()),
                Span::Key("C-x C-s".into()),
                Span::Text(" for ".into()),
                Span::Code("save <buffer>".into()),
            ])],
        );
        let html = render_html(&d);
        assert!(html.starts_with("<article id=\"command-frobnicate\"><h1>frobnicate</h1>"));
        assert!(html.contains("<p class=\"summary\">&lt;Save&gt; &amp; quit</p>"));
        assert!(html.contains("<kbd>C-x C-s</kbd>"));
        assert!(html.contains("<code>save &lt;buffer&gt;</code>"));
        assert!(html.ends_with("</article>"));
    }

    #[test]
    fn render_html_renders_examples_as_pre() {
        let d = doc("x", vec![Block::Example("a < b".into())]);
        assert!(render_html(&d).contains("<pre><code>a &lt; b</code></pre>"));
    }

    #[test]
    fn render_html_renders_lists_headings_tables_and_notes() {
        let d = doc(
            "x",
            vec![
                Block::Heading(3, vec![Span::Text("Keys".into())]),
                Block::List {
                    ordered: false,
                    items: vec![vec![Block::Para(vec![Span::Text("first".into())])]],
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![Block::Para(vec![Span::Text("one".into())])]],
                },
                Block::Table {
                    header: vec![vec![Span::Text("Key".into())]],
                    rows: vec![vec![vec![Span::Text("C-x".into())]]],
                },
                Block::Note(vec![Block::Para(vec![Span::Text("Careful.".into())])]),
            ],
        );
        let html = render_html(&d);
        assert!(html.contains("<h3>Keys</h3>"));
        assert!(html.contains("<ul><li><p>first</p></li></ul>"));
        assert!(html.contains("<ol><li><p>one</p></li></ol>"));
        assert!(html.contains("<table><thead><tr><td>Key</td></tr></thead><tbody><tr><td>C-x</td></tr></tbody></table>"));
        assert!(html.contains("<aside class=\"note\"><p>Careful.</p></aside>"));
    }

    #[test]
    fn render_html_links_to_docs_nodes_and_urls() {
        let d = doc(
            "x",
            vec![Block::Para(vec![
                Span::Link {
                    target: Ref {
                        kind: Kind::Command,
                        name: "save-buffer".into(),
                        label: None,
                    },
                    label: vec![Span::Code("save-buffer".into())],
                },
                Span::NodeLink {
                    node: NodeId("Saving files".into()),
                    label: vec![Span::Text("Saving files".into())],
                },
                Span::External {
                    url: "https://example.com/a?b=1&c=2".into(),
                    label: vec![Span::Text("example".into())],
                },
            ])],
        );
        let html = render_html(&d);
        assert!(html.contains("<a href=\"#command-save-buffer\"><code>save-buffer</code></a>"));
        assert!(html.contains("<a href=\"#node-saving-files\">Saving files</a>"));
        assert!(html.contains("<a href=\"https://example.com/a?b=1&amp;c=2\">example</a>"));
    }

    #[test]
    fn body_text_flattens_every_block_shape() {
        let d = doc(
            "",
            vec![
                Block::Para(vec![Span::Text("para".into())]),
                Block::List {
                    ordered: false,
                    items: vec![vec![Block::Para(vec![Span::Text("item".into())])]],
                },
                Block::Example("example".into()),
                Block::Table {
                    header: vec![vec![Span::Text("head".into())]],
                    rows: vec![vec![vec![Span::Text("cell".into())]]],
                },
                Block::Note(vec![Block::Para(vec![Span::Text("note".into())])]),
                Block::Heading(1, vec![Span::Text("heading".into())]),
            ],
        );
        let text = body_text(&d);
        for word in ["para", "item", "example", "head", "cell", "note", "heading"] {
            assert!(text.contains(word), "{word:?} missing from {text:?}");
        }
    }

    #[test]
    fn render_text_handles_node_links() {
        let d = doc(
            "",
            vec![Block::Para(vec![Span::NodeLink {
                node: NodeId("Files".into()),
                label: vec![Span::Text("the Files node".into())],
            }])],
        );
        assert_eq!(render_text(&d), "the Files node\n");
    }

    #[test]
    fn render_manual_html_has_toc_nodes_and_menus() {
        let m = crate::manual::Manual::parse(
            "#+title: T\n* Top\nBody.\n** Child\nKid body.\n*** Grandkid\nDeep.\n",
        );
        let html = render_manual_html(&m);
        assert!(html.contains("<title>T</title>"));
        assert!(html.contains("<a href=\"#node-top\">Top</a>"));
        assert!(html.contains("id=\"node-child\""));
        assert!(html.contains("class=\"menu\""));
        assert!(html.contains("<a href=\"#node-child\">Child</a>"));
        // The navigation line links a child back up to Top.
        assert!(html.contains("Up: <a href=\"#node-top\">Top</a>"));
    }

    #[test]
    fn render_texinfo_makes_nodes_sections_and_menus() {
        let m = crate::manual::Manual::parse(
            "#+title: T\n* Top\nBody.\n** Child\nKid body with ~C-x~ and [[*Top][top]].\n",
        );
        let texi = render_texinfo(&m);
        assert!(texi.starts_with("\\input texinfo\n"));
        assert!(texi.contains("@settitle T"));
        assert!(texi.contains("@node Top\n@top"));
        assert!(texi.contains("@node Child\n@chapter"));
        assert!(texi.contains("@menu"));
        assert!(texi.contains("* Child:: "));
        assert!(texi.contains("@kbd{C-x}"));
        assert!(texi.contains("@ref{Top}"));
        assert!(texi.ends_with("@bye\n"));
    }

    #[test]
    fn texinfo_escapes_at_braces() {
        let m = crate::manual::Manual::parse("* Top\nUse @code{x} and {y}.\n");
        assert!(render_texinfo(&m).contains("Use @@code@{x@} and @{y@}."));
    }

    #[test]
    fn render_info_makes_the_header_body_and_menu_with_links() {
        let m = crate::manual::Manual::parse(
            "* Top\nBody refers to [[*Child][the Child node]].\n** Child\nKid body.\n",
        );
        let top = m.index_of("Top").expect("Top exists");
        let (text, links) = render_info(&m, top);
        assert!(text.starts_with("File: forge.org,  Node: Top\n"));
        assert!(text.contains("Top\n===\n"));
        assert!(text.contains("* Menu:\n\nChild::  Kid body\n"), "{text:?}");
        let body = links
            .iter()
            .find(|l| l.target == InfoTarget::Node("Child".into()) && l.start < 60)
            .expect("body link to Child");
        assert_eq!(&text[body.start..body.end], "the Child node");
        let menu = links
            .iter()
            .find(|l| l.target == InfoTarget::Node("Child".into()) && l.start >= 60)
            .expect("menu link to Child");
        assert_eq!(&text[menu.start..menu.end], "Child");
    }

    #[test]
    fn render_info_covers_the_real_manual() {
        let m = crate::manual::the_manual();
        let top = m.index_of("Top").expect("manual has a Top node");
        let (text, links) = render_info(m, top);
        assert!(text.contains("File: forge.org,  Node: Top"));
        assert!(text.contains("* Menu:"));
        assert!(!links.is_empty(), "the Top node links at least its menu");
    }
}
