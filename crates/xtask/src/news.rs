// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask news`: refresh the generated block in `NEWS.org`.
//!
//! Hand-written prose lives above and below the markers; the block between
//! `#+begin_generated` and `#+end_generated` inside the release for the
//! current version is rewritten from the documentation registry: every doc
//! whose `:since:` names this version is listed, grouped by kind. The
//! transformations are pure functions so they can be tested without a build.

use forge_docs::{Doc, Kind};

/// The markers that fence the generated block.
const BEGIN: &str = "#+begin_generated";
const END: &str = "#+end_generated";

/// The human name of a kind, for a section heading.
fn kind_heading(kind: Kind) -> &'static str {
    match kind {
        Kind::Command => "Commands",
        Kind::Option => "Options",
        Kind::Mode => "Modes",
        Kind::Face => "Faces",
        Kind::Hook => "Hooks",
        Kind::Event => "Events",
        Kind::ApiFunction => "Plugin API functions",
        Kind::Plugin => "Plugins",
        Kind::Node => "Manual",
        Kind::Key => "Keys",
        Kind::Concept => "Concepts",
    }
}

/// Render the generated block for `version`: a bullet for every doc whose
/// `since` names it, grouped by kind. Deterministic order (kind, then name).
pub fn generated(version: &str, docs: &[&Doc]) -> String {
    let mut by_kind: Vec<(Kind, Vec<&Doc>)> = Vec::new();
    let mut wanted: Vec<&Doc> = docs
        .iter()
        .copied()
        .filter(|d| d.since.as_deref() == Some(version))
        .collect();
    wanted.sort_by(|a, b| a.name.cmp(&b.name));
    for doc in wanted {
        match by_kind.iter_mut().find(|(k, _)| *k == doc.kind) {
            Some((_, v)) => v.push(doc),
            None => by_kind.push((doc.kind, vec![doc])),
        }
    }

    let mut out = String::new();
    out.push_str(BEGIN);
    out.push('\n');
    if by_kind.is_empty() {
        out.push_str(&format!(
            "No documented items carry =:since: {version}= yet.\n"
        ));
    } else {
        for (kind, docs) in &by_kind {
            out.push_str(&format!("** {}\n", kind_heading(*kind)));
            for doc in docs {
                let summary = doc.summary.trim_end_matches('.');
                out.push_str(&format!("- ={}= — {summary}\n", doc.name));
            }
        }
    }
    out.push_str(END);
    out
}

/// Replace the fenced generated block inside the top-level section headed
/// `* <version>` with `block`. Returns `None` when there is no such section
/// or it has no fence, so the caller can report a precise error.
pub fn replace_block(source: &str, version: &str, block: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let heading = format!("* {version}");
    let start = lines.iter().position(|l| l.trim_end() == heading)?;
    let mut i = start + 1;
    // The section ends at the next top-level heading.
    while i < lines.len() && !is_top_heading(lines[i]) {
        i += 1;
    }
    let section = &lines[start..i];
    let begin = section.iter().position(|l| l.trim() == BEGIN)?;
    let end = section.iter().position(|l| l.trim() == END)?;
    if end < begin {
        return None;
    }
    let mut out = String::new();
    for line in &lines[..start + begin] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(block);
    out.push('\n');
    for line in &lines[start + end + 1..] {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// A line that starts a new top-level Org section (`* Foo`, not `** Foo`).
fn is_top_heading(line: &str) -> bool {
    line.starts_with("* ") || line.trim_end() == "*"
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_docs::{Doc, Kind, Source};

    fn doc(name: &str, kind: Kind, since: Option<&str>) -> Doc {
        Doc {
            kind,
            name: name.to_string(),
            summary: format!("Do {name}"),
            body: Vec::new(),
            args: Vec::new(),
            see_also: Vec::new(),
            examples: Vec::new(),
            since: since.map(str::to_string),
            deprecated: None,
            source: Source::Builtin,
            manual: None,
            category: None,
        }
    }

    #[test]
    fn generated_groups_by_kind_and_skips_other_versions() {
        let docs = [
            doc("save-buffer", Kind::Command, Some("0.2.0")),
            doc("open-line", Kind::Command, Some("0.2.0")),
            doc("tabs", Kind::Option, Some("0.2.0")),
            doc("load", Kind::ApiFunction, Some("0.1.0")),
        ];
        let block = generated("0.2.0", &docs.iter().collect::<Vec<_>>());
        assert!(block.contains("** Commands"));
        assert!(block.contains("** Options"));
        assert!(!block.contains("Plugin API"));
        assert!(block.contains("- =open-line= — Do open-line"));
        assert!(block.contains("- =save-buffer= — Do save-buffer"));
        let oi = block.find("open-line").unwrap();
        let si = block.find("save-buffer").unwrap();
        assert!(oi < si);
        assert!(block.starts_with(BEGIN));
        assert!(block.trim_end().ends_with(END));
    }

    #[test]
    fn generated_says_so_when_a_version_has_nothing() {
        let docs = [doc("load", Kind::ApiFunction, Some("0.1.0"))];
        let block = generated("9.9.9", &docs.iter().collect::<Vec<_>>());
        assert!(block.contains("No documented items carry"));
    }

    #[test]
    fn replace_block_rewrites_only_the_fenced_block() {
        let source = "\
#+title: NEWS

* 0.2.0
Hand-written intro.
#+begin_generated
old
#+end_generated
Trailing prose.

* 0.1.0
The first release.
";
        let block = format!("{BEGIN}\nnew\n{END}");
        let out = replace_block(source, "0.2.0", &block).unwrap();
        assert!(out.contains("Hand-written intro."));
        assert!(out.contains("Trailing prose."));
        assert!(out.contains("new"));
        assert!(!out.contains("old"));
        assert!(out.contains("* 0.1.0"));
        assert!(out.contains("The first release."));
    }

    #[test]
    fn replace_block_is_none_without_a_section_or_fence() {
        assert!(replace_block("* 0.1.0\nno fence here\n", "0.1.0", "x").is_none());
        assert!(replace_block("* 0.1.0\n#+begin_generated\nx\n", "0.1.0", "y").is_none());
        assert!(replace_block("no sections\n", "0.1.0", "y").is_none());
    }
}
