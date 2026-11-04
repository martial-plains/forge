// SPDX-License-Identifier: MIT OR Apache-2.0
//! What Forge remembers between sessions, like bundled
//! save-place-mode (point in each file), recentf (recently opened files)
//! and savehist-mode (minibuffer history). One small TOML file in the
//! state directory (`config::state_dir`), written when things change;
//! a missing or broken file just means starting fresh.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const MAX_PLACES: usize = 500;
const MAX_RECENT: usize = 100;
const MAX_HISTORY: usize = 100;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    /// Char offset of point.
    pub offset: usize,
}

/// A named place, bookmark: a file and a char offset of point
/// in it, kept between sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: String,
    /// Char offset of point.
    pub offset: usize,
}

/// A web page saved from few, the text browser: shown by title, opened
/// by address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FewBookmark {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Most recently left last.
    #[serde(default)]
    places: Vec<Place>,
    /// Most recent last.
    #[serde(default)]
    recent: Vec<String>,
    /// By name, most recently set last.
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    /// few's web bookmarks, most recently added last.
    #[serde(default)]
    few_bookmarks: Vec<FewBookmark>,
    /// Minibuffer history by kind ("command", "file", "buffer", ...),
    /// most recent last.
    #[serde(default)]
    history: BTreeMap<String, Vec<String>>,
    /// Where it's saved (None: nowhere, e.g. with -q or in tests).
    #[serde(skip)]
    file: Option<PathBuf>,
    /// Save the minibuffer history too (savehist-mode); it's kept for
    /// this session either way.
    #[serde(skip)]
    pub keep_history: bool,
}

fn push_limited(list: &mut Vec<String>, item: &str, max: usize) {
    list.retain(|x| x != item);
    list.push(item.to_string());
    if list.len() > max {
        list.drain(..list.len() - max);
    }
}

impl Session {
    /// Load from `file` (None: remember only for this session).
    pub fn load(file: Option<PathBuf>) -> Self {
        let mut s: Session = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        s.file = file;
        s.keep_history = true;
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = if self.keep_history {
            toml::to_string(self)
        } else {
            let without = Session {
                places: self.places.clone(),
                recent: self.recent.clone(),
                ..Session::default()
            };
            toml::to_string(&without)
        }
        .map_err(std::io::Error::other)?;
        crate::fsutil::write_atomic(file, text)
    }

    /// save-place: where point was when `path` was last left.
    pub fn place(&self, path: &str) -> Option<usize> {
        self.places
            .iter()
            .rev()
            .find(|p| p.path == path)
            .map(|p| p.offset)
    }

    pub fn remember_place(&mut self, path: &str, offset: usize) {
        self.places.retain(|p| p.path != path);
        self.places.push(Place {
            path: path.to_string(),
            offset,
        });
        if self.places.len() > MAX_PLACES {
            self.places.drain(..self.places.len() - MAX_PLACES);
        }
    }

    /// recentf: most recent first.
    pub fn recent_files(&self) -> Vec<String> {
        self.recent.iter().rev().cloned().collect()
    }

    pub fn add_recent(&mut self, path: &str) {
        push_limited(&mut self.recent, path, MAX_RECENT);
    }

    /// Bookmarks by name, most recently set last.
    pub fn bookmark_names(&self) -> Vec<String> {
        self.bookmarks.iter().map(|b| b.name.clone()).collect()
    }

    pub fn bookmark(&self, name: &str) -> Option<Bookmark> {
        self.bookmarks
            .iter()
            .rev()
            .find(|b| b.name == name)
            .cloned()
    }

    /// Setting a bookmark of the same name again moves it.
    pub fn set_bookmark(&mut self, name: &str, path: &str, offset: usize) {
        self.bookmarks.retain(|b| b.name != name);
        self.bookmarks.push(Bookmark {
            name: name.to_string(),
            path: path.to_string(),
            offset,
        });
        if self.bookmarks.len() > MAX_PLACES {
            self.bookmarks.drain(..self.bookmarks.len() - MAX_PLACES);
        }
    }

