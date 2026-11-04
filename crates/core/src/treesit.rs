// SPDX-License-Identifier: MIT OR Apache-2.0
//! Structural navigation from tree-sitter trees: defuns, things, sexps
//! and lists. The editor turns these into commands; they only move offsets
//! and never touch point themselves.

use crate::buffer::Buffer;
use crate::syntax::Definition;
use std::ops::Range;
use tree_sitter as ts;

/// Which end of a thing to stop at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Beg,
    End,
}

/// How to move between nested things.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tactic {
    #[default]
    Nested,
    TopLevel,
    Restricted,
    ParentFirst,
}

impl Tactic {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "nested" => Self::Nested,
            "top-level" => Self::TopLevel,
            "restricted" => Self::Restricted,
            "parent-first" | "parent" => Self::ParentFirst,
            _ => return None,
        })
    }
}

/// Things treesit can navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thing {
    Defun,
    Function,
    Class,
    Heading,
    Comment,
    String,
    Number,
    Identifier,
    List,
    Expression,
    Statement,
}

impl Thing {
    pub const ALL: [Thing; 11] = [
        Self::Defun,
        Self::Function,
        Self::Class,
        Self::Heading,
        Self::Comment,
        Self::String,
        Self::Number,
        Self::Identifier,
        Self::List,
        Self::Expression,
        Self::Statement,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "defun" => Self::Defun,
            "function" => Self::Function,
            "class" => Self::Class,
            "heading" => Self::Heading,
            "comment" => Self::Comment,
            "string" => Self::String,
            "number" => Self::Number,
            "identifier" | "symbol" => Self::Identifier,
            "list" | "sexp" => Self::List,
            "expression" => Self::Expression,
            "statement" => Self::Statement,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Defun => "defun",
            Self::Function => "function",
            Self::Class => "class",
            Self::Heading => "heading",
            Self::Comment => "comment",
            Self::String => "string",
            Self::Number => "number",
            Self::Identifier => "identifier",
            Self::List => "list",
            Self::Expression => "expression",
            Self::Statement => "statement",
        }
    }

    fn tagged(self) -> bool {
        matches!(
            self,
            Self::Defun | Self::Function | Self::Class | Self::Heading
        )
    }
}

/// Definitions from the grammar's tags query, with whole ranges.
pub fn definitions(buf: &mut Buffer) -> Vec<Definition> {
    let Some(s) = buf.syntax.0.as_mut() else {
        return Vec::new();
    };
    s.definitions(&buf.text)
}

pub fn is_defun_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function" | "method" | "class" | "interface" | "module" | "macro"
    )
}

fn matches_kind(kind: &str, thing: Thing) -> bool {
    match thing {
        Thing::Defun => is_defun_kind(kind),
        Thing::Function => matches!(kind, "function" | "method"),
        Thing::Class => matches!(kind, "class" | "interface"),
        Thing::Heading => kind == "heading",
        _ => false,
    }
}

fn tagged_ranges(buf: &mut Buffer, thing: Thing) -> Vec<Range<usize>> {
    definitions(buf)
        .into_iter()
        .filter(|d| matches_kind(&d.kind, thing))
        .map(|d| d.start..d.end)
        .collect()
}

fn tag_ranges(buf: &mut Buffer, offset: usize, thing: Thing) -> Vec<Range<usize>> {
    definitions(buf)
        .into_iter()
        .filter(|d| matches_kind(&d.kind, thing))
        .map(|d| d.start..d.end)
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|r| r.start <= offset && offset <= r.end)
        .collect()
}

fn node_is_comment(kind: &str) -> bool {
    kind.contains("comment")
}

fn node_is_string(kind: &str) -> bool {
    kind.contains("string")
        && !kind.contains("content")
        && !kind.contains("fragment")
        && !kind.contains("escape")
        && !kind.contains("interpolation")
}

fn node_is_number(kind: &str) -> bool {
    kind.contains("number")
        || kind.contains("integer")
        || kind.contains("float")
        || matches!(kind, "int" | "decimal" | "numeric_literal")
}

fn node_is_identifier(kind: &str) -> bool {
    kind.contains("identifier") || matches!(kind, "ident" | "name" | "field_name")
}

