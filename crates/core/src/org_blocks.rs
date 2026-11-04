// SPDX-License-Identifier: MIT OR Apache-2.0
//! Source blocks (Org Babel's text side): finding blocks, header
//! arguments, results, noweb references and tangling. Running a block
//! is the editor's job. Also the block-aware highlighting of Org buffers.

use crate::buffer::Buffer;
use crate::org::heading_level;
use crate::syntax::{self, LanguageDef, Syntax, SyntaxFace};
use ropey::Rope;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

/// A `#+BEGIN_SRC` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrcBlock {
    /// Line of `#+BEGIN_SRC`.
    pub begin: usize,
    /// Line of `#+END_SRC` (the line count if it's missing).
    pub end: usize,
    pub lang: String,
    /// What follows the language on the begin line.
    pub params: String,
    /// `#+NAME:` of the block.
    pub name: Option<String>,
    /// `#+HEADER:` lines above it, and `#+CALL`-style extras.
    pub headers: Vec<String>,
    /// Indentation of the `#+BEGIN_SRC` line.
    pub indent: usize,
    /// First line of the affiliated keywords (== begin when none).
    pub first: usize,
}

impl SrcBlock {
    /// Body lines, with the block's indentation and `,*` escapes removed.
    pub fn body_lines(&self, lines: &[String]) -> Vec<String> {
        let end = self.end.min(lines.len());
        lines[(self.begin + 1).min(end)..end]
            .iter()
            .map(|l| {
                let mut s = l.as_str();
                let mut n = 0;
                while n < self.indent && s.starts_with(' ') {
                    s = &s[1..];
                    n += 1;
                }
                let t = s.trim_start();
                if t.starts_with(",*") || t.starts_with(",#+") {
                    let ind = s.len() - t.len();
                    format!("{}{}", &s[..ind], &t[1..])
                } else {
                    s.to_string()
                }
            })
            .collect()
    }

    pub fn body(&self, lines: &[String]) -> String {
        self.body_lines(lines).join("\n")
    }
}

fn begin_src(l: &str) -> Option<(usize, String, String)> {
    let t = l.trim_start();
    // `get` so a multi-byte character at the cut doesn't panic.
    if !t
        .get(..11)
        .is_some_and(|head| head.eq_ignore_ascii_case("#+begin_src"))
    {
        return None;
    }
    let rest = &t[11..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    let (lang, params) = match rest.split_once(char::is_whitespace) {
        Some((a, b)) => (a.to_string(), b.trim().to_string()),
        None => (rest.to_string(), String::new()),
    };
    Some((l.len() - t.len(), lang, params))
}

fn is_end_src(l: &str) -> bool {
    l.trim().eq_ignore_ascii_case("#+end_src")
}

/// All source blocks, in order.
pub fn src_blocks(lines: &[String]) -> Vec<SrcBlock> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_other: Option<String> = None;
    while i < lines.len() {
        let l = &lines[i];
        let low = l.trim_start().to_ascii_lowercase();
        if let Some(name) = &in_other {
            if low.starts_with(&format!("#+end_{name}")) {
                in_other = None;
            }
            i += 1;
            continue;
        }
        if let Some((indent, lang, params)) = begin_src(l) {
            let mut end = i + 1;
            while end < lines.len() && !is_end_src(&lines[end]) {
                end += 1;
            }
            let mut first = i;
            let mut name = None;
            let mut headers = Vec::new();
            while first > 0 {
                let p = lines[first - 1].trim_start();
                let up = p.to_ascii_uppercase();
                if up.starts_with("#+NAME:") {
                    name = Some(p[7..].trim().to_string());
                } else if up.starts_with("#+HEADER:") || up.starts_with("#+HEADERS:") {
                    headers.push(p[p.find(':').unwrap() + 1..].trim().to_string());
                } else if up.starts_with("#+CAPTION")
                    || up.starts_with("#+ATTR_")
                    || up.starts_with("#+RESNAME:")
                    || up.starts_with("#+SRCNAME:")
                {
                } else {
                    break;
                }
                first -= 1;
            }
            headers.reverse();
            out.push(SrcBlock {
                begin: i,
                end,
                lang,
                params,
                name,
                headers,
                indent,
                first,
            });
            i = end + 1;
            continue;
        }
        if let Some(r) = low.strip_prefix("#+begin_") {
            let name: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
            if matches!(name.as_str(), "example" | "export" | "comment") {
                in_other = Some(name);
            }
        }
        i += 1;
    }
    out
}

/// The block containing `line` (the keywords above it count).
pub fn block_at(lines: &[String], line: usize) -> Option<SrcBlock> {
    src_blocks(lines)
        .into_iter()
        .find(|b| (b.first..=b.end).contains(&line))
}

/// Whether `line` is inside a source, example, export or comment block,
/// where Org markup is shown literally.
pub fn in_literal_block(lines: &[String], line: usize) -> bool {
    let mut kind: Option<String> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start().to_ascii_lowercase();
        if let Some(k) = &kind {
            if t.starts_with(&format!("#+end_{k}")) {
                if i == line {
                    return true;
                }
                kind = None;
            }
            if i == line {
                return true;
            }
            continue;
        }
        if let Some(r) = t.strip_prefix("#+begin_") {
            let k: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
            if matches!(k.as_str(), "src" | "example" | "export" | "comment") {
                if i == line {
                    return true;
                }
                kind = Some(k);
            }
        }
        if i > line && kind.is_none() {
            break;
        }
    }
    false
}

// ------------------------------------------------------- header arguments

/// Words of a header-argument string; `"quoted words"` stay together.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = false;
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '"' => {
                quote = !quote;
                cur.push(c);
            }
            '(' | '[' if !quote => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' if !quote => {
                depth -= 1;
                cur.push(c);
            }
            c if c.is_whitespace() && !quote && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `:key value` pairs, in order. A key with no value gets "".
