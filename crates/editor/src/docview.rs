// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rendering `Block`/`Span` documents into a text buffer with a link layer.
//!
//! The tutorial is an Org document parsed by `forge_docs` and shown in a
//! read-only buffer. This walks its blocks,
//! appending text and recording a link for every key (to `describe-key`), for
//! every command name (to `describe-symbol`) and for every explicit link, so
//! the same source that the manual is built from also makes a clickable
//! buffer here.

use forge_core::{LinkSpan, LinkTarget};
use forge_docs::{Block, Span};

use crate::app::App;

/// Render a list of blocks, appending to `out` and recording links.
pub fn blocks(list: &[Block], app: &App, out: &mut String, links: &mut Vec<LinkSpan>) {
    for block in list {
        match block {
            Block::Heading(_, spans) | Block::Para(spans) => {
                spans_to(spans, app, out, links);
                out.push('\n');
            }
            Block::List { items, ordered } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        out.push_str(&format!("{}. ", i + 1));
                    } else {
                        out.push_str("* ");
                    }
                    blocks(item, app, out, links);
                }
            }
            Block::Example(e) => {
                out.push_str(e);
                out.push('\n');
            }
            Block::Note(inner) => blocks(inner, app, out, links),
            Block::Table { header, rows } => {
                for row in header {
                    spans_to(row, app, out, links);
                    out.push('\n');
                }
                for row in rows {
                    let mut first = true;
                    for cell in row {
                        if !first {
                            out.push_str(" | ");
                        }
                        first = false;
                        spans_to(cell, app, out, links);
                    }
                    out.push('\n');
                }
            }
        }
    }
}

/// Render inline spans, recording links for keys and command names.
pub fn spans_to(spans: &[Span], app: &App, out: &mut String, links: &mut Vec<LinkSpan>) {
    for span in spans {
        match span {
            Span::Text(t) => out.push_str(t),
            Span::Key(k) => {
                let start = out.chars().count();
                out.push_str(k);
                links.push(LinkSpan {
                    start,
                    end: out.chars().count(),
                    target: LinkTarget::Key(k.clone()),
                    face: None,
                });
            }
            Span::Code(c) => {
                let start = out.chars().count();
                out.push_str(c);
                if app.editor.commands.contains(c) {
                    links.push(LinkSpan {
                        start,
                        end: out.chars().count(),
                        target: LinkTarget::Symbol(c.clone()),
                        face: None,
                    });
                }
            }
            Span::Link { target, label } => {
                let start = out.chars().count();
                spans_to(label, app, out, links);
                links.push(LinkSpan {
                    start,
                    end: out.chars().count(),
                    target: LinkTarget::Symbol(target.name.clone()),
                    face: None,
                });
            }
            Span::NodeLink { node, label } => {
                let start = out.chars().count();
                spans_to(label, app, out, links);
                links.push(LinkSpan {
                    start,
                    end: out.chars().count(),
                    target: LinkTarget::Node(node.0.clone()),
                    face: None,
                });
            }
            Span::External { url, label } => {
                let start = out.chars().count();
                spans_to(label, app, out, links);
                links.push(LinkSpan {
                    start,
                    end: out.chars().count(),
                    target: LinkTarget::Url(url.clone()),
                    face: None,
                });
            }
            Span::Emph(inner) | Span::Strong(inner) => spans_to(inner, app, out, links),
        }
    }
}
