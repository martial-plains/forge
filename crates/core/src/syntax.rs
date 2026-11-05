// SPDX-License-Identifier: MIT OR Apache-2.0
//! Syntax trees, highlighting and indentation, with tree-sitter.
//!
//! Each buffer whose major mode has a grammar keeps a syntax tree next to
//! its text. Every edit goes through [`Buffer::apply`](crate::buffer),
//! which tells the tree what changed; the next redisplay reparses
//! incrementally (only what the edit touched), so highlighting a big file
//! costs about the same per keystroke as a small one. Highlighting then
//! runs the grammar's highlight query over the visible lines only.
//!
//! Grammars are compiled into Forge (see [`LANGUAGES`]). A mode derived
//! from one of these modes (by a plugin or init.rhai) gets its grammar too.

use crate::buffer::Edit;
use ropey::Rope;
#[cfg(feature = "tree-sitter")]
use std::ops::ControlFlow;
use std::ops::Range;
#[cfg(feature = "tree-sitter")]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(feature = "tree-sitter")]
use std::time::Instant;
#[cfg(feature = "tree-sitter")]
use tree_sitter as ts;
#[cfg(feature = "tree-sitter")]
use tree_sitter::StreamingIterator;

#[cfg(feature = "grammars")]
mod rhai_grammar {
    use tree_sitter_language::LanguageFn;

    unsafe extern "C" {
        fn tree_sitter_rhai() -> *const ();
    }

    pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_rhai) };
}

#[cfg(all(feature = "tree-sitter", feature = "grammars"))]
macro_rules! builtin_grammar {
    ($grammar:expr) => {
        GrammarSource::Builtin($grammar)
    };
}

#[cfg(all(feature = "tree-sitter", not(feature = "grammars")))]
macro_rules! builtin_grammar {
    ($grammar:expr) => {
        GrammarSource::None
    };
}

#[cfg(all(feature = "tree-sitter", feature = "grammars"))]
macro_rules! builtin_query {
    ($query:expr) => {
        $query
    };
}

#[cfg(all(feature = "tree-sitter", not(feature = "grammars")))]
macro_rules! builtin_query {
    ($query:expr) => {
        ""
    };
}

/// Buffers bigger than this aren't parsed.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// A parse that takes longer than this gives up (and the buffer goes
/// without highlighting) rather than freezing the editor.
pub const PARSE_TIME_LIMIT: Duration = Duration::from_secs(1);

/// What a piece of code is, for colors. Frontends pick the colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxFace {
    Keyword,
    String,
    /// Escapes in strings (`\n`).
    Escape,
    Comment,
    Function,
    Type,
    /// Numbers, booleans, constants.
    Constant,
    /// Built-in functions and variables (`self`, `print`).
    Builtin,
    /// Fields, properties, keys.
    Property,
    /// Attributes, decorators, labels.
    Attribute,
    /// A link (an Org `[[url][desc]]`, a Markdown link): colored and
    /// underlined, like `link` / `org-link`.
    Link,
    /// Org emphasis: `*bold*`, `/italic/`, `_underline_`, `+strike+`, drawn
    /// the way `bold`, `italic`, `underline` and `strike-through`
    /// faces are. Only Org's highlighter produces them.
    OrgBold,
    OrgItalic,
    OrgUnderline,
    OrgStrike,
}

impl SyntaxFace {
    pub const ALL: [SyntaxFace; 15] = [
        Self::Keyword,
        Self::String,
        Self::Escape,
        Self::Comment,
        Self::Function,
        Self::Type,
        Self::Constant,
        Self::Builtin,
        Self::Property,
        Self::Attribute,
        Self::Link,
        Self::OrgBold,
        Self::OrgItalic,
        Self::OrgUnderline,
        Self::OrgStrike,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Escape => "escape",
            Self::Comment => "comment",
            Self::Function => "function",
            Self::Type => "type",
            Self::Constant => "constant",
            Self::Builtin => "builtin",
            Self::Property => "property",
            Self::Attribute => "attribute",
            Self::Link => "link",
            Self::OrgBold => "org-bold",
            Self::OrgItalic => "org-italic",
            Self::OrgUnderline => "org-underline",
            Self::OrgStrike => "org-strike-through",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The face for a highlight query capture name ("function.method").
    /// None: shown in the default face (operators, punctuation, variables).
    pub fn from_capture(capture: &str) -> Option<Self> {
        let face = match capture {
            "link" | "link.uri" => Self::Link,
            "function.builtin" | "variable.builtin" => Self::Builtin,
            "constant.builtin" | "boolean" | "number" | "constant" => Self::Constant,
            "string.escape" | "escape" => Self::Escape,
            "string.special.key" => Self::Property,
            "constructor" => Self::Type,
            "label" => Self::Attribute,
            _ => match capture.split('.').next().unwrap_or("") {
                "keyword" | "repeat" | "conditional" | "preproc" | "include" | "exception" => {
                    Self::Keyword
                }
                "string" | "character" => Self::String,
                "comment" => Self::Comment,
                "function" | "method" => Self::Function,
                "type" => Self::Type,
                "constant" | "number" | "float" => Self::Constant,
                "property" | "field" => Self::Property,
                "attribute" | "decorator" => Self::Attribute,
                _ => return None,
            },
        };
        Some(face)
    }
}

/// A definition from the grammar's tags query, with the whole
/// definition's range (not just its name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub kind: String,
    /// First and last char of the definition.
    pub start: usize,
    pub end: usize,
    /// First and last char of its name.
    pub name_start: usize,
    pub name_end: usize,
}

/// How a language indents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentStyle {
    /// Columns per level.
    pub width: usize,
    /// Indent with tabs (8 columns each), padding with spaces.
    pub tabs: bool,
}

#[cfg(feature = "tree-sitter")]
pub(crate) enum GrammarSource {
    #[allow(dead_code)]
    #[cfg(feature = "grammars")]
    Builtin(fn() -> ts::Language),
    Dynamic {
        library: &'static libloading::Library,
        symbol: &'static str,
    },
    #[allow(dead_code)]
    None,
}

#[cfg(feature = "tree-sitter")]
impl GrammarSource {
    fn language(&self) -> Option<ts::Language> {
        match self {
            #[cfg(feature = "grammars")]
            Self::Builtin(grammar) => Some(grammar()),
            Self::Dynamic { library, symbol } => unsafe {
                let get = library
                    .get::<unsafe extern "C" fn() -> *const ()>(symbol.as_bytes())
                    .ok()?;
                Some(tree_sitter_language::LanguageFn::from_raw(*get).into())
            },
            Self::None => None,
        }
    }
}