fn node_is_expression(kind: &str) -> bool {
    kind.contains("expression")
        || kind.contains("call")
        || kind.contains("literal")
        || matches!(kind, "binary" | "unary" | "assignment")
}

fn node_is_statement(kind: &str) -> bool {
    kind.contains("statement") || kind.contains("declaration")
}

fn node_is_sexp(kind: &str) -> bool {
    node_is_expression(kind)
        || node_is_identifier(kind)
        || node_is_number(kind)
        || node_is_string(kind)
}

fn is_delimited_list(node: ts::Node, text: &ropey::Rope) -> bool {
    if node.named_child_count() == 0 {
        return false;
    }
    let slice = text.byte_slice(node.byte_range());
    let n = slice.len_chars();
    let Some(first) = (0..n).find_map(|i| slice.get_char(i).filter(|c| !c.is_whitespace())) else {
        return false;
    };
    let Some(last) = (0..n)
        .rev()
        .find_map(|i| slice.get_char(i).filter(|c| !c.is_whitespace()))
    else {
        return false;
    };
    matches!((first, last), ('(', ')') | ('[', ']') | ('{', '}'))
}

fn node_matches(node: ts::Node, text: &ropey::Rope, thing: Thing) -> bool {
    let kind = node.kind();
    match thing {
        Thing::Comment => node_is_comment(kind),
        Thing::String => node_is_string(kind),
        Thing::Number => node_is_number(kind),
        Thing::Identifier => node_is_identifier(kind),
        Thing::List => is_delimited_list(node, text),
        Thing::Expression => node_is_expression(kind),
        Thing::Statement => node_is_statement(kind),
        Thing::Defun | Thing::Function | Thing::Class | Thing::Heading => false,
    }
}