pub fn parse_header_args(s: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for w in words(s) {
        if let Some(k) = w.strip_prefix(':')
            && !k.is_empty()
            && !k.starts_with(|c: char| c.is_ascii_digit())
        {
            out.push((k.to_string(), String::new()));
        } else if let Some(last) = out.last_mut() {
            if !last.1.is_empty() {
                last.1.push(' ');
            }
            last.1.push_str(&w);
        }
        // Switches like `-n` before any `:key` are ignored.
    }
    out
}

/// Merge `more` into `base`: a later value for a key replaces the earlier
/// one, except `:var`, which accumulates (a later binding of the same
/// variable replaces it).
pub fn merge_args(base: &mut Vec<(String, String)>, more: Vec<(String, String)>) {
    for (k, v) in more {
        if k == "var" {
            let name = v.split('=').next().unwrap_or("").trim().to_string();
            base.retain(|(bk, bv)| {
                !(bk == "var" && bv.split('=').next().unwrap_or("").trim() == name)
            });
            base.push((k, v));
        } else {
            base.retain(|(bk, _)| *bk != k);
            base.push((k, v));
        }
    }
}

/// The value of `key` in `args`.
pub fn arg<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Header arguments in force for `block`: `#+PROPERTY: header-args`, the
/// `:header-args:` properties of the headings above it, `#+HEADER:` lines
/// and the block's own.
pub fn effective_args(lines: &[String], block: &SrcBlock) -> Vec<(String, String)> {
    let lang = normalize_lang(&block.lang);
    let mut args: Vec<(String, String)> = Vec::new();
    let lang_key = format!("header-args:{}", block.lang.to_lowercase());
    let lang_key2 = format!("header-args:{lang}");
    for l in lines {
        let t = l.trim_start();
        if t.len() > 11
            && t.get(..11)
                .is_some_and(|head| head.eq_ignore_ascii_case("#+PROPERTY:"))
        {
            let r = t[11..].trim();
            if let Some(v) = r.strip_prefix("header-args:")
                && let Some((lg, rest)) = v.split_once(char::is_whitespace)
            {
                if lg.eq_ignore_ascii_case(&block.lang) || lg == lang {
                    merge_args(&mut args, parse_header_args(rest));
                }
            } else if let Some(v) = r.strip_prefix("header-args") {
                merge_args(&mut args, parse_header_args(v));
            }
        }
    }
    let mut chain: Vec<usize> = Vec::new();
    let mut level = usize::MAX;
    for i in (0..block.begin.min(lines.len())).rev() {
        if let Some(n) = heading_level(&lines[i])
            && n < level
        {
            chain.push(i);
            level = n;
        }
    }
    chain.reverse();
    for h in chain {
        let mut j = h + 1;
        while j < lines.len()
            && !lines[j].trim().eq_ignore_ascii_case(":END:")
            && (j == h + 1 || lines[j].trim_start().starts_with(':'))
        {
            let t = lines[j].trim();
            let low = t.to_ascii_lowercase();
            if let Some(v) = low.strip_prefix(":header-args:") {
                let _ = v;
                // `:header-args:LANG: args` or `:header-args: args`
                let rest = &t[":header-args".len()..];
                if let Some(r) = rest.strip_prefix(':')
                    && !r.starts_with(char::is_whitespace)
                    && let Some((lg, a)) = r.split_once(':')
                {
                    if lg.eq_ignore_ascii_case(&block.lang) || lg == lang {
                        merge_args(&mut args, parse_header_args(a));
                    }
                } else {
                    merge_args(&mut args, parse_header_args(rest.trim_start_matches(':')));
                }
            } else if low.starts_with(":header-args+:") {
                merge_args(&mut args, parse_header_args(&t[":header-args+:".len()..]));
            }
            j += 1;
        }
    }
    let _ = (lang_key, lang_key2);
    for h in &block.headers {
        merge_args(&mut args, parse_header_args(h));
    }
    merge_args(&mut args, parse_header_args(&block.params));
    args
}

/// Language aliases: "py" -> "python", "sh" -> "sh"...
pub fn normalize_lang(l: &str) -> String {
    match l.to_lowercase().as_str() {
        "py" | "python3" => "python".into(),
        "js" | "node" | "javascript" => "js".into(),
        "bash" | "shell" | "zsh" => "sh".into(),
        "rb" => "ruby".into(),
        "elisp" | "emacs-lisp" | "lisp" => "emacs-lisp".into(),
        "c++" | "cpp" => "cpp".into(),
        "rs" => "rust".into(),
        "golang" => "go".into(),
        o => o.to_string(),
    }
}

/// File extension tangled code of this language gets.
pub fn lang_extension(l: &str) -> &'static str {
    match normalize_lang(l).as_str() {
        "python" => "py",
        "js" => "js",
        "sh" => "sh",
        "ruby" => "rb",
        "perl" => "pl",
        "lua" => "lua",
        "rust" => "rs",
        "go" => "go",
        "c" => "c",
        "cpp" => "cpp",
        "emacs-lisp" => "el",
        "java" => "java",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        "awk" => "awk",
        "r" => "R",
        "haskell" => "hs",
        "makefile" | "make" => "mk",
        "latex" | "tex" => "tex",
        "toml" => "toml",
        "json" => "json",
        "yaml" => "yaml",
        "org" => "org",
        _ => "txt",
    }
}

/// How a language writes a comment.
pub fn comment_prefix(l: &str) -> &'static str {
    match normalize_lang(l).as_str() {
        "js" | "rust" | "go" | "c" | "cpp" | "java" | "css" => "//",
        "lua" | "sql" | "haskell" => "--",
        "emacs-lisp" => ";;",
        "latex" | "tex" => "%",
        _ => "#",
    }
}

// ------------------------------------------------------------- variables

/// A value passed to a block.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(String),
    Str(String),
    Table(Vec<Vec<String>>),
}

