// SPDX-License-Identifier: MIT OR Apache-2.0
//! The manual: a small Org-subset parser and the node tree it produces.
//!
//! The manual is authored in Org (see Part 2 of the documentation design) and
//! parsed once into [`ManualNode`]s whose bodies are [`Block`]/[`Span`]
//! documents, so the in-editor browser and every exporter share one parsed
//! form. This crate must not depend on `forge-core`'s Org engine (that would
//! pull in Wasmtime), so the subset the manual actually uses is parsed here:
//! headings, property drawers, paragraphs, lists, tables, `#+begin_example`
//! and `#+begin_src` blocks, `~code~` / `=verbatim=` and the inline markup.
//!
//! The parser is deliberately forgiving: unknown `#+` keywords and drawers
//! are skipped rather than rejected, so a manual can grow new constructs
//! without breaking older parsers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Block, Doc, Kind, NodeId, Ref, Source, Span};

/// The manual source, assembled from its chapters. `forge.org` only sets
/// metadata, so it is safe to concatenate; the chapters' headings become
/// the node tree.
pub const MANUAL_SRC: &str = concat!(
    include_str!("../../../docs/manual/forge.org"),
    "\n",
    include_str!("../../../docs/manual/top.org"),
    "\n",
    include_str!("../../../docs/manual/getting-started.org"),
    "\n",
    include_str!("../../../docs/manual/keys-and-commands.org"),
    "\n",
    include_str!("../../../docs/manual/text.org"),
    "\n",
    include_str!("../../../docs/manual/images.org"),
    "\n",
    include_str!("../../../docs/manual/canvas.org"),
    "\n",
    include_str!("../../../docs/manual/frontends.org"),
    "\n",
    include_str!("../../../docs/manual/display.org"),
    "\n",
    include_str!("../../../docs/manual/tips.org"),
);

fn the_manual_once() -> &'static Manual {
    static MANUAL: std::sync::OnceLock<Manual> = std::sync::OnceLock::new();
    MANUAL.get_or_init(|| Manual::parse(MANUAL_SRC))
}

/// The manual, parsed once and shared by the editor, the exporters and
/// the tests, so the in-editor browser shows the same nodes CI checks.
pub fn the_manual() -> &'static Manual {
    the_manual_once()
}

/// One node of the manual: an Org heading and everything under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualNode {
    /// The node's stable id (its title).
    pub id: NodeId,
    /// The heading title, with tags stripped.
    pub title: String,
    /// The heading level (1 for the first-level headings of `forge.org`).
    pub level: u8,
    /// The `:PROPERTIES:` drawer, keyed by name (`ID`, `COMMANDS`, ...).
    pub properties: BTreeMap<String, String>,
    /// The heading's trailing tags (`:TIP:`), without the colons.
    pub tags: Vec<String>,
    /// `#+CINDEX:` terms attached to the node.
    pub index_terms: Vec<String>,
    /// The node's prose.
    pub body: Vec<Block>,
    /// The index of the parent node, if any.
    pub parent: Option<usize>,
}

impl ManualNode {
    /// The whitespace-separated value of a property, if present
    /// (`:COMMANDS: save-buffer write-file`).
    pub fn property_list(&self, key: &str) -> Vec<&str> {
        self.properties
            .get(key)
            .map(|v| v.split_whitespace().collect())
            .unwrap_or_default()
    }

    /// The node's one-line summary: its `:SUMMARY:` property, or, when a
    /// node has none, the first sentence of its first paragraph, trimmed to
    /// a word boundary within [`SUMMARY_MAX`](crate::lint::SUMMARY_MAX)
    /// columns.
    pub fn summary(&self) -> String {
        if let Some(s) = self.properties.get("SUMMARY") {
            let s = s.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
        self.derived_summary()
    }

    fn derived_summary(&self) -> String {
        let max = crate::lint::SUMMARY_MAX;
        for block in &self.body {
            if let Block::Para(spans) = block {
                let text = crate::render::spans_text(spans);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let first = text.split(['.', '\n']).next().unwrap_or(text).trim();
                let mut summary: String = first.chars().take(max).collect();
                if first.chars().count() > max
                    && let Some(pos) = summary.rfind(' ')
                {
                    summary.truncate(pos);
                }
                return summary;
            }
        }
        self.title.clone()
    }
}

/// A parsed manual: a flat list of nodes in document order plus an id index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manual {
    /// The `#+title:` of the manual, if it set one.
    pub title: Option<String>,
    /// The `#+author:` of the manual, if it set one.
    pub author: Option<String>,
    /// Every node, in document order.
    pub nodes: Vec<ManualNode>,
    /// Node id to index into `nodes`. Not serialized; rebuilt on demand.
    #[serde(skip)]
    index: BTreeMap<NodeId, usize>,
}

