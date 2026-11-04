// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org to Org: the same text with includes and macros expanded and the
//! parts that don't export (`:noexport:`, COMMENT subtrees) removed.

use super::common::Settings;
use super::parse::preprocess;

pub fn export(text: &str, read: &dyn Fn(&str) -> Option<String>, st: &Settings) -> String {
    let out = preprocess(text, read);
    if !st.body_only {
        return out;
    }
    // Body only: no `#+KEY:` header lines at the top.
    let mut lines = out.lines().peekable();
    let mut kept = Vec::new();
    let mut header = true;
    for l in &mut lines {
        if header && (l.starts_with("#+") || l.trim().is_empty()) {
            continue;
        }
        header = false;
        kept.push(l);
    }
    let mut s = kept.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noexport_comment_and_macros_are_applied() {
        let src = "#+TITLE: T\n#+MACRO: m MAC $1\n* Keep\n{{{m(x)}}}\n** sub\n* Drop :noexport:\nsecret\n* COMMENT Gone\nx\n* After\n#+INCLUDE: \"i.org\"\n";
        let out = export(
            src,
            &|f| (f == "i.org").then(|| "included".to_string()),
            &Settings::default(),
        );
        assert!(out.contains("* Keep\nMAC x\n** sub\n"), "{out}");
        assert!(!out.contains("secret") && !out.contains("Gone"));
        assert!(out.contains("* After\nincluded"));
        assert!(out.starts_with("#+TITLE: T"));
        let body = export(
            src,
            &|_| None,
            &Settings {
                body_only: true,
                ..Default::default()
            },
        );
        assert!(body.starts_with("* Keep"));
    }

    #[test]
    fn code_blocks_are_not_mistaken_for_headings() {
        let src = "* A\n#+BEGIN_SRC org\n* Drop :noexport:\n#+END_SRC\n";
        let out = export(src, &|_| None, &Settings::default());
        assert!(out.contains("* Drop :noexport:"));
    }
}
