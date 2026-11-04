// SPDX-License-Identifier: MIT OR Apache-2.0
//! The structured documentation model.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// What kind of documented thing this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    /// An interactive command (`save-buffer`).
    Command,
    /// A user option / variable (`forge.opt.hl_line`).
    Option,
    /// A major or minor mode (`org-mode`).
    Mode,
    /// A face, i.e. a named visual style.
    Face,
    /// A hook, i.e. a list of functions run at a named time.
    Hook,
    /// A plugin event, with its payload fields.
    Event,
    /// A function in the plugin API (a WIT function).
    ApiFunction,
    /// A plugin as a whole (its `describe-plugin` page).
    Plugin,
    /// A node of the manual.
    Node,
    /// A key sequence, documented on its own (`C-x C-s`).
    Key,
    /// A concept explained in prose (no single defining symbol).
    Concept,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Kind::Command => "command",
            Kind::Option => "option",
            Kind::Mode => "mode",
            Kind::Face => "face",
            Kind::Hook => "hook",
            Kind::Event => "event",
            Kind::ApiFunction => "api-function",
            Kind::Plugin => "plugin",
            Kind::Node => "node",
            Kind::Key => "key",
            Kind::Concept => "concept",
        };
        f.write_str(name)
    }
}

/// A typed cross-reference to another doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    /// The kind of the referenced doc.
    pub kind: Kind,
    /// The referenced doc's name.
    pub name: String,
    /// Optional display text; the name is shown when absent.
    pub label: Option<String>,
}

/// A command argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arg {
    /// The argument's name, as shown in a signature.
    pub name: String,
    /// What the argument means.
    pub desc: String,
    /// Whether the caller may omit it.
    pub optional: bool,
}

/// A runnable or illustrative example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    /// A heading for the example, if it needs one.
    pub title: Option<String>,
    /// The example text (source, a session transcript, ...).
    pub body: String,
    /// The language of `body`, for syntax highlighting (`"lua"`, `"rust"`).
    pub lang: Option<String>,
}

/// What replaced this doc entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deprecated {
    /// The version in which it was deprecated.
    pub since: Option<String>,
    /// Why it was deprecated, and what to use instead.
    pub reason: Option<String>,
}

/// Where a doc came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Compiled into the editor.
    Builtin,
    /// Provided by the named plugin (removed when it unloads).
    Plugin(String),
    /// Defined in `init.rhai` (or another config file).
    Config,
    /// A node of the manual.
    Manual,
}

/// An identifier into the manual.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub struct NodeId(
    /// The node's title, unique within the manual.
    pub String,
);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for NodeId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An inline span of documentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Span {
    /// Plain running text.
    Text(String),
    /// Verbatim code, rendered monospaced (`save-buffer`, `forge.opt.hl_line`).
    Code(String),
    /// A key sequence, rendered as keys (`C-x C-f`).
    Key(String),
    /// A link to another [`Doc`] in the registry.
    Link {
        /// The doc this points at.
        target: Ref,
        /// The display text.
        label: Vec<Span>,
    },
    /// A link to a node of the manual.
    NodeLink {
        /// The target node.
        node: NodeId,
        /// The display text.
        label: Vec<Span>,
    },
    /// A link to a URL outside Forge.
    External {
        /// The target URL.
        url: String,
        /// The display text.
        label: Vec<Span>,
    },
    /// Emphasised (typically italic) text.
    Emph(Vec<Span>),
    /// Strong (typically bold) text.
    Strong(Vec<Span>),
}

/// A block of documentation content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    /// A paragraph of inline spans.
    Para(Vec<Span>),
    /// An ordered or unordered list; each item is a list of blocks.
    List {
        /// `true` for a numbered list, `false` for a bulleted one.
        ordered: bool,
        /// The items, in order.
        items: Vec<Vec<Block>>,
    },
    /// A preformatted, non-reflowed example.
    Example(String),
    /// A table with a header row and body rows of spans.
    Table {
        /// The header row(s).
        header: Vec<Vec<Span>>,
        /// The body rows, each a list of cells.
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// A callout (Note, Warning, Tip).
    Note(Vec<Block>),
    /// A heading at the given level (1 = top), then its text.
    Heading(u8, Vec<Span>),
}

/// The documentation for one thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    /// What kind of thing this documents.
    pub kind: Kind,
    /// The name looked up by (`"save-buffer"`, `"forge.opt.hl_line"`).
    pub name: String,
    /// One line, no trailing period required, at most 80 columns.
    pub summary: String,
    /// The structured body; the summary is not repeated here.
    pub body: Vec<Block>,
    /// Arguments, for commands that take a prefix and for API functions.
    pub args: Vec<Arg>,
    /// Typed cross-references to related docs.
    pub see_also: Vec<Ref>,
    /// Examples, runnable where possible.
    pub examples: Vec<Example>,
    /// The version the thing was added in.
    pub since: Option<String>,
    /// Set when the thing is deprecated.
    pub deprecated: Option<Deprecated>,
    /// Where this doc came from.
    pub source: Source,
    /// The manual node that explains it, if any.
    pub manual: Option<NodeId>,
    /// A grouping label for indexes and menus (`"Files"`, `"Editing"`).
    pub category: Option<String>,
}