/// A language Forge knows: its major mode, grammar and indentation rules.
pub struct LanguageDef {
    pub name: &'static str,
    pub mode: &'static str,
    /// File name extensions without the dot.
    pub extensions: &'static [&'static str],
    /// Whole file names ("Makefile").
    pub file_names: &'static [&'static str],
    /// `#!` interpreters ("python3", "bash").
    pub interpreters: &'static [&'static str],
    pub indent: IndentStyle,
    #[cfg(feature = "tree-sitter")]
    grammar: GrammarSource,
    #[cfg(feature = "tree-sitter")]
    highlights: &'static str,
    /// Definitions for imenu (tree-sitter's tags.scm; "" for none).
    #[cfg(feature = "tree-sitter")]
    tags: &'static str,
    #[cfg(feature = "tree-sitter")]
    compiled: OnceLock<Option<Compiled>>,
    /// Indentation from the tree: nodes whose lines after the first are
    /// indented one level (empty: use the line-based rules only).
    #[allow(dead_code)]
    indent_nodes: &'static [&'static str],
    /// A line starting with one of these belongs to the level of the node
    /// it closes (`}`, `end`, `else`).
    #[allow(dead_code)]
    dedent_tokens: &'static [&'static str],
    /// Line-based rules (fallback, and languages like Python): a line
    /// ending with an opener indents the next one; one starting with a
    /// closer goes back a level.
    openers: &'static [&'static str],
    closers: &'static [&'static str],
    /// Python: after these statements the block is over.
    block_enders: &'static [&'static str],
    /// What starts a comment to the end of the line (M-;), if anything.
    pub line_comment: Option<&'static str>,
    /// Characters electric-pair-mode inserts in pairs: open, close, ...
    pub pairs: &'static str,
}

/// Pairs for buffers without a language.
pub const DEFAULT_PAIRS: &str = "()[]{}\"\"";

const BRACKETS_OPEN: &[&str] = &["{", "(", "["];
const BRACKETS_CLOSE: &[&str] = &["}", ")", "]"];

/// The parent of the major mode a language's grammar is auto-registered
/// under: text for prose (Markdown), prog for code.
pub fn mode_parent(mode: &str) -> Option<&'static str> {
    match mode {
        "markdown-mode" => Some("text-mode"),
        _ => Some("prog-mode"),
    }
}

/// Highlighting for Markdown, in Forge's faces (the grammar ships none):
/// headings and their markers, code, links, emphasis and HTML.
#[cfg(feature = "tree-sitter")]
const MARKDOWN_HIGHLIGHTS: &str = r#"
(atx_heading (heading_content) @type)
(setext_heading (heading_content) @type)
[
  (atx_h1_marker) (atx_h2_marker) (atx_h3_marker)
  (atx_h4_marker) (atx_h5_marker) (atx_h6_marker)
  (setext_h1_underline) (setext_h2_underline)
  (thematic_break)
  (list_marker)
  (table_delimiter_row)
] @constant
[
  (fenced_code_block) (indented_code_block)
  (code_span) (code_fence_content)
  (link_title) (info_string) (html_attribute_value)
] @string
[
  (link_destination) (uri_autolink) (www_autolink) (email_autolink)
  (link_text) (link_label)
] @link
(image_description) @function
(emphasis) @property
(strong_emphasis) @keyword
(strikethrough) @comment
(html_comment) @comment
[
  (backslash_escape) (character_reference) (hard_line_break)
] @escape
[
  (html_tag_name) (html_close_tag) (html_declaration_name)
] @keyword
(html_attribute_key) @property
"#;

/// Definitions for imenu: the headings.
#[cfg(feature = "tree-sitter")]
const MARKDOWN_TAGS: &str = r#"
(atx_heading (heading_content) @name @definition.heading)
(setext_heading (heading_content) @name @definition.heading)
"#;

/// Highlighting for Rhai (the vendored grammar ships a query in Forge's
/// face vocabulary, but this keeps the compiled-in copy authoritative).
#[cfg(feature = "tree-sitter")]
const RHAI_HIGHLIGHTS: &str =
    include_str!("../third_party/tree-sitter-rhai/queries/highlights.scm");

/// Definitions for imenu: functions and variables.
#[cfg(feature = "tree-sitter")]
const RHAI_TAGS: &str = r#"
(ExprFn fn_name: (FnDeclName) @name) @definition.function
(ExprDeclareVar name: (ident) @name) @definition.variable
"#;

