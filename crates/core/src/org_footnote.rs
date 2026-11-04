// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org footnotes: finding references and definitions, and the edits that
//! renumber, sort, add and delete them (on the buffer's text).

use crate::org::heading_level;

/// A reference: `[fn:1]`, `[fn:name:inline text]` or `[fn::anonymous]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnRef {
    /// "" for an anonymous inline footnote.
    pub label: String,
    pub line: usize,
    /// Char columns, end exclusive.
    pub start: usize,
    pub end: usize,
    /// The definition inside the reference, if it has one.
    pub inline: Option<String>,
}

/// A definition: `[fn:1] text...` at the start of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDef {
    pub label: String,
    pub line: usize,
    /// The line after the last one of the definition.
    pub end: usize,
}

fn is_label(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Lines that are code and the like, where footnotes don't count.
fn verbatim_lines(lines: &[String]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    let mut inside: Option<String> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start().to_ascii_lowercase();
        if let Some(name) = &inside {
            out[i] = true;
            if t.starts_with(&format!("#+end_{name}")) {
                inside = None;
            }
            continue;
        }
        if let Some(r) = t.strip_prefix("#+begin_") {
            let name: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
            if matches!(name.as_str(), "src" | "example" | "export" | "comment") {
                out[i] = true;
                inside = Some(name);
            }
        } else if t.starts_with("# ") || t == "#" || t.starts_with("#+") {
            out[i] = true;
        }
    }
    out
}

/// The balanced `[...]` starting at `s[0]`: its byte length.
fn bracket_len(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every reference and definition, in buffer order.
pub fn scan(lines: &[String]) -> (Vec<FnRef>, Vec<FnDef>) {
    let skip = verbatim_lines(lines);
    let mut refs = Vec::new();
    let mut defs: Vec<FnDef> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if skip[i] {
            i += 1;
            continue;
        }
        // A definition begins at column 0.
        if let Some(r) = l.strip_prefix("[fn:")
            && let Some(e) = r.find(']')
            && is_label(&r[..e])
            && !r[..e].is_empty()
        {
            let label = r[..e].to_string();
            let mut j = i + 1;
            let mut blanks = 0;
            while j < lines.len() {
                let n = &lines[j];
                if heading_level(n).is_some() || n.starts_with("[fn:") {
                    break;
                }
                if n.trim().is_empty() {
                    blanks += 1;
                    if blanks >= 2 {
                        break;
                    }
                } else {
                    blanks = 0;
                }
                j += 1;
            }
            // Trailing blank lines aren't part of it.
            let mut end = j;
            while end > i + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            defs.push(FnDef {
                label,
                line: i,
                end,
            });
            // References inside the definition's own text still count.
            for k in i..end {
                if !skip[k] {
                    let start_byte = if k == i { 4 + e + 1 } else { 0 };
                    refs_in(&lines[k], k, start_byte, &mut refs);
                }
            }
            i = j;
            continue;
        }
        if heading_level(l).is_some() {
            // Footnotes in headings count too.
        }
        refs_in(l, i, 0, &mut refs);
        i += 1;
    }
    (refs, defs)
}

fn refs_in(line: &str, no: usize, from: usize, out: &mut Vec<FnRef>) {
    let mut i = from;
    while let Some(p) = line[i..].find("[fn:") {
        let at = i + p;
        match bracket_len(&line[at..]) {
            Some(len) => {
                let inner = &line[at + 4..at + len - 1];
                let (label, inline) = match inner.split_once(':') {
                    Some((l, d)) => (l, Some(d.to_string())),
                    None => (inner, None),
                };
                if is_label(label) && (!label.is_empty() || inline.is_some()) {
                    out.push(FnRef {
                        label: label.to_string(),
                        line: no,
                        start: line[..at].chars().count(),
                        end: line[..at + len].chars().count(),
                        inline,
                    });
                }
                i = at + len;
            }
            None => break,
        }
    }
}

/// The reference at (line, col), if point is in one.
pub fn ref_at(refs: &[FnRef], line: usize, col: usize) -> Option<&FnRef> {
    refs.iter()
        .find(|r| r.line == line && (r.start..r.end).contains(&col))
}