/// Collect tree-node ranges for things not defined by tags.
fn tree_ranges(buf: &mut Buffer, thing: Thing) -> Vec<Range<usize>> {
    let Some(s) = buf.syntax.0.as_mut() else {
        return Vec::new();
    };
    let Some(tree) = s.tree(&buf.text) else {
        return Vec::new();
    };
    let text = &buf.text;
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() && node_matches(node, text, thing) {
            out.push(text.byte_to_char(node.start_byte())..text.byte_to_char(node.end_byte()));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    out.sort_by_key(|r| (r.start, r.end));
    out.dedup();
    out
}

#[derive(Clone, PartialEq, Eq)]
struct SexpRange {
    range: Range<usize>,
    list: bool,
}

fn sexp_ranges(buf: &mut Buffer) -> Vec<SexpRange> {
    let Some(s) = buf.syntax.0.as_mut() else {
        return Vec::new();
    };
    let Some(tree) = s.tree(&buf.text) else {
        return Vec::new();
    };
    let text = &buf.text;
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() {
            let list = is_delimited_list(node, text);
            if list || node_is_sexp(node.kind()) {
                out.push(SexpRange {
                    range: text.byte_to_char(node.start_byte())..text.byte_to_char(node.end_byte()),
                    list,
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    out.sort_by_key(|r| (r.range.start, r.range.end, !r.list));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

/// The innermost sexp around `offset`, lists included.
pub fn sexp_at(buf: &mut Buffer, offset: usize) -> Option<Range<usize>> {
    sexp_ranges(buf)
        .into_iter()
        .filter(|r| r.range.start <= offset && offset <= r.range.end)
        .min_by_key(|r| r.range.end.saturating_sub(r.range.start))
        .map(|r| r.range)
}

/// The expression ending nearest before `offset`, for evaluating what is
/// before point. When several end there (an atom and the list or statement
/// holding it), the outermost one wins.
pub fn sexp_before(buf: &mut Buffer, offset: usize) -> Option<Range<usize>> {
    let s = buf.syntax.0.as_mut()?;
    let tree = s.tree(&buf.text)?;
    let text = &buf.text;
    let mut best: Option<Range<usize>> = None;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() && !node_is_comment(node.kind()) {
            let range = text.byte_to_char(node.start_byte())..text.byte_to_char(node.end_byte());
            if range.end <= offset
                && best.as_ref().is_none_or(|b| {
                    range.end > b.end
                        || (range.end == b.end
                            && range.end.saturating_sub(range.start)
                                > b.end.saturating_sub(b.start))
                })
            {
                best = Some(range);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    best
}

fn ranges_for(buf: &mut Buffer, thing: Thing) -> Vec<Range<usize>> {
    if thing.tagged() {
        tagged_ranges(buf, thing)
    } else {
        tree_ranges(buf, thing)
    }
}

fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && outer.end >= inner.end
}

fn innermost_containing(ranges: &[Range<usize>], offset: usize) -> Option<&Range<usize>> {
    ranges
        .iter()
        .filter(|r| r.start <= offset && offset <= r.end)
        .min_by_key(|r| r.end.saturating_sub(r.start))
}

fn apply_tactic(ranges: &[Range<usize>], offset: usize, tactic: Tactic) -> Vec<Range<usize>> {
    match tactic {
        Tactic::Nested | Tactic::ParentFirst => ranges.to_vec(),
        Tactic::TopLevel => ranges
            .iter()
            .filter(|r| !ranges.iter().any(|o| *o != **r && contains(o, r)))
            .cloned()
            .collect(),
        Tactic::Restricted => {
            let Some(parent) = innermost_containing(ranges, offset).cloned() else {
                return ranges.to_vec();
            };
            let inside: Vec<_> = ranges
                .iter()
                .filter(|r| contains(&parent, r))
                .cloned()
                .collect();
            if inside.is_empty() {
                ranges.to_vec()
            } else {
                inside
            }
        }
    }
}

fn next_boundary(ranges: &[Range<usize>], offset: usize, dir: i64, side: Side) -> Option<usize> {
    if dir > 0 {
        match side {
            Side::Beg => innermost_containing(ranges, offset)
                .filter(|r| r.start < offset)
                .map(|r| r.start)
                .or_else(|| {
                    ranges
                        .iter()
                        .filter(|r| r.start > offset)
                        .min_by_key(|r| r.start)
                        .map(|r| r.start)
                }),
            Side::End => innermost_containing(ranges, offset)
                .filter(|r| offset < r.end)
                .map(|r| r.end)
                .or_else(|| {
                    ranges
                        .iter()
                        .filter(|r| r.end > offset)
                        .min_by_key(|r| r.end)
                        .map(|r| r.end)
                }),
        }
    } else {
        match side {
            Side::Beg => ranges
                .iter()
                .filter(|r| r.start < offset)
                .max_by_key(|r| r.start)
                .map(|r| r.start),
            Side::End => ranges
                .iter()
                .filter(|r| r.end <= offset)
                .max_by_key(|r| r.end)
                .map(|r| r.end),
        }
    }
}

fn navigate_ranges(ranges: &[Range<usize>], offset: usize, arg: i64, side: Side) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        let next = next_boundary(ranges, pos, arg.signum(), side)?;
        if next == pos {
            return None;
        }
        pos = next;
    }
    Some(pos)
}

fn navigate(
    buf: &mut Buffer,
    offset: usize,
    arg: i64,
    side: Side,
    thing: Thing,
    tactic: Tactic,
) -> Option<usize> {
    let ranges = ranges_for(buf, thing);
    let ranges = apply_tactic(&ranges, offset, tactic);
    navigate_ranges(&ranges, offset, arg, side)
}

/// Move to the beginning of ARG defuns (negative ARG: forward).
pub fn beginning_of_defun(
    buf: &mut Buffer,
    offset: usize,
    arg: i64,
    tactic: Tactic,
) -> Option<usize> {
    navigate(buf, offset, -arg, Side::Beg, Thing::Defun, tactic)
}

/// Move to the end of ARG defuns (negative ARG: backward).
pub fn end_of_defun(buf: &mut Buffer, offset: usize, arg: i64, tactic: Tactic) -> Option<usize> {
    navigate(buf, offset, arg, Side::End, Thing::Defun, tactic)
}

/// The defun around `offset`, or the next one when point is between them.
pub fn defun_at(buf: &mut Buffer, offset: usize, tactic: Tactic) -> Option<Range<usize>> {
    let ranges = tagged_ranges(buf, Thing::Defun);
    let ranges = apply_tactic(&ranges, offset, tactic);
    if let Some(r) = innermost_containing(&ranges, offset) {
        return Some(r.clone());
    }
    ranges
        .iter()
        .filter(|r| r.start >= offset)
        .min_by_key(|r| r.start)
        .cloned()
}

/// The name of the defun at `offset`, with enclosing defuns joined by
/// `delimiter`.
pub fn add_log_current_defun(buf: &mut Buffer, offset: usize, delimiter: &str) -> Option<String> {
    let mut defs: Vec<Definition> = definitions(buf)
        .into_iter()
        .filter(|d| is_defun_kind(&d.kind) && d.start <= offset && offset <= d.end)
        .collect();
    defs.sort_by_key(|d| (d.start, std::cmp::Reverse(d.end)));
    let mut names: Vec<String> = Vec::new();
    let mut parent: Option<(usize, usize)> = None;
    for d in defs {
        let nested = parent.is_none_or(|(start, end)| start <= d.start && d.end <= end);
        if nested {
            parent = Some((d.start, d.end));
            names.push(d.name);
        }
    }
    (!names.is_empty()).then(|| names.join(delimiter))
}

/// The whole line of the heading at `offset`.
pub fn heading_at(buf: &mut Buffer, offset: usize) -> Option<Range<usize>> {
    let line = buf.text.char_to_line(offset);
    let is_heading = tagged_ranges(buf, Thing::Heading)
        .iter()
        .any(|r| buf.text.char_to_line(r.start) == line);
    if !is_heading {
        return None;
    }
    let start = buf.text.line_to_char(line);
    let len = buf
        .line(line)
        .trim_end_matches(['\n', '\r'])
        .chars()
        .count();
    Some(start..start + len)
}

fn heading_step(buf: &mut Buffer, offset: usize, forward: bool) -> Option<usize> {
    let starts: Vec<usize> = tagged_ranges(buf, Thing::Heading)
        .iter()
        .map(|r| buf.text.line_to_char(buf.text.char_to_line(r.start)))
        .collect();
    if forward {
        starts.into_iter().filter(|s| *s > offset).min()
    } else {
        starts.into_iter().filter(|s| *s < offset).max()
    }
}

/// Move to the start of the next outline heading (`outline-next-heading`).
/// A negative ARG moves backward.
pub fn next_heading(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        pos = heading_step(buf, pos, arg > 0)?;
    }
    Some(pos)
}

/// Move to the start of the previous outline heading. A negative ARG moves
/// forward.
pub fn previous_heading(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    next_heading(buf, offset, -arg)
}

/// The level of the heading at `offset`: the number of leading `#` (or
/// `*`) markers on its line (`outline-level`). None when point isn't on a
/// heading.
pub fn heading_level(buf: &mut Buffer, offset: usize) -> Option<usize> {
    let range = heading_at(buf, offset)?;
    let line = buf.text.char_to_line(range.start);
    let text = buf.line(line);
    let level = text
        .trim_start()
        .chars()
        .take_while(|c| matches!(c, '#' | '*'))
        .count();
    Some(level.max(1))
}

/// The innermost range of THING at `offset`.
pub fn thing_at(buf: &mut Buffer, offset: usize, thing: Thing) -> Option<Range<usize>> {
    let ranges = if thing.tagged() {
        tag_ranges(buf, offset, thing)
    } else {
        ranges_for(buf, thing)
    };
    innermost_containing(&ranges, offset).cloned()
}

/// `beginning-of-thing`: ARG positive moves backward.
pub fn beginning_of_thing(
    buf: &mut Buffer,
    offset: usize,
    arg: i64,
    thing: Thing,
    tactic: Tactic,
) -> Option<usize> {
    navigate(buf, offset, -arg, Side::Beg, thing, tactic)
}

/// `end-of-thing`: ARG positive moves forward.
pub fn end_of_thing(
    buf: &mut Buffer,
    offset: usize,
    arg: i64,
    thing: Thing,
    tactic: Tactic,
) -> Option<usize> {
    navigate(buf, offset, arg, Side::End, thing, tactic)
}

fn forward_sexp_step(ranges: &[SexpRange], offset: usize) -> Option<usize> {
    if let Some(r) = ranges
        .iter()
        .filter(|r| !r.list && r.range.start < offset && offset < r.range.end)
        .min_by_key(|r| r.range.end.saturating_sub(r.range.start))
    {
        return Some(r.range.end);
    }
    if let Some(r) = ranges
        .iter()
        .filter(|r| r.range.start == offset)
        .min_by_key(|r| {
            (
                if r.list { usize::MAX - 1 } else { usize::MAX },
                r.range.end.saturating_sub(r.range.start),
            )
        })
    {
        return Some(r.range.end);
    }
    ranges
        .iter()
        .filter(|r| r.range.start > offset)
        .min_by_key(|r| {
            (
                r.range.start,
                if r.list { usize::MAX - 1 } else { usize::MAX },
                r.range.end.saturating_sub(r.range.start),
            )
        })
        .map(|r| r.range.end)
}

fn backward_sexp_step(ranges: &[SexpRange], offset: usize) -> Option<usize> {
    if let Some(r) = ranges
        .iter()
        .filter(|r| !r.list && r.range.start < offset && offset < r.range.end)
        .min_by_key(|r| r.range.end.saturating_sub(r.range.start))
    {
        return Some(r.range.start);
    }
    ranges
        .iter()
        .filter(|r| r.range.end <= offset)
        .min_by_key(|r| (usize::MAX - r.range.end, r.range.start))
        .map(|r| r.range.start)
}

/// Move forward over ARG sexps.
pub fn forward_sexp(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let ranges = sexp_ranges(buf);
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        let next = if arg > 0 {
            forward_sexp_step(&ranges, pos)?
        } else {
            backward_sexp_step(&ranges, pos)?
        };
        if next == pos {
            return None;
        }
        pos = next;
    }
    Some(pos)
}

/// Move backward over ARG sexps.
pub fn backward_sexp(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    forward_sexp(buf, offset, -arg)
}

fn list_ranges(buf: &mut Buffer) -> Vec<Range<usize>> {
    tree_ranges(buf, Thing::List)
}

fn forward_list_step(ranges: &[Range<usize>], offset: usize) -> Option<usize> {
    if let Some(r) = ranges
        .iter()
        .filter(|r| r.start >= offset)
        .min_by_key(|r| r.start)
    {
        return Some(r.end);
    }
    innermost_containing(ranges, offset)
        .filter(|r| offset < r.end)
        .map(|r| r.end)
}

fn backward_list_step(ranges: &[Range<usize>], offset: usize) -> Option<usize> {
    if let Some(r) = innermost_containing(ranges, offset).filter(|r| r.start < offset) {
        return Some(r.start);
    }
    ranges
        .iter()
        .filter(|r| r.start < offset)
        .max_by_key(|r| r.start)
        .map(|r| r.start)
}

/// Move forward over ARG lists (`forward-list`).
pub fn forward_list(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let ranges = list_ranges(buf);
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        let next = if arg > 0 {
            forward_list_step(&ranges, pos)?
        } else {
            backward_list_step(&ranges, pos)?
        };
        if next == pos {
            return None;
        }
        pos = next;
    }
    Some(pos)
}

/// Move backward over ARG lists (`backward-list`).
pub fn backward_list(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    forward_list(buf, offset, -arg)
}

/// Move into ARG lists (`down-list`): just after their opening delimiter.
pub fn down_list(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let ranges = list_ranges(buf);
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        let next = if arg > 0 {
            ranges
                .iter()
                .filter(|r| r.start >= pos)
                .min_by_key(|r| (r.start, usize::MAX - r.end))
                .and_then(|r| r.start.checked_add(1))?
        } else {
            ranges
                .iter()
                .filter(|r| r.end <= pos)
                .max_by_key(|r| (r.end, r.start))
                .and_then(|r| r.end.checked_sub(1))?
        };
        if next == pos {
            return None;
        }
        pos = next;
    }
    Some(pos)
}

/// Move out of ARG lists (`up-list`): just after their closing delimiter.
pub fn up_list(buf: &mut Buffer, offset: usize, arg: i64) -> Option<usize> {
    if arg == 0 {
        return Some(offset);
    }
    let ranges = list_ranges(buf);
    let mut pos = offset;
    for _ in 0..arg.unsigned_abs() {
        let next = if arg > 0 {
            ranges
                .iter()
                .filter(|r| r.start <= pos && pos < r.end)
                .min_by_key(|r| r.end.saturating_sub(r.start))
                .map(|r| r.end)?
        } else {
            ranges
                .iter()
                .filter(|r| r.start < pos && pos <= r.end)
                .max_by_key(|r| r.start)
                .map(|r| r.start)?
        };
        if next == pos {
            return None;
        }
        pos = next;
    }
    Some(pos)
}

/// The language definition of a buffer, for callers that need its name.
pub fn language_name(buf: &Buffer) -> Option<&'static str> {
    buf.language().map(|l| l.name)
}