/// The languages built into Forge.
pub static LANGUAGES: [LanguageDef; 11] = [
    LanguageDef {
        name: "rust",
        mode: "rust-mode",
        extensions: &["rs"],
        file_names: &[],
        interpreters: &[],
        indent: IndentStyle {
            width: 4,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_rust::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_rust::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_rust::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "block",
            "declaration_list",
            "field_declaration_list",
            "ordered_field_declaration_list",
            "field_initializer_list",
            "enum_variant_list",
            "match_block",
            "arguments",
            "parameters",
            "array_expression",
            "tuple_expression",
            "token_tree",
            "use_list",
            "where_clause",
        ],
        dedent_tokens: BRACKETS_CLOSE,
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: Some("//"),
        pairs: "()[]{}\"\"",
    },
    LanguageDef {
        name: "python",
        mode: "python-mode",
        extensions: &["py", "pyi", "pyw"],
        file_names: &[],
        interpreters: &["python", "python3", "python2"],
        indent: IndentStyle {
            width: 4,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_python::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_python::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_python::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[],
        dedent_tokens: &[],
        openers: &[":", "(", "[", "{"],
        closers: &[")", "]", "}", "else", "elif", "except", "finally"],
        block_enders: &["return", "pass", "break", "continue", "raise"],
        line_comment: Some("#"),
        pairs: "()[]{}\"\"''",
    },
    LanguageDef {
        name: "javascript",
        mode: "js-mode",
        extensions: &["js", "mjs", "cjs", "jsx"],
        file_names: &[],
        interpreters: &["node"],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_javascript::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_javascript::HIGHLIGHT_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_javascript::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "statement_block",
            "class_body",
            "object",
            "array",
            "arguments",
            "formal_parameters",
            "switch_body",
            "switch_case",
            "switch_default",
            "object_pattern",
            "array_pattern",
            "named_imports",
            "export_clause",
            "parenthesized_expression",
        ],
        dedent_tokens: BRACKETS_CLOSE,
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: Some("//"),
        pairs: "()[]{}\"\"''``",
    },
    LanguageDef {
        name: "c",
        mode: "c-mode",
        extensions: &["c", "h"],
        file_names: &[],
        interpreters: &[],
        indent: IndentStyle {
            width: 4,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_c::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_c::HIGHLIGHT_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_c::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "compound_statement",
            "field_declaration_list",
            "enumerator_list",
            "initializer_list",
            "argument_list",
            "parameter_list",
            "case_statement",
        ],
        dedent_tokens: BRACKETS_CLOSE,
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: Some("//"),
        pairs: "()[]{}\"\"",
    },
    LanguageDef {
        name: "go",
        mode: "go-mode",
        extensions: &["go"],
        file_names: &[],
        interpreters: &[],
        indent: IndentStyle {
            width: 8,
            tabs: true,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_go::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_go::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_go::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "block",
            "field_declaration_list",
            "interface_type",
            "literal_value",
            "argument_list",
            "parameter_list",
            "import_spec_list",
            "const_declaration",
            "var_declaration",
            "expression_case",
            "default_case",
            "type_case",
            "communication_case",
        ],
        dedent_tokens: BRACKETS_CLOSE,
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: Some("//"),
        pairs: "()[]{}\"\"``",
    },
    LanguageDef {
        name: "json",
        mode: "json-mode",
        extensions: &["json"],
        file_names: &[],
        interpreters: &[],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_json::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_json::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(""),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &["object", "array"],
        dedent_tokens: &["}", "]"],
        openers: &["{", "["],
        closers: &["}", "]"],
        block_enders: &[],
        line_comment: None,
        pairs: "[]{}\"\"",
    },
    LanguageDef {
        name: "toml",
        mode: "toml-mode",
        extensions: &["toml"],
        file_names: &["Cargo.lock"],
        interpreters: &[],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_toml_ng::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(""),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[],
        dedent_tokens: &[],
        openers: &["[", "{"],
        closers: &["]", "}"],
        block_enders: &[],
        line_comment: Some("#"),
        pairs: "[]{}\"\"''",
    },
    LanguageDef {
        name: "bash",
        mode: "sh-mode",
        extensions: &["sh", "bash"],
        file_names: &[".bashrc", ".bash_profile", ".profile"],
        interpreters: &["sh", "bash", "zsh", "dash"],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_bash::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_bash::HIGHLIGHT_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(""),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[],
        dedent_tokens: &[],
        openers: &["then", "do", "else", "{", "(", "in"],
        closers: &["fi", "done", "esac", "else", "elif", "}", ")"],
        block_enders: &[],
        line_comment: Some("#"),
        pairs: "()[]{}\"\"''",
    },
    LanguageDef {
        name: "lua",
        mode: "lua-mode",
        extensions: &["lua"],
        file_names: &[],
        interpreters: &["lua", "luajit"],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| tree_sitter_lua::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(tree_sitter_lua::HIGHLIGHTS_QUERY),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(tree_sitter_lua::TAGS_QUERY),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "function_declaration",
            "function_definition",
            "if_statement",
            "for_statement",
            "while_statement",
            "repeat_statement",
            "do_statement",
            "table_constructor",
            "arguments",
            "parameters",
        ],
        dedent_tokens: &["end", "else", "elseif", "until", "}", ")"],
        openers: &["then", "do", "else", "repeat", "{", "("],
        closers: &["end", "else", "elseif", "until", "}", ")"],
        block_enders: &[],
        line_comment: Some("--"),
        pairs: "()[]{}\"\"''",
    },
    LanguageDef {
        name: "rhai",
        mode: "rhai-mode",
        extensions: &["rhai"],
        file_names: &[],
        interpreters: &["rhai"],
        indent: IndentStyle {
            width: 4,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(|| rhai_grammar::LANGUAGE.into()),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(RHAI_HIGHLIGHTS),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(RHAI_TAGS),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[
            "ExprBlock",
            "ExprArray",
            "ExprObject",
            "ArgList",
            "ParamList",
            "SwitchArmList",
            "ClosureParamList",
        ],
        dedent_tokens: &["}", "]", ")"],
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: Some("//"),
        pairs: DEFAULT_PAIRS,
    },
    LanguageDef {
        name: "markdown",
        mode: "markdown-mode",
        extensions: &["md", "markdown", "mkd", "mdown", "mkdn", "mdwn"],
        file_names: &["README.md"],
        interpreters: &[],
        indent: IndentStyle {
            width: 2,
            tabs: false,
        },
        #[cfg(feature = "tree-sitter")]
        grammar: builtin_grammar!(tree_sitter_markdown_fork::language),
        #[cfg(feature = "tree-sitter")]
        highlights: builtin_query!(MARKDOWN_HIGHLIGHTS),
        #[cfg(feature = "tree-sitter")]
        tags: builtin_query!(MARKDOWN_TAGS),
        #[cfg(feature = "tree-sitter")]
        compiled: OnceLock::new(),
        indent_nodes: &[],
        dedent_tokens: &[],
        openers: &[],
        closers: &[],
        block_enders: &[],
        line_comment: None,
        pairs: DEFAULT_PAIRS,
    },
];

#[cfg(feature = "tree-sitter")]
static DYNAMIC: OnceLock<std::sync::Mutex<Vec<&'static LanguageDef>>> = OnceLock::new();

#[cfg(feature = "tree-sitter")]
fn dynamic_languages() -> Vec<&'static LanguageDef> {
    DYNAMIC
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("grammar registry poisoned")
        .clone()
}

/// All languages Forge knows: built-ins plus grammars registered at runtime.
pub fn languages() -> Vec<&'static LanguageDef> {
    #[cfg(feature = "tree-sitter")]
    {
        let mut all: Vec<&'static LanguageDef> = LANGUAGES.iter().collect();
        all.extend(dynamic_languages());
        all
    }
    #[cfg(not(feature = "tree-sitter"))]
    {
        LANGUAGES.iter().collect()
    }
}

/// A grammar loaded from a shared library at runtime.
#[cfg(feature = "tree-sitter")]
#[derive(Debug, Clone)]
pub struct DynamicGrammar {
    pub name: String,
    pub mode: String,
    pub extensions: Vec<String>,
    pub file_names: Vec<String>,
    pub interpreters: Vec<String>,
    pub library: std::path::PathBuf,
    /// The exported `tree_sitter_*` symbol (default: from the name).
    pub symbol: Option<String>,
    pub highlights: String,
    pub tags: String,
    pub indent_width: usize,
    pub indent_tabs: bool,
    pub line_comment: Option<String>,
    pub pairs: String,
}

#[cfg(feature = "tree-sitter")]
impl DynamicGrammar {
    pub fn new(name: impl Into<String>, library: impl Into<std::path::PathBuf>) -> Self {
        let name = name.into();
        Self {
            mode: format!("{name}-mode"),
            name,
            extensions: Vec::new(),
            file_names: Vec::new(),
            interpreters: Vec::new(),
            library: library.into(),
            symbol: None,
            highlights: String::new(),
            tags: String::new(),
            indent_width: 4,
            indent_tabs: false,
            line_comment: None,
            pairs: DEFAULT_PAIRS.to_string(),
        }
    }
}

