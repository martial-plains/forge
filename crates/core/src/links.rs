// SPDX-License-Identifier: MIT OR Apache-2.0
//! The buffer link layer: ranges of text that point somewhere.
//!
//! A decoration attaches a face (and maybe a region keymap) to a stretch
//! of text; a link says where that text *points*. Help buffers fill theirs
//! in from the doc registry so RET and `<mouse-1>` can follow a command
//! name or a manual node. Dired, few and the compilation buffers can move
//! their ad hoc regions onto the same layer.
//!
//! Links are anchored like decorations: they follow edits and one whose
//! text is deleted disappears. [`Buffer::set_contents`] clears them, so a
//! caller fills the layer in right after writing the text.

use crate::buffer::{Buffer, Edit};
use crate::syntax::SyntaxFace;

/// Where a link in a buffer points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A documented symbol: a command, option, mode, face, hook or event.
    Symbol(String),
    /// A node of the manual.
    Node(String),
    /// A key sequence, to describe with `describe-key`.
    Key(String),
    /// A URL to open outside Forge.
    Url(String),
    /// A place in a file (a plugin's source, `init.rhai`).
    Source {
        /// The file to visit.
        file: String,
        /// The line to move to (1-based).
        line: usize,
    },
}

/// One link: a char range, where it points, and an optional face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    /// Char offsets, `start < end`.
    pub start: usize,
    /// Char offsets, `start < end`.
    pub end: usize,
    /// Where the text points.
    pub target: LinkTarget,
    /// How to draw it, when the buffer has no other face for the text.
    pub face: Option<SyntaxFace>,
}

impl LinkSpan {
    /// A link with no face of its own.
    pub fn new(start: usize, end: usize, target: LinkTarget) -> Self {
        LinkSpan {
            start,
            end,
            target,
            face: None,
        }
    }

    /// Follow an edit. False when the linked text is gone.
    pub(crate) fn follow(&mut self, e: &Edit) -> bool {
        let deleted = e.deleted.chars().count();
        let inserted = e.inserted.chars().count();
        // Text inserted right at the start goes before it, not into it.
        self.start = if deleted == 0 && self.start == e.at {
            self.start + inserted
        } else {
            e.transform(self.start)
        };
        self.end = e.transform(self.end);
        self.start < self.end
    }

    /// Is `offset` inside this link?
    pub fn covers(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }
}

impl Buffer {
    /// Replace the link layer (char offsets). Empty or backwards ranges are
    /// skipped. Cleared by [`Buffer::set_contents`].
    pub fn set_links(&mut self, links: Vec<LinkSpan>) {
        self.links = links.into_iter().filter(|l| l.start < l.end).collect();
    }

    /// The link layer, in the order the caller set it.
    pub fn links(&self) -> &[LinkSpan] {
        &self.links
    }

    /// The innermost link containing `offset`, if any.
    pub fn link_at(&self, offset: usize) -> Option<&LinkSpan> {
        self.links
            .iter()
            .filter(|l| l.covers(offset))
            .min_by_key(|l| l.end - l.start)
    }

    /// Move point to the start of the next (or previous) link's text.
    /// False when there is no such link.
    pub fn goto_link(&mut self, forward: bool) -> bool {
        let p = self.point_offset();
        let mut starts: Vec<usize> = self.links.iter().map(|l| l.start).collect();
        starts.sort_unstable();
        let target = if forward {
            starts.into_iter().find(|&s| s > p)
        } else {
            starts.into_iter().rev().find(|&s| s < p)
        };
        match target {
            Some(s) => {
                self.set_offset(s);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;

    #[test]
    fn set_and_look_up_links() {
        let mut b = Buffer::new("t");
        b.set_contents("alpha beta gamma\n");
        b.set_links(vec![
            LinkSpan::new(0, 5, LinkTarget::Symbol("alpha".into())),
            LinkSpan::new(6, 10, LinkTarget::Symbol("beta".into())),
        ]);
        assert_eq!(b.links().len(), 2);
        assert!(matches!(
            b.link_at(1).map(|l| &l.target),
            Some(LinkTarget::Symbol(n)) if n == "alpha"
        ));
        assert!(b.link_at(4).is_some());
        assert!(b.link_at(5).is_none(), "the end offset is outside");
        assert!(b.link_at(6).is_some());
        assert!(b.link_at(12).is_none());
    }

    #[test]
    fn backwards_and_empty_ranges_are_dropped() {
        let mut b = Buffer::new("t");
        b.set_contents("abc");
        b.set_links(vec![
            LinkSpan::new(2, 2, LinkTarget::Symbol("x".into())),
            LinkSpan::new(3, 1, LinkTarget::Symbol("y".into())),
        ]);
        assert!(b.links().is_empty());
    }

    #[test]
    fn set_contents_clears_links() {
        let mut b = Buffer::new("t");
        b.set_contents("hello");
        b.set_links(vec![LinkSpan::new(
            0,
            5,
            LinkTarget::Symbol("hello".into()),
        )]);
        assert_eq!(b.links().len(), 1);
        b.set_contents("hello again");
        assert!(b.links().is_empty());
    }

    #[test]
    fn links_follow_edits() {
        let mut b = Buffer::new("t");
        b.set_contents("alpha beta\n");
        b.set_links(vec![LinkSpan::new(
            6,
            10,
            LinkTarget::Symbol("beta".into()),
        )]);
        b.set_point(Pos::new(0, 0));
        b.insert_str(">> ");
        assert_eq!((b.links()[0].start, b.links()[0].end), (9, 13));
    }

    #[test]
    fn goto_link_walks_the_layer() {
        let mut b = Buffer::new("t");
        b.set_contents("one two three\n");
        b.set_links(vec![
            LinkSpan::new(0, 3, LinkTarget::Symbol("one".into())),
            LinkSpan::new(4, 7, LinkTarget::Symbol("two".into())),
            LinkSpan::new(8, 13, LinkTarget::Symbol("three".into())),
        ]);
        b.set_point(Pos::new(0, 0));
        assert!(b.goto_link(true));
        assert_eq!(b.point_offset(), 4);
        assert!(b.goto_link(true));
        assert_eq!(b.point_offset(), 8);
        assert!(!b.goto_link(true), "no link past the last");
        assert!(b.goto_link(false));
        assert_eq!(b.point_offset(), 4);
    }
}