/// The definition containing `line`.
pub fn def_at(defs: &[FnDef], line: usize) -> Option<&FnDef> {
    defs.iter()
        .find(|d| (d.line..d.end.max(d.line + 1)).contains(&line))
}

/// The smallest positive number no label uses.
pub fn next_label(refs: &[FnRef], defs: &[FnDef]) -> String {
    let used: Vec<usize> = refs
        .iter()
        .map(|r| r.label.as_str())
        .chain(defs.iter().map(|d| d.label.as_str()))
        .filter_map(|l| l.parse().ok())
        .collect();
    (1..).find(|n| !used.contains(n)).unwrap_or(1).to_string()
}

fn join(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

fn def_text(lines: &[String], d: &FnDef) -> Vec<String> {
    let mut v: Vec<String> = lines[d.line..d.end].to_vec();
    let first = &v[0];
    let after = first
        .find(']')
        .map(|e| first[e + 1..].trim_start().to_string())
        .unwrap_or_default();
    v[0] = after;
    v
}

/// Replace references by `map(label, inline)` in `line`, leaving the rest.
fn rewrite_refs(line: &str, refs: &[&FnRef], f: &mut dyn FnMut(&FnRef) -> String) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for r in refs {
        out.extend(&chars[at..r.start]);
        out.push_str(&f(r));
        at = r.end;
    }
    out.extend(&chars[at..]);
    out
}

/// `org-footnote-normalize`: number references in order of appearance,
/// turn inline definitions into definitions, and gather the definitions
/// in a `section` at the end of the buffer.
pub fn normalize(text: &str, section: &str) -> String {
    let lines = lines_of(text);
    let (refs, defs) = scan(&lines);
    if refs.is_empty() && defs.is_empty() {
        return text.to_string();
    }
    let mut order: Vec<String> = Vec::new(); // old label, "" + index for anonymous
    let number_of = |r: &FnRef, order: &mut Vec<String>| -> usize {
        let key = if r.label.is_empty() {
            format!("\u{0}{}:{}", r.line, r.start)
        } else {
            r.label.clone()
        };
        match order.iter().position(|k| *k == key) {
            Some(p) => p + 1,
            None => {
                order.push(key);
                order.len()
            }
        }
    };
    let mut new_defs: Vec<(usize, Vec<String>)> = Vec::new();
    let mut rewritten = lines.clone();
    let def_line: std::collections::HashSet<usize> =
        defs.iter().flat_map(|d| d.line..d.end).collect();
    let mut by_line: std::collections::BTreeMap<usize, Vec<&FnRef>> = Default::default();
    for r in &refs {
        by_line.entry(r.line).or_default().push(r);
    }
    for (ln, rs) in &by_line {
        if def_line.contains(ln) && !lines[*ln].starts_with("[fn:") && false {
            continue;
        }
        let mut f = |r: &FnRef| -> String {
            let n = number_of(r, &mut order);
            if let Some(d) = &r.inline
                && !new_defs.iter().any(|(k, _)| *k == n)
            {
                new_defs.push((n, vec![d.clone()]));
            }
            format!("[fn:{n}]")
        };
        rewritten[*ln] = rewrite_refs(&lines[*ln], rs, &mut f);
    }
    for d in &defs {
        let n = order.iter().position(|k| *k == d.label).map(|p| p + 1);
        let body = def_text(&lines, d);
        match n {
            Some(n) => {
                if !new_defs.iter().any(|(k, _)| *k == n) {
                    new_defs.push((n, body));
                }
            }
            None => {
                // Defined but never referenced: keep it, numbered after the rest.
                order.push(d.label.clone());
                new_defs.push((order.len(), body));
            }
        }
    }
    new_defs.sort_by_key(|(n, _)| *n);
    let mut kept: Vec<String> = Vec::new();
    for (i, l) in rewritten.iter().enumerate() {
        if !defs.iter().any(|d| (d.line..d.end).contains(&i)) {
            kept.push(l.clone());
        }
    }
    // An empty footnote section is left behind: remove it.
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    if let Some(last_heading) = kept.iter().rposition(|l| heading_level(l).is_some()) {
        let title = kept[last_heading].trim_start_matches('*').trim();
        if title == section && kept[last_heading + 1..].iter().all(|l| l.trim().is_empty()) {
            kept.truncate(last_heading);
            while kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
        }
    }
    kept.push(String::new());
    kept.push(format!("* {section}"));
    for (n, mut body) in new_defs {
        let first = body.remove(0);
        kept.push(format!("[fn:{n}] {first}").trim_end().to_string());
        kept.extend(body);
        kept.push(String::new());
    }
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    join(&kept)
}