#[cfg(feature = "tree-sitter")]
fn leak_string(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[cfg(feature = "tree-sitter")]
fn leak_strings(items: Vec<String>) -> &'static [&'static str] {
    Box::leak(
        items
            .into_iter()
            .map(|s| leak_string(s))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

/// Load a grammar shared library and register it as a language.
#[cfg(feature = "tree-sitter")]
pub fn register_grammar(spec: DynamicGrammar) -> Result<&'static LanguageDef, String> {
    // Google Play doesn't allow an app to download or build native code and
    // load it (Device and Network Abuse). The common grammars are compiled in.
    if spec.name.trim().is_empty() {
        return Err("grammar name can't be empty".into());
    }
    if spec.mode.trim().is_empty() {
        return Err("grammar mode can't be empty".into());
    }
    let all = languages();
    if all.iter().any(|l| l.name == spec.name) {
        return Err(format!("grammar {:?} is already registered", spec.name));
    }
    if all.iter().any(|l| l.mode == spec.mode) {
        return Err(format!("major mode {:?} is already registered", spec.mode));
    }
    let library = unsafe {
        libloading::Library::new(&spec.library)
            .map_err(|e| format!("loading {}: {e}", spec.library.display()))?
    };
    let library: &'static libloading::Library = Box::leak(Box::new(library));
    let symbol = spec
        .symbol
        .unwrap_or_else(|| format!("tree_sitter_{}", spec.name.replace(['-', '.'], "_")));
    let symbol: &'static str = leak_string(symbol);
    unsafe {
        library
            .get::<unsafe extern "C" fn() -> *const ()>(symbol.as_bytes())
            .map_err(|e| format!("symbol {symbol:?} in {}: {e}", spec.library.display()))?;
    }
    let def = LanguageDef {
        name: leak_string(spec.name),
        mode: leak_string(spec.mode),
        extensions: leak_strings(spec.extensions),
        file_names: leak_strings(spec.file_names),
        interpreters: leak_strings(spec.interpreters),
        indent: IndentStyle {
            width: spec.indent_width.max(1),
            tabs: spec.indent_tabs,
        },
        grammar: GrammarSource::Dynamic { library, symbol },
        highlights: leak_string(spec.highlights),
        tags: leak_string(spec.tags),
        compiled: OnceLock::new(),
        indent_nodes: &[],
        dedent_tokens: &[],
        openers: BRACKETS_OPEN,
        closers: BRACKETS_CLOSE,
        block_enders: &[],
        line_comment: spec.line_comment.map(leak_string),
        pairs: leak_string(spec.pairs),
    };
    let def: &'static LanguageDef = Box::leak(Box::new(def));
    DYNAMIC
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("grammar registry poisoned")
        .push(def);
    Ok(def)
}

/// A language by name ("rust"), built in or registered at runtime.
pub fn language(name: &str) -> Option<&'static LanguageDef> {
    languages().into_iter().find(|l| l.name == name)
}

/// The language whose major mode is `mode` itself (not a derived mode;
/// see ModeRegistry::language_for for that).
pub fn language_for_mode(mode: &str) -> Option<&'static LanguageDef> {
    languages().into_iter().find(|l| l.mode == mode)
}

/// The mode for a script's first line (`#!/usr/bin/env python3`).
pub fn mode_for_shebang(first_line: &str) -> Option<&'static str> {
    let rest = first_line.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut prog = words.next()?.rsplit('/').next()?;
    if prog == "env" {
        prog = words.find(|w| !w.starts_with('-'))?;
    }
    let prog = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    languages()
        .into_iter()
        .find(|l| {
            l.interpreters
                .iter()
                .any(|i| i.trim_end_matches(|c: char| c.is_ascii_digit()) == prog)
        })
        .map(|l| l.mode)
}

#[cfg(feature = "tree-sitter")]
struct Compiled {
    language: ts::Language,
    highlights: ts::Query,
    tags: Option<ts::Query>,
    /// Face for each capture index of `highlights`.
    faces: Vec<Option<SyntaxFace>>,
}

/// The grammar and its compiled queries (compiled once, on first use).
#[cfg(feature = "tree-sitter")]
fn compiled(def: &'static LanguageDef) -> Option<&'static Compiled> {
    def.compiled
        .get_or_init(|| {
            let language = def.grammar.language()?;
            let highlights = ts::Query::new(&language, def.highlights).ok()?;
            let faces = highlights
                .capture_names()
                .iter()
                .map(|n| SyntaxFace::from_capture(n))
                .collect();
            let tags = (!def.tags.is_empty())
                .then(|| ts::Query::new(&language, def.tags).ok())
                .flatten();
            Some(Compiled {
                language,
                highlights,
                tags,
                faces,
            })
        })
        .as_ref()
}

#[cfg(feature = "tree-sitter")]
fn point_at(text: &Rope, char_idx: usize) -> (usize, ts::Point) {
    let byte = text.char_to_byte(char_idx);
    let row = text.char_to_line(char_idx);
    (
        byte,
        ts::Point {
            row,
            column: byte - text.line_to_byte(row),
        },
    )
}

/// Expand a tags "definition" node to the whole definition: C's query
/// captures only the declarator, and decorated Python definitions include
/// their decorators. JavaScript variable functions include their `const`.
#[cfg(feature = "tree-sitter")]
fn definition_node<'a>(mut node: ts::Node<'a>, kind: &str) -> ts::Node<'a> {
    loop {
        let Some(parent) = node.parent() else {
            return node;
        };
        let expand = match kind {
            "function" | "method" => matches!(
                (node.kind(), parent.kind()),
                (
                    "function_declarator" | "pointer_declarator" | "parenthesized_declarator",
                    "function_definition" | "declaration" | "field_declaration"
                ) | (
                    "variable_declarator" | "assignment_expression",
                    "lexical_declaration"
                        | "variable_declaration"
                        | "export_statement"
                        | "expression_statement"
                ) | (
                    "function_definition" | "class_definition",
                    "decorated_definition"
                )
            ),
            "class" | "interface" => {
                matches!(
                    (node.kind(), parent.kind()),
                    ("class_definition", "decorated_definition")
                )
            }
            _ => false,
        };
        if !expand {
            return node;
        }
        node = parent;
    }
}

/// A buffer's syntax tree.
#[cfg(feature = "tree-sitter")]
pub struct Syntax {
    def: &'static LanguageDef,
    parser: ts::Parser,
    tree: Option<ts::Tree>,
    /// Edited since the last parse.
    stale: bool,
    /// Parsing failed or took too long: no highlighting for this buffer.
    failed: bool,
    definitions: Option<Vec<Definition>>,
}

#[cfg(feature = "tree-sitter")]
impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax")
            .field("language", &self.def.name)
            .field("failed", &self.failed)
            .finish()
    }
}

#[cfg(feature = "tree-sitter")]
impl Syntax {
    pub fn new(def: &'static LanguageDef) -> Option<Self> {
        let c = compiled(def)?;
        let mut parser = ts::Parser::new();
        parser.set_language(&c.language).ok()?;
        Some(Self {
            def,
            parser,
            tree: None,
            stale: true,
            failed: false,
            definitions: None,
        })
    }

