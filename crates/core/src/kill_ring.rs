// SPDX-License-Identifier: MIT OR Apache-2.0
//! The kill ring: clipboard history (C-k, C-w, M-w, C-y, M-y).

use std::collections::VecDeque;

const KILL_RING_MAX: usize = 60;

#[derive(Debug, Default, Clone)]
pub struct KillRing {
    entries: VecDeque<String>,
    yank_index: usize,
}

impl KillRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new entry.
    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.entries.push_front(text);
        self.entries.truncate(KILL_RING_MAX);
        self.yank_index = 0;
    }

    /// Consecutive forward kills (e.g. C-k C-k) accumulate into one entry.
    pub fn append(&mut self, text: &str) {
        match self.entries.front_mut() {
            Some(front) => front.push_str(text),
            None => self.push(text.to_string()),
        }
        self.yank_index = 0;
    }

    /// Consecutive backward kills (M-DEL M-DEL) accumulate at the front.
    pub fn prepend(&mut self, text: &str) {
        match self.entries.front_mut() {
            Some(front) => front.insert_str(0, text),
            None => self.push(text.to_string()),
        }
        self.yank_index = 0;
    }

    /// What C-y would insert.
    pub fn current(&self) -> Option<&str> {
        self.entries.get(self.yank_index).map(String::as_str)
    }

    /// The entry M-y would reach after `steps` rotations from the
    /// current one, without moving.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// M-y: step to the next-older entry (wrapping around).
    pub fn rotate(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        self.yank_index = (self.yank_index + 1) % self.entries.len();
        self.current()
    }

    /// Every entry, newest first.
    pub fn entries(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }

    /// Make the entry `index` steps behind the current one the current
    /// one, so a later yank or M-y starts there. Does nothing when the
    /// ring is empty.
    pub fn select(&mut self, index: usize) {
        if !self.entries.is_empty() {
            self.yank_index = index % self.entries.len();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_append_rotate() {
        let mut k = KillRing::new();
        k.push("one".into());
        k.push("two".into());
        k.append(" more");
        assert_eq!(k.current(), Some("two more"));
        assert_eq!(k.rotate(), Some("one"));
        assert_eq!(k.rotate(), Some("two more"));
        k.prepend(">");
        assert_eq!(k.current(), Some(">two more"));
    }
}
