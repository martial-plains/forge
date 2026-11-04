// SPDX-License-Identifier: MIT OR Apache-2.0
//! Cross-references (definitions, references) a plugin found, keyed by the
//! plugin that published them. consult-xref lists them; a language plugin
//! fills them from an LSP response.

/// One cross-reference: a symbol name and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xref {
    /// The symbol's name.
    pub name: String,
    /// What it is: "definition", "reference", ...
    pub kind: String,
    /// The file it is in.
    pub path: String,
    /// 1-based line and column, as compilers count.
    pub line: u32,
    pub column: u32,
}