    pub fn language(&self) -> &'static LanguageDef {
        self.def
    }

    /// Called before `e` is applied to `text`: move the tree's nodes along.
    pub(crate) fn edit(&mut self, text: &Rope, e: &Edit) {
        self.stale = true;
        self.definitions = None;
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let (start_byte, start_position) = point_at(text, e.at);
        let (old_end_byte, old_end_position) = point_at(text, e.at + e.deleted.chars().count());
        let new_end_byte = start_byte + e.inserted.len();
        let new_end_position = match e.inserted.rfind('\n') {
            Some(i) => ts::Point {
                row: start_position.row + e.inserted.matches('\n').count(),
                column: e.inserted.len() - i - 1,
            },
            None => ts::Point {
                row: start_position.row,
                column: start_position.column + e.inserted.len(),
            },
        };
        tree.edit(&ts::InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position,
        });
    }

    /// The text was replaced wholesale: parse from scratch next time.
    pub(crate) fn reset(&mut self) {
        self.tree = None;
        self.stale = true;
        self.failed = false;
        self.definitions = None;
    }

    /// The tree for `text`, parsing (incrementally) if it's out of date.
    pub fn tree(&mut self, text: &Rope) -> Option<&ts::Tree> {
        if self.failed {
            return None;
        }
        if self.stale || self.tree.is_none() {
            if text.len_bytes() > MAX_BYTES {
                self.failed = true;
                return None;
            }
            let started = Instant::now();
            let mut progress = |_: &ts::ParseState| {
                if started.elapsed() > PARSE_TIME_LIMIT {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            let mut read = |byte: usize, _: ts::Point| -> &[u8] {
                if byte >= text.len_bytes() {
                    return &[];
                }
                let (chunk, start, _, _) = text.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - start..]
            };
            let options = ts::ParseOptions::new().progress_callback(&mut progress);
            match self
                .parser
                .parse_with_options(&mut read, self.tree.as_ref(), Some(options))
            {
                Some(t) => self.tree = Some(t),
                None => {
                    self.parser.reset();
                    self.tree = None;
                    self.failed = true;
                    return None;
                }
            }
            self.stale = false;
        }
        self.tree.as_ref()
    }

    /// Faces for chars `range` (char offsets), one per char (None: default).
    ///
    /// When captures overlap, the innermost node wins; for the same node,
    /// the first pattern in the query wins (tree-sitter's convention).
    pub fn faces(&mut self, text: &Rope, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        let mut out = vec![None; range.len()];
        let Some(c) = compiled(self.def) else {
            return out;
        };
        let Some(tree) = self.tree(text) else {
            return out;
        };
        let (start_byte, end_byte) = (text.char_to_byte(range.start), text.char_to_byte(range.end));
        // (start, end, pattern, face), in bytes.
        let mut spans: Vec<(usize, usize, usize, Option<SyntaxFace>)> = Vec::new();
        let mut cursor = ts::QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let provider = |node: ts::Node| {
            text.byte_slice(node.byte_range())
                .chunks()
                .map(str::as_bytes)
        };
        let mut captures = cursor.captures(&c.highlights, tree.root_node(), provider);
        while let Some((m, i)) = captures.next() {
            let cap = m.captures()[*i];
            let r = cap.node.byte_range();
            spans.push((r.start, r.end, m.pattern_index, c.faces[cap.index as usize]));
        }
        // Paint outer spans first, and for equal ranges later patterns first.
        spans.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)).then(b.2.cmp(&a.2)));
        for (s, e, _, face) in spans {
            let s = text.byte_to_char(s.clamp(start_byte, end_byte));
            let e = text.byte_to_char(e.clamp(start_byte, end_byte));
            for slot in &mut out[s - range.start..e - range.start] {
                *slot = face;
            }
        }
        out
    }

    /// Definitions (functions, types, methods) for imenu: (name, kind,
    /// char offset of the name), in buffer order.
    pub fn tags(&mut self, text: &Rope) -> Vec<(String, String, usize)> {
        let Some(c) = compiled(self.def) else {
            return Vec::new();
        };
        let Some(query) = c.tags.as_ref() else {
            return Vec::new();
        };
        let Some(tree) = self.tree(text) else {
            return Vec::new();
        };
        let names = query.capture_names();
        let mut out: Vec<(String, String, usize)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = ts::QueryCursor::new();
        let provider = |node: ts::Node| {
            text.byte_slice(node.byte_range())
                .chunks()
                .map(str::as_bytes)
        };
        let mut matches = cursor.matches(query, tree.root_node(), provider);
        while let Some(m) = matches.next() {
            let kind = m
                .captures()
                .iter()
                .find_map(|c| names[c.index as usize].strip_prefix("definition."));
            let name = m
                .captures()
                .iter()
                .find(|c| names[c.index as usize] == "name");
            if let (Some(kind), Some(name)) = (kind, name) {
                // The same function can match "method" and "function".
                if seen.insert(name.node.byte_range()) {
                    // Markdown headings arrive with a leading space (# Head).
                    let text_of = text.byte_slice(name.node.byte_range()).to_string();
                    let text_of = text_of.trim().to_string();
                    out.push((
                        text_of,
                        kind.to_string(),
                        text.byte_to_char(name.node.start_byte()),
                    ));
                }
            }
        }
        out.sort_by_key(|t| t.2);
        out
    }

    /// Definitions with their whole ranges, from the grammar's tags query,
    /// in buffer order. Cached until the text changes.
    pub fn definitions(&mut self, text: &Rope) -> Vec<Definition> {
        if let Some(defs) = &self.definitions {
            return defs.clone();
        }
        let Some(c) = compiled(self.def) else {
            return Vec::new();
        };
        let Some(query) = c.tags.as_ref() else {
            return Vec::new();
        };
        let Some(tree) = self.tree(text) else {
            return Vec::new();
        };
        let names = query.capture_names();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = ts::QueryCursor::new();
        let provider = |node: ts::Node| {
            text.byte_slice(node.byte_range())
                .chunks()
                .map(str::as_bytes)
        };
        let mut matches = cursor.matches(query, tree.root_node(), provider);
        while let Some(m) = matches.next() {
            let kind = m
                .captures()
                .iter()
                .find_map(|c| names[c.index as usize].strip_prefix("definition."));
            let name = m
                .captures()
                .iter()
                .find(|c| names[c.index as usize] == "name");
            let definition = m
                .captures()
                .iter()
                .find(|c| names[c.index as usize].starts_with("definition."));
            let (Some(kind), Some(name), Some(definition)) = (kind, name, definition) else {
                continue;
            };
            // The same function can match "method" and "function".
            if !seen.insert(name.node.byte_range()) {
                continue;
            }
            let node = definition_node(definition.node, kind);
            let raw = text.byte_slice(name.node.byte_range()).to_string();
            let name_text = raw.trim();
            if name_text.is_empty() {
                continue;
            }
            let name_start = name.node.start_byte() + (raw.len() - raw.trim_start().len());
            let name_end = name_start + name_text.len();
            out.push(Definition {
                name: name_text.to_string(),
                kind: kind.to_string(),
                start: text.byte_to_char(node.start_byte()),
                end: text.byte_to_char(node.end_byte()),
                name_start: text.byte_to_char(name_start),
                name_end: text.byte_to_char(name_end),
            });
        }
        out.sort_by_key(|d| (d.start, d.name_start));
        self.definitions = Some(out.clone());
        out
    }

    /// The bracket matching the one at byte `at`, from the tree: None if
    /// the tree can't say (no tree, or it isn't a bracket token there).
    pub fn bracket_match(&mut self, text: &Rope, at: usize) -> Option<usize> {
        let tree = self.tree(text)?;
        let node = tree.root_node().descendant_for_byte_range(at, at + 1)?;
        let kind = node.kind();
        let (open, close) = match kind {
            "(" | ")" => ("(", ")"),
            "[" | "]" => ("[", "]"),
            "{" | "}" => ("{", "}"),
            _ => return None,
        };
        if node.start_byte() != at || node.child_count() != 0 {
            return None;
        }
        let parent = node.parent()?;
        let mut cursor = parent.walk();
        let children: Vec<ts::Node> = parent.children(&mut cursor).collect();
        let i = children.iter().position(|c| c.id() == node.id())?;
        let found = if kind == open {
            children[i + 1..].iter().find(|c| c.kind() == close)
        } else {
            children[..i].iter().rev().find(|c| c.kind() == open)
        };
        found.map(|c| c.start_byte())
    }

    /// Columns line `line` should be indented to, from the syntax tree.
    /// None when the tree can't tell (no tree-based rules for this
    /// language, or the line is in a syntax error); use [`heuristic_indent`].
    pub fn indent(&mut self, text: &Rope, line: usize) -> Option<usize> {
        let def = self.def;
        if def.indent_nodes.is_empty() {
            return None;
        }
        let tree = self.tree(text)?;
        let line_start = text.line_to_byte(line);
        let line_text = text.line(line).to_string();
        let ws = line_text.len() - line_text.trim_start_matches([' ', '\t']).len();
        let blank = line_text.trim().is_empty();
        let byte = line_start
            + if blank {
                line_text.trim_end_matches(['\n', '\r']).len()
            } else {
                ws
            };
        let root = tree.root_node();
        // Just after a syntax error (typing `{` RET gives one until the
        // `}` is there), the tree can't be trusted.
        if root.has_error() {
            // The last non-blank byte before this line.
            let mut idx = text.line_to_char(line);
            for c in text.chars_at(idx).reversed() {
                idx -= 1;
                if !c.is_whitespace() {
                    break;
                }
            }
            let prev = if idx < text.line_to_char(line) {
                text.char_to_byte(idx) + 1
            } else {
                0
            };
            if prev > 0 {
                let mut n = root.descendant_for_byte_range(prev - 1, prev - 1);
                while let Some(x) = n {
                    if x.is_error() || x.is_missing() {
                        return None;
                    }
                    n = x.parent();
                }
            }
        }
        let mut node = root.descendant_for_byte_range(byte, byte)?;
        // The first token on the line (for closers like `}` or `end`).
        let first =
            (!blank && node.start_byte() == byte && node.child_count() == 0).then_some(node);
        let closes = |n: ts::Node| {
            first.is_some_and(|t| {
                def.dedent_tokens.contains(&t.kind())
                    && (t.parent() == Some(n) || t.parent().and_then(|p| p.parent()) == Some(n))
            })
        };
        let mut level = 0;
        let mut last_row = None;
        loop {
            if node.is_error() {
                return None;
            }
            let start_row = node.start_position().row;
            if start_row < line
                && def.indent_nodes.contains(&node.kind())
                && !closes(node)
                && last_row != Some(start_row)
            {
                level += 1;
                last_row = Some(start_row);
            }
            match node.parent() {
                Some(p) => node = p,
                None => break,
            }
        }
        Some(level * def.indent.width)
    }
}

