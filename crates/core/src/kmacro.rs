// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keyboard macros: C-x ( ... C-x ) records keys, C-x e plays them back.
//!
//! Macros record key presses, not edits: replaying "search for foo, then
//! change the next word" has to search again from wherever point is now,
//! which only works if the keys are re-run.
//!
//! Keys are stored as descriptions ("C-x", "f", "RET"), the same spelling
//! [`crate::keymap::parse_keys`] understands, so the recording lives in
//! core and the frontend turns each one back into an event to replay it.

/// Longest a C-u 0 C-x e ("repeat until something fails") may run.
pub const REPEAT_LIMIT: usize = 100_000;

/// How many earlier macros [`Kmacro`] keeps in its ring (consult-kmacro).
pub const RING_LIMIT: usize = 8;

#[derive(Debug, Default, Clone)]
pub struct Kmacro {
    /// Keys recorded so far while defining a macro.
    pub recording: Option<Vec<String>>,
    /// The macro `call-last-kbd-macro` runs.
    pub last: Option<Vec<String>>,
    /// Macros defined before `last`, newest first.
    pub ring: Vec<Vec<String>>,
    pub executing: bool,
    /// Where the key sequence being typed started in `recording` (so the
    /// C-x ) that ends the macro isn't part of it).
    pub seq_start: usize,
}

impl Kmacro {
    /// Finish recording: keep the keys recorded, push the previous macro
    /// onto the ring, and make this the one `call-last-kbd-macro` runs.
    pub fn finish(&mut self, keys: Vec<String>) {
        if let Some(prev) = self.last.take() {
            self.ring.retain(|k| *k != prev);
            self.ring.insert(0, prev);
            self.ring.truncate(RING_LIMIT);
        }
        self.last = Some(keys);
    }

    /// Make entry `index` the last macro (0: the last one, then the ring).
    /// False if there is no such entry.
    pub fn select(&mut self, index: usize) -> bool {
        if index == 0 {
            return self.last.is_some();
        }
        match self.ring.get(index - 1).cloned() {
            Some(keys) => {
                self.last = Some(keys);
                true
            }
            None => false,
        }
    }

    /// Every macro, the last one first: what consult-kmacro lists.
    pub fn entries(&self) -> Vec<Vec<String>> {
        let mut all = Vec::with_capacity(self.ring.len() + 1);
        if let Some(last) = &self.last {
            all.push(last.clone());
        }
        all.extend(self.ring.iter().cloned());
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn finish_pushes_the_previous_macro_onto_the_ring() {
        let mut k = Kmacro::default();
        k.finish(keys("a"));
        k.finish(keys("b c"));
        assert_eq!(k.last.as_deref(), Some(&keys("b c")[..]));
        assert_eq!(k.entries(), vec![keys("b c"), keys("a")]);
    }

    #[test]
    fn the_ring_is_bounded() {
        let mut k = Kmacro::default();
        for i in 0..RING_LIMIT + 4 {
            k.finish(keys(&i.to_string()));
        }
        assert_eq!(k.ring.len(), RING_LIMIT);
        // The last macro is the newest, and it isn't duplicated on the ring:
        // the ring holds the ones defined before it, newest first.
        assert_eq!(
            k.last.as_deref(),
            Some(&keys(&(RING_LIMIT + 3).to_string())[..])
        );
        assert!(!k.ring.contains(k.last.as_ref().unwrap()));
        assert_eq!(
            k.ring.first().unwrap(),
            &keys(&(RING_LIMIT + 2).to_string())
        );
    }

    #[test]
    fn select_makes_an_entry_the_last_macro() {
        let mut k = Kmacro::default();
        k.finish(keys("a"));
        k.finish(keys("b"));
        assert!(k.select(1));
        assert_eq!(k.last.as_deref(), Some(&keys("a")[..]));
        assert!(!k.select(99));
        assert!(!Kmacro::default().select(0));
    }
}
