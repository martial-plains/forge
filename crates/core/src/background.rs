// SPDX-License-Identifier: MIT OR Apache-2.0
//! Background plugin calls: running a plugin on another thread with a
//! snapshot of the editor, and merging what it did back in.
//!
//! A command registered with `commands.register-background` (or an event
//! subscribed with `subscribe-background`) doesn't block the editor: the
//! plugin gets a copy of the editor (buffers are ropes, so copying is
//! cheap) and runs on a worker thread while you keep typing. When it
//! returns, its edits come back as a transaction per buffer:
//!
//! - If the buffer didn't change meanwhile, its edits are replayed
//!   exactly (one undo step).
//! - If it did, the plugin's change and yours are each reduced to the
//!   span they touched (a three-way comparison with the text the plugin
//!   started from). If the spans don't overlap, the plugin's change is
//!   applied where its text now is; if they do, it's dropped and the
//!   plugin is told why (a message; nothing is lost from your edits).
//!
//! Decorations the plugin set or cleared in the call are carried back,
//! their positions moved through the same edits. Messages, questions and
//! logs come back as from any call.

use crate::app::{BufferId, Editor};
use crate::decoration::DecoSpec;
use ropey::Rope;
use std::collections::{BTreeMap, BTreeSet};

/// Text of each buffer when the snapshot was taken: (version, text).
pub(crate) type Base = BTreeMap<BufferId, (u64, Rope)>;

pub(crate) fn base_of(editor: &Editor) -> Base {
    editor
        .state
        .buffers
        .iter()
        .map(|(id, b)| (*id, (b.version(), b.rope())))
        .collect()
}

/// One replacement: chars `start..start + old_len` became `new_len` chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Replace {
    start: usize,
    old_len: usize,
    new_len: usize,
}

impl Replace {
    fn old_end(&self) -> usize {
        self.start + self.old_len
    }

    /// A position before the change -> after it.
    fn forward(&self, pos: usize) -> usize {
        if pos <= self.start {
            pos
        } else if pos >= self.old_end() {
            pos - self.old_len + self.new_len
        } else {
            self.start
        }
    }

    /// A position after the change -> before it.
    fn backward(&self, pos: usize) -> usize {
        if pos <= self.start {
            pos
        } else if pos >= self.start + self.new_len {
            pos - self.new_len + self.old_len
        } else {
            self.start
        }
    }
}

/// The one span where `a` and `b` differ (common prefix and suffix
/// removed), with `b`'s text for it. None if they're equal.
fn diff(a: &[char], b: &[char]) -> Option<(Replace, String)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if prefix == a.len() && prefix == b.len() {
        return None;
    }
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let r = Replace {
        start: prefix,
        old_len: a.len() - prefix - suffix,
        new_len: b.len() - prefix - suffix,
    };
    Some((r, b[prefix..prefix + r.new_len].iter().collect()))
}

