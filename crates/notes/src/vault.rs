// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder of notes as an index: properties, tags, links, backlinks.

use std::collections::{BTreeMap, BTreeSet};

use crate::value::Value;

/// A file as read from disk.
#[derive(Debug, Clone, Default)]
pub struct RawNote {
    /// Relative to the vault, with `/` separators.
    pub path: String,
    pub text: String,
    pub size: u64,
    pub ctime: i64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Note {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub folder: String,
    pub size: u64,
    pub ctime: i64,
    pub mtime: i64,
    pub props: BTreeMap<String, Value>,
    pub tags: Vec<String>,
    /// Link targets as written (`Note`, `folder/Note`), without `|alias`
    /// or `#heading`.
    pub links: Vec<String>,
    pub excerpt: String,
}

/// (folder, name, extension) of a vault-relative path.
fn split_path(path: &str) -> (String, String, String) {
    let (folder, file) = match path.rsplit_once('/') {
        Some((d, f)) => (d.to_string(), f),
        None => (String::new(), path),
    };
    let (name, ext) = match file.rsplit_once('.') {
        Some((n, e)) => (n.to_string(), e.to_string()),
        None => (file.to_string(), String::new()),
    };
    (folder, name, ext)
}

/// The part of `path` after the vault `root`, for a listing that came back
/// with `~` expanded (`root` is `~/org`, the path `/Users/me/org/a.org`).
pub fn relative_to(root: &str, path: &str) -> String {
    let root = root.trim_end_matches('/');
    if let Some(rest) = path.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        return rest.to_string();
    }
    if let Some(tail) = root.strip_prefix('~') {
        // `~/org` listed as `/Users/me/org/...`: cut at the first `/org/`.
        if let Some(at) = path.find(&format!("{tail}/")) {
            return path[at + tail.len() + 1..].to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod relative_tests {
    use super::relative_to;

    #[test]
    fn a_listing_with_the_home_directory_expanded_is_still_relative() {
        assert_eq!(
            relative_to("~/org", "/Users/me/org/areas/a.org"),
            "areas/a.org"
        );
        assert_eq!(relative_to("/Users/me/org", "/Users/me/org/a.org"), "a.org");
        assert_eq!(
            relative_to("~/org/", "/Users/me/org/org/b.org"),
            "org/b.org"
        );
        assert_eq!(relative_to("/v", "/elsewhere/a.org"), "/elsewhere/a.org");
    }
}

pub fn is_note_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".md") || p.ends_with(".markdown") || p.ends_with(".org")
}

/// Split `---` front matter from the body: (yaml, body).
pub fn front_matter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, text);
    };
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" || line.trim_end() == "..." {
            return (Some(&rest[..at]), &rest[at + line.len()..]);
        }
        at += line.len();
    }
    (None, text)
}

/// Set (or, with empty `value`, remove) a top-level property in a note's
/// front matter, leaving the rest of the text alone. Makes the front matter
/// when there is none. `value` is written as given, so quote it yourself if
/// YAML would read it differently.
pub fn set_property(text: &str, key: &str, value: &str) -> String {
    let line = |v: &str| format!("{key}: {v}\n");
    let (fm, body) = front_matter(text);
    let Some(fm) = fm else {
        return if value.is_empty() {
            text.to_string()
        } else {
            format!("---\n{}---\n{text}", line(value))
        };
    };
    let mut out = String::new();
    let mut done = false;
    let mut skipping = false;
    for l in fm.split_inclusive('\n') {
        let top = !l.starts_with([' ', '\t', '-']) && !l.trim().is_empty();
        if skipping && !top {
            continue;
        }
        skipping = false;
        if top && l.split(':').next().is_some_and(|k| k.trim() == key) && l.contains(':') {
            done = true;
            skipping = true;
            if !value.is_empty() {
                out.push_str(&line(value));
            }
            continue;
        }
        out.push_str(l);
        if !l.ends_with('\n') {
            out.push('\n');
        }
    }
    if !done && !value.is_empty() {
        out.push_str(&line(value));
    }
    format!("---\n{out}---\n{body}")
}

