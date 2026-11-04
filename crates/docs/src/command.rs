// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ergonomic command declarations (Part 1 of the documentation design).
//!
//! A [`CommandDoc`] is a `const`-constructible declaration that is easy to
//! write and easy to lint. It is the migration target for the legacy
//! `(&str, &str)` command tables: an old entry becomes
//! [`CommandDoc::line`], a richer one adds [`CommandDoc::body`],
//! [`CommandDoc::see_also`] and [`CommandDoc::manual`]. [`CommandDoc::to_doc`]
//! turns a declaration into the structured [`Doc`] the registry holds, and
//! [`CommandDoc::legacy_string`] reproduces the old one-string doc so the
//! command registry keeps working during the migration.

use crate::{Block, Doc, Kind, NodeId, Ref, Source, Span};

/// A `const` declaration of one command's documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDoc {
    name: &'static str,
    summary: &'static str,
    body: Option<&'static str>,
    see_also: &'static [&'static str],
    manual: Option<&'static str>,
}

impl CommandDoc {
    /// A command with a one-line summary.
    pub const fn new(name: &'static str, summary: &'static str) -> Self {
        CommandDoc {
            name,
            summary,
            body: None,
            see_also: &[],
            manual: None,
        }
    }

    /// An old-style one-line entry; spelled the same as [`CommandDoc::new`]
    /// but named to make a migration audit easy to read.
    pub const fn line(name: &'static str, summary: &'static str) -> Self {
        CommandDoc::new(name, summary)
    }

    /// Add the prose that follows the summary, as blank-line-separated
    /// paragraphs.
    pub const fn body(mut self, body: &'static str) -> Self {
        self.body = Some(body);
        self
    }

    /// Add commands this one relates to, by name.
    pub const fn see_also(mut self, names: &'static [&'static str]) -> Self {
        self.see_also = names;
        self
    }

    /// Point at the manual node that explains this command.
    pub const fn manual(mut self, node: &'static str) -> Self {
        self.manual = Some(node);
        self
    }

    /// The command's name, as registered.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The command's one-line summary.
    pub const fn summary(&self) -> &'static str {
        self.summary
    }

    /// The old one-string doc: the summary, then a blank line and the body.
    /// Identical to what the `(&str, &str)` table held when there is no body.
    pub fn legacy_string(&self) -> String {
        match self.body {
            Some(body) => format!("{}\n\n{}", self.summary, body),
            None => self.summary.to_string(),
        }
    }

    /// The structured [`Doc`] for this declaration.
    pub fn to_doc(&self) -> Doc {
        Doc {
            kind: Kind::Command,
            name: self.name.to_string(),
            summary: self.summary.to_string(),
            body: self.body.map(paragraphs).unwrap_or_default(),
            args: Vec::new(),
            see_also: self
                .see_also
                .iter()
                .map(|name| Ref {
                    kind: Kind::Command,
                    name: name.to_string(),
                    label: None,
                })
                .collect(),
            examples: Vec::new(),
            since: None,
            deprecated: None,
            source: Source::Builtin,
            manual: self.manual.map(|node| NodeId(node.to_string())),
            category: None,
        }
    }
}

/// Split prose into paragraphs on blank lines.
fn paragraphs(text: &str) -> Vec<Block> {
    text.trim()
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| Block::Para(vec![Span::Text(p.to_string())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_migrates_the_old_string_verbatim() {
        let cd = CommandDoc::line("save-buffer", "Write the buffer to its file.");
        assert_eq!(cd.legacy_string(), "Write the buffer to its file.");
        let doc = cd.to_doc();
        assert_eq!(doc.kind, Kind::Command);
        assert_eq!(doc.name, "save-buffer");
        assert_eq!(doc.summary, "Write the buffer to its file.");
        assert!(doc.body.is_empty());
    }

    #[test]
    fn body_becomes_paragraphs_and_the_legacy_string() {
        let cd = CommandDoc::new("save-buffer", "Write the buffer to its file.")
            .body("First save makes a backup.\n\nWith an argument, save as.");
        assert_eq!(
            cd.legacy_string(),
            "Write the buffer to its file.\n\nFirst save makes a backup.\n\nWith an argument, save as."
        );
        let doc = cd.to_doc();
        assert_eq!(doc.body.len(), 2);
        assert_eq!(doc.summary, "Write the buffer to its file.");
    }

    #[test]
    fn see_also_and_manual_become_structured_fields() {
        let cd = CommandDoc::new("save-buffer", "Write the buffer to its file.")
            .see_also(&["write-file", "revert-buffer"])
            .manual("Saving files");
        let doc = cd.to_doc();
        assert_eq!(doc.see_also.len(), 2);
        assert_eq!(doc.see_also[0].kind, Kind::Command);
        assert_eq!(doc.see_also[0].name, "write-file");
        assert_eq!(
            doc.manual.as_ref().map(|n| n.0.as_str()),
            Some("Saving files")
        );
    }
}