impl Manual {
    /// Parse an Org document into nodes and blocks.
    pub fn parse(source: &str) -> Manual {
        let lines: Vec<&str> = source.lines().collect();
        let mut manual = Manual::default();
        let mut stack: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if let Some((level, rest)) = heading(lines[i]) {
                i += 1;
                let (title, tags) = split_tags(rest);
                let properties = parse_drawer(&lines, &mut i);
                while let Some(&top) = stack.last() {
                    if manual.nodes[top].level >= level {
                        stack.pop();
                    } else {
                        break;
                    }
                }
                let parent = stack.last().copied();
                let idx = manual.nodes.len();
                manual.index.insert(crate::NodeId(title.clone()), idx);
                manual.nodes.push(ManualNode {
                    id: NodeId(title.clone()),
                    title,
                    level,
                    properties,
                    tags,
                    index_terms: Vec::new(),
                    body: Vec::new(),
                    parent,
                });
                stack.push(idx);
            } else if let Some((key, value)) = document_keyword(lines[i])
                && matches!(key, "title" | "author")
            {
                match key {
                    "title" => manual.title = Some(value.to_string()),
                    "author" => manual.author = Some(value.to_string()),
                    _ => unreachable!(),
                }
                i += 1;
            } else if lines[i].trim().is_empty() {
                i += 1;
            } else {
                // Prose before the first heading (or a stray directive):
                // parse it into the current node, or drop it at the top.
                let node = stack.last().copied();
                let mut terms = Vec::new();
                let blocks = parse_blocks(&lines, &mut i, &mut terms);
                if let Some(node) = node {
                    manual.nodes[node].body.extend(blocks);
                    manual.nodes[node].index_terms.extend(terms);
                }
            }
        }
        manual
    }

    /// Look up a node by id.
    pub fn get(&self, id: &str) -> Option<&ManualNode> {
        self.index
            .get(&NodeId(id.to_string()))
            .map(|&i| &self.nodes[i])
    }

    /// The index of a node id.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(&NodeId(id.to_string())).copied()
    }

    /// The first node (the manual's Top).
    pub fn top(&self) -> Option<&ManualNode> {
        self.nodes.first()
    }

    /// Every node, in document order.
    pub fn iter(&self) -> impl Iterator<Item = &ManualNode> {
        self.nodes.iter()
    }

    /// The indices of nodes that are tips, in document order: those tagged
    /// `:TIP:` or carrying a `:TIP:` property. The tip-of-the-day plugin
    /// draws from this pool.
    pub fn tips(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.tags.iter().any(|t| t.eq_ignore_ascii_case("tip"))
                    || !n.property_list("TIP").is_empty()
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The children of the node at `index`, in document order.
    pub fn children(&self, index: usize) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|&i| self.nodes[i].parent == Some(index))
            .collect()
    }

    /// The parent of the node at `index`.
    pub fn up(&self, index: usize) -> Option<usize> {
        self.nodes.get(index).and_then(|n| n.parent)
    }

    /// The next sibling of the node at `index`, if any.
    pub fn next_sibling(&self, index: usize) -> Option<usize> {
        let parent = self.nodes.get(index)?.parent;
        (index + 1..self.nodes.len()).find(|&i| self.nodes[i].parent == parent)
    }

    /// The previous sibling of the node at `index`, if any.
    pub fn prev_sibling(&self, index: usize) -> Option<usize> {
        let parent = self.nodes.get(index)?.parent;
        (0..index).rev().find(|&i| self.nodes[i].parent == parent)
    }

    /// The next node in document order.
    pub fn next_in_order(&self, index: usize) -> Option<usize> {
        (index + 1 < self.nodes.len()).then_some(index + 1)
    }

    /// The previous node in document order.
    pub fn prev_in_order(&self, index: usize) -> Option<usize> {
        index.checked_sub(1)
    }

    /// Every node as a [`Kind::Node`] [`Doc`], in document order, ready to
    /// be added to a [`DocRegistry`](crate::DocRegistry).
    ///
    /// The node's `:COMMANDS:` and `:OPTIONS:` properties become typed
    /// `see_also` references, so the registry lint checks that each one
    /// resolves; the node's body becomes the doc's body and the node title
    /// is both the doc's name and its [`Manual::get`] id.
    pub fn as_docs(&self) -> Vec<Doc> {
        self.nodes.iter().map(|n| self.doc_for(n)).collect()
    }

    fn doc_for(&self, node: &ManualNode) -> Doc {
        let mut see_also = Vec::new();
        for name in node.property_list("COMMANDS") {
            see_also.push(Ref {
                kind: Kind::Command,
                name: name.to_string(),
                label: None,
            });
        }
        for name in node.property_list("OPTIONS") {
            see_also.push(Ref {
                kind: Kind::Option,
                name: name.to_string(),
                label: None,
            });
        }
        Doc {
            kind: Kind::Node,
            name: node.title.clone(),
            summary: node.summary(),
            body: node.body.clone(),
            args: Vec::new(),
            see_also,
            examples: Vec::new(),
            since: None,
            deprecated: None,
            source: Source::Manual,
            manual: None,
            category: None,
        }
    }

    /// Lint the manual's own structure (Part 2 of the documentation design).
    ///
    /// Node titles must be non-empty, unique and free of `:` `.` `,`, so
    /// that an Info-style reference (`*Title`) is unambiguous, and every
    /// `[[*Node]]` link in a body must resolve to a node.
    pub fn lint(&self) -> Vec<crate::lint::Problem> {
        use crate::lint::Problem;
        use std::collections::BTreeSet;

        let mut problems = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for node in &self.nodes {
            let item = format!("node:{}", node.title);
            if node.title.trim().is_empty() {
                problems.push(Problem {
                    item: "node:<empty>".to_string(),
                    message: "empty node title".to_string(),
                });
                continue;
            }
            if let Some(c) = node.title.chars().find(|c| matches!(c, ':' | '.' | ',')) {
                problems.push(Problem {
                    item: item.clone(),
                    message: format!("node title contains {c:?}"),
                });
            }
            if !seen.insert(node.title.as_str()) {
                problems.push(Problem {
                    item: item.clone(),
                    message: "duplicate node title".to_string(),
                });
            }
            for id in node_links(&node.body) {
                if self.index_of(&id).is_none() {
                    problems.push(Problem {
                        item: item.clone(),
                        message: format!("link does not resolve: [[*{id}]]"),
                    });
                }
            }
        }
        problems
    }
}