/// [`set_property`] for the kind of note `path` is: an Org note keeps its
/// properties as `#+KEY: value` lines at the top (what [`Note::parse`]
/// reads), a Markdown one in YAML front matter. For Org, `value` is the
/// plain text, not YAML.
pub fn set_property_in(path: &str, text: &str, key: &str, value: &str) -> String {
    if !path.to_lowercase().ends_with(".org") {
        return set_property(text, key, value);
    }
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let finish = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        if text.ends_with('\n') || out.is_empty() {
            out.push('\n');
        }
        out
    };
    // The header: the leading `#+` lines (and blanks and drawers) Note::parse reads.
    let header = lines
        .iter()
        .position(|l| !(l.starts_with("#+") || l.trim().is_empty() || l.starts_with(':')))
        .unwrap_or(lines.len());
    let keyword = lines[..header].iter().position(|l| {
        l.strip_prefix("#+")
            .and_then(|r| r.split_once(':'))
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
    });
    // The drawers Note::parse reads: the top of the file, and the first heading's.
    let first_heading = lines
        .iter()
        .position(|l| l.starts_with('*') && l.trim_start_matches('*').starts_with(' '));
    let drawers: Vec<usize> = std::iter::once(0)
        .chain(first_heading.map(|h| h + 1))
        .filter(|&i| {
            lines
                .get(i)
                .is_some_and(|l| l.trim().eq_ignore_ascii_case(":properties:"))
        })
        .collect();
    let end_of = |lines: &[String], open: usize| {
        (open + 1..lines.len()).find(|&j| lines[j].trim().eq_ignore_ascii_case(":end:"))
    };
    // A property the note already has, in a drawer: change it where it is.
    for &open in &drawers {
        let Some(end) = end_of(&lines, open) else {
            continue;
        };
        if let Some(at) = (open + 1..end)
            .find(|&j| drawer_line(&lines[j]).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)))
        {
            if value.is_empty() {
                // Org leaves the empty property in place (`:RATING:`), as its own commands do.
                lines[at] = format!(":{}:", drawer_line(&lines[at]).map_or(key, |(k, _)| k));
            } else {
                let k = drawer_line(&lines[at]).map_or(key, |(k, _)| k).to_string();
                lines[at] = format!(":{k}: {value}");
            }
            return finish(lines);
        }
    }
    match (keyword, value.is_empty()) {
        (Some(i), true) => {
            lines.remove(i);
        }
        (Some(i), false) => lines[i] = format!("#+{key}: {value}"),
        (None, true) => return text.to_string(),
        (None, false) => {
            // A note about one thing keeps its properties in the first
            // heading's drawer; add the new one there.
            if let Some(&open) = drawers
                .last()
                .filter(|&&o| first_heading.is_some_and(|h| o == h + 1))
                && let Some(end) = end_of(&lines, open)
            {
                lines.insert(end, format!(":{}: {value}", key.to_uppercase()));
            } else {
                // After the last keyword line, before any blank line that ends the header.
                let last = lines[..header]
                    .iter()
                    .rposition(|l| l.starts_with("#+"))
                    .map_or(0, |i| i + 1);
                lines.insert(last, format!("#+{key}: {value}"));
            }
        }
    }
    finish(lines)
}

fn clean_tag(t: &str) -> Option<String> {
    let t = t.trim().trim_start_matches('#').trim_matches(',');
    (!t.is_empty()).then(|| t.to_string())
}

/// `#tag` and `#nested/tag` in prose (not headings, code or URLs).
pub fn inline_tags(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut in_code = false;
        for (i, &c) in chars.iter().enumerate() {
            if c == '`' {
                in_code = !in_code;
            }
            if c != '#'
                || in_code
                || (i > 0 && !chars[i - 1].is_whitespace() && chars[i - 1] != '(')
            {
                continue;
            }
            let word: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
                .collect();
            if word.chars().any(|c| c.is_alphabetic()) && !word.ends_with('/') {
                out.push(word);
            }
        }
    }
    out
}

/// Wiki, embed and markdown links to other notes, as targets.
pub fn links_in(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' && b.get(i + 1) == Some(&b'[') {
            if let Some(end) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + end];
                let target = inner.split("][").next().unwrap_or("");
                let target = target.split('|').next().unwrap_or("");
                let target = target.split('#').next().unwrap_or("").trim();
                let target = target.strip_prefix("file:").unwrap_or(target);
                if !target.is_empty()
                    && !target.starts_with("id:")
                    && !target.contains("://")
                    && !inner.contains('\n')
                {
                    out.push(target.to_string());
                }
                i += end + 4;
                continue;
            }
        } else if b[i] == b']'
            && b.get(i + 1) == Some(&b'(')
            && let Some(end) = body[i + 2..].find(')')
        {
            let url = body[i + 2..i + 2 + end]
                .split('#')
                .next()
                .unwrap_or("")
                .trim();
            if !url.is_empty()
                && !url.contains("://")
                && !url.starts_with("mailto:")
                && is_note_file(url)
            {
                out.push(url.replace("%20", " "));
            }
            i += end + 3;
            continue;
        }
        i += 1;
    }
    out
}