#[cfg(all(test, feature = "tree-sitter", feature = "grammars"))]
mod tests {
    use super::*;
    use crate::syntax;

    fn buffer(lang: &str, src: &str) -> Buffer {
        let mut buf = Buffer::new("t");
        buf.set_contents(src);
        buf.set_language(syntax::language(lang));
        buf
    }

    fn text_at(buf: &Buffer, range: Range<usize>) -> String {
        buf.text.slice(range).to_string()
    }

    #[test]
    fn defun_navigation_nested_and_top_level() {
        let src = "fn outer() {\n    fn inner() {}\n}\n";
        let mut buf = buffer("rust", src);
        let inner = src.find("inner").unwrap();
        let inner = buf.byte_to_char(inner);
        assert_eq!(
            beginning_of_defun(&mut buf, inner, 1, Tactic::Nested),
            Some(buf.byte_to_char(src.find("fn inner").unwrap()))
        );
        assert_eq!(
            beginning_of_defun(&mut buf, inner, 1, Tactic::TopLevel),
            Some(0)
        );
        let end = buf.char_count();
        assert_eq!(
            beginning_of_defun(&mut buf, end, 1, Tactic::Nested),
            Some(buf.byte_to_char(src.find("fn inner").unwrap()))
        );
        let start = src.find("fn outer").unwrap();
        let start = buf.byte_to_char(start);
        let end = end_of_defun(&mut buf, start, 1, Tactic::Nested).unwrap();
        assert_eq!(
            text_at(&buf, start..end),
            "fn outer() {\n    fn inner() {}\n}"
        );
    }