/// `x=5`, `s="a b"`, `t=named`: (name, value). A name of a table or block
/// result in `lines` is looked up.
pub fn resolve_var(lines: &[String], spec: &str) -> Option<(String, Value)> {
    let (name, v) = spec.split_once('=')?;
    let name = name.trim().to_string();
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return Some((name, Value::Str(v[1..v.len() - 1].to_string())));
    }
    if v.parse::<f64>().is_ok() {
        return Some((name, Value::Num(v.to_string())));
    }
    if let Some(val) = lookup_named(lines, v) {
        return Some((name, val));
    }
    // Anything else is text.
    Some((name, Value::Str(v.to_string())))
}

/// Like [`resolve_var`], with `hlines` passed to [`lookup_named_with`].
pub fn resolve_var_with(lines: &[String], spec: &str, hlines: bool) -> Option<(String, Value)> {
    let (name, v) = spec.split_once('=')?;
    let name = name.trim().to_string();
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return Some((name, Value::Str(v[1..v.len() - 1].to_string())));
    }
    if v.parse::<f64>().is_ok() {
        return Some((name, Value::Num(v.to_string())));
    }
    if let Some(val) = lookup_named_with(lines, v, hlines) {
        return Some((name, val));
    }
    Some((name, Value::Str(v.to_string())))
}

/// Apply `:colnames` and `:rownames` to a table passed to a block: with
/// `:colnames yes` the first row names the columns and is not data; with
/// `:rownames yes` the first column names the rows and is dropped.
pub fn table_for_babel(v: Value, args: &[(String, String)]) -> Value {
    let truthy = |key: &str| {
        matches!(
            arg(args, key).map(str::trim),
            Some("yes" | "t" | "true" | "1")
        )
    };
    let Value::Table(mut rows) = v else {
        return v;
    };
    if truthy("colnames") && !rows.is_empty() {
        rows.remove(0);
    }
    if truthy("rownames") {
        for r in &mut rows {
            if !r.is_empty() {
                r.remove(0);
            }
        }
    }
    Value::Table(rows)
}

fn parse_table_lines_with(rows: &[String], hlines: bool) -> Vec<Vec<String>> {
    rows.iter()
        .filter(|r| hlines || !r.trim_start().starts_with("|-"))
        .map(|r| {
            let t = r.trim();
            let t = t.strip_prefix('|').unwrap_or(t);
            let t = t.strip_suffix('|').unwrap_or(t);
            t.split('|').map(|c| c.trim().to_string()).collect()
        })
        .collect()
}

/// The value of the named element: a table's cells, the results of a
/// named block, a named example's text. Horizontal rule lines are
/// dropped, as Org does with `:hlines no` (the default).
pub fn lookup_named(lines: &[String], name: &str) -> Option<Value> {
    lookup_named_with(lines, name, false)
}

/// Like [`lookup_named`], but `hlines` keeps horizontal rule lines in a
/// table (`:hlines yes`).
pub fn lookup_named_with(lines: &[String], name: &str, hlines: bool) -> Option<Value> {
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let up = t.to_ascii_uppercase();
        let is_name = up.starts_with("#+NAME:") && t[7..].trim() == name;
        let is_res = up.starts_with("#+RESULTS:") && t[10..].trim() == name;
        if !(is_name || is_res) {
            continue;
        }
        let mut j = i + 1;
        while j < lines.len()
            && lines[j].trim_start().to_ascii_uppercase().starts_with("#+")
            && !lines[j]
                .trim_start()
                .to_ascii_uppercase()
                .starts_with("#+BEGIN")
        {
            j += 1;
        }
        let next = lines.get(j)?;
        let nt = next.trim_start();
        if nt.starts_with('|') {
            let mut k = j;
            while k < lines.len() && lines[k].trim_start().starts_with('|') {
                k += 1;
            }
            return Some(Value::Table(parse_table_lines_with(&lines[j..k], hlines)));
        }
        if nt.starts_with(": ") || nt == ":" {
            let mut k = j;
            let mut out = Vec::new();
            while k < lines.len()
                && (lines[k].trim_start().starts_with(": ") || lines[k].trim() == ":")
            {
                out.push(lines[k].trim_start().get(2..).unwrap_or("").to_string());
                k += 1;
            }
            return Some(value_of_text(&out.join("\n")));
        }
        if is_name && let Some(b) = src_blocks(lines).into_iter().find(|b| b.begin == j) {
            // A named block's value is its result; without one, its body.
            if let Some(r) = results_range(lines, &b) {
                let r_lines = &lines[r.0..r.1];
                if let Some(v) = lookup_in_results_with(r_lines, hlines) {
                    return Some(v);
                }
            }
            return Some(Value::Str(b.body(lines)));
        }
    }
    None
}

fn value_of_text(s: &str) -> Value {
    if s.parse::<f64>().is_ok() {
        Value::Num(s.to_string())
    } else {
        Value::Str(s.to_string())
    }
}

fn lookup_in_results_with(r: &[String], hlines: bool) -> Option<Value> {
    let body: Vec<&String> = r.iter().skip(1).collect();
    let first = body.first()?.trim_start();
    if first.starts_with('|') {
        let rows: Vec<String> = body
            .iter()
            .take_while(|l| l.trim_start().starts_with('|'))
            .map(|l| (*l).clone())
            .collect();
        return Some(Value::Table(parse_table_lines_with(&rows, hlines)));
    }
    if first.starts_with(": ") || first == ":" {
        let t: Vec<String> = body
            .iter()
            .take_while(|l| l.trim_start().starts_with(':'))
            .map(|l| l.trim_start().get(2..).unwrap_or("").to_string())
            .collect();
        return Some(value_of_text(&t.join("\n")));
    }
    None
}

fn q(s: &str, d: char) -> String {
    let esc: String = s
        .chars()
        .flat_map(|c| {
            if c == d || c == '\\' {
                vec!['\\', c]
            } else if c == '\n' {
                vec!['\\', 'n']
            } else {
                vec![c]
            }
        })
        .collect();
    format!("{d}{esc}{d}")
}

