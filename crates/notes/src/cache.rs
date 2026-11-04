// SPDX-License-Identifier: MIT OR Apache-2.0
//! A vault's index, and a graph's layout, saved as text so that opening a
//! vault of hundreds of thousands of notes doesn't mean reading each one.
//!
//! The index file has a header line and then one line per note, with tabs
//! between the fields:
//!
//! ```text
//! path  mtime  size  tags  who-it-links-to  dangling-links
//! ```
//!
//! (tags and dangling links are joined by U+001F, link targets are note
//! numbers joined by spaces). Edits made since the index was saved go in a
//! second, small file so that saving a note doesn't rewrite the whole index.
//! A file that doesn't parse is ignored: the cache is only ever a shortcut.

use std::collections::HashMap;

use crate::graph::{Graph, Kind};
use crate::vault::{Index, Note};

const INDEX: &str = "forge-index 1";
const EDITS: &str = "forge-edits 1";
const LAYOUT: &str = "forge-layout 1";
const SEP: char = '\u{1f}';

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            SEP => out.push(' '),
            c => out.push(c),
        }
    }
}

fn unesc(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn list(items: &[String], out: &mut String) {
    for (k, t) in items.iter().enumerate() {
        if k > 0 {
            out.push(SEP);
        }
        esc(t, out);
    }
}

fn unlist(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(SEP).map(unesc).collect()
}

/// The header of a saved file: which vault it is of, and when it was saved
/// (milliseconds since 1970).
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub vault: String,
    pub saved: i64,
}

fn header(kind: &str, h: &Header) -> String {
    let mut s = format!("{kind}\t{}\t", h.saved);
    esc(&h.vault, &mut s);
    s.push('\n');
    s
}

fn read_header<'a>(kind: &str, text: &'a str) -> Option<(Header, &'a str)> {
    let (first, rest) = text.split_once('\n')?;
    let mut f = first.splitn(3, '\t');
    if f.next()? != kind {
        return None;
    }
    let saved = f.next()?.parse().ok()?;
    Some((Header { saved, vault: unesc(f.next()?) }, rest))
}

/// The whole index as text.
pub fn encode_index(idx: &Index, h: &Header) -> String {
    let mut s = header(INDEX, h);
    s.reserve(idx.notes.len() * 96);
    for (i, n) in idx.notes.iter().enumerate() {
        esc(&n.path, &mut s);
        s.push('\t');
        s.push_str(&n.mtime.to_string());
        s.push('\t');
        s.push_str(&n.size.to_string());
        s.push('\t');
        list(&n.tags, &mut s);
        s.push('\t');
        for (k, j) in idx.out[i].iter().enumerate() {
            if k > 0 {
                s.push(' ');
            }
            s.push_str(&j.to_string());
        }
        s.push('\t');
        list(&idx.unresolved[i], &mut s);
        s.push('\n');
    }
    s
}

/// An index back from [`encode_index`]; `None` if the text isn't one.
pub fn decode_index(text: &str) -> Option<(Header, Index)> {
    let (h, rest) = read_header(INDEX, text)?;
    let mut notes = Vec::new();
    let mut out = Vec::new();
    let mut unresolved = Vec::new();
    for line in rest.lines() {
        let mut f = line.split('\t');
        let mut n = Note::at(&unesc(f.next()?));
        n.mtime = f.next()?.parse().ok()?;
        n.size = f.next()?.parse().ok()?;
        n.tags = unlist(f.next()?);
        let targets: Option<Vec<usize>> = f.next()?.split(' ').filter(|t| !t.is_empty()).map(|t| t.parse().ok()).collect();
        out.push(targets?);
        unresolved.push(unlist(f.next()?));
        notes.push(n);
    }
    Some((h, Index::from_parts(notes, out, unresolved)))
}