fn tag_list(v: &Value) -> Vec<String> {
    match v {
        Value::List(l) => l.iter().filter_map(|x| clean_tag(&x.to_string())).collect(),
        Value::Str(s) => s.split([',', ' ']).filter_map(clean_tag).collect(),
        _ => Vec::new(),
    }
}

fn org_meta(text: &str) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut props = BTreeMap::new();
    let mut tags = Vec::new();
    for line in text
        .lines()
        .take_while(|l| l.starts_with("#+") || l.trim().is_empty() || l.starts_with(':'))
    {
        let Some(rest) = line.strip_prefix("#+") else {
            continue;
        };
        let Some((k, v)) = rest.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        if k == "filetags" {
            tags.extend(v.split(':').filter_map(clean_tag));
        } else if !v.is_empty() {
            props.insert(k, Value::from_text(v));
        }
    }
    (props, tags)
}

/// A property drawer line: `:KEY: value` (not the drawer's own markers).
fn drawer_line(l: &str) -> Option<(&str, &str)> {
    let t = l.trim();
    let rest = t.strip_prefix(':')?;
    let (k, v) = rest.split_once(':')?;
    let k = k.trim();
    (!k.is_empty()
        && !k.eq_ignore_ascii_case("properties")
        && !k.eq_ignore_ascii_case("end")
        && k.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'))
    .then_some((k, v.trim()))
}

/// What a property's text means: a number for the keys that are numbers,
/// a list for the ones that list things, otherwise as [`Value::from_text`].
fn drawer_value(key: &str, v: &str) -> Value {
    match key {
        "rating" | "year" | "hours" | "pages" | "series_number" => v
            .parse::<f64>()
            .map(Value::Num)
            .unwrap_or_else(|_| Value::from_text(v)),
        "genres" | "recommend_to" | "recommended_by" | "aliases" | "cssclasses" => Value::List(
            v.split([',', ';'])
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(|x| Value::Str(x.to_string()))
                .collect(),
        ),
        _ => Value::from_text(v),
    }
}

/// Org keeps properties in drawers as well as `#+KEY:` lines: the one at
/// the very top of the file, and the one under the first heading (where a
/// note about one thing, a film or a book, usually keeps what it knows:
/// `:RATING: 8`). Empty ones are left out. Also the first heading's tags.
pub fn org_drawers(text: &str) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut props = BTreeMap::new();
    let mut tags = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut read_drawer = |from: usize| {
        if lines
            .get(from)
            .is_some_and(|l| l.trim().eq_ignore_ascii_case(":properties:"))
        {
            for l in &lines[from + 1..] {
                if l.trim().eq_ignore_ascii_case(":end:") {
                    break;
                }
                if let Some((k, v)) = drawer_line(l)
                    && !v.is_empty()
                {
                    let k = k.to_lowercase();
                    props
                        .entry(k.clone())
                        .or_insert_with(|| drawer_value(&k, v));
                }
            }
        }
    };
    read_drawer(0);
    if let Some(h) = lines
        .iter()
        .position(|l| l.starts_with('*') && l.trim_start_matches('*').starts_with(' '))
    {
        if let Some(last) = lines[h].split_whitespace().last()
            && last.len() > 2
            && last.starts_with(':')
            && last.ends_with(':')
        {
            tags.extend(last.split(':').filter_map(clean_tag));
        }
        read_drawer(h + 1);
    }
    (props, tags)
}

impl Note {
    pub fn parse(raw: &RawNote) -> Note {
        Note::parse_with(raw, false)
    }

    /// Just what a link graph needs: name, folder, tags and links. No
    /// excerpt and no properties, so a vault of hundreds of thousands of
    /// notes stays small in memory.
    pub fn parse_light(raw: &RawNote) -> Note {
        Note::parse_with(raw, true)
    }

