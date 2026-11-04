// SPDX-License-Identifier: MIT OR Apache-2.0
//! Registry lints (Part 7 of the documentation design).
//!
//! These are the checks `cargo xtask docs --check` runs in CI, and they are
//! also callable from tests. A lint never panics while it runs: it collects
//! every problem first, so a caller can print them all before failing.

use std::collections::BTreeSet;
use std::fmt;

use crate::{Doc, DocRegistry, Kind};

/// The longest a summary may be, in columns.
pub const SUMMARY_MAX: usize = 80;

/// Phrases a doc must never contain: leftover placeholders. "todo" used to
/// be one of them, but Org's TODO keyword is a domain term that the
/// `org-*-todo` commands have to talk about, so it is no longer banned.
const BANNED: &[&str] = &["see below", "fixme", "tbd"];

/// One problem found by [`lint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The doc the problem is about, as `kind:name`.
    pub item: String,
    /// What is wrong.
    pub message: String,
}

impl Problem {
    fn new(item: impl Into<String>, message: impl Into<String>) -> Self {
        Problem {
            item: item.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.item, self.message)
    }
}

/// Lint every doc in `registry`, including the cross-references between
/// them. Returns an empty vector when everything is fine.
pub fn lint(registry: &DocRegistry) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut seen: BTreeSet<(Kind, String)> = BTreeSet::new();
    for doc in registry.iter() {
        let item = format!("{}:{}", doc.kind, doc.name);
        if !seen.insert((doc.kind, doc.name.clone())) {
            problems.push(Problem::new(&item, "duplicate (kind, name)"));
        }
        lint_summary(doc, &item, &mut problems);
        lint_see_also(registry, doc, &item, &mut problems);
        lint_manual(registry, doc, &item, &mut problems);
        lint_text(doc, &item, &mut problems);
    }
    problems
}

fn lint_summary(doc: &Doc, item: &str, problems: &mut Vec<Problem>) {
    let summary = doc.summary.trim();
    if summary.is_empty() {
        problems.push(Problem::new(item, "summary is empty"));
        return;
    }
    if summary.contains('\n') {
        problems.push(Problem::new(item, "summary must be a single line"));
    }
    let len = summary.chars().count();
    if len > SUMMARY_MAX {
        problems.push(Problem::new(
            item,
            format!("summary is {len} columns (max {SUMMARY_MAX})"),
        ));
    }
    if let Some(c) = summary.chars().next().filter(|c| c.is_alphabetic())
        && !c.is_uppercase()
    {
        problems.push(Problem::new(
            item,
            "summary should start with a capital letter",
        ));
    }
}

fn lint_see_also(registry: &DocRegistry, doc: &Doc, item: &str, problems: &mut Vec<Problem>) {
    for r in &doc.see_also {
        if registry.get(r.kind, &r.name).is_none() {
            problems.push(Problem::new(
                item,
                format!("see_also does not resolve: {}:{}", r.kind, r.name),
            ));
        }
    }
}

fn lint_manual(registry: &DocRegistry, doc: &Doc, item: &str, problems: &mut Vec<Problem>) {
    if registry.of_kind(Kind::Node).is_empty() {
        return;
    }
    if let Some(node) = &doc.manual
        && registry.get(Kind::Node, node).is_none()
    {
        problems.push(Problem::new(item, format!("manual node not found: {node}")));
    }
}

fn lint_text(doc: &Doc, item: &str, problems: &mut Vec<Problem>) {
    let mut hay = doc.summary.clone();
    hay.push('\n');
    hay.push_str(&crate::body_text(doc));
    for arg in &doc.args {
        hay.push_str(&arg.desc);
        hay.push('\n');
    }
    for ex in &doc.examples {
        hay.push_str(&ex.body);
        hay.push('\n');
    }
    let lower = hay.to_lowercase();
    for phrase in BANNED {
        if lower.contains(phrase) {
            problems.push(Problem::new(
                item,
                format!("contains banned phrase {phrase:?}"),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Doc, Kind, Ref, Source, Span};

    fn doc(name: &str, summary: &str) -> Doc {
        Doc {
            kind: Kind::Command,
            name: name.to_string(),
            summary: summary.to_string(),
            body: Vec::new(),
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

    fn registry_of(docs: Vec<Doc>) -> DocRegistry {
        let mut r = DocRegistry::new();
        for d in docs {
            r.add(d);
        }
        r
    }

    #[test]
    fn a_clean_registry_reports_nothing() {
        let r = registry_of(vec![doc("save-buffer", "Write the buffer to its file.")]);
        assert_eq!(lint(&r), Vec::new());
    }

    #[test]
    fn empty_summary_is_a_problem() {
        let r = registry_of(vec![doc("save-buffer", "  ")]);
        let problems = lint(&r);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("empty"));
    }

    #[test]
    fn long_or_lowercase_or_multiline_summaries_are_problems() {
        let long = "x".repeat(SUMMARY_MAX + 1);
        let r = registry_of(vec![
            doc("a", &long),
            doc("b", "lowercase start"),
            doc("c", "two\nlines"),
        ]);
        let problems = lint(&r);
        let messages: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("max")));
        assert!(messages.iter().any(|m| m.contains("capital")));
        assert!(messages.iter().any(|m| m.contains("single line")));
    }

    #[test]
    fn dangling_see_also_is_a_problem() {
        let mut d = doc("save-buffer", "Write the buffer.");
        d.see_also.push(Ref {
            kind: Kind::Command,
            name: "write-file".into(),
            label: None,
        });
        let r = registry_of(vec![d]);
        let problems = lint(&r);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("write-file"));
    }

    #[test]
    fn resolving_see_also_is_fine() {
        let mut d = doc("save-buffer", "Write the buffer.");
        d.see_also.push(Ref {
            kind: Kind::Command,
            name: "write-file".into(),
            label: None,
        });
        let r = registry_of(vec![d, doc("write-file", "Write to a named file.")]);
        assert_eq!(lint(&r), Vec::new());
    }

    #[test]
    fn banned_phrases_are_found_in_the_body() {
        let mut d = doc("save-buffer", "Write the buffer.");
        d.body = vec![Block::Para(vec![Span::Text("Details see below.".into())])];
        let r = registry_of(vec![d]);
        let problems = lint(&r);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("see below"));
    }

    #[test]
    fn manual_targets_are_checked_only_when_a_manual_exists() {
        let mut d = doc("save-buffer", "Write the buffer.");
        d.manual = Some(crate::NodeId("Saving files".into()));
        // No nodes loaded: the dangling manual link is not reported yet.
        let r = registry_of(vec![d.clone()]);
        assert_eq!(lint(&r), Vec::new());
        // Once a manual exists, a missing node is reported.
        let mut node = doc("Other chapter", "How saving works.");
        node.kind = Kind::Node;
        let r = registry_of(vec![d, node]);
        let problems = lint(&r);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("Saving files"));
    }
}