    #[test]
    fn defun_navigation_c_and_mark() {
        let src = "int main(void) {\n    return 0;\n}\n";
        let mut buf = buffer("c", src);
        let at = buf.byte_to_char(src.find("return").unwrap());
        let range = defun_at(&mut buf, at, Tactic::Nested).unwrap();
        assert_eq!(
            text_at(&buf, range.clone()),
            "int main(void) {\n    return 0;\n}"
        );
        assert_eq!(
            beginning_of_defun(&mut buf, at, 1, Tactic::Nested),
            Some(range.start)
        );
        assert_eq!(
            end_of_defun(&mut buf, at, 1, Tactic::Nested),
            Some(range.end)
        );
    }

    #[test]
    fn forward_and_backward_lists() {
        let src = "fn f() { g(a, b); }\n";
        let mut buf = buffer("rust", src);
        let g = buf.byte_to_char(src.find('g').unwrap());
        let open = buf.byte_to_char(src.find("g(").unwrap() + 1);
        let close = buf.byte_to_char(src.find("b)").unwrap() + 1);
        assert_eq!(forward_list(&mut buf, g, 1), Some(close + 1));
        assert_eq!(backward_list(&mut buf, close + 1, 1), Some(open));
        assert_eq!(down_list(&mut buf, g, 1), Some(open + 1));
        assert_eq!(up_list(&mut buf, open + 1, 1), Some(close + 1));

        let src = "fn f() { g(h(a), b); }\n";
        let mut buf = buffer("rust", src);
        let outer_open = buf.byte_to_char(src.find("g(").unwrap() + 1);
        let outer_close = buf.byte_to_char(src.rfind(')').unwrap());
        let inner_close = buf.byte_to_char(src.find("a)").unwrap() + 1);
        assert_eq!(
            forward_list(&mut buf, outer_open + 1, 1),
            Some(inner_close + 1)
        );
        let inner_open = buf.byte_to_char(src.find("h(").unwrap() + 1);
        assert_eq!(up_list(&mut buf, inner_open + 1, 2), Some(outer_close + 1));
    }

