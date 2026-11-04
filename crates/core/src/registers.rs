// SPDX-License-Identifier: MIT OR Apache-2.0
//! Registers: named by one character, holding text, a rectangle, a
//! position, or a number.
//!
//! Positions are kept as (offset, buffer version) and carried across later
//! edits by the buffer's edit journal, so C-x r j still lands on the same
//! text after you've edited above it. The map lives on
//! [`crate::app::Editor`] so the plugin runtime can read it.

use crate::BufferId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Register {
    Text(String),
    Rectangle(Vec<String>),
    Position {
        buffer: BufferId,
        offset: usize,
        version: u64,
    },
    Number(i64),
}

/// The one-line description `list-registers` and plugins show.
pub fn describe(r: &Register) -> String {
    match r {
        Register::Text(t) => {
            let one_line: String = t
                .chars()
                .take(60)
                .map(|c| if c == '\n' { '⏎' } else { c })
                .collect();
            format!(
                "text: {one_line}{}",
                if t.chars().count() > 60 { "…" } else { "" }
            )
        }
        Register::Rectangle(lines) => format!(
            "rectangle, {} lines: {}",
            lines.len(),
            lines.first().map_or("", |l| l.as_str())
        ),
        Register::Position { offset, .. } => format!("position {offset}"),
        Register::Number(n) => format!("number {n}"),
    }
}

/// The text a register holds for insertion, if it holds any.
pub fn as_text(r: &Register) -> Option<String> {
    match r {
        Register::Text(t) => Some(t.clone()),
        Register::Rectangle(lines) => Some(lines.join("\n")),
        Register::Number(n) => Some(n.to_string()),
        Register::Position { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_names_the_kind() {
        assert_eq!(describe(&Register::Text("hi".into())), "text: hi");
        assert_eq!(describe(&Register::Number(3)), "number 3");
        assert_eq!(
            describe(&Register::Position {
                buffer: BufferId(1),
                offset: 12,
                version: 0
            }),
            "position 12"
        );
        assert_eq!(
            describe(&Register::Rectangle(vec!["a".into(), "b".into()])),
            "rectangle, 2 lines: a"
        );
    }

    #[test]
    fn text_only_for_insertable_registers() {
        assert_eq!(as_text(&Register::Number(7)).as_deref(), Some("7"));
        assert_eq!(
            as_text(&Register::Rectangle(vec!["a".into(), "b".into()])).as_deref(),
            Some("a\nb")
        );
        assert_eq!(
            as_text(&Register::Position {
                buffer: BufferId(1),
                offset: 0,
                version: 0
            }),
            None
        );
    }
}