/// Every `[[*Node]]` target appearing in `blocks`, in document order.
fn node_links(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        collect_block_links(block, &mut out);
    }
    out
}

fn collect_block_links(block: &Block, out: &mut Vec<String>) {
    match block {
        Block::Para(spans) | Block::Heading(_, spans) => collect_span_links(spans, out),
        Block::List { items, .. } => {
            for item in items {
                for block in item {
                    collect_block_links(block, out);
                }
            }
        }
        Block::Note(inner) => {
            for block in inner {
                collect_block_links(block, out);
            }
        }
        Block::Table { header, rows } => {
            for row in header {
                collect_span_links(row, out);
            }
            for row in rows {
                for cell in row {
                    collect_span_links(cell, out);
                }
            }
        }
        Block::Example(_) => {}
    }
}

fn collect_span_links(spans: &[Span], out: &mut Vec<String>) {
    for span in spans {
        match span {
            Span::NodeLink { node, label } => {
                out.push(node.0.clone());
                collect_span_links(label, out);
            }
            Span::Link { label, .. } | Span::Emph(label) | Span::Strong(label) => {
                collect_span_links(label, out);
            }
            Span::Text(_) | Span::Code(_) | Span::Key(_) | Span::External { .. } => {}
        }
    }
}

/// A heading line: `* Title`, `** Title :tag:`. Returns the level and the
/// text after the stars.
fn heading(line: &str) -> Option<(u8, &str)> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    if stars == 0 || stars > 8 {
        return None;
    }
    let rest = &line[stars..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }
    Some((stars as u8, rest.trim()))
}

/// Whether a line is a `#+key: value` document keyword; returns the
/// lowercased key and the value.
fn document_keyword(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("#+")?;
    let (key, value) = rest.split_once(':')?;
    Some((key.trim(), value.trim()))
}