/// A buffer's language metadata without a syntax tree (Forge built
/// without tree-sitter).
#[cfg(not(feature = "tree-sitter"))]
pub struct Syntax {
    def: &'static LanguageDef,
}

#[cfg(not(feature = "tree-sitter"))]
impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax")
            .field("language", &self.def.name)
            .finish()
    }
}

#[cfg(not(feature = "tree-sitter"))]
impl Syntax {
    pub fn new(def: &'static LanguageDef) -> Option<Self> {
        Some(Self { def })
    }

    pub fn language(&self) -> &'static LanguageDef {
        self.def
    }

    pub(crate) fn edit(&mut self, _text: &Rope, _e: &Edit) {}

    pub(crate) fn reset(&mut self) {}

    pub fn faces(&mut self, _text: &Rope, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        vec![None; range.len()]
    }

    pub fn tags(&mut self, _text: &Rope) -> Vec<(String, String, usize)> {
        Vec::new()
    }

    pub fn definitions(&mut self, _text: &Rope) -> Vec<Definition> {
        Vec::new()
    }

    pub fn bracket_match(&mut self, _text: &Rope, _at: usize) -> Option<usize> {
        None
    }

    pub fn indent(&mut self, _text: &Rope, _line: usize) -> Option<usize> {
        None
    }
}

/// Columns of leading whitespace (tabs to the next multiple of 8).
pub fn indentation_width(line: &str) -> usize {
    let mut w = 0;
    for c in line.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += crate::width::TAB_WIDTH - w % crate::width::TAB_WIDTH,
            _ => break,
        }
    }
    w
}

/// Byte index where a line comment starts in `line` (not inside a
/// string, as far as a quote count can tell).
pub fn comment_start(line: &str, marker: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = line[from..].find(marker) {
        let at = from + i;
        if line[..at].matches(['"', '\'']).count().is_multiple_of(2) {
            return Some(at);
        }
        from = at + marker.len();
    }
    None
}

/// The text of a line without a trailing comment (roughly: good enough to
/// see whether it ends with an opener).
fn code_part<'a>(line: &'a str, def: &LanguageDef) -> &'a str {
    let Some(marker) = def.line_comment else {
        return line;
    };
    match line.find(marker) {
        // Not inside a string, as far as a quote count can tell.
        Some(i) if line[..i].matches(['"', '\'']).count().is_multiple_of(2) => &line[..i],
        _ => line,
    }
}