/// The statements that bind `name` to `v` in `lang`.
pub fn assign(lang: &str, name: &str, v: &Value) -> String {
    let l = normalize_lang(lang);
    let list = |rows: &Vec<Vec<String>>, open: &str, close: &str, quote: char| {
        let cell = |c: &str| {
            if c.parse::<f64>().is_ok() {
                c.to_string()
            } else {
                q(c, quote)
            }
        };
        let rows: Vec<String> = rows
            .iter()
            .map(|r| {
                format!(
                    "{open}{}{close}",
                    r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        format!("{open}{}{close}", rows.join(", "))
    };
    match (l.as_str(), v) {
        ("python", Value::Num(n)) | ("ruby", Value::Num(n)) | ("lua", Value::Num(n)) => {
            format!("{name} = {n}")
        }
        ("python", Value::Str(s)) | ("ruby", Value::Str(s)) => format!("{name} = {}", q(s, '"')),
        ("python", Value::Table(t)) | ("ruby", Value::Table(t)) => {
            format!("{name} = {}", list(t, "[", "]", '"'))
        }
        ("lua", Value::Str(s)) => format!("{name} = {}", q(s, '"')),
        ("lua", Value::Table(t)) => format!("{name} = {}", list(t, "{", "}", '"')),
        ("js", Value::Num(n)) => format!("const {name} = {n};"),
        ("js", Value::Str(s)) => format!("const {name} = {};", q(s, '"')),
        ("js", Value::Table(t)) => format!("const {name} = {};", list(t, "[", "]", '"')),
        ("perl", Value::Num(n)) => format!("my ${name} = {n};"),
        ("perl", Value::Str(s)) => format!("my ${name} = {};", q(s, '\'')),
        ("perl", Value::Table(t)) => format!("my ${name} = {};", list(t, "[", "]", '\'')),
        ("awk", Value::Num(n)) => format!("{name}={n}"),
        (_, Value::Num(n)) => format!("{name}={n}"),
        (_, Value::Str(s)) => format!("{name}='{}'", s.replace('\'', "'\\''")),
        (_, Value::Table(t)) => {
            // A shell has no tables: tab-separated rows in one string.
            let s = t
                .iter()
                .map(|r| r.join("\t"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{name}='{}'", s.replace('\'', "'\\''"))
        }
    }
}

// ------------------------------------------------------------- results

/// The lines of the results of `block`, `#+RESULTS:` line included: the
/// (start, end) lines, end exclusive.
pub fn results_range(lines: &[String], b: &SrcBlock) -> Option<(usize, usize)> {
    let mut i = b.end + 1;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    let l = lines.get(i)?;
    let up = l.trim_start().to_ascii_uppercase();
    if !up.starts_with("#+RESULTS") {
        return None;
    }
    let start = i;
    let mut j = i + 1;
    let first = lines
        .get(j)
        .map(|s| s.trim_start().to_string())
        .unwrap_or_default();
    if first.starts_with(": ") || first == ":" {
        while j < lines.len() && (lines[j].trim_start().starts_with(": ") || lines[j].trim() == ":")
        {
            j += 1;
        }
    } else if first.starts_with('|') {
        while j < lines.len() && lines[j].trim_start().starts_with('|') {
            j += 1;
        }
    } else if first.starts_with("- ") || first.starts_with("+ ") {
        while j < lines.len()
            && (lines[j].trim_start().starts_with("- ") || lines[j].starts_with("  "))
            && !lines[j].trim().is_empty()
        {
            j += 1;
        }
    } else if first.to_ascii_lowercase().starts_with("#+begin_") {
        let name: String = first[8..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        while j < lines.len()
            && !lines[j]
                .trim()
                .to_ascii_lowercase()
                .starts_with(&format!("#+end_{name}"))
        {
            j += 1;
        }
        j = (j + 1).min(lines.len());
    } else if first.eq_ignore_ascii_case(":RESULTS:") {
        while j < lines.len() && !lines[j].trim().eq_ignore_ascii_case(":END:") {
            j += 1;
        }
        j = (j + 1).min(lines.len());
    } else if first.starts_with("[[") {
        j += 1;
    } else {
        // Raw results run to the next blank line.
        while j < lines.len() && !lines[j].trim().is_empty() {
            j += 1;
        }
    }
    Some((start, j))
}

/// Format `out` as results per the `:results` argument: the lines to put
/// after `#+RESULTS:` (and the name for the keyword).
pub fn format_results(
    out: &str,
    results: &str,
    file: Option<&str>,
    wrap: Option<&str>,
) -> Vec<String> {
    let w: Vec<&str> = results.split_whitespace().collect();
    let has = |x: &str| w.contains(&x);
    let text = out.trim_end_matches('\n');
    let body: Vec<String> = if has("file") {
        match file {
            Some(f) => vec![format!("[[file:{f}]]")],
            None => vec![],
        }
    } else if has("raw") || has("org") && false {
        text.lines().map(String::from).collect()
    } else if has("table") || has("vector") {
        let rows: Vec<Vec<String>> = text
            .lines()
            .map(|l| {
                if l.contains('\t') {
                    l.split('\t').map(|c| c.trim().to_string()).collect()
                } else {
                    vec![l.trim().to_string()]
                }
            })
            .collect();
        table_lines(&rows)
    } else if has("list") {
        text.lines().map(|l| format!("- {l}")).collect()
    } else if has("html") {
        let mut v = vec!["#+BEGIN_EXPORT html".to_string()];
        v.extend(text.lines().map(String::from));
        v.push("#+END_EXPORT".into());
        v
    } else if has("latex") {
        let mut v = vec!["#+BEGIN_EXPORT latex".to_string()];
        v.extend(text.lines().map(String::from));
        v.push("#+END_EXPORT".into());
        v
    } else if has("org") {
        let mut v = vec!["#+BEGIN_SRC org".to_string()];
        v.extend(text.lines().map(String::from));
        v.push("#+END_SRC".into());
        v
    } else if has("code") {
        let mut v = vec!["#+BEGIN_SRC".to_string()];
        v.extend(text.lines().map(String::from));
        v.push("#+END_SRC".into());
        v
    } else if has("drawer") {
        let mut v = vec![":RESULTS:".to_string()];
        v.extend(text.lines().map(|l| format!(": {l}")));
        v.push(":END:".into());
        v
    } else if text.is_empty() {
        vec![]
    } else {
        text.lines()
            .map(|l| {
                if l.is_empty() {
                    ":".to_string()
                } else {
                    format!(": {l}")
                }
            })
            .collect()
    };
    match wrap {
        Some(wr) => {
            let (kind, rest) = wr
                .split_once(char::is_whitespace)
                .map(|(a, b)| (a, b.trim()))
                .unwrap_or((wr, ""));
            let kind = kind.to_ascii_uppercase();
            let head = if rest.is_empty() {
                format!("#+BEGIN_{kind}")
            } else {
                format!("#+BEGIN_{kind} {rest}")
            };
            let mut v = vec![head];
            v.extend(text.lines().map(String::from));
            v.push(format!("#+END_{kind}"));
            v
        }
        None => body,
    }
}

/// Rows as an aligned Org table.
pub fn table_lines(rows: &[Vec<String>]) -> Vec<String> {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut w = vec![1usize; cols];
    for r in rows {
        for (k, c) in r.iter().enumerate() {
            w[k] = w[k].max(c.chars().count());
        }
    }
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = (0..cols)
                .map(|k| {
                    format!(
                        "{:<width$}",
                        r.get(k).map(String::as_str).unwrap_or(""),
                        width = w[k]
                    )
                })
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect()
}

// -------------------------------------------------------------- noweb

/// The name in a line that is nothing but a `<<name>>` reference.
fn whole_line_ref(l: &str) -> Option<&str> {
    let inner = l.trim().strip_prefix("<<")?.strip_suffix(">>")?;
    if inner.is_empty() || inner.contains("<<") {
        return None;
    }
    Some(inner.split('(').next().unwrap_or("").trim())
}

/// The state of one noweb expansion: the blocks being expanded now, and
/// the ones that must not be expanded again.
#[derive(Default)]
struct Noweb {
    /// Names whose expansion is in progress.
    active: Vec<String>,
    /// Names that must not be expanded again: the ones that turned out to
    /// refer to themselves, and the ones whose expansion was cut short by
    /// the depth limit.
    refused: BTreeSet<String>,
}

impl Noweb {
    /// May `name` be expanded? A name that is being expanded already
    /// refers to itself, so expanding it again would never end.
    fn expandable(&mut self, name: &str) -> bool {
        if self.refused.contains(name) {
            return false;
        }
        if self.active.iter().any(|n| n == name) {
            self.refused.insert(name.to_string());
            return false;
        }
        true
    }

    /// Replace `<<name>>` by the body of the block called `name`.
    fn expand(&mut self, lines: &[String], body: &str, depth: usize) -> String {
        if depth > 8 || !body.contains("<<") {
            return body.to_string();
        }
        let blocks = src_blocks(lines);
        let mut out = Vec::new();
        for l in body.lines() {
            if let Some(name) = whole_line_ref(l)
                && let Some(b) = blocks.iter().find(|b| b.name.as_deref() == Some(name))
                && self.expandable(name)
            {
                let ind = &l[..l.len() - l.trim_start().len()];
                self.active.push(name.to_string());
                let inner = self.expand(lines, &b.body(lines), depth + 1);
                self.active.pop();
                for il in inner.lines() {
                    out.push(format!("{ind}{il}"));
                }
                continue;
            }
            let mut line = l.to_string();
            while let Some(a) = line.find("<<") {
                let Some(e) = line[a..].find(">>") else { break };
                let name = line[a + 2..a + e]
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let Some(b) = blocks
                    .iter()
                    .find(|b| b.name.as_deref() == Some(name.as_str()))
                else {
                    break;
                };
                if !self.expandable(&name) {
                    // A block that refers to itself: leave the reference
                    // as it is.
                    break;
                }
                let body = b.body(lines);
                // Cut short by the depth limit: expanding it again would
                // only repeat the same cut.
                let cut = depth + 1 > 8 && body.contains("<<");
                self.active.push(name.clone());
                let inner = self.expand(lines, &body, depth + 1);
                self.active.pop();
                if cut {
                    self.refused.insert(name.clone());
                }
                line = format!(
                    "{}{}{}",
                    &line[..a],
                    inner.trim_end_matches('\n'),
                    &line[a + e + 2..]
                );
            }
            out.push(line);
        }
        out.join("\n")
    }
}

/// Replace `<<name>>` by the body of the block called `name` (several
/// levels deep, cycles cut). A block that refers to itself is left
/// alone: its reference stays in the text.
pub fn expand_noweb(lines: &[String], body: &str, depth: usize) -> String {
    Noweb::default().expand(lines, body, depth)
}

// ------------------------------------------------------------- tangle

/// A file a tangle produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tangled {
    pub path: String,
    pub text: String,
    pub shebang: Option<String>,
    pub mkdirp: bool,
    pub blocks: usize,
}

/// The tangle of `lines`. `source` is the Org file's name, for link
/// comments; `default_file` is the stem used for `:tangle yes`.
pub fn tangle(lines: &[String], source: &str, default_file: &str) -> Vec<Tangled> {
    let mut out: Vec<Tangled> = Vec::new();
    for b in src_blocks(lines) {
        let args = effective_args(lines, &b);
        let target = match arg(&args, "tangle") {
            None | Some("no") | Some("") => continue,
            Some("yes") => format!("{default_file}.{}", lang_extension(&b.lang)),
            Some(f) => f.trim_matches('"').to_string(),
        };
        let noweb = matches!(
            arg(&args, "noweb"),
            Some("yes") | Some("tangle") | Some("strip-export")
        );
        let mut body = b.body(lines);
        if noweb {
            body = expand_noweb(lines, &body, 0);
        }
        // `:prologue` / `:epilogue`.
        if let Some(p) = arg(&args, "prologue") {
            body = format!("{}\n{body}", p.trim_matches('"'));
        }
        if let Some(e) = arg(&args, "epilogue") {
            body = format!("{body}\n{}", e.trim_matches('"'));
        }
        let comments = arg(&args, "comments").unwrap_or("no").to_string();
        let cp = comment_prefix(&b.lang);
        let mut chunk = String::new();
        if comments == "link" || comments == "yes" || comments == "org" {
            let title = (0..b.begin)
                .rev()
                .find_map(|i| {
                    heading_level(&lines[i])
                        .map(|_| lines[i].trim_start_matches('*').trim().to_string())
                })
                .unwrap_or_default();
            chunk.push_str(&format!(
                "{cp} [[file:{source}::{}][{}:{}]]\n",
                b.name
                    .clone()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("*{title}")),
                if title.is_empty() {
                    "block".to_string()
                } else {
                    title.clone()
                },
                b.begin + 1
            ));
        }
        chunk.push_str(&body);
        chunk.push('\n');
        let shebang = arg(&args, "shebang").map(|s| s.trim_matches('"').to_string());
        let mkdirp = matches!(arg(&args, "mkdirp"), Some("yes"));
        match out.iter_mut().find(|t| t.path == target) {
            Some(t) => {
                t.text.push('\n');
                t.text.push_str(&chunk);
                t.blocks += 1;
                if t.shebang.is_none() {
                    t.shebang = shebang;
                }
            }
            None => out.push(Tangled {
                path: target,
                text: chunk,
                shebang,
                mkdirp,
                blocks: 1,
            }),
        }
    }
    for t in &mut out {
        if let Some(s) = &t.shebang {
            t.text = format!("{s}\n{}", t.text);
        }
    }
    out
}

// ------------------------------------------------------- highlighting

thread_local! {
    static CACHE: RefCell<HashMap<(String, String), Vec<Option<SyntaxFace>>>> = RefCell::new(HashMap::new());
}

/// The built-in grammar for an Org source block's language.
pub fn grammar_for(lang: &str) -> Option<&'static LanguageDef> {
    let name = match normalize_lang(lang).as_str() {
        "python" => "python",
        "js" => "javascript",
        "sh" => "bash",
        "rust" => "rust",
        "c" => "c",
        "go" => "go",
        "json" => "json",
        "toml" => "toml",
        "lua" => "lua",
        _ => return None,
    };
    syntax::language(name)
}

/// Highlight `code` as `lang`, one face per char.
pub fn code_faces(lang: &str, code: &str) -> Vec<Option<SyntaxFace>> {
    let n = code.chars().count();
    let Some(def) = grammar_for(lang) else {
        return vec![None; n];
    };
    let key = (def.name.to_string(), code.to_string());
    if let Some(f) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return f;
    }
    let faces = match Syntax::new(def) {
        Some(mut s) => {
            let rope = Rope::from_str(code);
            s.faces(&rope, 0..n)
        }
        None => vec![None; n],
    };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, faces.clone());
    });
    faces
}

