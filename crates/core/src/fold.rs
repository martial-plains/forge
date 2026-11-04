// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folding: hiding whole lines, shown as "..." after the line before them
//! (org-mode's cycling, outline folding). A fold is text-anchored like a
//! decoration: from the end of the visible line (its newline) to the end
//! of the last hidden line, so it moves with edits and goes away when its
//! text does.
//!
//! Redisplay skips hidden lines, vertical motion steps over them, and
//! point never stays inside one: redisplay opens the folds around it
//!, so search and jumps just work.

use crate::buffer::{Buffer, Edit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// The newline ending the line that stays visible.
    pub start: usize,
    /// The end of the last hidden line (before its newline).
    pub end: usize,
}

impl Fold {
    /// Follow an edit; false when the hidden text is gone. Text inserted
    /// right at the start (typing at the end of the visible line) stays
    /// visible.
    pub(crate) fn follow(&mut self, e: &Edit) -> bool {
        let deleted = e.deleted.chars().count();
        let inserted = e.inserted.chars().count();
        self.start = if deleted == 0 && self.start == e.at {
            self.start + inserted
        } else {
            e.transform(self.start)
        };
        self.end = e.transform(self.end);
        self.start < self.end
    }

    /// Is `offset` hidden?
    pub fn hides(&self, offset: usize) -> bool {
        self.start < offset && offset <= self.end
    }
}

impl Buffer {
    /// Hide lines `first..=last` (`first` > 0: the line before stays
    /// visible and shows "..."). Folds inside the new one are merged
    /// into it. Returns false for an empty or invalid range.
    pub fn fold_lines(&mut self, first: usize, last: usize) -> bool {
        if first == 0 || first > last || last >= self.line_count() {
            return false;
        }
        let start = self.line_end_offset(first - 1);
        let end = self.line_end_offset(last);
        if start >= end {
            return false;
        }
        self.folds.retain(|f| f.end < start || f.start > end);
        self.folds.push(Fold { start, end });
        self.folds.sort_by_key(|f| f.start);
        true
    }

    /// Show the lines hidden after `line` (and any fold hiding `line`
    /// itself). Returns whether anything was hidden.
    pub fn unfold_line(&mut self, line: usize) -> bool {
        let at = self.line_end_offset(line);
        let start = self.line_start_offset(line);
        let before = self.folds.len();
        self.folds.retain(|f| f.start != at && !f.hides(start));
        before != self.folds.len()
    }

    /// Show everything between lines `first` and `last` (inclusive).
    pub fn unfold_lines(&mut self, first: usize, last: usize) {
        let a = self.line_start_offset(first);
        let z = self.line_end_offset(last.min(self.line_count().saturating_sub(1)));
        self.folds.retain(|f| f.end < a || f.start > z);
    }

    pub fn unfold_all(&mut self) {
        self.folds.clear();
    }

    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    /// Is `offset` inside hidden text?
    pub fn is_hidden(&self, offset: usize) -> bool {
        self.folds.iter().any(|f| f.hides(offset))
    }

    /// Is line `line` hidden?
    pub fn line_hidden(&self, line: usize) -> bool {
        !self.folds.is_empty()
            && line > 0
            && line < self.line_count()
            && self.is_hidden(self.line_start_offset(line))
    }

    /// Does a fold start at the end of `line` (show "...")?
    pub fn line_folded(&self, line: usize) -> bool {
        if self.folds.is_empty() {
            return false;
        }
        let at = self.line_end_offset(line);
        self.folds.iter().any(|f| f.start == at)
    }

    /// Open the folds hiding `offset`. Returns whether any were.
    pub fn reveal(&mut self, offset: usize) -> bool {
        let before = self.folds.len();
        self.folds.retain(|f| !f.hides(offset));
        before != self.folds.len()
    }

    /// The first visible line after `line`, if any.
    pub fn next_visible_line(&self, line: usize) -> Option<usize> {
        (line + 1..self.line_count()).find(|&l| !self.line_hidden(l))
    }

    /// The last visible line before `line`, if any.
    pub fn prev_visible_line(&self, line: usize) -> Option<usize> {
        (0..line).rev().find(|&l| !self.line_hidden(l))
    }

    /// The visible line showing `line` (itself, or the line its fold hangs
    /// off).
    pub fn visible_line(&self, line: usize) -> usize {
        if self.line_hidden(line) {
            self.prev_visible_line(line).unwrap_or(0)
        } else {
            line
        }
    }