/// Parse the optional `:PROPERTIES:` drawer that may follow a heading,
/// advancing `i` past it.
fn parse_drawer(lines: &[&str], i: &mut usize) -> BTreeMap<String, String> {
    let mut properties = BTreeMap::new();
    if *i >= lines.len() || lines[*i].trim() != ":PROPERTIES:" {
        return properties;
    }
    *i += 1;
    while *i < lines.len() {
        if lines[*i].trim() == ":END:" {
            *i += 1;
            break;
        }
        let Some((key, value)) = split_property(lines[*i]) else {
            *i += 1;
            continue;
        };
        let key = key.to_string();
        let mut value = value.trim().to_string();
        // Continuation lines are indented and do not start with a colon.
        let mut j = *i + 1;
        while j < lines.len() {
            let next = lines[j];
            let t = next.trim();
            if t.is_empty() || t.starts_with(':') || !next.starts_with([' ', '\t']) {
                break;
            }
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(t);
            j += 1;
        }
        *i = j;
        properties.insert(key, value);
    }
    properties
}

/// Split a `:KEY: value` property line.
fn split_property(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    if !t.starts_with(':') {
        return None;
    }
    let rest = &t[1..];
    let end = rest.find(':')?;
    let key = &rest[..end];
    let value = &rest[end + 1..];
    if key.is_empty() || key == "END" || key == "PROPERTIES" {
        return None;
    }
    Some((key, value.trim()))
}

/// Split trailing `:tag:tag:` tags off a heading.
fn split_tags(text: &str) -> (String, Vec<String>) {
    let trimmed = text.trim_end();
    let mut tags = Vec::new();
    let mut end = trimmed.len();
    while end > 0 {
        let rest = &trimmed[..end];
        if !rest.ends_with(':') {
            break;
        }
        let without = rest.trim_end_matches(':');
        let start = without
            .rfind(|c: char| c.is_whitespace() || c == ':')
            .map(|p| p + 1)
            .unwrap_or(0);
        let tag = &without[start..];
        if tag.is_empty() || tag.contains([' ', '\t']) {
            break;
        }
        tags.insert(0, tag.to_string());
        end = start;
        // Skip the whitespace before the tag.
        end = trimmed[..end].trim_end_matches(':').trim_end().len();
        if end == 0 || !trimmed[..end].ends_with(':') {
            // Only treat as tags if there is a run of them; otherwise this
            // was a trailing colon in the title.
            if tags.iter().any(|t| trimmed.ends_with(&format!("{t}:"))) {
                break;
            }
            break;
        }
    }
    // Only strip the tags when the heading actually ended in a tag run: a
    // title like "Keys and commands" is left untouched.
    let title = if tags.is_empty() {
        trimmed.to_string()
    } else {
        trimmed[..end].trim_end().to_string()
    };
    (title, tags)
}

/// Parse a run of body lines into blocks, stopping before the next heading.
fn parse_blocks(lines: &[&str], i: &mut usize, terms: &mut Vec<String>) -> Vec<Block> {
    let mut blocks = Vec::new();
    while *i < lines.len() {
        let line = lines[*i];
        if heading(line).is_some() {
            break;
        }
        if line.trim().is_empty() {
            *i += 1;
            continue;
        }
        if line.trim_start().starts_with('#') {
            let t = line.trim_start();
            match directive_name(t).as_deref() {
                Some("cindex" | "index") => {
                    if let Some((_, value)) = document_keyword(t) {
                        terms.push(value.to_string());
                    }
                    *i += 1;
                }
                Some("begin_example" | "begin_src" | "begin_verse") => {
                    *i += 1;
                    let mut text = String::new();
                    while *i < lines.len() {
                        let l = lines[*i];
                        if matches!(
                            directive_name(l.trim_start()).as_deref(),
                            Some("end_example" | "end_src" | "end_verse")
                        ) {
                            *i += 1;
                            break;
                        }
                        text.push_str(l);
                        text.push('\n');
                        *i += 1;
                    }
                    blocks.push(Block::Example(text.trim_end_matches('\n').to_string()));
                }
                Some("begin_quote" | "begin_center") => {
                    *i += 1;
                    let start = *i;
                    while *i < lines.len() {
                        if matches!(
                            directive_name(lines[*i].trim_start()).as_deref(),
                            Some("end_quote" | "end_center")
                        ) {
                            break;
                        }
                        *i += 1;
                    }
                    let mut j = 0;
                    let mut sub_terms = Vec::new();
                    let inner = parse_blocks(&lines[start..*i], &mut j, &mut sub_terms);
                    if *i < lines.len() {
                        *i += 1;
                    }
                    if !inner.is_empty() {
                        blocks.push(Block::Note(inner));
                    }
                }
                _ => {
                    // A comment or an unknown directive.
                    *i += 1;
                }
            }
            continue;
        }
        if line.trim_start().starts_with('|') {
            let table = parse_table(lines, i);
            if let Some(table) = table {
                blocks.push(table);
            }
            continue;
        }
        if let Some(item) = list_marker(line) {
            let list = parse_list(lines, i, item.0, item.1);
            blocks.push(list);
            continue;
        }
        // A paragraph: consecutive lines that are not blank, not headings
        // and not another block's start.
        let mut text = String::new();
        while *i < lines.len() {
            let l = lines[*i];
            if l.trim().is_empty()
                || heading(l).is_some()
                || l.trim_start().starts_with('#')
                || l.trim_start().starts_with('|')
                || list_marker(l).is_some()
                || split_property(l).is_some()
            {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(l.trim());
            *i += 1;
        }
        if !text.is_empty() {
            blocks.push(Block::Para(parse_inline(&text)));
        }
    }
    blocks
}

/// The name of a `#+` directive: the text after `#+` up to the first space,
/// lowercased and with a trailing colon removed (`#+CINDEX:` -> `cindex`,
/// `#+begin_src rust` -> `begin_src`).
fn directive_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("#+")?;
    let name = rest.split_whitespace().next()?;
    Some(name.trim_end_matches(':').to_ascii_lowercase())
}