fn ends_with_word(s: &str, w: &str) -> bool {
    s.ends_with(w)
        && (w.chars().all(|c| !c.is_alphanumeric())
            || !s[..s.len() - w.len()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

fn starts_with_word(s: &str, w: &str) -> bool {
    s.starts_with(w)
        && (w.chars().all(|c| !c.is_alphanumeric())
            || !s[w.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

/// Line-based indentation: like the previous non-blank line, one level
/// more after an opener (`{`, `:` in Python, `then`), one less for a
/// line starting with a closer.
pub fn heuristic_indent(text: &Rope, line: usize, def: &LanguageDef) -> usize {
    let unit = def.indent.width;
    let prev = (0..line)
        .rev()
        .map(|l| text.line(l).to_string())
        .find(|l| !l.trim().is_empty());
    let Some(prev) = prev else { return 0 };
    let mut indent = indentation_width(&prev);
    let prev_code = code_part(prev.trim(), def).trim_end();
    if def.openers.iter().any(|o| ends_with_word(prev_code, o)) {
        indent += unit;
    } else if def
        .block_enders
        .iter()
        .any(|w| starts_with_word(prev_code, w))
    {
        indent = indent.saturating_sub(unit);
    }
    let this = text.line(line).to_string();
    let this = this.trim_start();
    // Python: `else:` and friends line up with their `if` / `try`.
    if def.name == "python" {
        let partners: &[&str] = match this
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("")
        {
            "else" => &["if", "elif", "for", "while", "try", "except"],
            "elif" => &["if", "elif"],
            "except" => &["try", "except"],
            "finally" => &["try", "except", "else"],
            _ => &[],
        };
        if !partners.is_empty() {
            let body = indentation_width(&prev);
            let found = (0..line)
                .rev()
                .map(|l| text.line(l).to_string())
                .find_map(|l| {
                    let w = indentation_width(&l);
                    let t = l.trim_start();
                    (w < body && !t.is_empty() && partners.iter().any(|p| starts_with_word(t, p)))
                        .then_some(w)
                });
            if let Some(w) = found {
                return w;
            }
        }
    }
    if def.closers.iter().any(|c| starts_with_word(this, c)) {
        indent = indent.saturating_sub(unit);
    }
    indent
}

/// Leading whitespace for `columns` of indentation.
pub fn indent_string(columns: usize, style: IndentStyle) -> String {
    if style.tabs {
        let tw = crate::width::TAB_WIDTH;
        format!("{}{}", "\t".repeat(columns / tw), " ".repeat(columns % tw))
    } else {
        " ".repeat(columns)
    }
}

#[cfg(all(test, feature = "tree-sitter", feature = "grammars"))]
mod tests {
    use super::*;

    fn syntax(lang: &str, text: &str) -> (Syntax, Rope) {
        (
            Syntax::new(language(lang).unwrap()).unwrap(),
            Rope::from_str(text),
        )
    }

    /// "face:text" for each run of the same face.
    fn runs(s: &mut Syntax, text: &Rope) -> Vec<String> {
        let faces = s.faces(text, 0..text.len_chars());
        let chars: Vec<char> = text.chars().collect();
        let mut out: Vec<(Option<SyntaxFace>, String)> = Vec::new();
        for (c, f) in chars.into_iter().zip(faces) {
            match out.last_mut() {
                Some((lf, t)) if *lf == f => t.push(c),
                _ => out.push((f, c.to_string())),
            }
        }
        out.into_iter()
            .filter_map(|(f, t)| f.map(|f| format!("{}:{t}", f.name())))
            .collect()
    }

    #[test]
    fn every_language_compiles_and_highlights() {
        let samples = [
            (
                "rust",
                "fn main() { let s = \"hi\"; // note\n}\n",
                "keyword:fn",
            ),
            ("python", "def f():\n    return 'x'  # c\n", "keyword:def"),
            (
                "javascript",
                "function f() { return 1; }\n",
                "keyword:function",
            ),
            ("c", "int main(void) { return 0; }\n", "keyword:return"),
            ("go", "package main\nfunc main() {}\n", "keyword:package"),
            ("json", "{\"a\": [1, true]}\n", "constant:1"),
            ("toml", "[x]\na = \"b\"\n", "string:\"b\""),
            ("bash", "if true; then echo hi; fi\n", "keyword:if"),
            (
                "lua",
                "local function f() return nil end\n",
                "keyword:local",
            ),
            ("rhai", "let x = 1; // note\nfn f() { x }\n", "keyword:fn"),
            (
                "markdown",
                "# Title\n\nSome **bold** and `code`.\n",
                "type: Title",
            ),
        ];
        for (lang, src, want) in samples {
            let (mut s, text) = syntax(lang, src);
            let got = runs(&mut s, &text);
            assert!(got.iter().any(|r| r == want), "{lang}: {got:?}");
        }
    }

    #[test]
    fn rust_faces_and_precedence() {
        let (mut s, text) = syntax("rust", "// c\nfn add(x: u32) -> u32 { x + 1 }\n");
        let got = runs(&mut s, &text);
        assert_eq!(got[0], "comment:// c");
        assert!(got.contains(&"function:add".to_string()), "{got:?}");
        assert!(got.contains(&"type:u32".to_string()), "{got:?}");
        assert!(got.contains(&"constant:1".to_string()), "{got:?}");
    }

    #[test]
    fn rhai_highlights_keywords_functions_and_strings() {
        let src = "// note\nfn add(a, b) { a + b }\nlet s = \"hi\";\nlet on = true;\n";
        let (mut s, text) = syntax("rhai", src);
        let got = runs(&mut s, &text);
        for want in [
            "comment:// note",
            "keyword:fn",
            "function:add",
            "keyword:let",
            "string:\"hi\"",
            "constant:true",
        ] {
            assert!(got.iter().any(|r| r == want), "{want}: {got:?}");
        }
        let tags = s.tags(&text);
        assert!(
            tags.iter().any(|(n, k, _)| n == "add" && k == "function"),
            "{tags:?}"
        );
        assert!(
            tags.iter().any(|(n, k, _)| n == "s" && k == "variable"),
            "{tags:?}"
        );
    }

    #[test]
    fn markdown_highlights_blocks_and_inline() {
        let src = "# Head\n\n- item\n\n**bold** and *em* and `code` and [t](u)\n\n\
                   > quote\n\n```rust\nfn f() {}\n```\n";
        let (mut s, text) = syntax("markdown", src);
        let got = runs(&mut s, &text).join(" ");
        for want in [
            "constant:#",       // heading marker
            "type: Head",       // heading text (the space is the marker's)
            "constant:-",       // list marker
            "keyword:**bold**", // strong emphasis
            "property:*em*",    // emphasis
            "string:`code`",    // inline code, delimiters included
            "link:t",           // link text
            "link:u",           // link destination
            "string:```rust",   // fence and its info string
        ] {
            assert!(got.contains(want), "missing {want:?} in {got}");
        }
        let tags = s.tags(&text);
        assert!(
            tags.iter().any(|(n, k, _)| n == "Head" && k == "heading"),
            "{tags:?}"
        );
    }

    #[test]
    fn definitions_include_whole_ranges() {
        let (mut s, text) = syntax("rust", "fn a() {}\nfn b() { let x = 1; }\n");
        let defs = s.definitions(&text);
        assert_eq!(defs.len(), 2, "{defs:?}");
        let a = &defs[0];
        assert_eq!(a.name, "a");
        assert_eq!(a.kind, "function");
        assert_eq!(a.name_start, 3);
        assert_eq!(a.name_end, 4);
        assert_eq!(text.slice(a.start..a.end).to_string(), "fn a() {}");

        let (mut s, text) = syntax("c", "int main(void) {\n    return 0;\n}\n");
        let defs = s.definitions(&text);
        let main = &defs[0];
        assert_eq!(main.name, "main");
        let range = text.slice(main.start..main.end).to_string();
        assert!(range.starts_with("int main(void)"), "{range:?}");
        assert!(range.ends_with('}'), "{range:?}");

        let (mut s, text) = syntax("javascript", "const f = () => 1;\n");
        let defs = s.definitions(&text);
        let f = &defs[0];
        assert_eq!(f.name, "f");
        assert!(
            text.slice(f.start..f.end)
                .to_string()
                .starts_with("const f"),
            "{f:?}"
        );

        let (mut s, text) = syntax("markdown", "# Head\n\ntext\n");
        let defs = s.definitions(&text);
        let head = &defs[0];
        assert_eq!(head.name, "Head");
        assert_eq!(head.name_start, 2);
        assert_eq!(
            text.slice(head.name_start..head.name_end).to_string(),
            "Head"
        );

        let (mut s, text) = syntax("python", "@decorator\ndef f():\n    pass\n");
        let defs = s.definitions(&text);
        let f = &defs[0];
        assert!(
            text.slice(f.start..f.end)
                .to_string()
                .starts_with("@decorator"),
            "{f:?}"
        );
    }

    #[test]
    fn edits_reparse_incrementally() {
        let (mut s, mut text) = syntax("rust", "fn a() {}\n");
        assert!(s.tree(&text).is_some());
        let e = Edit {
            at: 0,
            deleted: String::new(),
            inserted: "// ".into(),
        };
        s.edit(&text, &e);
        text.insert(0, "// ");
        assert_eq!(runs(&mut s, &text), ["comment:// fn a() {}"]);
        let e = Edit {
            at: 0,
            deleted: "// ".into(),
            inserted: String::new(),
        };
        s.edit(&text, &e);
        text.remove(0..3);
        assert!(runs(&mut s, &text).contains(&"keyword:fn".to_string()));
    }

    fn tree_indents(lang: &str, src: &str) -> Vec<Option<usize>> {
        let (mut s, text) = syntax(lang, src);
        (0..text.len_lines()).map(|l| s.indent(&text, l)).collect()
    }

    #[test]
    fn tree_indentation_rhai() {
        let src = "fn f() {\n    let x = [\n        1,\n    ];\n}\n";
        assert_eq!(
            tree_indents("rhai", src),
            [Some(0), Some(4), Some(8), Some(4), Some(0), Some(0)]
        );
    }

    #[test]
    fn tree_indentation_rust() {
        let src = "fn f(\n    a: u32,\n) {\n    if a {\n        g(1,\n          2);\n    }\n\n    match a {\n        _ => {}\n    }\n}\n";
        let want: Vec<usize> = src.lines().map(indentation_width).collect();
        let got = tree_indents("rust", src);
        for (i, w) in want.iter().enumerate() {
            // Line 5 is aligned by hand; the tree says one level in.
            if i == 5 {
                assert_eq!(got[i], Some(12));
                continue;
            }
            let expect = if src.lines().nth(i).unwrap().is_empty() {
                4
            } else {
                *w
            };
            assert_eq!(got[i], Some(expect), "line {i}: {:?}", src.lines().nth(i));
        }
    }

    #[test]
    fn tree_indentation_go_lua_js() {
        let go = "func f() {\n\tswitch x {\n\tcase 1:\n\t\tg()\n\t}\n}\n";
        let got = tree_indents("go", go);
        assert_eq!(
            &got[..6],
            &[Some(0), Some(8), Some(8), Some(16), Some(8), Some(0)]
        );

        let lua = "function f()\n  if x then\n    y()\n  else\n    z()\n  end\nend\n";
        let got = tree_indents("lua", lua);
        assert_eq!(
            &got[..7],
            &[
                Some(0),
                Some(2),
                Some(4),
                Some(2),
                Some(4),
                Some(2),
                Some(0)
            ]
        );

        let js = "const o = {\n  a: [\n    1,\n  ],\n};\n";
        let got = tree_indents("javascript", js);
        assert_eq!(&got[..5], &[Some(0), Some(2), Some(4), Some(2), Some(0)]);
    }

    #[test]
    fn after_a_syntax_error_the_tree_passes() {
        // Right after typing `{` RET the tree has an error: the line-based
        // rules take over.
        let got = tree_indents("rust", "fn f() {\n\n");
        assert_eq!(got[1], None);
        let (_, text) = syntax("rust", "fn f() {\n\n");
        assert_eq!(heuristic_indent(&text, 1, language("rust").unwrap()), 4);
    }

    #[test]
    fn heuristic_indentation() {
        let py = language("python").unwrap();
        let text = Rope::from_str("def f():\n    if x:\n        return 1\n\n    else:\n");
        assert_eq!(heuristic_indent(&text, 1, py), 4);
        assert_eq!(heuristic_indent(&text, 2, py), 8);
        assert_eq!(
            heuristic_indent(&text, 3, py),
            4,
            "after return the block is over"
        );
        assert_eq!(
            heuristic_indent(&text, 4, py),
            4,
            "else lines up with its if"
        );
        let text = Rope::from_str("    if x:  # why\n        y\n    else:\n");
        assert_eq!(
            heuristic_indent(&text, 1, py),
            8,
            "a comment doesn't hide the colon"
        );
        assert_eq!(heuristic_indent(&text, 2, py), 4);

        let sh = language("bash").unwrap();
        let text = Rope::from_str("if x; then\n  y\nfi\n");
        assert_eq!(heuristic_indent(&text, 1, sh), 2);
        assert_eq!(heuristic_indent(&text, 2, sh), 0);
        let text = Rope::from_str("athen\nx\n");
        assert_eq!(heuristic_indent(&text, 1, sh), 0, "only whole words");
    }

    #[test]
    fn big_files_parse_once_then_incrementally() {
        let src: String = (0..20_000)
            .map(|i| format!("fn f{i}(x: u32) -> u32 {{\n    x + {i} // c\n}}\n"))
            .collect();
        let (mut s, mut text) = syntax("rust", &src);
        let t = Instant::now();
        assert!(s.tree(&text).is_some(), "60k lines parse within the limit");
        let first = t.elapsed();
        let e = Edit {
            at: 10,
            deleted: String::new(),
            inserted: "y".into(),
        };
        s.edit(&text, &e);
        text.insert(10, "y");
        let t = Instant::now();
        let faces = s.faces(&text, 0..2000);
        let again = t.elapsed();
        assert_eq!(faces[0], Some(SyntaxFace::Keyword));
        eprintln!("first parse {first:?}, edit + reparse + highlight {again:?}");
        assert!(
            again < first / 4 + Duration::from_millis(20),
            "incremental: {again:?} vs {first:?}"
        );
    }

    #[test]
    fn every_tags_query_compiles() {
        for def in LANGUAGES.iter() {
            let compiled = compiled(def).unwrap_or_else(|| panic!("{} has no grammar", def.name));
            if !def.tags.is_empty() {
                assert!(
                    compiled.tags.is_some(),
                    "{}'s tags query doesn't compile",
                    def.name
                );
            }
        }
    }

    #[test]
    fn dynamic_grammar_errors_are_clear() {
        let err = register_grammar(DynamicGrammar::new("", "/nowhere.dylib"))
            .err()
            .expect("empty name is rejected");
        assert!(err.contains("empty"), "{err}");
        let err = register_grammar(DynamicGrammar::new("rust", "/nowhere.dylib"))
            .err()
            .expect("duplicate name is rejected");
        assert!(err.contains("already registered"), "{err}");
        let err = register_grammar(DynamicGrammar::new(
            "forge-no-such-grammar",
            "/definitely/not/here.dylib",
        ))
        .err()
        .expect("missing library is rejected");
        assert!(err.contains("loading"), "{err}");
    }

    #[test]
    fn shebangs_and_indent_strings() {
        assert_eq!(
            mode_for_shebang("#!/usr/bin/env python3"),
            Some("python-mode")
        );
        assert_eq!(mode_for_shebang("#!/bin/bash -e"), Some("sh-mode"));
        assert_eq!(
            mode_for_shebang("#!/usr/bin/env -S lua5.4"),
            Some("lua-mode")
        );
        assert_eq!(mode_for_shebang("#!/usr/bin/env rhai"), Some("rhai-mode"));
        assert_eq!(mode_for_shebang("#!/usr/bin/perl"), None);
        assert_eq!(mode_for_shebang("print(1)"), None);
        let tabs = IndentStyle {
            width: 8,
            tabs: true,
        };
        assert_eq!(indent_string(20, tabs), "\t\t    ");
        assert_eq!(
            indent_string(
                3,
                IndentStyle {
                    width: 4,
                    tabs: false
                }
            ),
            "   "
        );
        assert_eq!(indentation_width("\t  x"), 10);
    }
}