impl Buffer {
    /// Org highlighting that knows about blocks: the code in a source block
    /// has its language's colours, example and export blocks have none.
    pub(crate) fn org_faces_blocks(&self, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        let kw = crate::org::keywords(self);
        let first = self.offset_to_pos(range.start).line;
        let last = self.offset_to_pos(range.end).line;
        // State at `first`: look back for the last block marker.
        let mut state: Option<(String, String, usize)> = None; // (kind, lang, begin line)
        let lowest = first.saturating_sub(4000);
        for l in (lowest..first).rev() {
            let t = self.line(l);
            let low = t.trim_start().to_ascii_lowercase();
            if low.starts_with("#+end_") {
                break;
            }
            if let Some(r) = low.strip_prefix("#+begin_") {
                let kind: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
                let lang = if kind == "src" {
                    t.trim_start()[11..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                } else {
                    String::new()
                };
                state = Some((kind, lang, l));
                break;
            }
        }
        let mut out = Vec::with_capacity(range.len());
        let mut src_cache: Option<(usize, Vec<Option<SyntaxFace>>, Vec<usize>)> = None;
        for l in first..=last {
            let text = self.line(l);
            let low = text.trim_start().to_ascii_lowercase();
            let n = text.chars().count();
            let mut f: Vec<Option<SyntaxFace>>;
            if let Some(r) = low.strip_prefix("#+begin_") {
                let kind: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
                let lang = if kind == "src" {
                    text.trim_start()[11..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                } else {
                    String::new()
                };
                state = Some((kind, lang, l));
                f = crate::org::line_faces(&text, &kw);
            } else if low.starts_with("#+end_") && state.is_some() {
                state = None;
                f = crate::org::line_faces(&text, &kw);
            } else if let Some((kind, lang, begin)) = &state {
                match kind.as_str() {
                    "src" => {
                        if src_cache.as_ref().map(|c| c.0) != Some(*begin) {
                            let mut end = *begin + 1;
                            let total = self.line_count();
                            while end < total && !is_end_src(&self.line(end)) {
                                end += 1;
                            }
                            let mut code = String::new();
                            let mut starts = Vec::new();
                            let mut pos = 0;
                            for i in *begin + 1..end {
                                starts.push(pos);
                                let ln = self.line(i);
                                pos += ln.chars().count() + 1;
                                code.push_str(&ln);
                                code.push('\n');
                            }
                            src_cache = Some((*begin, code_faces(lang, &code), starts));
                        }
                        let (_, faces, starts) = src_cache.as_ref().unwrap();
                        let k = l - (*begin + 1);
                        let s = starts.get(k).copied().unwrap_or(0);
                        f = (0..n)
                            .map(|i| faces.get(s + i).copied().flatten())
                            .collect();
                    }
                    "example" | "export" => f = vec![None; n],
                    "comment" => f = vec![Some(SyntaxFace::Comment); n],
                    _ => f = crate::org::line_faces(&text, &kw),
                }
            } else {
                f = crate::org::line_faces(&text, &kw);
            }
            f.resize(n, None);
            f.push(None); // the newline
            let start = self.pos_to_offset(crate::Pos::new(l, 0));
            for (i, face) in f.into_iter().enumerate() {
                if (range.start..range.end).contains(&(start + i)) {
                    out.push(face);
                }
            }
        }
        out.resize(range.len(), None);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls(t: &str) -> Vec<String> {
        t.lines().map(String::from).collect()
    }

    const DOC: &str = "#+PROPERTY: header-args :results output\n* H\n:PROPERTIES:\n:header-args:python: :session s\n:END:\n#+NAME: first\n#+HEADER: :var x=2\n#+BEGIN_SRC python :results value -n\nprint(x)\n#+END_SRC\n\n#+RESULTS: first\n: 2\n\ntext\n#+BEGIN_SRC sh\n  echo hi\n,* escaped\n#+END_SRC\n";

    #[test]
    fn finds_blocks_with_names_and_headers() {
        let l = ls(DOC);
        let b = src_blocks(&l);
        assert_eq!(b.len(), 2);
        assert_eq!(
            (b[0].lang.as_str(), b[0].name.as_deref()),
            ("python", Some("first"))
        );
        assert_eq!((b[0].first, b[0].begin, b[0].end), (5, 7, 9));
        assert_eq!(b[0].headers, vec![":var x=2"]);
        assert_eq!(b[0].params, ":results value -n");
        assert_eq!(b[1].body(&l), "  echo hi\n* escaped");
        assert_eq!(block_at(&l, 6).unwrap().begin, 7);
        assert!(block_at(&l, 3).is_none());
    }

    #[test]
    fn header_args_merge_by_level() {
        let l = ls(DOC);
        let b = src_blocks(&l);
        let a = effective_args(&l, &b[0]);
        assert_eq!(
            arg(&a, "results"),
            Some("value -n")
                .map(|_| "value -n")
                .or(Some("value"))
                .filter(|_| false)
                .or(arg(&a, "results"))
        );
        assert_eq!(arg(&a, "results"), Some("value -n"));
        assert_eq!(arg(&a, "session"), Some("s"));
        assert_eq!(arg(&a, "var"), Some("x=2"));
        let a2 = effective_args(&l, &b[1]);
        assert_eq!(arg(&a2, "results"), Some("output"));
        assert_eq!(arg(&a2, "session"), None);
    }

    #[test]
    fn parses_header_args() {
        let a = parse_header_args(r#"-n :var a=1 :var b="x y" :results output replace :dir /tmp"#);
        assert_eq!(
            a,
            vec![
                ("var".into(), "a=1".into()),
                ("var".into(), "b=\"x y\"".into()),
                ("results".into(), "output replace".into()),
                ("dir".into(), "/tmp".into())
            ]
        );
        let mut base = a.clone();
        merge_args(&mut base, parse_header_args(":var a=9 :results table"));
        assert_eq!(arg(&base, "results"), Some("table"));
        assert_eq!(base.iter().filter(|(k, _)| k == "var").count(), 2);
        assert!(base.contains(&("var".into(), "a=9".into())));
    }

    #[test]
    fn variables_resolve_and_assign() {
        let l = ls(
            "#+NAME: tbl\n| a | 1 |\n| b | 2 |\n\n#+NAME: txt\n#+BEGIN_SRC sh\necho x\n#+END_SRC\n\n#+RESULTS: txt\n: 42\n",
        );
        assert_eq!(
            resolve_var(&l, "n=5"),
            Some(("n".into(), Value::Num("5".into())))
        );
        assert_eq!(
            resolve_var(&l, "s=\"a b\""),
            Some(("s".into(), Value::Str("a b".into())))
        );
        assert_eq!(
            resolve_var(&l, "t=tbl"),
            Some((
                "t".into(),
                Value::Table(vec![
                    vec!["a".into(), "1".into()],
                    vec!["b".into(), "2".into()]
                ])
            ))
        );
        assert_eq!(
            resolve_var(&l, "r=txt"),
            Some(("r".into(), Value::Num("42".into())))
        );
        assert_eq!(
            assign(
                "python",
                "t",
                &Value::Table(vec![vec!["a".into(), "1".into()]])
            ),
            "t = [[\"a\", 1]]"
        );
        assert_eq!(
            assign("sh", "s", &Value::Str("it's".into())),
            "s='it'\\''s'"
        );
        assert_eq!(assign("js", "n", &Value::Num("3".into())), "const n = 3;");
    }

    #[test]
    fn babel_table_headers_and_hline_rules() {
        let l = ls(
            "#+NAME: tbl\n| name | n |\n| a    | 1 |\n| b    | 2 |\n\n#+NAME: h\n| a | 1 |\n|---+---|\n| b | 2 |\n",
        );
        let (_, v) = resolve_var_with(&l, "t=tbl", false).unwrap();
        let rows: Vec<Vec<String>> = match table_for_babel(v, &[]) {
            Value::Table(r) => r,
            _ => panic!("not a table"),
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["name", "n"]);
        let (_, v) = resolve_var_with(&l, "t=tbl", false).unwrap();
        let args = vec![
            ("colnames".into(), "yes".into()),
            ("rownames".into(), "yes".into()),
        ];
        let rows: Vec<Vec<String>> = match table_for_babel(v, &args) {
            Value::Table(r) => r,
            _ => panic!("not a table"),
        };
        assert_eq!(rows, vec![vec!["1".to_string()], vec!["2".to_string()]]);
        let (_, v) = resolve_var_with(&l, "h=h", false).unwrap();
        let Value::Table(rows) = v else {
            panic!("not a table")
        };
        assert_eq!(rows.len(), 2);
        let (_, v) = resolve_var_with(&l, "h=h", true).unwrap();
        let Value::Table(rows) = v else {
            panic!("not a table")
        };
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn finds_and_formats_results() {
        let l = ls(DOC);
        let b = src_blocks(&l);
        assert_eq!(results_range(&l, &b[0]), Some((11, 13)));
        assert_eq!(results_range(&l, &b[1]), None);
        assert_eq!(
            format_results("a\nb\n", "output", None, None),
            vec![": a", ": b"]
        );
        assert_eq!(
            format_results("a\tb\n1\t2", "table", None, None),
            vec!["| a | b |", "| 1 | 2 |"]
        );
        assert_eq!(
            format_results("x\ny", "list", None, None),
            vec!["- x", "- y"]
        );
        assert_eq!(
            format_results("", "file", Some("o.png"), None),
            vec!["[[file:o.png]]"]
        );
        assert_eq!(
            format_results("<b>", "html", None, None),
            vec!["#+BEGIN_EXPORT html", "<b>", "#+END_EXPORT"]
        );
        assert_eq!(
            format_results("t", "output", None, Some("example")),
            vec!["#+BEGIN_EXAMPLE", "t", "#+END_EXAMPLE"]
        );
        assert_eq!(
            format_results("", "output", None, None),
            Vec::<String>::new()
        );
        assert_eq!(
            format_results("a\n\nb", "output", None, None),
            vec![": a", ":", ": b"]
        );
    }

    #[test]
    fn results_extents_by_kind() {
        let l = ls("#+BEGIN_SRC sh\nx\n#+END_SRC\n#+RESULTS:\n| a |\n| b |\nafter\n");
        let b = &src_blocks(&l)[0];
        assert_eq!(results_range(&l, b), Some((3, 6)));
        let l = ls(
            "#+BEGIN_SRC sh\nx\n#+END_SRC\n\n#+RESULTS:\n#+BEGIN_EXAMPLE\nq\n#+END_EXAMPLE\nafter\n",
        );
        assert_eq!(results_range(&l, &src_blocks(&l)[0]), Some((4, 8)));
    }

    #[test]
    fn noweb_and_tangle() {
        let l = ls(
            "#+NAME: hello\n#+BEGIN_SRC python\nprint('hi')\n#+END_SRC\n\n#+BEGIN_SRC python :tangle out.py :noweb yes :shebang \"#!/usr/bin/python\"\ndef f():\n    <<hello>>\n#+END_SRC\n\n#+BEGIN_SRC sh :tangle out.py\necho more\n#+END_SRC\n\n#+BEGIN_SRC sh\nnot tangled\n#+END_SRC\n",
        );
        let t = tangle(&l, "doc.org", "doc");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].path, "out.py");
        assert_eq!(t[0].blocks, 2);
        assert!(
            t[0].text
                .starts_with("#!/usr/bin/python\ndef f():\n    print('hi')\n\necho more\n"),
            "{}",
            t[0].text
        );
        assert!(!t[0].text.contains("not tangled"));
        let l2 = ls("#+BEGIN_SRC rust :tangle yes :comments link\nfn main() {}\n#+END_SRC\n");
        let t2 = tangle(&l2, "x.org", "x");
        assert_eq!(t2[0].path, "x.rs");
        assert!(
            t2[0].text.starts_with("// [[file:x.org::*"),
            "{}",
            t2[0].text
        );
    }

    #[test]
    fn multibyte_block_and_property_lines_are_not_panics() {
        // The prefix ends inside a multi-byte character.
        let l = ls("#+begin_srç\nx\n#+end_src\n");
        assert!(src_blocks(&l).is_empty());
        let l = ls("#+PROPERTé\n#+BEGIN_SRC sh\nx\n#+END_SRC\n");
        let b = &src_blocks(&l)[0];
        assert!(effective_args(&l, b).is_empty());
    }

    #[test]
    fn noweb_self_reference_keeps_the_reference() {
        let l = ls("#+NAME: A\n#+BEGIN_SRC sh\nx = <<A>> + 1\n#+END_SRC\n");
        let out = expand_noweb(&l, "x = <<A>> + 1", 0);
        assert!(out.contains("<<A>>"), "{out}");
    }

    #[test]
    fn noweb_cycle_keeps_the_reference() {
        let l = ls(
            "#+NAME: A\n#+BEGIN_SRC sh\n<<B>>\n#+END_SRC\n#+NAME: B\n#+BEGIN_SRC sh\n<<A>>\n#+END_SRC\n",
        );
        let out = expand_noweb(&l, "<<A>>", 0);
        assert!(out.contains("<<A>>"), "{out}");
    }

    #[test]
    fn highlights_code_in_blocks() {
        let mut b = Buffer::new("t.org");
        b.set_contents("text\n#+BEGIN_SRC python\ndef f(): return 1\n#+END_SRC\n#+BEGIN_EXAMPLE\n*not bold*\n#+END_EXAMPLE\n=v= after\n");
        b.set_major_mode(crate::org::MODE);
        let all = b.char_count();
        let f = b.syntax_faces(0..all);
        let off = |line: usize, col: usize| b.pos_to_offset(crate::Pos::new(line, col));
        assert_eq!(f[off(2, 0)], Some(SyntaxFace::Keyword), "def keyword");
        assert_eq!(f[off(2, 9)], Some(SyntaxFace::Keyword), "return keyword");
        assert_eq!(f[off(1, 2)], Some(SyntaxFace::Comment));
        assert_eq!(f[off(5, 1)], None, "example block is plain");
        assert_eq!(f[off(7, 1)], Some(SyntaxFace::String), "outside again");
        let part = b.syntax_faces(off(2, 0)..off(2, 3));
        assert_eq!(part[0], Some(SyntaxFace::Keyword));
    }
}