/// A list item marker: returns `(ordered, marker_len)`.
fn list_marker(line: &str) -> Option<(bool, usize)> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if t.starts_with("- ") || t.starts_with("+ ") || t.starts_with("* ") {
        return Some((false, indent + 2));
    }
    let digits = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && t[digits..].starts_with(". ") {
        return Some((true, indent + digits + 2));
    }
    None
}

/// Parse a flat list starting at `*i`.
fn parse_list(lines: &[&str], i: &mut usize, ordered: bool, indent: usize) -> Block {
    let mut items: Vec<Vec<Block>> = Vec::new();
    while *i < lines.len() {
        let line = lines[*i];
        match list_marker(line) {
            Some((_, marker_len)) if marker_len == indent => {
                let text = line[marker_len..].trim().to_string();
                *i += 1;
                // Continuation lines (indented more than the marker).
                let mut extra = String::new();
                while *i < lines.len() {
                    let l = lines[*i];
                    if l.trim().is_empty() || list_marker(line_of(lines, *i)).is_some() {
                        break;
                    }
                    if line_indent(l) < indent {
                        break;
                    }
                    if !extra.is_empty() {
                        extra.push(' ');
                    }
                    extra.push_str(l.trim());
                    *i += 1;
                }
                let mut text2 = text.to_string();
                if !extra.is_empty() {
                    text2.push(' ');
                    text2.push_str(&extra);
                }
                items.push(vec![Block::Para(parse_inline(&text2))]);
            }
            Some((_, marker_len)) if marker_len > indent => {
                // A nested list: fold it into the previous item as a block.
                let nested = parse_list(lines, i, false, marker_len);
                if let Some(last) = items.last_mut() {
                    last.push(nested);
                }
            }
            _ => break,
        }
    }
    Block::List { ordered, items }
}

fn line_indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn line_of<'a>(lines: &[&'a str], i: usize) -> &'a str {
    lines.get(i).copied().unwrap_or("")
}

/// Parse a table starting at `*i`.
fn parse_table(lines: &[&str], i: &mut usize) -> Option<Block> {
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut separators: Vec<usize> = Vec::new();
    while *i < lines.len() {
        let line = lines[*i];
        let t = line.trim();
        if !t.starts_with('|') {
            break;
        }
        *i += 1;
        let cells: Vec<Vec<Span>> = t
            .trim_matches('|')
            .split('|')
            .map(|c| parse_inline(c.trim()))
            .collect();
        let is_sep = !cells.is_empty()
            && cells.iter().all(|c| {
                let text = crate::render::spans_text(c);
                let text = text.trim();
                !text.is_empty() && text.chars().all(|ch| ch == '-' || ch == '+')
            });
        if is_sep {
            separators.push(rows.len());
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return None;
    }
    let header: Vec<Vec<Vec<Span>>> = if separator_present(&separators) && separators[0] > 0 {
        rows[0..separators[0]].to_vec()
    } else if !rows.is_empty() {
        rows[0..1].to_vec()
    } else {
        Vec::new()
    };
    let start = if separator_present(&separators) {
        separators[0] + 1
    } else {
        1
    };
    let body: Vec<Vec<Vec<Span>>> = rows
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx >= start && !separators.contains(idx))
        .map(|(_, r)| r.clone())
        .collect();
    Some(Block::Table {
        header: header.concat(),
        rows: body,
    })
}

