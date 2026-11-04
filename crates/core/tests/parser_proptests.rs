// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property tests for the code that takes arbitrary text: the buffer's
//! rope edits, the Org parser, the table formatter and the exporters.
//!
//! These are the paths where hidden panics (a slice off a char boundary,
//! an index past the end, a bad line/column) live, and where a hand-written
//! example test only covers what someone thought to write down. proptest
//! runs a few hundred random inputs per case and shrinks any failure to a
//! small reproducer.
//!
//! `cd crates/core && cargo test --test parser_proptests`

use forge_core::Buffer;
use forge_core::export;
use forge_core::org_table::{self, Row};
use proptest::prelude::*;

fn buf(text: &str) -> Buffer {
    let mut b = Buffer::new("t");
    b.set_contents(text);
    b
}

/// Arbitrary text, newlines and all.
fn any_text(max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::char::any(), 0..max).prop_map(|cs| cs.into_iter().collect())
}

/// Arbitrary text that is never empty (so an edit always has something
/// to undo).
fn nonempty_text(max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::char::any(), 1..max).prop_map(|cs| cs.into_iter().collect())
}

/// One line shaped like the things the Org parser looks at: headings,
/// list items with checkboxes and cookies, tables, keywords, or plain
/// prose.
fn org_line() -> impl Strategy<Value = String> {
    proptest::prop_oneof![
        Just(String::new()),
        Just("* Heading".to_string()),
        Just("* TODO A [1/2]".to_string()),
        Just("** DONE B [50%]".to_string()),
        Just("- [ ] item [0/1]".to_string()),
        Just("  - [X] child".to_string()),
        Just("1. ordered".to_string()),
        Just("| a | b |".to_string()),
        Just("|---+---|".to_string()),
        Just("#+TBLFM: $1=$2".to_string()),
        Just("#+BEGIN_SRC rust".to_string()),
        Just("#+END_SRC".to_string()),
        Just(": fixed width".to_string()),
        Just("[[link][desc]]".to_string()),
        "[^\n]{0,24}",
    ]
}

/// A whole Org-ish document.
fn org_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(org_line(), 0..14).prop_map(|lines| lines.join("\n"))
}

proptest! {
    // ---- rope edits ----

    /// The rope and the line model agree: every line, joined back with
    /// newlines, is exactly the buffer's text. (The buffer normalizes
    /// CRLF on load, so the text to match is the buffer's own.)
    #[test]
    fn joining_every_line_reproduces_the_text(text in any_text(200)) {
        let b = buf(&text);
        let mut joined = String::new();
        for i in 0..b.line_count() {
            if i > 0 {
                joined.push('\n');
            }
            joined.push_str(&b.line(i));
        }
        prop_assert_eq!(joined, b.get_contents());
    }

    /// char_count is the number of characters, not bytes or lines.
    #[test]
    fn char_count_counts_characters(text in any_text(300)) {
        let b = buf(&text);
        prop_assert_eq!(b.char_count(), b.get_contents().chars().count());
    }

    /// A char offset and a (line, column) name the same place.
    #[test]
    fn offset_and_position_agree(text in any_text(200), offset in 0usize..400) {
        let b = buf(&text);
        let o = offset.min(b.char_count());
        prop_assert_eq!(b.pos_to_offset(b.offset_to_pos(o)), o);
    }

    /// Inserting then undoing puts the text back, wherever it landed.
    #[test]
    fn insert_then_undo_restores(
        text in any_text(120),
        insert in nonempty_text(40),
        offset in 0usize..200,
    ) {
        let mut b = buf(&text);
        let before = b.get_contents();
        b.set_offset(offset.min(b.char_count()));
        b.insert_str(&insert);
        b.break_undo_group();
        prop_assert!(b.undo());
        prop_assert_eq!(b.get_contents(), before);
    }

    /// Cutting a range and pasting it back at the same place changes
    /// nothing, at any offsets.
    #[test]
    fn cutting_and_pasting_back_restores(
        text in any_text(120),
        at in 0usize..160,
        len in 0usize..40,
    ) {
        let mut b = buf(&text);
        let before = b.get_contents();
        let n = b.char_count();
        let start = at.min(n);
        let end = (start + len).min(n);
        let cut = b.delete_range(b.offset_to_pos(start), b.offset_to_pos(end));
        // delete_range leaves point at the start of the cut.
        b.insert_str(&cut);
        prop_assert_eq!(b.get_contents(), before);
    }

    // ---- the Org parser ----

    /// Recounting statistics cookies is stable: a second pass over
    /// already-corrected text changes nothing (and neither pass panics).
    #[test]
    fn org_cookies_are_stable(text in org_text()) {
        let mut b = buf(&text);
        b.org_update_cookies();
        let once = b.get_contents();
        b.org_update_cookies();
        prop_assert_eq!(b.get_contents(), once);
    }

    /// The structure commands survive any line, including one that isn't
    /// there.
    #[test]
    fn org_structure_commands_never_panic(text in org_text(), line in 0usize..16) {
        let mut b = buf(&text);
        let l = line % b.line_count().max(1);
        let _ = b.org_cycle(l);
        let _ = b.org_global_cycle();
        let _ = b.org_toggle_checkbox(l);
        let _ = b.org_todo(l, true);
        let _ = b.org_next_heading(l, true, false);
        let _ = b.org_up_heading(l);
        let _ = b.org_mark_subtree(l);
        let _ = b.org_subtree_end(l);
        let _ = b.org_update_cookies();
    }

    // ---- the table formatter ----

    /// Formatting a table and formatting its own output again are the
    /// same: the formatter is idempotent on arbitrary rows.
    #[test]
    fn formatting_a_table_is_idempotent(lines in proptest::collection::vec(org_line(), 1..6)) {
        let rows: Vec<Row> = lines.iter().map(|l| org_table::parse_row(l)).collect();
        let once = org_table::format_rows(&rows, "");
        let again: Vec<Row> = once.iter().map(|l| org_table::parse_row(l)).collect();
        prop_assert_eq!(org_table::format_rows(&again, ""), once);
    }

    // ---- the exporters ----

    /// Every backend accepts arbitrary Org-ish text and produces the same
    /// bytes twice (no hidden global state, no panic on odd input).
    #[test]
    fn every_backend_exports_deterministically(text in org_text()) {
        let read = |_: &str| None;
        let settings = export::Settings::default();
        for backend in export::BACKENDS {
            let first = export::export(backend.name, &text, &read, &settings);
            let second = export::export(backend.name, &text, &read, &settings);
            prop_assert!(first.is_ok(), "{} rejected its own text", backend.name);
            prop_assert_eq!(
                first.map(|o| o.bytes().to_vec()),
                second.map(|o| o.bytes().to_vec()),
                "{} is not deterministic",
                backend.name
            );
        }
    }
}