    /// A note known only by where it is, as when read back from a saved
    /// index: its name, folder and kind follow from the path.
    pub fn at(path: &str) -> Note {
        let (folder, name, ext) = split_path(path);
        Note {
            path: path.to_string(),
            name,
            ext,
            folder,
            ..Default::default()
        }
    }

    fn parse_with(raw: &RawNote, light: bool) -> Note {
        let path = raw.path.clone();
        let (folder, name, ext) = split_path(&path);
        let org = ext.eq_ignore_ascii_case("org");
        let (mut props, mut tags, body) = if org {
            let (mut p, mut t) = org_meta(&raw.text);
            let (dp, dt) = org_drawers(&raw.text);
            for (k, v) in dp {
                p.entry(k).or_insert(v);
            }
            t.extend(dt);
            (p, t, raw.text.as_str())
        } else {
            let (yaml, body) = front_matter(&raw.text);
            let props: BTreeMap<String, Value> = yaml
                .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
                .and_then(|v| match Value::from_yaml(&v) {
                    Value::Obj(o) => Some(o),
                    _ => None,
                })
                .unwrap_or_default();
            (props, Vec::new(), body)
        };
        if let Some(v) = props.get("tags").or_else(|| props.get("tag")) {
            tags.extend(tag_list(v));
        }
        tags.extend(inline_tags(body));
        let mut seen = BTreeSet::new();
        tags.retain(|t| seen.insert(t.clone()));
        props
            .entry("tags".into())
            .or_insert_with(|| Value::List(tags.iter().cloned().map(Value::Str).collect()));
        let mut links = links_in(body);
        // Links written in properties count too.
        for v in props.values() {
            collect_links(v, &mut links);
        }
        if light {
            props.clear();
        }
        let excerpt: String = if light {
            String::new()
        } else {
            body.lines()
                .map(str::trim)
                .filter(|l| {
                    !l.is_empty()
                        && !l.starts_with('#')
                        && !l.starts_with("---")
                        && !l.starts_with(':')
                })
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(200)
                .collect()
        };
        Note {
            path,
            name,
            ext,
            folder,
            size: raw.size,
            ctime: raw.ctime,
            mtime: raw.mtime,
            props,
            tags,
            links,
            excerpt,
        }
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        let tag = tag.trim_start_matches('#');
        self.tags
            .iter()
            .any(|t| t == tag || t.starts_with(&format!("{tag}/")))
    }
}

fn collect_links(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Link(l) => out.push(l.clone()),
        Value::List(l) => l.iter().for_each(|x| collect_links(x, out)),
        _ => {}
    }
}

fn names_of(notes: &[Note]) -> BTreeMap<String, Vec<usize>> {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in notes.iter().enumerate() {
        by_name.entry(n.name.to_lowercase()).or_default().push(i);
    }
    by_name
}

/// The notes of a vault and how they link.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub notes: Vec<Note>,
    /// Notes by lower-cased name. Made the first time a link is looked up,
    /// so an index read back from a save (links already resolved) doesn't
    /// pay for it.
    by_name: std::sync::OnceLock<BTreeMap<String, Vec<usize>>>,
    /// Per note: the notes it links to (resolved), and the targets that
    /// didn't resolve.
    pub(crate) out: Vec<Vec<usize>>,
    pub(crate) unresolved: Vec<Vec<String>>,
    pub(crate) back: Vec<Vec<usize>>,
}

impl Index {
    pub fn build(raws: &[RawNote]) -> Index {
        Index::from_notes(
            raws.iter()
                .filter(|r| is_note_file(&r.path))
                .map(Note::parse)
                .collect(),
        )
    }

    /// An index of notes already parsed (one at a time as they were read, so
    /// the text of a huge vault is never all in memory at once).
    pub fn from_notes(notes: Vec<Note>) -> Index {
        let mut idx = Index {
            out: vec![Vec::new(); notes.len()],
            unresolved: vec![Vec::new(); notes.len()],
            back: vec![Vec::new(); notes.len()],
            by_name: Default::default(),
            notes,
        };
        for i in 0..idx.notes.len() {
            idx.link(i);
        }
        idx
    }