/// Merge what a background call did to `snapshot` into `live`. Returns
/// messages about changes that couldn't be merged.
pub(crate) fn merge(
    live: &mut Editor,
    snapshot: &Editor,
    base: &Base,
    plugin: &str,
    touched: &BTreeSet<BufferId>,
    touched_images: &BTreeSet<BufferId>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (id, (base_version, base_text)) in base {
        let Some(snap) = snapshot.state.get_buffer(*id) else {
            continue;
        };
        let edits = snap.edits_since(*base_version);
        let changed_text = snap.version() != *base_version;
        let decorations = touched.contains(id);
        let images = touched_images.contains(id);
        if !changed_text && !decorations && !images {
            continue;
        }
        let Some(buf) = live.state.get_buffer_mut(*id) else {
            if changed_text {
                problems.push(format!(
                    "{plugin}: {} was killed while it ran; its changes there were dropped",
                    snap.name()
                ));
            }
            continue;
        };
        let live_changed = buf.version() != *base_version;
        // How positions in the snapshot map to the live buffer afterwards.
        let map: Box<dyn Fn(usize) -> usize>;
        match edits {
            Some(edits) if !live_changed => {
                for e in edits {
                    buf.replace_offsets(e.at, e.at + e.deleted_len(), &e.inserted);
                }
                buf.break_undo_group();
                map = Box::new(|p| p);
            }
            _ => {
                let b: Vec<char> = base_text.chars().collect();
                let s: Vec<char> = snap.get_contents().chars().collect();
                let l: Vec<char> = buf.get_contents().chars().collect();
                let pd = diff(&b, &s);
                let ld = diff(&b, &l).map(|(r, _)| r);
                match (pd, ld) {
                    (None, None) => map = Box::new(|p| p),
                    (None, Some(ld)) => map = Box::new(move |p| ld.forward(p)),
                    (Some((pd, text)), None) => {
                        buf.replace_offsets(pd.start, pd.old_end(), &text);
                        buf.break_undo_group();
                        map = Box::new(|p| p);
                    }
                    (Some((pd, text)), Some(ld)) => {
                        let apart = pd.old_end() <= ld.start || pd.start >= ld.old_end();
                        if !apart {
                            problems.push(format!(
                                "{plugin}: {} changed while it ran, in the same place; its change was dropped",
                                buf.name()
                            ));
                            continue;
                        }
                        let at = ld.forward(pd.start);
                        let applied = Replace {
                            start: at,
                            old_len: pd.old_len,
                            new_len: pd.new_len,
                        };
                        buf.replace_offsets(at, at + pd.old_len, &text);
                        buf.break_undo_group();
                        // Text the plugin wrote keeps its place in its new
                        // text; around it, positions follow both changes.
                        map = Box::new(move |p| {
                            if p <= pd.start {
                                ld.forward(p)
                            } else if p <= pd.start + pd.new_len {
                                at + (p - pd.start)
                            } else {
                                applied.forward(ld.forward(pd.backward(p)))
                            }
                        });
                    }
                }
            }
        }
        if decorations {
            let len = buf.char_count();
            let items: Vec<_> = snap
                .decorations()
                .iter()
                .filter(|d| d.owner == plugin)
                .map(|d| {
                    DecoSpec::new(map(d.start).min(len), map(d.end).min(len), d.face)
                        .message_opt(d.message.clone())
                        .keymap_opt(d.keymap.clone())
                })
                .collect();
            buf.set_decorations(plugin, items);
        }
        if images {
            // Image pins are by line, not offset, so they move with the
            // line a plugin chose rather than a rewritten position. Copy
            // this plugin's pins straight across.
            let items: Vec<(usize, std::path::PathBuf)> = snap
                .plugin_images
                .iter()
                .filter_map(|(line, owners)| owners.get(plugin).map(|p| (*line, p.clone())))
                .collect();
            buf.set_plugin_images(plugin, items);
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::decoration::DecoFace;

    fn editor(text: &str) -> (Editor, BufferId) {
        let mut ed = Editor::new();
        let id = ed.add_buffer(Buffer::new("t"));
        ed.state.get_buffer_mut(id).unwrap().set_contents(text);
        (ed, id)
    }

    #[test]
    fn replays_when_the_buffer_is_unchanged() {
        let (mut live, id) = editor("hello world\n");
        let mut snap = live.clone();
        let base = base_of(&snap);
        let b = snap.state.get_buffer_mut(id).unwrap();
        b.replace_offsets(0, 5, "HELLO");
        b.replace_offsets(12, 12, "!");
        b.set_decorations("p", vec![(6, 11, DecoFace::Error, None)]);
        let touched = BTreeSet::from([id]);
        assert!(merge(&mut live, &snap, &base, "p", &touched, &BTreeSet::new()).is_empty());
        let b = live.state.get_buffer_mut(id).unwrap();
        assert_eq!(b.get_contents(), "HELLO world\n!");
        assert_eq!((b.decorations()[0].start, b.decorations()[0].end), (6, 11));
        assert!(b.undo());
        assert_eq!(b.get_contents(), "hello world\n", "one undo step");
    }

    #[test]
    fn merges_apart_changes_and_drops_overlapping_ones() {
        let (mut live, id) = editor("one two three\n");
        let mut snap = live.clone();
        let base = base_of(&snap);
        // The plugin upcases "three" and marks it...
        let b = snap.state.get_buffer_mut(id).unwrap();
        b.replace_offsets(8, 13, "THREE");
        b.set_decorations("p", vec![(8, 13, DecoFace::Warning, None)]);
        // ...while the user types at the start.
        live.state
            .get_buffer_mut(id)
            .unwrap()
            .replace_offsets(0, 0, ">> ");
        let problems = merge(
            &mut live,
            &snap,
            &base,
            "p",
            &BTreeSet::from([id]),
            &BTreeSet::new(),
        );
        assert!(problems.is_empty(), "{problems:?}");
        let b = live.state.get_buffer(id).unwrap();
        assert_eq!(b.get_contents(), ">> one two THREE\n");
        assert_eq!((b.decorations()[0].start, b.decorations()[0].end), (11, 16));

        // Both change "two": the plugin's change is dropped.
        let mut snap = live.clone();
        let base = base_of(&snap);
        snap.state
            .get_buffer_mut(id)
            .unwrap()
            .replace_offsets(7, 10, "2");
        live.state
            .get_buffer_mut(id)
            .unwrap()
            .replace_offsets(8, 9, "W");
        let problems = merge(
            &mut live,
            &snap,
            &base,
            "p",
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(problems.len(), 1);
        assert_eq!(
            live.state.get_buffer(id).unwrap().get_contents(),
            ">> one tWo THREE\n"
        );
    }

    #[test]
    fn decorations_move_with_the_users_edits() {
        let (mut live, id) = editor("abc def\n");
        let mut snap = live.clone();
        let base = base_of(&snap);
        snap.state
            .get_buffer_mut(id)
            .unwrap()
            .set_decorations("p", vec![(4, 7, DecoFace::Info, None)]);
        live.state
            .get_buffer_mut(id)
            .unwrap()
            .replace_offsets(0, 0, "xx");
        merge(
            &mut live,
            &snap,
            &base,
            "p",
            &BTreeSet::from([id]),
            &BTreeSet::new(),
        );
        let d = &live.state.get_buffer(id).unwrap().decorations()[0];
        assert_eq!((d.start, d.end), (6, 9));
    }

    #[test]
    fn replace_maps_positions() {
        let r = Replace {
            start: 2,
            old_len: 3,
            new_len: 1,
        };
        assert_eq!((r.forward(1), r.forward(3), r.forward(6)), (1, 2, 4));
        assert_eq!((r.backward(1), r.backward(4)), (1, 6));
        assert_eq!(
            diff(&['a', 'b', 'c'], &['a', 'X', 'c']).unwrap(),
            (
                Replace {
                    start: 1,
                    old_len: 1,
                    new_len: 1
                },
                "X".into()
            )
        );
        assert_eq!(diff(&['a'], &['a']), None);
    }
}