    #[test]
    fn sexps_and_things() {
        let src = "fn f() { g(1); }\n";
        let mut buf = buffer("rust", src);
        let list_start = buf.byte_to_char(src.find("(1)").unwrap()) + 1;
        assert_eq!(up_list(&mut buf, list_start, 1), Some(list_start + 2));

        let number = buf.byte_to_char(src.find('1').unwrap());
        let range = thing_at(&mut buf, number, Thing::Number).unwrap();
        assert_eq!(text_at(&buf, range), "1");

        let src = "fn f() { let x = 1; // hi\n}\n";
        let mut buf = buffer("rust", src);
        let comment = buf.byte_to_char(src.find("// hi").unwrap());
        let range = thing_at(&mut buf, comment, Thing::Comment).unwrap();
        assert_eq!(text_at(&buf, range), "// hi");
    }

    #[test]
    fn sexp_before_finds_the_expression_to_evaluate() {
        let src = "1 + 2\n";
        let mut buf = buffer("rhai", src);
        let end = buf.byte_to_char(src.trim_end().len());
        let range = sexp_before(&mut buf, end).expect("an expression before point");
        assert_eq!(text_at(&buf, range), "1 + 2");

        let src = "let x = 1 + 2;\n";
        let mut buf = buffer("rhai", src);
        let end = buf.byte_to_char(src.trim_end().len());
        let range = sexp_before(&mut buf, end).expect("a statement before point");
        assert_eq!(text_at(&buf, range), "let x = 1 + 2;");
    }