    /// Work out what note `i` links to (and what it can't find).
    fn link(&mut self, i: usize) {
        let links = std::mem::take(&mut self.notes[i].links);
        let mut seen = BTreeSet::new();
        for l in &links {
            match self.resolve(l, i) {
                Some(j) if j != i => {
                    if seen.insert(j) {
                        self.out[i].push(j);
                        self.back[j].push(i);
                    }
                }
                Some(_) => {}
                None => {
                    if !self.unresolved[i].contains(l) {
                        self.unresolved[i].push(l.clone());
                    }
                }
            }
        }
        self.notes[i].links = links;
    }

    /// An index restored from what [`crate::cache`] saved: the notes (without
    /// their link text), who each links to, and the links that went nowhere.
    /// Targets out of range are dropped.
    pub fn from_parts(
        notes: Vec<Note>,
        out: Vec<Vec<usize>>,
        unresolved: Vec<Vec<String>>,
    ) -> Index {
        let n = notes.len();
        let mut back = vec![Vec::new(); n];
        let mut clean: Vec<Vec<usize>> = Vec::with_capacity(n);
        for (i, targets) in out.into_iter().enumerate() {
            let targets: Vec<usize> = targets.into_iter().filter(|&j| j < n && j != i).collect();
            for &j in &targets {
                back[j].push(i);
            }
            clean.push(targets);
        }
        clean.resize(n, Vec::new());
        let mut unresolved = unresolved;
        unresolved.resize(n, Vec::new());
        Index {
            by_name: Default::default(),
            notes,
            out: clean,
            unresolved,
            back,
        }
    }

    /// A note was saved: replace it (or add it) and work out its links
    /// again. Cheap enough to do on every save. A note that this one now
    /// satisfies a dangling link of is picked up by the next rescan.
    pub fn update_note(&mut self, note: Note) -> usize {
        let i = self.place(note);
        self.link(i);
        i
    }

    /// Several notes saved at once. All are put in place before any is
    /// linked, so they can link to each other whatever their order.
    pub fn update_notes(&mut self, notes: Vec<Note>) {
        let placed: Vec<usize> = notes.into_iter().map(|n| self.place(n)).collect();
        for i in placed {
            self.link(i);
        }
    }

    /// Put `note` where its path says, clearing the links it had.
    fn place(&mut self, note: Note) -> usize {
        match self.find_path(&note.path) {
            Some(i) => {
                for j in std::mem::take(&mut self.out[i]) {
                    self.back[j].retain(|&x| x != i);
                }
                self.unresolved[i].clear();
                self.notes[i] = note;
                i
            }
            None => {
                if let Some(names) = self.by_name.get_mut() {
                    names
                        .entry(note.name.to_lowercase())
                        .or_default()
                        .push(self.notes.len());
                }
                self.notes.push(note);
                self.out.push(Vec::new());
                self.back.push(Vec::new());
                self.unresolved.push(Vec::new());
                self.notes.len() - 1
            }
        }
    }

    /// The index after a rescan: `kept` are the notes of this index that
    /// haven't changed on disk (they keep their links without being read
    /// again), `fresh` are the notes read anew (changed or added), and any
    /// other note is gone. Links to notes that appeared are picked up for
    /// every note that had a dangling link.
    pub fn merged(&self, kept: &[usize], fresh: Vec<Note>) -> Index {
        let mut notes: Vec<Note> = kept.iter().map(|&i| self.notes[i].clone()).collect();
        let n_kept = notes.len();
        notes.extend(fresh);
        let mut by_path: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::with_capacity(notes.len());
        for (i, n) in notes.iter().enumerate() {
            by_path.insert(n.path.as_str(), i);
        }
        let mut new_of_old = vec![usize::MAX; self.notes.len()];
        for (old, n) in self.notes.iter().enumerate() {
            if let Some(&new) = by_path.get(n.path.as_str()) {
                new_of_old[old] = new;
            }
        }
        let mut out = vec![Vec::new(); notes.len()];
        let mut unresolved = vec![Vec::new(); notes.len()];
        for (new, &old) in kept.iter().enumerate() {
            out[new] = self.out[old]
                .iter()
                .map(|&j| new_of_old[j])
                .filter(|&j| j != usize::MAX && j != new)
                .collect();
            unresolved[new] = self.unresolved[old].clone();
        }
        drop(by_path);
        let mut back = vec![Vec::new(); notes.len()];
        for (i, targets) in out.iter().enumerate() {
            for &j in targets {
                back[j].push(i);
            }
        }
        let mut idx = Index {
            by_name: Default::default(),
            notes,
            out,
            unresolved,
            back,
        };
        // Fresh notes are linked from their text; a kept note retries only
        // the links that had nowhere to go (a new note may be their target).
        for i in n_kept..idx.notes.len() {
            idx.link(i);
        }
        for i in 0..n_kept {
            for l in std::mem::take(&mut idx.unresolved[i]) {
                match idx.resolve(&l, i) {
                    Some(j) if j != i && !idx.out[i].contains(&j) => {
                        idx.out[i].push(j);
                        idx.back[j].push(i);
                    }
                    Some(_) => {}
                    None => idx.unresolved[i].push(l),
                }
            }
        }
        // A changed note's old edges into it were remapped by path; links
        // that other kept notes made to a *removed* note are gone with it.
        idx
    }