fn separator_present(separators: &[usize]) -> bool {
    !separators.is_empty()
}

/// Parse inline Org markup into spans.
/// Parse a standalone Org snippet that has no level-1 headings (a NEWS.org
/// release body, say) into blocks, so other documents share the manual's
/// markup pipeline.
pub fn parse_body(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    let mut terms = Vec::new();
    parse_blocks(&lines, &mut i, &mut terms)
}

/// Parse Org inline markup (`=verbatim=`, `*bold*`, `[[links]]`, and so on)
/// into spans.
pub fn parse_inline(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("[[")
            && let Some(end) = find_link_end(rest)
        {
            flush(&mut buf, &mut spans);
            spans.push(link_span(&rest[2..end]));
            i += end + 2;
            continue;
        }
        let c = rest.chars().next().unwrap();
        if (c == '~' || c == '=')
            && let Some(end) = find_closing(rest, c)
        {
            let content = &rest[1..end];
            flush(&mut buf, &mut spans);
            spans.push(code_span(c, content));
            i += end + 1;
            continue;
        }
        if (c == '*' || c == '/')
            && let Some(end) = find_emphasis(rest, c)
        {
            let content = &rest[1..end];
            flush(&mut buf, &mut spans);
            let inner = parse_inline(content);
            spans.push(if c == '*' {
                Span::Strong(inner)
            } else {
                Span::Emph(inner)
            });
            i += end + 1;
            continue;
        }
        buf.push(c);
        i += c.len_utf8();
    }
    flush(&mut buf, &mut spans);
    spans
}

fn flush(buf: &mut String, spans: &mut Vec<Span>) {
    if !buf.is_empty() {
        spans.push(Span::Text(std::mem::take(buf)));
    }
}

fn code_span(delim: char, content: &str) -> Span {
    if delim == '~' && looks_like_key(content) {
        Span::Key(content.to_string())
    } else {
        Span::Code(content.to_string())
    }
}

/// Whether `s` has the shape of a key sequence (`C-x C-s`, `M-x`, `RET`).
pub fn looks_like_key(s: &str) -> bool {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.is_empty() || tokens.len() > 4 {
        return false;
    }
    let mut strong = false;
    for t in &tokens {
        if !key_token(t) {
            return false;
        }
        if strong_key(t) {
            strong = true;
        }
    }
    strong
}

fn key_token(t: &str) -> bool {
    if t.is_empty() {
        return false;
    }
    if t.starts_with('<') && t.ends_with('>') && t.len() > 2 {
        return true;
    }
    let mut core = t;
    loop {
        let mut stripped = false;
        for m in ["C-", "M-", "S-"] {
            if let Some(rest) = core.strip_prefix(m) {
                core = rest;
                stripped = true;
            }
        }
        if !stripped {
            break;
        }
    }
    const NAMED: &[&str] = &["RET", "TAB", "SPC", "ESC", "DEL", "BS", "LFD", "NUL", "DEL"];
    if NAMED.contains(&core) {
        return true;
    }
    let chars = core.chars().count();
    chars == 1 || (chars > 1 && core.chars().all(|c| c.is_ascii_digit()))
}

fn strong_key(t: &str) -> bool {
    if t.starts_with('<') && t.ends_with('>') {
        return true;
    }
    const NAMED: &[&str] = &["RET", "TAB", "SPC", "ESC", "DEL", "BS", "LFD", "NUL"];
    if NAMED.contains(&t) {
        return true;
    }
    let core = t
        .trim_start_matches("C-")
        .trim_start_matches("M-")
        .trim_start_matches("S-");
    t.len() != core.len() && core.chars().count() == 1
}

fn find_link_end(rest: &str) -> Option<usize> {
    // rest starts with "[["; find the closing "]]".
    rest[2..].find("]]").map(|p| p + 2)
}

fn find_closing(rest: &str, delim: char) -> Option<usize> {
    let after = rest[1..].chars().next()?;
    if after.is_whitespace() {
        return None;
    }
    let mut prev = after;
    for (off, ch) in rest.char_indices().skip(1) {
        if off == 0 {
            continue;
        }
        if ch == delim && !prev.is_whitespace() {
            return Some(off);
        }
        prev = ch;
    }
    None
}