    #[test]
    fn forward_sexp_moves_over_atoms_then_lists() {
        let src = "fn f() { foo(1, 2); }\n";
        let mut buf = buffer("rust", src);
        let foo = buf.byte_to_char(src.find("foo").unwrap());
        let first = forward_sexp(&mut buf, foo, 1).unwrap();
        assert_eq!(text_at(&buf, foo..first), "foo");
        let second = forward_sexp(&mut buf, first, 1).unwrap();
        assert_eq!(text_at(&buf, first..second), "(1, 2)");
        assert_eq!(backward_sexp(&mut buf, second, 1), Some(foo));
    }

    #[test]
    fn thing_navigation_uses_tactics() {
        let src = "fn outer() {\n    fn inner() {}\n}\n";
        let mut buf = buffer("rust", src);
        let inner = buf.byte_to_char(src.find("inner").unwrap());
        assert_eq!(
            beginning_of_thing(&mut buf, inner, 1, Thing::Defun, Tactic::Nested),
            Some(buf.byte_to_char(src.find("fn inner").unwrap()))
        );
        assert_eq!(
            beginning_of_thing(&mut buf, inner, 1, Thing::Defun, Tactic::TopLevel),
            Some(0)
        );
    }

    #[test]
    fn add_log_names_join_enclosing_defuns() {
        let src = "class A:\n    def method(self):\n        return 1\n";
        let mut buf = buffer("python", src);
        let at = buf.byte_to_char(src.find("return").unwrap());
        assert_eq!(
            add_log_current_defun(&mut buf, at, ".").as_deref(),
            Some("A.method")
        );
        let outside = buf.byte_to_char(src.find("class").unwrap());
        assert_eq!(
            add_log_current_defun(&mut buf, outside, ".").as_deref(),
            Some("A")
        );
    }

    #[test]
    fn headings_move_and_report_levels() {
        let src = "# One\ntext\n## Two\nbody\n";
        let mut buf = buffer("markdown", src);
        let one = buf.byte_to_char(src.find("# One").unwrap());
        let two = buf.byte_to_char(src.find("## Two").unwrap());
        assert_eq!(next_heading(&mut buf, one, 1), Some(two));
        assert_eq!(next_heading(&mut buf, two, -1), Some(one));
        assert_eq!(previous_heading(&mut buf, two, 1), Some(one));
        assert_eq!(previous_heading(&mut buf, one, -1), Some(two));
        assert_eq!(heading_level(&mut buf, one), Some(1));
        assert_eq!(heading_level(&mut buf, two), Some(2));
        assert_eq!(heading_at(&mut buf, two), Some(two..two + "## Two".len()));
    }
}