    /// The note a link written in `from` means: by path if it has a
    /// folder, else by name (the nearest folder wins a tie).
    pub fn resolve(&self, target: &str, from: usize) -> Option<usize> {
        let t = target.trim().trim_start_matches("./").to_lowercase();
        let mut t = t.as_str();
        for e in ["md", "markdown", "org"] {
            if let Some(stem) = t.strip_suffix(e).and_then(|s| s.strip_suffix('.')) {
                t = stem;
                break;
            }
        }
        let name = t.rsplit('/').next().unwrap_or(t);
        let candidates = self
            .by_name
            .get_or_init(|| names_of(&self.notes))
            .get(name)?;
        let slashed = t.contains('/');
        if !slashed && candidates.len() == 1 {
            return Some(candidates[0]);
        }
        let suffix = format!("/{t}");
        let from_folder = self.notes.get(from).map_or("", |n| n.folder.as_str());
        let rank = |i: usize| {
            (
                self.notes[i].folder != from_folder,
                self.notes[i].path.matches('/').count(),
            )
        };
        candidates
            .iter()
            .copied()
            .filter(|&i| {
                !slashed || {
                    let p = self.notes[i].path.to_lowercase();
                    let stem = p.rsplit_once('.').map_or(p.as_str(), |(s, _)| s);
                    stem == t || stem.ends_with(&suffix)
                }
            })
            .min_by(|&a, &b| {
                rank(a)
                    .cmp(&rank(b))
                    .then_with(|| self.notes[a].path.cmp(&self.notes[b].path))
            })
    }

    /// The index of each of `paths`, in one pass over the notes (where
    /// `find_path` for each would be a pass apiece).
    pub fn positions_of(&self, paths: &[String]) -> Vec<Option<usize>> {
        let want: std::collections::HashMap<&str, usize> = paths
            .iter()
            .enumerate()
            .map(|(k, p)| (p.as_str(), k))
            .collect();
        let mut out = vec![None; paths.len()];
        for (i, n) in self.notes.iter().enumerate() {
            if let Some(&k) = want.get(n.path.as_str()) {
                out[k] = Some(i);
            }
        }
        out
    }

    pub fn find_path(&self, path: &str) -> Option<usize> {
        self.notes.iter().position(|n| n.path == path)
    }

    pub fn outgoing(&self, i: usize) -> &[usize] {
        &self.out[i]
    }

    pub fn backlinks(&self, i: usize) -> &[usize] {
        &self.back[i]
    }

    pub fn unresolved(&self, i: usize) -> &[String] {
        &self.unresolved[i]
    }

