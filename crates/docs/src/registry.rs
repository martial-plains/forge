// SPDX-License-Identifier: MIT OR Apache-2.0
//! The doc registry: maps typed keys to [`Doc`]s and answers queries.
//!
//! Besides the map itself the registry keeps two indexes, both maintained
//! as docs are added or removed: a per-[`Kind`] set of names (so `of_kind`
//! and `commands` never scan every doc) and a word index over names,
//! summaries and bodies (so `apropos` looks up candidate words instead of
//! scanning every doc's text).

use std::collections::{BTreeMap, BTreeSet};

use crate::{Doc, Kind, NodeId};

/// A map from `(kind, name)` to [`Doc`], with kind and word indexes.
#[derive(Debug, Default)]
pub struct DocRegistry {
    docs: BTreeMap<(Kind, String), Doc>,
    by_kind: BTreeMap<Kind, BTreeSet<String>>,
    words: BTreeMap<String, BTreeSet<(Kind, String)>>,
}

impl DocRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a doc.
    pub fn add(&mut self, doc: Doc) -> &Doc {
        let key = (doc.kind, doc.name.clone());
        if let Some(old) = self.docs.get(&key).cloned() {
            self.deindex(&old);
        }
        self.index(&doc);
        self.docs.insert(key.clone(), doc);
        self.docs.get(&key).unwrap()
    }

    /// Look up a doc by kind and name.
    pub fn get(&self, kind: Kind, name: &str) -> Option<&Doc> {
        self.docs.get(&(kind, name.to_string()))
    }

    /// Whether a doc exists for the kind and name.
    pub fn has(&self, kind: Kind, name: &str) -> bool {
        self.docs.contains_key(&(kind, name.to_string()))
    }

    /// Drop every doc registered by a plugin.
    pub fn remove_plugin(&mut self, plugin: &str) {
        self.docs
            .retain(|_, d| !matches!(d.source, crate::Source::Plugin(ref p) if p == plugin));
        self.reindex();
    }

    /// Keep only the docs `keep` returns true for.
    pub fn retain<F: FnMut(&Doc) -> bool>(&mut self, mut keep: F) {
        self.docs.retain(|_, d| keep(d));
        self.reindex();
    }

    /// All registered docs, in insertion order (BTreeMap: key order).
    pub fn iter(&self) -> impl Iterator<Item = &Doc> {
        self.docs.values()
    }

    /// The registered docs of one kind, sorted by name.
    pub fn of_kind(&self, kind: Kind) -> Vec<&Doc> {
        self.by_kind
            .get(&kind)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| self.docs.get(&(kind, name.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Commands, sorted by name, with documentation.
    pub fn commands(&self) -> Vec<&Doc> {
        self.of_kind(Kind::Command)
    }

    /// The manual nodes, in the order their docs were added (which is the
    /// manual's document order, since [`Manual::as_docs`] yields nodes in
    /// order).
    ///
    /// [`Manual::as_docs`]: crate::manual::Manual::as_docs
    pub fn nodes(&self) -> Vec<&Doc> {
        self.iter().filter(|d| d.kind == Kind::Node).collect()
    }

    /// The manual node that documents `kind:name`, if the doc names one
    /// (`Doc::manual`).
    pub fn node_of(&self, kind: Kind, name: &str) -> Option<&NodeId> {
        self.get(kind, name).and_then(|d| d.manual.as_ref())
    }

    /// The docs a manual node links to, by resolving the node doc's
    /// `see_also` references (the `:COMMANDS:` / `:OPTIONS:` properties of
    /// the heading).
    pub fn docs_for_node(&self, id: &NodeId) -> Vec<&Doc> {
        let Some(node) = self.get(Kind::Node, &id.0) else {
            return Vec::new();
        };
        node.see_also
            .iter()
            .filter_map(|r| self.get(r.kind, &r.name))
            .collect()
    }

    /// Approximate lookup over a loose query: every word of `query` must be
    /// a prefix of a word in the doc's name, summary or body. Case does not
    /// matter. Results are sorted by name.
    pub fn apropos(&self, query: &str) -> Vec<&Doc> {
        self.apropos_filtered(query, false)
    }

    /// Like [`apropos`](Self::apropos), but every query word must be a
    /// prefix of a word in the doc's *name*; the summary and body are not
    /// searched. This backs `apropos-command` (C-h a), which lists commands
    /// by name.
    pub fn apropos_names(&self, query: &str) -> Vec<&Doc> {
        self.apropos_filtered(query, true)
    }

    fn apropos_filtered(&self, query: &str, names_only: bool) -> Vec<&Doc> {
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut acc: Option<BTreeSet<(Kind, String)>> = None;
        for token in &tokens {
            let mut hits: BTreeSet<(Kind, String)> = BTreeSet::new();
            for (word, keys) in self.words.range(token.clone()..) {
                if !word.starts_with(token.as_str()) {
                    break;
                }
                hits.extend(keys.iter().cloned());
            }
            acc = Some(match acc {
                None => hits,
                Some(prev) => prev.intersection(&hits).cloned().collect(),
            });
        }
        let mut keys: Vec<(Kind, String)> = acc.unwrap_or_default().into_iter().collect();
        if names_only {
            keys.retain(|(_, name)| {
                let words = tokenize(name);
                tokens
                    .iter()
                    .all(|t| words.iter().any(|w| w.starts_with(t.as_str())))
            });
        }
        keys.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        keys.iter().filter_map(|k| self.docs.get(k)).collect()
    }

    /// Add one doc's words to the indexes.
    fn index(&mut self, doc: &Doc) {
        let key = (doc.kind, doc.name.clone());
        self.by_kind
            .entry(doc.kind)
            .or_default()
            .insert(doc.name.clone());
        for word in doc_words(doc) {
            self.words.entry(word).or_default().insert(key.clone());
        }
    }

    /// Remove one doc's words from the indexes.
    fn deindex(&mut self, doc: &Doc) {
        let key = (doc.kind, doc.name.clone());
        if let Some(names) = self.by_kind.get_mut(&doc.kind) {
            names.remove(&doc.name);
        }
        for word in doc_words(doc) {
            if let Some(keys) = self.words.get_mut(&word) {
                keys.remove(&key);
                if keys.is_empty() {
                    self.words.remove(&word);
                }
            }
        }
    }

    /// Rebuild both indexes from scratch (after `retain` / `remove_plugin`).
    fn reindex(&mut self) {
        self.by_kind.clear();
        self.words.clear();
        let keys: Vec<(Kind, String)> = self.docs.keys().cloned().collect();
        for key in keys {
            let words = doc_words(&self.docs[&key]);
            self.by_kind.entry(key.0).or_default().insert(key.1.clone());
            for word in words {
                self.words.entry(word).or_default().insert(key.clone());
            }
        }
    }
}

/// Every lowercased word in a doc's name, summary and body.
fn doc_words(doc: &Doc) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    for text in [doc.name.as_str(), doc.summary.as_str()] {
        words.extend(tokenize(text));
    }
    words.extend(tokenize(&crate::render::body_text(doc)));
    words
}

/// Split text into lowercased words on any non-alphanumeric character.
fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Arg, Block, Doc, Kind, Ref, Source, Span};

    fn doc(kind: Kind, name: &str, summary: &str, body: Vec<Block>) -> Doc {
        Doc {
            kind,
            name: name.to_string(),
            summary: summary.to_string(),
            body,
            args: Vec::new(),
            see_also: Vec::new(),
            examples: Vec::new(),
            since: None,
            deprecated: None,
            source: Source::Builtin,
            manual: None,
            category: None,
        }
    }

    #[test]
    fn add_get_has_and_kind_filter() {
        let mut r = DocRegistry::new();
        r.add(doc(
            Kind::Command,
            "save-buffer",
            "Save the buffer.",
            vec![Block::Para(vec![Span::Text("Saves it.".into())])],
        ));
        r.add(doc(
            Kind::Option,
            "fill-column",
            "The fill column.",
            Vec::new(),
        ));

        assert!(r.has(Kind::Command, "save-buffer"));
        assert!(!r.has(Kind::Command, "fill-column"));
        assert_eq!(
            r.get(Kind::Command, "save-buffer")
                .map(|d| d.summary.as_str()),
            Some("Save the buffer.")
        );
        assert_eq!(r.of_kind(Kind::Command).len(), 1);
        assert_eq!(r.of_kind(Kind::Option).len(), 1);
        assert_eq!(r.commands().len(), 1);
    }

    #[test]
    fn add_replaces_same_kind_and_name() {
        let mut r = DocRegistry::new();
        r.add(doc(Kind::Command, "foo", "First.", Vec::new()));
        r.add(doc(Kind::Command, "foo", "Second.", Vec::new()));
        assert_eq!(r.iter().count(), 1);
        assert_eq!(r.get(Kind::Command, "foo").unwrap().summary, "Second.");
    }

    #[test]
    fn the_same_name_in_two_kinds_coexists() {
        let mut r = DocRegistry::new();
        r.add(doc(Kind::Command, "occur", "A command.", Vec::new()));
        r.add(doc(Kind::Mode, "occur", "A mode.", Vec::new()));
        assert_eq!(r.iter().count(), 2);
        assert_eq!(r.get(Kind::Command, "occur").unwrap().summary, "A command.");
        assert_eq!(r.get(Kind::Mode, "occur").unwrap().summary, "A mode.");
    }

    #[test]
    fn remove_plugin_drops_only_that_plugins_docs() {
        let mut r = DocRegistry::new();
        let mut mine = doc(Kind::Command, "p-hello", "Hi.", Vec::new());
        mine.source = Source::Plugin("mine".into());
        let mut theirs = doc(Kind::Command, "q-hello", "Hi.", Vec::new());
        theirs.source = Source::Plugin("theirs".into());
        r.add(mine);
        r.add(theirs);
        r.add(doc(Kind::Command, "builtin", "Built in.", Vec::new()));

        r.remove_plugin("mine");
        assert!(!r.has(Kind::Command, "p-hello"));
        assert!(r.has(Kind::Command, "q-hello"));
        assert!(r.has(Kind::Command, "builtin"));
    }

    #[test]
    fn commands_are_sorted_by_name() {
        let mut r = DocRegistry::new();
        r.add(doc(Kind::Command, "zebra", "", Vec::new()));
        r.add(doc(Kind::Command, "apple", "", Vec::new()));
        r.add(doc(Kind::Mode, "mid", "", Vec::new()));
        let names: Vec<&str> = r.commands().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["apple", "zebra"]);
    }

    #[test]
    fn apropos_matches_name_summary_and_body() {
        let mut r = DocRegistry::new();
        r.add(doc(
            Kind::Command,
            "forward-char",
            "Move point.",
            Vec::new(),
        ));
        r.add(doc(
            Kind::Command,
            "backward-char",
            "Move the other way.",
            vec![Block::Para(vec![Span::Text(
                "Moves point backward, a character.".into(),
            )])],
        ));
        r.add(doc(
            Kind::Command,
            "save-buffer",
            "Write it out.",
            Vec::new(),
        ));

        let hits: Vec<&str> = r.apropos("char").iter().map(|d| d.name.as_str()).collect();
        assert_eq!(hits, ["backward-char", "forward-char"]);
        assert_eq!(r.apropos("WRITE")[0].name, "save-buffer");
        assert_eq!(r.apropos("character")[0].name, "backward-char");
        assert!(r.apropos("   ").is_empty());
        assert!(r.apropos("nothing-here").is_empty());
    }

    #[test]
    fn apropos_names_ignores_the_summary_and_body() {
        let mut r = DocRegistry::new();
        r.add(doc(
            Kind::Command,
            "backward-char",
            "Move point.",
            vec![Block::Para(vec![Span::Text(
                "Moves point backward, a character.".into(),
            )])],
        ));
        r.add(doc(
            Kind::Command,
            "save-buffer",
            "Write the buffer to its file.",
            Vec::new(),
        ));

        // "character" is in the body of backward-char, not its name.
        assert_eq!(r.apropos("character").len(), 1);
        assert!(r.apropos_names("character").is_empty());
        // Name words still match, by prefix.
        assert_eq!(r.apropos_names("char")[0].name, "backward-char");
        assert_eq!(r.apropos_names("char").len(), 1);
    }

    #[test]
    fn apropos_reads_all_block_shapes() {
        let mut r = DocRegistry::new();
        r.add(doc(
            Kind::Concept,
            "everything",
            "",
            vec![
                Block::List {
                    ordered: false,
                    items: vec![vec![Block::Para(vec![Span::Text("listed word".into())])]],
                },
                Block::Example("example text".into()),
                Block::Note(vec![Block::Para(vec![Span::Text("noted word".into())])]),
                Block::Heading(1, vec![Span::Text("heading word".into())]),
            ],
        ));
        for q in ["listed", "example", "noted", "heading"] {
            assert_eq!(r.apropos(q).len(), 1, "query {q:?}");
        }
    }

    #[test]
    fn args_and_see_also_round_trip() {
        let mut d = doc(Kind::Command, "frobnicate", "Frob it.", Vec::new());
        d.args.push(Arg {
            name: "count".into(),
            desc: "How many.".into(),
            optional: true,
        });
        d.see_also.push(crate::Ref {
            kind: Kind::Command,
            name: "unfrobnicate".into(),
            label: None,
        });
        let mut r = DocRegistry::new();
        r.add(d);
        let got = r.get(Kind::Command, "frobnicate").unwrap();
        assert_eq!(got.args.len(), 1);
        assert!(got.args[0].optional);
        assert_eq!(got.see_also[0].name, "unfrobnicate");
    }

    #[test]
    fn adding_a_doc_keeps_the_indexes_in_step() {
        let mut r = DocRegistry::new();
        r.add(doc(Kind::Command, "alpha", "First.", Vec::new()));
        assert_eq!(r.apropos("alpha").len(), 1);
        assert_eq!(r.apropos("first").len(), 1);

        r.add(doc(Kind::Command, "alpha", "Second.", Vec::new()));
        assert_eq!(r.apropos("first").len(), 0, "old summary word removed");
        assert_eq!(r.apropos("second").len(), 1);

        r.retain(|d| d.name != "alpha");
        assert!(r.apropos("second").is_empty(), "retain clears the index");
        assert!(r.of_kind(Kind::Command).is_empty());
    }

    #[test]
    fn docs_for_node_resolves_the_nodes_commands() {
        let mut node = doc(Kind::Node, "Saving files", "How saving works.", Vec::new());
        node.see_also.push(Ref {
            kind: Kind::Command,
            name: "save-buffer".into(),
            label: None,
        });
        node.see_also.push(Ref {
            kind: Kind::Command,
            name: "gone".into(),
            label: None,
        });
        let mut r = DocRegistry::new();
        r.add(node);
        r.add(doc(Kind::Command, "save-buffer", "Save it.", Vec::new()));

        let id = NodeId("Saving files".into());
        let docs = r.docs_for_node(&id);
        assert_eq!(docs.len(), 1, "the dangling command is dropped");
        assert_eq!(docs[0].name, "save-buffer");
        assert!(r.docs_for_node(&NodeId("Nope".into())).is_empty());
    }

    #[test]
    fn node_of_reads_the_docs_manual_field() {
        let mut d = doc(Kind::Command, "save-buffer", "Save it.", Vec::new());
        d.manual = Some(NodeId("Saving files".into()));
        let mut r = DocRegistry::new();
        r.add(d);
        assert_eq!(
            r.node_of(Kind::Command, "save-buffer")
                .map(|n| n.0.as_str()),
            Some("Saving files")
        );
        assert!(r.node_of(Kind::Command, "missing").is_none());
    }

    #[test]
    fn apropos_matches_word_prefixes() {
        let mut r = DocRegistry::new();
        r.add(doc(
            Kind::Command,
            "save-buffer",
            "Write it out.",
            Vec::new(),
        ));
        assert_eq!(r.apropos("sav").len(), 1, "prefix of a name word");
        assert_eq!(r.apropos("wr").len(), 1, "prefix of a summary word");
    }
}