/// Notes saved since the index was: their links as written, so they can be
/// worked out again once the index is back.
pub fn encode_edits(notes: &[Note], h: &Header) -> String {
    let mut s = header(EDITS, h);
    for n in notes {
        esc(&n.path, &mut s);
        s.push('\t');
        s.push_str(&n.mtime.to_string());
        s.push('\t');
        s.push_str(&n.size.to_string());
        s.push('\t');
        list(&n.tags, &mut s);
        s.push('\t');
        list(&n.links, &mut s);
        s.push('\n');
    }
    s
}

pub fn decode_edits(text: &str) -> Option<(Header, Vec<Note>)> {
    let (h, rest) = read_header(EDITS, text)?;
    let mut notes = Vec::new();
    for line in rest.lines() {
        let mut f = line.split('\t');
        let mut n = Note::at(&unesc(f.next()?));
        n.mtime = f.next()?.parse().ok()?;
        n.size = f.next()?.parse().ok()?;
        n.tags = unlist(f.next()?);
        n.links = unlist(f.next()?);
        notes.push(n);
    }
    Some((h, notes))
}

/// A drawn graph, saved with where each note settled and keyed by `key` (the
/// settings that decided which notes are drawn). A graph of just notes can
/// be opened from this alone, without the index: see [`decode_snapshot`].
/// Tags and dangling links have no path to be found by, so a graph with
/// them isn't saved.
pub fn encode_layout(g: &Graph, idx: &Index, key: &str, h: &Header) -> Option<String> {
    let mut s = header(LAYOUT, h);
    esc(key, &mut s);
    s.push('\n');
    s.push_str(&format!("{}\t{}\t{}\n", g.hidden, g.nodes.len(), g.edges.len()));
    for n in &g.nodes {
        let Kind::Note(i) = n.kind else { return None };
        esc(&idx.notes[i].path, &mut s);
        s.push_str(&format!("\t{}\t{}\t{}\n", n.group, n.x, n.y));
    }
    for &(a, b) in &g.edges {
        s.push_str(&format!("{a}\t{b}\n"));
    }
    Some(s)
}

/// A saved graph read back without the index: the paths of its notes, in
/// node order. The nodes' `Kind::Note` numbers are placeholders until the
/// index is back (see `Index::positions_of`).
pub struct Snapshot {
    pub graph: Graph,
    pub paths: Vec<String>,
}

fn body<'a>(text: &'a str, key: &str) -> Option<(usize, usize, usize, std::str::Lines<'a>)> {
    let (_, rest) = read_header(LAYOUT, text)?;
    let (saved_key, rest) = rest.split_once('\n')?;
    if unesc(saved_key) != key {
        return None;
    }
    let mut lines = rest.lines();
    let mut counts = lines.next()?.split('\t');
    let hidden = counts.next()?.parse().ok()?;
    let nodes = counts.next()?.parse().ok()?;
    let edges = counts.next()?.parse().ok()?;
    Some((hidden, nodes, edges, lines))
}

pub fn decode_snapshot(text: &str, key: &str) -> Option<Snapshot> {
    let (hidden, n, e, mut lines) = body(text, key)?;
    let mut g = Graph { hidden, ..Graph::default() };
    let mut paths = Vec::with_capacity(n);
    for _ in 0..n {
        let mut f = lines.next()?.split('\t');
        let path = unesc(f.next()?);
        let note = Note::at(&path);
        g.nodes.push(crate::graph::Node { label: note.name, kind: Kind::Note(usize::MAX), x: 0.0, y: 0.0, pinned: false, degree: 0, group: f.next()?.parse().ok()?, depth: 0 });
        let last = g.nodes.last_mut()?;
        last.x = f.next()?.parse().ok()?;
        last.y = f.next()?.parse().ok()?;
        paths.push(path);
    }
    for _ in 0..e {
        let mut f = lines.next()?.split('\t');
        let (a, b): (usize, usize) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        if a >= n || b >= n {
            return None;
        }
        g.nodes[a].degree += 1;
        g.nodes[b].degree += 1;
        g.edges.push((a, b));
    }
    Some(Snapshot { graph: g, paths })
}