    /// `file.*` for the expression language.
    pub fn file_value(&self, i: usize) -> Value {
        let n = &self.notes[i];
        let mut m = BTreeMap::new();
        m.insert("name".into(), Value::Str(n.name.clone()));
        m.insert("path".into(), Value::Str(n.path.clone()));
        m.insert("folder".into(), Value::Str(n.folder.clone()));
        m.insert("ext".into(), Value::Str(n.ext.clone()));
        m.insert("size".into(), Value::Num(n.size as f64));
        m.insert("ctime".into(), Value::Date(n.ctime));
        m.insert("mtime".into(), Value::Date(n.mtime));
        m.insert(
            "tags".into(),
            Value::List(n.tags.iter().cloned().map(Value::Str).collect()),
        );
        m.insert(
            "links".into(),
            Value::List(
                self.out[i]
                    .iter()
                    .map(|&j| Value::Link(self.notes[j].name.clone()))
                    .collect(),
            ),
        );
        m.insert(
            "backlinks".into(),
            Value::List(
                self.back[i]
                    .iter()
                    .map(|&j| Value::Link(self.notes[j].name.clone()))
                    .collect(),
            ),
        );
        Value::Obj(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(path: &str, text: &str) -> RawNote {
        RawNote {
            path: path.into(),
            text: text.into(),
            size: text.len() as u64,
            ..Default::default()
        }
    }

    #[test]
    fn front_matter_properties_tags_and_links() {
        let n = Note::parse(&raw(
            "projects/Alpha.md",
            "---\nstatus: active\ndue: 2024-05-01\ntags: [work, p1]\nowner: \"[[Alice]]\"\n---\n# Alpha\nSee [[Beta|the beta]] and [x](Gamma.md). #idea and #a/b\n```\n#notatag\n```\n",
        ));
        assert_eq!(
            (n.name.as_str(), n.folder.as_str(), n.ext.as_str()),
            ("Alpha", "projects", "md")
        );
        assert_eq!(n.props["status"], Value::Str("active".into()));
        assert!(matches!(n.props["due"], Value::Date(_)));
        assert_eq!(n.tags, ["work", "p1", "idea", "a/b"]);
        assert!(n.has_tag("a") && n.has_tag("#work") && !n.has_tag("notatag"));
        assert_eq!(n.links, ["Beta", "Gamma.md", "Alice"]);
        assert!(n.excerpt.starts_with("See [[Beta"));
    }

    #[test]
    fn org_files_have_keywords_and_filetags() {
        let n = Note::parse(&raw(
            "n.org",
            "#+TITLE: Hi\n#+FILETAGS: :a:b:\n\nSee [[file:other.org][other]] and [[Third]].",
        ));
        assert_eq!(n.tags, ["a", "b"]);
        assert_eq!(n.props["title"], Value::Str("Hi".into()));
        assert_eq!(n.links, ["other.org", "Third"]);
    }

    #[test]
    fn links_resolve_by_name_and_path_and_back() {
        let idx = Index::build(&[
            raw("A.md", "[[B]] [[docs/C]] [[Missing]] [[B]]"),
            raw("B.md", "[[A]]"),
            raw("docs/C.md", "[[B]]"),
            raw("other/B.md", ""),
            raw("img.png", ""),
        ]);
        assert_eq!(idx.notes.len(), 4, "images aren't notes");
        let a = idx.find_path("A.md").unwrap();
        let names: Vec<&str> = idx
            .outgoing(a)
            .iter()
            .map(|&i| idx.notes[i].path.as_str())
            .collect();
        assert_eq!(
            names,
            ["B.md", "docs/C.md"],
            "the nearer B wins, duplicates fold"
        );
        assert_eq!(idx.unresolved(a), ["Missing"]);
        let b = idx.find_path("B.md").unwrap();
        let backs: Vec<&str> = idx
            .backlinks(b)
            .iter()
            .map(|&i| idx.notes[i].path.as_str())
            .collect();
        assert_eq!(backs, ["A.md", "docs/C.md"]);
        let c = idx.find_path("docs/C.md").unwrap();
        assert_eq!(idx.resolve("c", a), Some(c), "case-insensitive");
        assert_eq!(idx.resolve("zzz", a), None);
        // From docs/, a bare [[B]] prefers a B beside it, else the shallowest.
        assert_eq!(idx.resolve("B", c), Some(b));
    }

    #[test]
    fn file_value_carries_links_and_times() {
        let mut r = raw("A.md", "[[B]]");
        r.mtime = 5000;
        let idx = Index::build(&[r, raw("B.md", "")]);
        let Value::Obj(f) = idx.file_value(0) else {
            panic!()
        };
        assert_eq!(f["name"], Value::Str("A".into()));
        assert_eq!(f["mtime"], Value::Date(5000));
        assert_eq!(f["links"], Value::List(vec![Value::Link("B".into())]));
    }
}

#[cfg(test)]
mod set_property_tests {
    use super::*;

    fn raw(path: &str, text: &str) -> RawNote {
        RawNote {
            path: path.into(),
            text: text.into(),
            size: text.len() as u64,
            ..Default::default()
        }
    }

    #[test]
    fn replaces_adds_and_removes() {
        let t = "---\ntitle: A\nstatus: todo\ntags:\n  - x\n---\nBody\n";
        assert_eq!(
            set_property(t, "status", "done"),
            "---\ntitle: A\nstatus: done\ntags:\n  - x\n---\nBody\n"
        );
        assert_eq!(
            set_property(t, "owner", "me"),
            "---\ntitle: A\nstatus: todo\ntags:\n  - x\nowner: me\n---\nBody\n"
        );
        assert_eq!(
            set_property(t, "tags", ""),
            "---\ntitle: A\nstatus: todo\n---\nBody\n"
        );
        assert_eq!(
            set_property("Body\n", "status", "todo"),
            "---\nstatus: todo\n---\nBody\n"
        );
        assert_eq!(set_property("Body\n", "status", ""), "Body\n");
    }

    #[test]
    fn org_notes_get_keyword_lines_not_yaml() {
        let t = "#+TITLE: Hi\n#+STATUS: todo\n\nBody\n";
        let moved = set_property_in("a.org", t, "status", "done");
        assert_eq!(moved, "#+TITLE: Hi\n#+status: done\n\nBody\n");
        let added = set_property_in("a.org", t, "owner", "Ann");
        assert_eq!(added, "#+TITLE: Hi\n#+STATUS: todo\n#+owner: Ann\n\nBody\n");
        assert_eq!(
            set_property_in("a.org", t, "status", ""),
            "#+TITLE: Hi\n\nBody\n"
        );
        assert_eq!(
            set_property_in("a.org", "Body\n", "status", "done"),
            "#+status: done\nBody\n"
        );
        assert!(!added.contains("---"));
        // Markdown is unchanged.
        assert!(set_property_in("a.md", "x\n", "k", "v").starts_with("---\nk: v\n---\n"));
    }

    const MOVIE: &str = "#+title: Dune (2021)\n#+filetags: :media:\n\n* Dune (2021)                       :movie:scifi:\n:PROPERTIES:\n:ID: movie-dune-2021\n:TYPE: movie\n:YEAR: 2021\n:STATUS: plan-to-watch\n:RATING:\n:GENRES: Sci-Fi, Adventure\n:END:\n\n** Review\n";

    #[test]
    fn org_property_drawers_are_properties() {
        let n = Note::parse(&raw("resources/media/movies/dune.org", MOVIE));
        assert_eq!(n.props["type"], Value::Str("movie".into()));
        assert_eq!(n.props["year"], Value::Num(2021.0));
        assert_eq!(n.props["status"], Value::Str("plan-to-watch".into()));
        assert!(
            !n.props.contains_key("rating"),
            "an empty property is no property"
        );
        assert_eq!(
            n.props["genres"],
            Value::List(vec![
                Value::Str("Sci-Fi".into()),
                Value::Str("Adventure".into())
            ])
        );
        assert!(
            n.tags.contains(&"movie".to_string())
                && n.tags.contains(&"scifi".to_string())
                && n.tags.contains(&"media".to_string()),
            "{:?}",
            n.tags
        );
        // The drawer at the top of a note (a game's guide) counts too.
        let g = Note::parse(&raw(
            "g.org",
            ":PROPERTIES:\n:ID: guide-x\n:END:\n#+title: Guide\n\n* Overview\n",
        ));
        assert_eq!(g.props["id"], Value::Str("guide-x".into()));
    }

    #[test]
    fn setting_a_property_in_an_org_drawer_keeps_the_drawer() {
        // An existing property changes where it is, an empty one is filled.
        let a = set_property_in("m.org", MOVIE, "status", "watched");
        assert!(
            a.contains(":STATUS: watched\n") && !a.contains("plan-to-watch"),
            "{a}"
        );
        let b = set_property_in("m.org", MOVIE, "rating", "9");
        assert!(b.contains(":RATING: 9\n"), "{b}");
        // A property the drawer lacks is added to it, before :END:.
        let c = set_property_in("m.org", MOVIE, "finished", "2026-11-03");
        assert!(c.contains(":FINISHED: 2026-11-03\n:END:"), "{c}");
        assert!(!c.contains("---"));
        // Clearing leaves the empty property, like Org.
        let d = set_property_in("m.org", MOVIE, "status", "");
        assert!(d.contains("\n:STATUS:\n"), "{d}");
        // And what was written reads back.
        let n = Note::parse(&raw("m.org", &b));
        assert_eq!(n.props["rating"], Value::Num(9.0));
    }
}