/// `org-footnote-sort`: definitions in the order the buffer first refers
/// to them (unreferenced ones last).
pub fn sort_defs(text: &str) -> String {
    let lines = lines_of(text);
    let (refs, defs) = scan(&lines);
    if defs.len() < 2 {
        return text.to_string();
    }
    let mut keyed: Vec<(usize, &FnDef)> = defs
        .iter()
        .map(|d| {
            (
                refs.iter()
                    .position(|r| r.label == d.label)
                    .unwrap_or(usize::MAX),
                d,
            )
        })
        .collect();
    keyed.sort_by_key(|(k, _)| *k);
    let first = defs[0].line;
    let mut blocks: Vec<Vec<String>> = keyed
        .iter()
        .map(|(_, d)| lines[d.line..d.end].to_vec())
        .collect();
    // Keep a blank line between blocks.
    for b in &mut blocks {
        while b.last().is_some_and(|l| l.trim().is_empty()) {
            b.pop();
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut inserted = false;
    for (i, l) in lines.iter().enumerate() {
        if defs.iter().any(|d| (d.line..d.end).contains(&i)) {
            if !inserted && i == first {
                for (k, b) in blocks.iter().enumerate() {
                    out.extend(b.iter().cloned());
                    if k + 1 < blocks.len() {
                        out.push(String::new());
                    }
                }
                inserted = true;
            }
            continue;
        }
        // Blank lines between the old blocks vanish with them.
        if inserted
            && l.trim().is_empty()
            && defs.iter().any(|d| d.end <= i && i < d.end + 2)
            && out.last().is_some_and(|p| p.trim().is_empty())
        {
            continue;
        }
        out.push(l.clone());
    }
    join(&out)
}

/// Remove the references and the definition of `label`.
pub fn delete(text: &str, label: &str) -> String {
    let lines = lines_of(text);
    let (refs, defs) = scan(&lines);
    let mut out = lines.clone();
    let mut by_line: std::collections::BTreeMap<usize, Vec<&FnRef>> = Default::default();
    for r in refs.iter().filter(|r| r.label == label) {
        by_line.entry(r.line).or_default().push(r);
    }
    for (ln, rs) in by_line {
        out[ln] = rewrite_refs(&lines[ln], &rs, &mut |_| String::new());
    }
    let mut res: Vec<String> = Vec::new();
    for (i, l) in out.iter().enumerate() {
        if defs
            .iter()
            .any(|d| d.label == label && (d.line..d.end).contains(&i))
        {
            continue;
        }
        res.push(l.clone());
    }
    // Two blank lines left where the definition was become one.
    let mut cleaned: Vec<String> = Vec::new();
    for l in res {
        if l.trim().is_empty() && cleaned.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        cleaned.push(l);
    }
    join(&cleaned)
}

/// Add the empty definition `[fn:label] ` in the footnote `section` (made
/// at the end if there is none): the new text and the line of the new
/// definition.
pub fn add_definition(text: &str, label: &str, section: &str) -> (String, usize) {
    let mut lines = lines_of(text);
    let (_, defs) = scan(&lines);
    let sec_line = lines
        .iter()
        .rposition(|l| heading_level(l).is_some() && l.trim_start_matches('*').trim() == section);
    let insert_at = match (defs.last(), sec_line) {
        (Some(d), _) => d.end,
        (None, Some(s)) => {
            let mut e = lines.len();
            while e > s + 1 && lines[e - 1].trim().is_empty() {
                e -= 1;
            }
            e
        }
        (None, None) => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            lines.push(String::new());
            lines.push(format!("* {section}"));
            lines.len()
        }
    };
    let mut add = Vec::new();
    if defs.last().is_some() || sec_line.is_some() {
        add.push(String::new());
    }
    let at = insert_at + add.len();
    add.push(format!("[fn:{label}] "));
    lines.splice(insert_at..insert_at, add);
    (join(&lines), at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls(t: &str) -> Vec<String> {
        lines_of(t)
    }

    #[test]
    fn finds_references_and_definitions() {
        let l = ls(
            "Text[fn:1] more [fn:name:inline def] and [fn::anon].\n\n[fn:1] The def\nspanning lines.\n\n\nAfter.\n#+BEGIN_SRC c\n[fn:9]\n#+END_SRC\n",
        );
        let (refs, defs) = scan(&l);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].label, "1");
        assert_eq!((refs[0].start, refs[0].end), (4, 10));
        assert_eq!(refs[1].label, "name");
        assert_eq!(refs[1].inline.as_deref(), Some("inline def"));
        assert_eq!(refs[2].label, "");
        assert_eq!(defs.len(), 1);
        assert_eq!((defs[0].line, defs[0].end), (2, 4));
        assert_eq!(ref_at(&refs, 0, 5).map(|r| r.label.as_str()), Some("1"));
        assert!(ref_at(&refs, 0, 0).is_none());
        assert_eq!(def_at(&defs, 3).map(|d| d.label.as_str()), Some("1"));
    }

    #[test]
    fn next_label_skips_used_numbers() {
        let (r, d) = scan(&ls("a[fn:1] b[fn:3]\n\n[fn:2] x\n"));
        assert_eq!(next_label(&r, &d), "4");
        assert_eq!(next_label(&[], &[]), "1");
    }

    #[test]
    fn normalize_renumbers_and_gathers() {
        let t = "First[fn:b] second[fn:a] again[fn:b] inline[fn::anon text].\n\n[fn:a] Def A.\n[fn:b] Def B.\n";
        let n = normalize(t, "Footnotes");
        assert!(
            n.starts_with("First[fn:1] second[fn:2] again[fn:1] inline[fn:3]."),
            "{n}"
        );
        assert!(
            n.contains("\n* Footnotes\n[fn:1] Def B.\n\n[fn:2] Def A.\n\n[fn:3] anon text"),
            "{n}"
        );
        // Normalizing again changes nothing.
        assert_eq!(normalize(&n, "Footnotes"), n);
        assert_eq!(normalize("no notes\n", "Footnotes"), "no notes\n");
    }

    #[test]
    fn sort_orders_definitions_by_reference() {
        let t = "A[fn:2] B[fn:1].\n\n[fn:1] One.\n\n[fn:2] Two.\n\n[fn:3] Orphan.\n";
        let s = sort_defs(t);
        let (_, defs) = scan(&ls(&s));
        let order: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(order, vec!["2", "1", "3"], "{s}");
        assert!(
            s.contains("[fn:2] Two.\n\n[fn:1] One.\n\n[fn:3] Orphan."),
            "{s}"
        );
    }

    #[test]
    fn delete_removes_references_and_definition() {
        let t = "A[fn:1] and B[fn:2] and again[fn:1].\n\n[fn:1] One.\n\n[fn:2] Two.\n";
        let d = delete(t, "1");
        assert_eq!(d, "A and B[fn:2] and again.\n\n[fn:2] Two.\n");
    }

    #[test]
    fn add_definition_creates_or_extends_the_section() {
        let (t, line) = add_definition("Text[fn:1]\n", "1", "Footnotes");
        assert_eq!(t, "Text[fn:1]\n\n* Footnotes\n[fn:1] \n");
        assert_eq!(line, 3);
        let (t2, line2) = add_definition(&t, "2", "Footnotes");
        assert_eq!(t2, "Text[fn:1]\n\n* Footnotes\n[fn:1] \n\n[fn:2] \n");
        assert_eq!(line2, 5);
    }
}