    /// Visible lines in `a..b`.
    pub fn visible_lines_between(&self, a: usize, b: usize) -> usize {
        if self.folds.is_empty() {
            return b.saturating_sub(a);
        }
        (a..b).filter(|&l| !self.line_hidden(l)).count()
    }

    /// The line `n` shown rows below `line`, clamping at the last
    /// visible line. Scrolling steps through these, so folds scroll by
    /// what shows rather than stalling in hidden text.
    pub fn forward_visible_line(&self, line: usize, n: usize) -> usize {
        let mut cur = line;
        for _ in 0..n {
            match self.next_visible_line(cur) {
                Some(t) => cur = t,
                None => break,
            }
        }
        cur
    }

    /// The line `n` shown rows above `line`, clamping at the first
    /// visible line.
    pub fn backward_visible_line(&self, line: usize, n: usize) -> usize {
        let mut cur = line;
        for _ in 0..n {
            match self.prev_visible_line(cur) {
                Some(t) => cur = t,
                None => break,
            }
        }
        cur
    }

    pub(crate) fn line_start_offset(&self, line: usize) -> usize {
        self.pos_to_offset(crate::Pos::new(line, 0))
    }

    pub(crate) fn line_end_offset(&self, line: usize) -> usize {
        self.pos_to_offset(crate::Pos::new(line, self.line_len(line)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("t");
        b.set_contents(text);
        b
    }

    #[test]
    fn folds_hide_lines_and_follow_edits() {
        let mut b = buf("a\nb\nc\nd\n");
        assert!(b.fold_lines(1, 2));
        assert!(!b.line_hidden(0) && b.line_hidden(1) && b.line_hidden(2) && !b.line_hidden(3));
        assert!(b.line_folded(0));
        assert_eq!(b.next_visible_line(0), Some(3));
        assert_eq!(b.prev_visible_line(3), Some(0));
        assert_eq!(b.visible_line(2), 0);
        assert_eq!(b.visible_lines_between(0, 4), 2);
        // Typing at the end of the visible line stays visible.
        b.set_point(Pos::new(0, 1));
        b.insert_str("xy");
        assert_eq!(b.line(0), "axy");
        assert!(b.line_folded(0) && b.line_hidden(1));
        // Deleting the hidden text removes the fold.
        let (s, e) = (
            b.pos_to_offset(Pos::new(0, 3)),
            b.pos_to_offset(Pos::new(2, 1)),
        );
        b.replace_offsets(s, e, "");
        assert!(b.folds().is_empty());
    }

    #[test]
    fn vertical_motion_skips_hidden_lines_and_reveal_opens() {
        let mut b = buf("h\n1\n2\nnext\n");
        b.fold_lines(1, 2);
        b.set_point(Pos::new(0, 0));
        assert!(b.move_down());
        assert_eq!(b.point().line, 3);
        assert!(b.move_up());
        assert_eq!(b.point().line, 0);
        assert!(b.reveal(b.pos_to_offset(Pos::new(2, 0))));
        assert!(b.folds().is_empty());
        b.fold_lines(1, 2);
        assert!(b.unfold_line(0));
        assert!(!b.fold_lines(0, 1) && !b.fold_lines(2, 1));
    }

    #[test]
    fn visible_stepping_skips_folds_and_clamps() {
        let mut b = buf("h\n1\n2\nnext\nlast\n");
        b.fold_lines(1, 2);
        assert_eq!(b.forward_visible_line(0, 1), 3);
        assert_eq!(b.forward_visible_line(0, 2), 4);
        assert_eq!(
            b.forward_visible_line(3, 99),
            5,
            "clamps at the last visible line"
        );
        assert_eq!(b.backward_visible_line(4, 1), 3);
        assert_eq!(
            b.backward_visible_line(3, 2),
            0,
            "hidden lines take no steps"
        );
        assert_eq!(
            b.backward_visible_line(0, 9),
            0,
            "clamps at the first visible line"
        );
        assert_eq!(b.forward_visible_line(4, 0), 4);
        // Without folds this is plain arithmetic.
        let c = buf("a\nb\nc\n");
        assert_eq!(c.forward_visible_line(0, 2), 2);
        assert_eq!(c.backward_visible_line(2, 5), 0);
    }
}