    /// True when something was forgotten.
    pub fn remove_bookmark(&mut self, name: &str) -> bool {
        let n = self.bookmarks.len();
        self.bookmarks.retain(|b| b.name != name);
        self.bookmarks.len() != n
    }

    /// few's web bookmarks, oldest first.
    pub fn few_bookmarks(&self) -> Vec<(String, String)> {
        self.few_bookmarks
            .iter()
            .map(|b| (b.title.clone(), b.url.clone()))
            .collect()
    }

    /// Save a page; saving its address again refreshes the title.
    pub fn add_few_bookmark(&mut self, title: &str, url: &str) {
        self.few_bookmarks.retain(|b| b.url != url);
        self.few_bookmarks.push(FewBookmark {
            title: title.to_string(),
            url: url.to_string(),
        });
        const MAX_FEW_BOOKMARKS: usize = 500;
        if self.few_bookmarks.len() > MAX_FEW_BOOKMARKS {
            self.few_bookmarks
                .drain(..self.few_bookmarks.len() - MAX_FEW_BOOKMARKS);
        }
    }

    /// True when something was forgotten.
    pub fn remove_few_bookmark(&mut self, url: &str) -> bool {
        let n = self.few_bookmarks.len();
        self.few_bookmarks.retain(|b| b.url != url);
        self.few_bookmarks.len() != n
    }

    /// savehist: most recent last.
    pub fn history(&self, kind: &str) -> &[String] {
        self.history.get(kind).map_or(&[], Vec::as_slice)
    }

    pub fn add_history(&mut self, kind: &str, item: &str) {
        if !item.is_empty() {
            push_limited(
                self.history.entry(kind.to_string()).or_default(),
                item,
                MAX_HISTORY,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_saves() {
        let f =
            std::env::temp_dir().join(format!("forge-session-{}/session.toml", std::process::id()));
        let mut s = Session::load(Some(f.clone()));
        s.remember_place("/a", 10);
        s.remember_place("/b", 5);
        s.remember_place("/a", 12);
        s.add_recent("/a");
        s.add_recent("/b");
        s.add_recent("/a");
        s.add_history("command", "goto-line");
        s.add_history("command", "");
        s.save().unwrap();

        let s = Session::load(Some(f.clone()));
        assert_eq!(s.place("/a"), Some(12));
        assert_eq!(s.place("/c"), None);
        assert_eq!(s.recent_files(), ["/a", "/b"]);
        assert_eq!(s.history("command"), ["goto-line"]);
        std::fs::remove_dir_all(f.parent().unwrap()).unwrap();
        assert!(Session::load(None).save().is_ok());
    }

    #[test]
    fn bookmarks_persist() {
        let f = std::env::temp_dir().join(format!(
            "forge-bookmarks-{}/session.toml",
            std::process::id()
        ));
        let mut s = Session::load(Some(f.clone()));
        assert!(s.bookmark_names().is_empty());
        assert!(s.bookmark("home").is_none());
        s.set_bookmark("home", "/a/init.rhai", 10);
        s.set_bookmark("work", "/b/main.rs", 99);
        // Setting the same name again moves it.
        s.set_bookmark("home", "/a/other.lua", 3);
        assert_eq!(s.bookmark_names(), ["work", "home"]);
        assert_eq!(
            s.bookmark("home"),
            Some(Bookmark {
                name: "home".into(),
                path: "/a/other.lua".into(),
                offset: 3,
            })
        );
        assert!(s.remove_bookmark("work"));
        assert!(!s.remove_bookmark("work"), "already gone");
        s.save().unwrap();

        let s = Session::load(Some(f.clone()));
        assert_eq!(s.bookmark_names(), ["home"]);
        assert_eq!(s.bookmark("home").map(|b| b.offset), Some(3));
        std::fs::remove_dir_all(f.parent().unwrap()).unwrap();
    }
}
