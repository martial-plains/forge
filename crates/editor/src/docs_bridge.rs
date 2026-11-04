// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bridge between legacy one-string command docs and the structured
//! [`forge_docs`] model.
//!
//! Built-ins still carry a single documentation string. This turns each
//! into a [`Doc`] whose summary is the first non-blank line and whose body
//! is the remaining text, split into paragraphs on blank lines. As richer
//! structured docs are written, they replace the legacy string and this
//! bridge stops being used for them.

use forge_core::commands::{CommandRegistry, CommandSource};
use forge_docs::{Block, CommandDoc, Doc, DocRegistry, Kind, Source, Span};

/// The first non-blank line, trimmed (the doc's summary).
fn summary_of(doc: &str) -> String {
    doc.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Everything after the first non-blank line, split into paragraphs.
fn body_of(doc: &str) -> Vec<Block> {
    let mut seen_summary = false;
    let mut rest = String::new();
    for line in doc.lines() {
        if !seen_summary {
            if line.trim().is_empty() {
                continue;
            }
            seen_summary = true;
            continue;
        }
        rest.push_str(line);
        rest.push('\n');
    }
    rest.trim()
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| Block::Para(vec![Span::Text(p.to_string())]))
        .collect()
}

/// Build a structured [`Doc`] from a legacy one-string doc of any kind.
pub fn legacy_to_doc_kind(name: &str, doc: &str, kind: Kind, source: Source) -> Doc {
    let body = body_of(doc);
    Doc {
        kind,
        name: name.to_string(),
        summary: summary_of(doc),
        body,
        args: Vec::new(),
        see_also: Vec::new(),
        examples: Vec::new(),
        since: None,
        deprecated: None,
        source,
        manual: None,
        category: None,
    }
}

/// Build a structured [`Doc`] from a legacy one-string command doc.
pub fn legacy_to_doc(name: &str, doc: &str, source: Source) -> Doc {
    legacy_to_doc_kind(name, doc, Kind::Command, source)
}

pub use legacy_to_doc as legacy_to_doc_command;

/// The structured doc source that matches a command's origin.
fn source_of(source: &CommandSource) -> Source {
    match source {
        CommandSource::Builtin => Source::Builtin,
        CommandSource::Plugin(name) => Source::Plugin(name.clone()),
        CommandSource::Config => Source::Config,
    }
}

/// Reconcile the command docs in `docs` with the commands the editor
/// knows. One [`Doc`] is added or refreshed per command, tagged with the
/// command's origin, and docs for commands that are gone (an unloaded or
/// crashed plugin) are dropped. Run after plugins load or unload and after
/// init.rhai registers its commands.
pub fn sync_command_docs(docs: &mut DocRegistry, commands: &CommandRegistry) {
    let mut present: Vec<String> = Vec::new();
    for name in commands.names() {
        present.push(name.to_string());
        let info = commands.get(name);
        let doc = info.map(|i| i.doc.as_str()).unwrap_or("");
        let source = info
            .map(|i| source_of(&i.source))
            .unwrap_or(Source::Builtin);
        docs.add(legacy_to_doc_kind(name, doc, Kind::Command, source));
    }
    docs.retain(|d| d.kind != Kind::Command || present.iter().any(|n| n == &d.name));
}

/// Like [`sync_command_docs`], then overlays the structured declarations in
/// `rich` on top. Commands declared with a [`CommandDoc`] keep their body,
/// see-also and manual link even after the legacy registry is re-synced
/// (plugins loading or unloading, `init.rhai` registering commands).
pub fn sync_command_docs_with(
    docs: &mut DocRegistry,
    commands: &CommandRegistry,
    rich: &[CommandDoc],
) {
    sync_command_docs(docs, commands);
    overlay_command_docs(docs, commands, rich);
}

/// Overlay the structured declarations in `rich` on top of the command docs
/// already in `docs`, without first re-syncing from the registry. One
/// [`Doc`] is written per command that the registry still knows, so callers
/// can overlay several tables in a row without one erasing another.
pub fn overlay_command_docs(
    docs: &mut DocRegistry,
    commands: &CommandRegistry,
    rich: &[CommandDoc],
) {
    for cd in rich {
        if commands.get(cd.name()).is_some() {
            docs.add(cd.to_doc());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_summary_and_paragraphs() {
        let doc = legacy_to_doc(
            "save-buffer",
            "Save the buffer.\nThe first save makes a backup.\n\nWith an argument, save as.",
            Source::Builtin,
        );
        assert_eq!(doc.summary, "Save the buffer.");
        assert_eq!(doc.body.len(), 2);
    }

    #[test]
    fn single_line_doc_has_no_body() {
        let doc = legacy_to_doc("foo", "Do the foo thing.", Source::Builtin);
        assert_eq!(doc.summary, "Do the foo thing.");
        assert!(doc.body.is_empty());
    }

    #[test]
    fn sync_tags_each_command_with_its_origin() {
        let mut commands = CommandRegistry::new();
        commands.register_builtin("save-buffer", "Save the buffer.");
        commands
            .register_plugin("greet", "Say hello.", "hello")
            .unwrap();
        commands
            .register_config("my-command", "From init.rhai.")
            .unwrap();
        let mut docs = DocRegistry::new();
        sync_command_docs(&mut docs, &commands);

        assert_eq!(
            docs.get(Kind::Command, "save-buffer").unwrap().source,
            Source::Builtin
        );
        assert_eq!(
            docs.get(Kind::Command, "greet").unwrap().source,
            Source::Plugin("hello".into())
        );
        assert_eq!(
            docs.get(Kind::Command, "my-command").unwrap().source,
            Source::Config
        );
        assert_eq!(docs.of_kind(Kind::Command).len(), 3);
    }

    #[test]
    fn sync_refreshes_a_changed_doc() {
        let mut commands = CommandRegistry::new();
        commands.register_builtin("foo", "First.");
        let mut docs = DocRegistry::new();
        sync_command_docs(&mut docs, &commands);
        commands.register_builtin("foo", "Second.");
        sync_command_docs(&mut docs, &commands);
        assert_eq!(docs.get(Kind::Command, "foo").unwrap().summary, "Second.");
        assert_eq!(docs.of_kind(Kind::Command).len(), 1);
    }

    #[test]
    fn sync_drops_commands_that_are_gone() {
        let mut commands = CommandRegistry::new();
        commands.register_builtin("keep", "Keep me.");
        commands
            .register_plugin("gone", "Remove me.", "temp")
            .unwrap();
        let mut docs = DocRegistry::new();
        sync_command_docs(&mut docs, &commands);
        assert!(docs.has(Kind::Command, "gone"));

        commands.remove_plugin("temp");
        sync_command_docs(&mut docs, &commands);
        assert!(docs.has(Kind::Command, "keep"));
        assert!(!docs.has(Kind::Command, "gone"));
    }

    #[test]
    fn sync_leaves_modes_and_options_alone() {
        let mut commands = CommandRegistry::new();
        commands.register_builtin("foo", "A command.");
        let mut docs = DocRegistry::new();
        docs.add(legacy_to_doc_kind(
            "text-mode",
            "For prose.",
            Kind::Mode,
            Source::Builtin,
        ));
        docs.add(legacy_to_doc_kind(
            "fill-column",
            "Where M-q fills.",
            Kind::Option,
            Source::Builtin,
        ));
        sync_command_docs(&mut docs, &commands);
        assert!(docs.has(Kind::Mode, "text-mode"));
        assert!(docs.has(Kind::Option, "fill-column"));
        assert!(docs.has(Kind::Command, "foo"));
    }
}