/// Put a saved layout on `g`; false (and `g` untouched) unless it is for the
/// same `key` and has a place for every note in `g`.
pub fn apply_layout(text: &str, g: &mut Graph, idx: &Index, key: &str) -> bool {
    let Some((_, n, _, lines)) = body(text, key) else { return false };
    let mut at: HashMap<String, (f32, f32)> = HashMap::with_capacity(n);
    for line in lines.take(n) {
        let mut f = line.split('\t');
        let (Some(p), Some(_), Some(x), Some(y)) = (f.next(), f.next(), f.next(), f.next()) else { return false };
        let (Ok(x), Ok(y)) = (x.parse(), y.parse()) else { return false };
        at.insert(unesc(p), (x, y));
    }
    let mut found = Vec::with_capacity(g.nodes.len());
    for n in &g.nodes {
        let Kind::Note(i) = n.kind else { return false };
        match at.get(&idx.notes[i].path) {
            Some(&p) => found.push(p),
            None => return false,
        }
    }
    for (n, (x, y)) in g.nodes.iter_mut().zip(found) {
        n.x = x;
        n.y = y;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Options, global, layout};
    use crate::vault::RawNote;

    fn vault() -> Index {
        let raws: Vec<RawNote> = (0..40)
            .map(|i| RawNote {
                path: format!("d{}/n {i}.md", i % 3),
                text: format!("---\ntags: [t{}]\n---\nSee [[n {}]] and [[Nowhere{}]]\n", i % 4, (i + 1) % 40, i % 2),
                size: 10 + i as u64,
                mtime: 1000 + i as i64,
                ..Default::default()
            })
            .collect();
        Index::build(&raws)
    }

    const H: Header = Header { saved: 5, vault: String::new() };

    fn header_for(v: &str) -> Header {
        Header { saved: 77, vault: v.into() }
    }

    #[test]
    fn an_index_survives_being_saved_and_read_back() {
        let idx = vault();
        let (h, back) = decode_index(&encode_index(&idx, &header_for("~/my notes"))).unwrap();
        assert_eq!(h, header_for("~/my notes"));
        assert_eq!(back.notes.len(), idx.notes.len());
        for i in 0..idx.notes.len() {
            assert_eq!(back.notes[i].path, idx.notes[i].path);
            assert_eq!(back.notes[i].name, idx.notes[i].name);
            assert_eq!(back.notes[i].folder, idx.notes[i].folder);
            assert_eq!((back.notes[i].mtime, back.notes[i].size), (idx.notes[i].mtime, idx.notes[i].size));
            assert_eq!(back.notes[i].tags, idx.notes[i].tags);
            assert_eq!(back.outgoing(i), idx.outgoing(i));
            assert_eq!(back.backlinks(i), idx.backlinks(i));
            assert_eq!(back.unresolved(i), idx.unresolved(i));
        }
        // Names resolve against the restored index too.
        assert_eq!(back.resolve("n 5", 0), idx.resolve("n 5", 0));
    }

    #[test]
    fn odd_paths_and_garbage_are_handled() {
        let mut idx = Index::build(&[RawNote { path: "we\tird\\name.md".into(), text: "x".into(), ..Default::default() }]);
        idx.notes[0].tags = vec!["a b".into()];
        let (_, back) = decode_index(&encode_index(&idx, &H)).unwrap();
        assert_eq!(back.notes[0].path, "we\tird\\name.md");
        assert!(decode_index("not an index").is_none());
        assert!(decode_index("forge-index 1\t5\tv\nonly\ttwo\n").is_none());
        assert!(decode_edits("forge-index 1\t5\tv\n").is_none());
    }

    #[test]
    fn saved_edits_are_applied_to_a_restored_index() {
        let idx = vault();
        let (_, mut back) = decode_index(&encode_index(&idx, &H)).unwrap();
        let mut edited = Note::at("d1/n 4.md");
        edited.links = vec!["n 10".into(), "brand new".into()];
        let mut added = Note::at("brand new.md");
        added.tags = vec!["fresh".into()];
        let text = encode_edits(&[edited, added], &H);
        let (_, notes) = decode_edits(&text).unwrap();
        back.update_notes(notes);
        let four = back.find_path("d1/n 4.md").unwrap();
        let names: Vec<&str> = back.outgoing(four).iter().map(|&j| back.notes[j].name.as_str()).collect();
        assert_eq!(names, ["n 10", "brand new"]);
        // The old link from n 4 to n 5 is gone, from both ends.
        let five = back.find_path("d2/n 5.md").unwrap();
        assert!(!back.backlinks(five).contains(&four));
        assert_eq!(back.notes.len(), 41);
    }

    #[test]
    fn a_rescan_keeps_unchanged_notes_and_picks_up_changes() {
        let idx = vault();
        let gone = idx.find_path("d0/n 3.md").unwrap();
        let changed = idx.find_path("d1/n 4.md").unwrap();
        let kept: Vec<usize> = (0..idx.notes.len()).filter(|&i| i != gone && i != changed).collect();
        let mut again = Note::at("d1/n 4.md");
        again.links = vec!["n 20".into()];
        let mut new = Note::at("d0/Nowhere0.md");
        new.links = vec!["n 0".into()];
        let m = idx.merged(&kept, vec![again, new]);
        assert_eq!(m.notes.len(), 40 - 1 + 1);
        assert!(m.find_path("d0/n 3.md").is_none());
        let four = m.find_path("d1/n 4.md").unwrap();
        assert_eq!(m.outgoing(four).len(), 1);
        // Who linked to n 4 still does (by path, so the changed note keeps its backlinks).
        let three_before = idx.backlinks(changed).len();
        assert_eq!(m.backlinks(four).len(), three_before - usize::from(idx.backlinks(changed).contains(&gone)));
        // Links that had nowhere to go now reach the note that appeared.
        let nowhere = m.find_path("d0/Nowhere0.md").unwrap();
        assert!(!m.backlinks(nowhere).is_empty());
        // Everything is consistent: each edge has its mirror.
        for i in 0..m.notes.len() {
            for &j in m.outgoing(i) {
                assert!(m.backlinks(j).contains(&i));
            }
        }
    }

    #[test]
    fn a_layout_is_reused_only_for_the_same_notes_and_settings() {
        let idx = vault();
        let o = Options::default();
        let mut g = global(&idx, &o);
        layout(&mut g, None);
        let text = encode_layout(&g, &idx, "k1", &H).unwrap();
        let mut again = global(&idx, &o);
        assert!(apply_layout(&text, &mut again, &idx, "k1"));
        for (a, b) in g.nodes.iter().zip(&again.nodes) {
            assert_eq!((a.x, a.y), (b.x, b.y));
        }
        let mut other = global(&idx, &o);
        assert!(!apply_layout(&text, &mut other, &idx, "k2"), "different settings");
        assert!(other.nodes.iter().all(|n| n.x == 0.0 && n.y == 0.0), "untouched");
        // The same file opens a graph with no index at all.
        let snap = decode_snapshot(&text, "k1").unwrap();
        assert_eq!(snap.graph.nodes.len(), g.nodes.len());
        assert_eq!(snap.graph.edges, g.edges);
        for (i, n) in g.nodes.iter().enumerate() {
            assert_eq!(snap.paths[i], idx.notes[match n.kind { Kind::Note(k) => k, _ => unreachable!() }].path);
            assert_eq!((snap.graph.nodes[i].x, snap.graph.nodes[i].y), (n.x, n.y));
            assert_eq!((snap.graph.nodes[i].label.as_str(), snap.graph.nodes[i].degree, snap.graph.nodes[i].group), (n.label.as_str(), n.degree, n.group));
        }
        assert!(decode_snapshot(&text, "other").is_none());
        assert!(decode_snapshot("junk", "k1").is_none());
        let tags = global(&idx, &Options { tags: true, ..o });
        assert!(encode_layout(&tags, &idx, "k1", &H).is_none(), "tag dots can't be found again by path");
    }
}
