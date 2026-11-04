// SPDX-License-Identifier: MIT OR Apache-2.0
//! Structured documentation for symbols that have no home of their own:
//! faces and events. Commands and options are registered from their own
//! tables in [`crate::app::App::new`]; modes from the table there too.
//! This fills in the rest of what `describe-symbol` and the manual can
//! show: what each face styles, and when each event fires.

use forge_docs::{DocRegistry, Kind, Source};

use crate::docs_bridge::legacy_to_doc_kind;

/// The plain faces and the syntax faces, by name, and what each styles.
/// The parameterised families (rainbow depths, `vc-annotate-age-N`) and
/// the larger families (version control, Org Modern) get their one line
/// from the `face_description` family generically.
pub const FACES: &[(&str, &str)] = &[
    // Plain faces.
    (
        "default",
        "The ordinary text face: what the rest of a buffer uses.",
    ),
    ("region", "The selected text (the region)."),
    ("match", "A search or isearch match."),
    (
        "truncation",
        "The marker at a line's edge when text is cut off or wraps.",
    ),
    ("mode-line", "The mode line of the selected window."),
    (
        "mode-line-inactive",
        "The mode line of windows that are not selected.",
    ),
    ("border", "The frame's border cells."),
    ("line-number", "Line numbers in the margin."),
    (
        "line-number-current",
        "The line number of the line point is on.",
    ),
    ("paren-match", "The delimiter matching the one at point."),
    ("paren-mismatch", "A delimiter with no match."),
    ("trailing-whitespace", "Blanks at the ends of lines."),
    ("whitespace", "Spaces and tabs made visible."),
    ("fold", "The marker standing in for hidden (folded) text."),
    ("popup", "The completion popup's background."),
    (
        "popup-selected",
        "The highlighted candidate in the completion popup.",
    ),
    (
        "popup-annotation",
        "The annotation shown to the right of a candidate.",
    ),
    (
        "popup-margin",
        "The left margin of the completion popup: the quick-select key.",
    ),
    (
        "popup-match",
        "The part of a candidate that matches what you typed.",
    ),
    ("popup-deprecated", "A candidate that is deprecated."),
    ("menu-bar", "The menu bar across the top."),
    ("menu-selected", "The highlighted menu item."),
    ("menu-disabled", "A menu item you cannot pick."),
    ("tab-bar", "The tab bar, one tab per window configuration."),
    ("tab-bar-selected", "The current tab in the tab bar."),
    ("tool-bar", "The tool bar across the top or bottom."),
    ("tool-bar-selected", "The tool bar item under the pointer."),
    (
        "tool-bar-disabled",
        "A tool bar item that cannot be chosen here.",
    ),
    (
        "error",
        "Error messages in the echo area and in *Messages*.",
    ),
    ("warning", "Warning messages."),
    ("info", "Informational messages."),
    (
        "spell-error",
        "A misspelled word, as reported by a spell checker.",
    ),
    (
        "highlight",
        "A general-purpose highlight, such as a line you jumped to.",
    ),
    ("symbol-highlight", "Occurrences of the symbol at point."),
    (
        "underline",
        "Drawn under text that has no underline of its own, as for diagnostics.",
    ),
    ("plain", "Text drawn with no decoration at all."),
    // Syntax faces (from the language's grammar).
    ("keyword", "Language keywords: if, fn, let and the like."),
    ("string", "String and character literals."),
    (
        "escape",
        "Escape sequences in strings, and control characters.",
    ),
    ("comment", "Comments."),
    (
        "function",
        "Function and method names, where they are defined or called.",
    ),
    ("type", "Type names and constructors."),
    ("constant", "Numeric, boolean and other literal constants."),
    (
        "builtin",
        "Built-ins from the language or standard library.",
    ),
    ("property", "Property, field and key names."),
    ("attribute", "Attributes, decorators and labels."),
    (
        "link",
        "Links: an Org [[url][description]] or a Markdown link.",
    ),
];

/// The events a plugin can subscribe to (see `forge.on`), by name.
pub const EVENTS: &[(&str, &str)] = &[
    ("buffer-opened", "A buffer was opened or first shown."),
    ("buffer-killed", "A buffer was killed."),
    (
        "buffer-changed",
        "A buffer's text changed. Sent once per buffer, after you stop typing.",
    ),
    ("buffer-saved", "A buffer was written to disk."),
    (
        "mode-changed",
        "A buffer's major mode, or its set of minor modes, changed.",
    ),
];

/// Register a [`Kind::Face`] doc for every face and a [`Kind::Event`] doc
/// for every event.
pub fn register_faces_and_events(docs: &mut DocRegistry) {
    for name in forge_core::theme::all_faces() {
        let doc = face_description(&name);
        docs.add(legacy_to_doc_kind(&name, &doc, Kind::Face, Source::Builtin));
    }
    for name in forge_core::events::EVENT_NAMES {
        let doc = EVENTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| *d)
            .unwrap_or("An event a plugin can subscribe to.");
        docs.add(legacy_to_doc_kind(name, doc, Kind::Event, Source::Builtin));
    }
}

/// What a face styles: the table's line when we have one, else a line for
/// the family it belongs to.
fn face_description(name: &str) -> String {
    if let Some((_, doc)) = FACES.iter().find(|(n, _)| *n == name) {
        return (*doc).to_string();
    }
    if name.starts_with("org-modern-") {
        return "An Org Modern decoration face (see org-modern).".to_string();
    }
    if name.starts_with("rainbow-") {
        return "A rainbow-delimiter face: nested brackets are tinted by depth.".to_string();
    }
    if name.starts_with("vc-annotate-age-") {
        return "One of ten ages in vc-annotate, from the newest lines to the oldest.".to_string();
    }
    if name.starts_with("diff-")
        || name.starts_with("log-view-")
        || name.starts_with("vc-dir-")
        || name.starts_with("log-edit-")
        || name.starts_with("smerge-")
    {
        return "A version-control face (diffs, logs, vc-dir, smerge).".to_string();
    }
    "A face.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_docs::Kind;

    #[test]
    fn every_face_gets_a_doc() {
        let mut docs = DocRegistry::new();
        register_faces_and_events(&mut docs);
        for name in forge_core::theme::all_faces() {
            let d = docs
                .get(Kind::Face, &name)
                .unwrap_or_else(|| panic!("no doc for face {name}"));
            assert!(!d.summary.is_empty(), "empty summary for face {name}");
        }
    }

    #[test]
    fn every_event_gets_a_doc() {
        let mut docs = DocRegistry::new();
        register_faces_and_events(&mut docs);
        for name in forge_core::events::EVENT_NAMES {
            assert!(docs.has(Kind::Event, name), "no doc for event {name}");
        }
    }

    #[test]
    fn table_faces_use_their_own_line() {
        assert_eq!(
            face_description("keyword"),
            "Language keywords: if, fn, let and the like."
        );
        assert!(face_description("rainbow-depth-3").contains("rainbow"));
        assert!(face_description("diff-added").contains("version-control"));
        assert!(face_description("org-modern-todo").contains("Org Modern"));
        assert!(face_description("vc-annotate-age-2").contains("vc-annotate"));
    }
}