fn find_emphasis(rest: &str, delim: char) -> Option<usize> {
    let after = rest[1..].chars().next()?;
    if after.is_whitespace() || after == delim {
        return None;
    }
    let mut prev = after;
    for (off, ch) in rest.char_indices().skip(1) {
        if ch == delim && !prev.is_whitespace() {
            let next = rest[off + ch.len_utf8()..].chars().next();
            if delim == '*' && next.is_some_and(|n| !n.is_whitespace() && n != '*') {
                continue;
            }
            return Some(off);
        }
        prev = ch;
    }
    None
}

fn link_span(inner: &str) -> Span {
    let (target, label) = match inner.split_once("][") {
        Some((t, l)) => (t.trim(), Some(parse_inline(l))),
        None => (inner.trim(), None),
    };
    let name = target.strip_prefix('*').unwrap_or(target);
    let label = label.unwrap_or_else(|| vec![Span::Text(name.to_string())]);
    if target.starts_with("http://") || target.starts_with("https://") {
        Span::External {
            url: target.to_string(),
            label,
        }
    } else {
        Span::NodeLink {
            node: NodeId(name.to_string()),
            label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"#+title: The Test Manual
#+author: Nobody

* Top
  :PROPERTIES:
  :ID: Top
  :END:

Forge is an editor.

* Saving files
  :PROPERTIES:
  :COMMANDS: save-buffer write-file
               save-some-buffers
  :OPTIONS: make_backup_files
  :END:

Use ~C-x C-s~ (save-buffer) or *M-x* save-buffer. See [[*Top][the top]].

** Backups
   :PROPERTIES:
   :ID: Backups
   :END:

A list:

- first item
- second item with ~C-h f~

#+begin_example
line one
line two
#+end_example

| Name | Key      |
|------+----------|
| Save | C-x C-s  |
| Quit | C-g      |
"#;

    #[test]
    fn parses_nodes_and_the_tree() {
        let m = Manual::parse(SRC);
        assert_eq!(m.title.as_deref(), Some("The Test Manual"));
        assert_eq!(m.author.as_deref(), Some("Nobody"));
        let titles: Vec<&str> = m.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["Top", "Saving files", "Backups"]);
        let top = m.index_of("Top").unwrap();
        let saving = m.index_of("Saving files").unwrap();
        let backups = m.index_of("Backups").unwrap();
        // Top and Saving files are both level 1 (chapters of forge.org).
        assert_eq!(m.up(saving), None);
        assert_eq!(m.next_sibling(top), Some(saving));
        assert_eq!(m.prev_sibling(saving), Some(top));
        // Backups is a child of Saving files.
        assert_eq!(m.up(backups), Some(saving));
        assert_eq!(m.children(saving), vec![backups]);
        assert_eq!(m.up(0), None);
        assert_eq!(m.top().unwrap().title, "Top");
    }

    #[test]
    fn property_values_continue_on_indented_lines() {
        let m = Manual::parse(SRC);
        let saving = m.get("Saving files").unwrap();
        assert_eq!(
            saving.property_list("COMMANDS"),
            ["save-buffer", "write-file", "save-some-buffers"]
        );
        assert_eq!(saving.property_list("OPTIONS"), ["make_backup_files"]);
    }

    #[test]
    fn inline_markup_becomes_spans() {
        let spans = parse_inline("Use ~C-x C-s~ (save-buffer) or *M-x* save-buffer.");
        assert!(matches!(spans[1], Span::Key(ref k) if k == "C-x C-s"));
        assert!(spans.iter().any(|s| matches!(s, Span::Strong(_))));
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, Span::Code(c) if c == "save-buffer" )
                    || matches!(s, Span::Text(t) if t.contains("save-buffer")))
        );
    }

    #[test]
    fn links_become_node_links_and_external_urls() {
        let spans = parse_inline("See [[*Top][the top]] and [[https://example.com][example]].");
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, Span::NodeLink { node, .. } if node.0 == "Top"))
        );
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, Span::External { url, .. } if url == "https://example.com"))
        );
    }

    #[test]
    fn parses_lists_examples_and_tables() {
        let m = Manual::parse(SRC);
        let backups = m.get("Backups").unwrap();
        assert!(backups.body.iter().any(|b| matches!(
            b,
            Block::List { items, ordered } if !*ordered && items.len() == 2
        )));
        assert!(backups.body.iter().any(|b| matches!(
            b,
            Block::Example(e) if e == "line one\nline two"
        )));
        let table = backups
            .body
            .iter()
            .find_map(|b| match b {
                Block::Table { header, rows } => Some((header, rows)),
                _ => None,
            })
            .expect("a table");
        assert_eq!(crate::render::spans_text(&table.0[0]), "Name");
        assert_eq!(table.1.len(), 2);
    }

    #[test]
    fn keys_are_recognized_from_code_spans() {
        assert!(looks_like_key("C-x C-s"));
        assert!(looks_like_key("M-x"));
        assert!(looks_like_key("C-h f"));
        assert!(looks_like_key("RET"));
        assert!(looks_like_key("<mouse-1>"));
        assert!(!looks_like_key("save-buffer"));
        assert!(!looks_like_key("forge.opt.hl_line"));
        assert!(!looks_like_key("init.rhai"));
    }

    #[test]
    fn cindex_terms_are_collected() {
        let m = Manual::parse("* Top\n#+CINDEX: manual, how to read\nText.\n");
        assert_eq!(m.top().unwrap().index_terms, ["manual, how to read"]);
    }

    #[test]
    fn parse_then_render_does_not_panic() {
        let m = Manual::parse(SRC);
        for node in m.iter() {
            for block in &node.body {
                let _ = crate::render::spans_text(match block {
                    Block::Para(s) => s,
                    _ => &[],
                });
            }
        }
    }

    #[test]
    fn as_docs_derives_node_docs_with_summary_and_references() {
        let m = Manual::parse(SRC);
        let docs = m.as_docs();
        assert_eq!(docs.len(), m.nodes.len());
        let saving = docs.iter().find(|d| d.name == "Saving files").unwrap();
        assert_eq!(saving.kind, Kind::Node);
        assert_eq!(saving.source, Source::Manual);
        // No :SUMMARY: property, so it is derived from the first paragraph.
        assert_eq!(
            saving.summary,
            "Use C-x C-s (save-buffer) or M-x save-buffer"
        );
        let commands: Vec<&str> = saving
            .see_also
            .iter()
            .filter(|r| r.kind == Kind::Command)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(commands, ["save-buffer", "write-file", "save-some-buffers"]);
        assert!(
            saving
                .see_also
                .iter()
                .any(|r| r.kind == Kind::Option && r.name == "make_backup_files")
        );
    }

    #[test]
    fn summary_property_wins_over_the_derived_one() {
        let m = Manual::parse("* Node\n:PROPERTIES:\n:SUMMARY: A short line\n:END:\n\nLong prose.");
        assert_eq!(m.nodes[0].summary(), "A short line");
    }

    #[test]
    fn lint_flags_bad_and_dangling_titles() {
        let src = "* Good\n* Bad: title\n* Good\nSee [[*Nowhere][nowhere]].\n";
        let m = Manual::parse(src);
        let messages: Vec<String> = m.lint().into_iter().map(|p| p.message).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("': '") || m.contains("':'"))
        );
        assert!(messages.iter().any(|m| m.contains("duplicate")));
        assert!(messages.iter().any(|m| m.contains("Nowhere")));
    }

    /// The real manual, with `#+INCLUDE:` resolved, parsed as one document.
    fn real_manual() -> String {
        fn expand(path: &std::path::Path, out: &mut String) {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("#+INCLUDE:") {
                    let name = rest.trim().trim_matches('"');
                    expand(&path.parent().unwrap().join(name), out);
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
        let mut out = String::new();
        expand(&dir.join("forge.org"), &mut out);
        out
    }

    #[test]
    fn the_real_manual_parses_and_lints_cleanly() {
        let src = real_manual();
        let m = Manual::parse(&src);
        assert_eq!(m.title.as_deref(), Some("The Forge Manual"));
        let top = m.index_of("Top").expect("the manual has a Top node");
        assert!(
            m.nodes.len() >= 12,
            "chapters and sections: {}",
            m.nodes.len()
        );
        // Top is the root: the chapters and the opening sections hang off it.
        for child in [
            "How to read this manual",
            "Conventions",
            "Getting started",
            "Keys and commands",
        ] {
            let id = m
                .index_of(child)
                .unwrap_or_else(|| panic!("missing node {child:?}"));
            assert_eq!(m.up(id), Some(top), "{child:?} is a child of Top");
        }
        let problems = m.lint();
        let report: Vec<String> = problems.iter().map(ToString::to_string).collect();
        assert!(problems.is_empty(), "manual lint:\n{}", report.join("\n"));
    }
}
