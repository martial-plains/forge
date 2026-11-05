// SPDX-License-Identifier: MIT OR Apache-2.0
//! Major and minor modes, by name.
//!
//! A major mode says what kind of text a buffer holds ("rust-mode"); a
//! buffer has exactly one. Minor modes are extra features switched on per
//! buffer. Modes are identified by name everywhere (keymaps, buffers, the
//! plugin API).

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeKind {
    Major,
    Minor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeDef {
    pub name: String,
    pub kind: ModeKind,
    /// Major modes: the mode this one derives from (its keymap applies too).
    pub parent: Option<String>,
    /// Major modes: file name extensions without the dot ("rs", "md").
    pub extensions: Vec<String>,
    /// The plugin that defined it; None for built-in modes.
    pub owner: Option<String>,
    /// Minor modes: what the mode line shows (None: the name).
    pub lighter: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModeRegistry {
    modes: BTreeMap<String, ModeDef>,
    /// (interpreter, mode): `#!` programs use_package's `interpreter` added.
    interpreters: Vec<(String, String)>,
    /// (regex, mode, fallback): file starts use_package's `magic` and
    /// `magic_fallback` added.
    magic: Vec<(regex::Regex, String, bool)>,
}

impl Default for ModeRegistry {
    fn default() -> Self {
        let mut r = Self {
            modes: BTreeMap::new(),
            interpreters: Vec::new(),
            magic: Vec::new(),
        };
        for (name, parent) in [
            ("fundamental-mode", None),
            ("special-mode", None),
            ("help-mode", Some("special-mode")),
            // The Info-style manual browser (C-h i), whose buffer is *Manual*.
            ("info-mode", Some("special-mode")),
            ("tutorial-mode", Some("special-mode")),
            ("messages-buffer-mode", Some("special-mode")),
            ("Buffer-menu-mode", Some("special-mode")),
            ("occur-mode", Some("special-mode")),
            ("compilation-mode", Some("special-mode")),
            ("grep-mode", Some("compilation-mode")),
            ("grep-edit-mode", Some("grep-mode")),
            ("shell-command-mode", Some("special-mode")),
            ("package-menu-mode", Some("special-mode")),
            ("dired-mode", Some("special-mode")),
            ("org-agenda-mode", Some("special-mode")),
            ("org-columns-mode", Some("special-mode")),
            ("few-mode", Some("special-mode")),
            ("webkit-mode", Some("special-mode")),
            // Version control's buffers (forge-editor's vc module).
            ("log-view-mode", Some("special-mode")),
            ("vc-dir-mode", Some("special-mode")),
            ("vc-annotate-mode", Some("special-mode")),
            ("vc-list-mode", Some("special-mode")),
            ("vc-output-mode", Some("special-mode")),
            ("diff-side-by-side-mode", Some("special-mode")),
            // Parents for the modes below and for user-defined ones.
            ("prog-mode", Some("fundamental-mode")),
        ] {
            r.define_major(name, parent, &[], None)
                .expect("built-in modes are valid");
        }
        r.define_major("text-mode", Some("fundamental-mode"), &["txt".into()], None)
            .expect("built-in modes are valid");
        r.define_major(crate::org::MODE, Some("text-mode"), &["org".into()], None)
            .expect("built-in modes are valid");
        r.define_major("log-edit-mode", Some("text-mode"), &[], None)
            .expect("built-in modes are valid");
        // Visiting an image file (png, jpg, gif, ...) selects Image mode.
        let exts: Vec<String> = crate::org_display::DEFAULT_IMAGE_EXTENSIONS
            .iter()
            .map(|e| e.to_string())
            .collect();
        r.define_major(
            crate::buffer::IMAGE_MODE,
            Some("fundamental-mode"),
            &exts,
            None,
        )
        .expect("built-in modes are valid");
        // A plugin's drawing (see canvas.rs): shown like a picture.
        r.define_major(crate::canvas::MODE, Some("fundamental-mode"), &[], None)
            .expect("built-in modes are valid");
        r.define_major(
            "diff-mode",
            Some("fundamental-mode"),
            &["diff".into(), "patch".into(), "rej".into()],
            None,
        )
        .expect("built-in modes are valid");
        // A major mode per built-in grammar (see syntax::LANGUAGES).
        for l in &crate::syntax::LANGUAGES {
            let exts: Vec<String> = l.extensions.iter().map(|e| e.to_string()).collect();
            r.define_major(l.mode, crate::syntax::mode_parent(l.mode), &exts, None)
                .expect("built-in modes are valid");
        }
        // *scratch*: Rhai, with C-j and C-x C-e to evaluate what is before
        // point, like lisp-interaction-mode.
        r.define_major(crate::rhai::INTERACTION_MODE, Some("rhai-mode"), &[], None)
            .expect("built-in modes are valid");
        // The one built-in minor mode: the menu bar's row at the top of the
        // frame, and the keys that open it (F10, M-`). It has no lighter,
        // since its name says what it is and the row it draws says it too.
        r.define_minor(MENU_BAR_MODE, None)
            .expect("built-in modes are valid");
        r.set_lighter(MENU_BAR_MODE, "", None).ok();
        // The right-click menu (see [`crate::context_menu`]). Off by
        // default, like Forge's.
        r.define_minor(CONTEXT_MENU_MODE, None)
            .expect("built-in modes are valid");
        r.set_lighter(CONTEXT_MENU_MODE, "", None).ok();
        // The tab bar's row at the top of the frame (tab-bar-mode). Off by
        // default, like Forge's; no lighter, since the row says it all.
        r.define_minor(TAB_BAR_MODE, None)
            .expect("built-in modes are valid");
        r.set_lighter(TAB_BAR_MODE, "", None).ok();
        // The tool bar's row (tool-bar-mode) and the row of modifier keys
        // (modifier-bar-mode). No lighters: the rows are the indication.
        for mode in [TOOL_BAR_MODE, MODIFIER_BAR_MODE] {
            r.define_minor(mode, None)
                .expect("built-in modes are valid");
            r.set_lighter(mode, "", None).ok();
        }
        // Resolving conflict markers, breaking lines as you type, and
        // cdlatex-style math input in Org.
        for (mode, lighter) in [
            ("smerge-mode", "SMerge"),
            ("auto-fill-mode", "Fill"),
            ("org-cdlatex-mode", "Cd"),
        ] {
            r.define_minor(mode, None)
                .expect("built-in modes are valid");
            r.set_lighter(mode, lighter, None).ok();
        }
        r
    }
}

/// The built-in minor mode that shows the menu bar (see [`crate::menu`]).
pub const MENU_BAR_MODE: &str = "menu-bar-mode";

/// The built-in minor mode that makes right-click (and S-F10) open a
/// context menu (see [`crate::context_menu`]).
pub const CONTEXT_MENU_MODE: &str = "context-menu-mode";

/// The built-in minor mode that draws the tab bar's row at the top of the
/// frame (see [`crate::window`]'s tabs and forge-editor's `tabs`).
pub const TAB_BAR_MODE: &str = "tab-bar-mode";

/// The built-in minor mode that draws the tool bar's row (see
/// [`crate::toolbar`] and forge-editor's `toolbar`).
pub const TOOL_BAR_MODE: &str = "tool-bar-mode";

/// The built-in minor mode that draws the modifier bar (see
/// [`crate::toolbar`] and forge-editor's `toolbar`).
pub const MODIFIER_BAR_MODE: &str = "modifier-bar-mode";

fn check_name(name: &str) -> Result<(), String> {
    if !name.ends_with("-mode") || name.len() <= "-mode".len() || name.contains(char::is_whitespace)
    {
        return Err(format!(
            "mode names end in \"-mode\" and have no spaces: {name:?}"
        ));
    }
    Ok(())
}

impl ModeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define_major(
        &mut self,
        name: &str,
        parent: Option<&str>,
        extensions: &[String],
        owner: Option<&str>,
    ) -> Result<(), String> {
        check_name(name)?;
        if let Some(p) = parent {
            if !self.is_major(p) {
                return Err(format!("unknown parent mode {p}"));
            }
            if p == name {
                return Err(format!("{name} can't derive from itself"));
            }
        }
        self.claim(name, owner)?;
        let extensions = extensions
            .iter()
            .map(|e| e.trim_start_matches('.').to_string())
            .collect();
        self.modes.insert(
            name.to_string(),
            ModeDef {
                name: name.to_string(),
                kind: ModeKind::Major,
                parent: parent.map(String::from),
                extensions,
                owner: owner.map(String::from),
                lighter: None,
            },
        );
        Ok(())
    }

    /// Make files with `ext` open in an existing major mode (use_package's
    /// `mode`). A built-in extension is added to the mode; nothing is
    /// removed from whichever mode had it first.
    pub fn associate(&mut self, ext: &str, mode: &str) -> Result<(), String> {
        let ext = ext.trim_start_matches('.').to_string();
        match self.modes.get_mut(mode) {
            Some(d) if d.kind == ModeKind::Major => {
                if !d.extensions.contains(&ext) {
                    d.extensions.push(ext);
                }
                Ok(())
            }
            Some(_) => Err(format!("{mode} isn't a major mode")),
            None => Err(format!("unknown mode {mode}")),
        }
    }

    /// Files whose `#!` names `interpreter` open in `mode`.
    pub fn add_interpreter(&mut self, interpreter: &str, mode: &str) -> Result<(), String> {
        if !self.is_major(mode) {
            return Err(format!("{mode} isn't a major mode"));
        }
        self.interpreters
            .push((interpreter.to_string(), mode.to_string()));
        Ok(())
    }

    /// Files whose start matches `pattern` open in `mode`; a `fallback`
    /// one is tried only after everything else.
    pub fn add_magic(&mut self, pattern: &str, mode: &str, fallback: bool) -> Result<(), String> {
        if !self.is_major(mode) {
            return Err(format!("{mode} isn't a major mode"));
        }
        let re = regex::Regex::new(pattern).map_err(|e| format!("bad pattern {pattern:?}: {e}"))?;
        self.magic.push((re, mode.to_string(), fallback));
        Ok(())
    }

    /// The mode a file's first lines pick: `magic` matches, a `#!` line
    /// (the built-in ones, then added interpreters), then `magic_fallback`.
    pub fn major_for_content(&self, head: &str) -> Option<String> {
        let magic = |fallback: bool| {
            self.magic
                .iter()
                .find(|(re, _, f)| *f == fallback && re.is_match(head))
                .map(|(_, m, _)| m.clone())
        };
        if let Some(m) = magic(false) {
            return Some(m);
        }
        let line0 = head.lines().next().unwrap_or("");
        if let Some(m) = crate::syntax::mode_for_shebang(line0) {
            return Some(m.to_string());
        }
        if let Some(rest) = line0.strip_prefix("#!") {
            let mut words = rest.split_whitespace();
            let mut prog = words
                .next()
                .and_then(|w| w.rsplit('/').next())
                .unwrap_or("");
            if prog == "env" {
                prog = words.find(|w| !w.starts_with('-')).unwrap_or("");
            }
            let prog = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
            if let Some((_, m)) = self.interpreters.iter().find(|(i, _)| i == prog) {
                return Some(m.clone());
            }
        }
        magic(true)
    }

    pub fn define_minor(&mut self, name: &str, owner: Option<&str>) -> Result<(), String> {
        check_name(name)?;
        self.claim(name, owner)?;
        let def = ModeDef {
            name: name.to_string(),
            kind: ModeKind::Minor,
            parent: None,
            extensions: Vec::new(),
            owner: owner.map(String::from),
            lighter: None,
        };
        self.modes.insert(name.to_string(), def);
        Ok(())
    }

    /// A name can be redefined only by whoever defined it (so a plugin can
    /// be reloaded), never taken over.
    fn claim(&self, name: &str, owner: Option<&str>) -> Result<(), String> {
        match self.modes.get(name) {
            Some(d) if d.owner.as_deref() != owner || d.owner.is_none() => {
                let by = d
                    .owner
                    .as_deref()
                    .map_or("Forge".to_string(), |o| format!("plugin {o}"));
                Err(format!("{name} is already defined by {by}"))
            }
            _ => Ok(()),
        }
    }

    pub fn get(&self, name: &str) -> Option<&ModeDef> {
        self.modes.get(name)
    }

    pub fn is_major(&self, name: &str) -> bool {
        self.get(name).is_some_and(|d| d.kind == ModeKind::Major)
    }

    pub fn is_minor(&self, name: &str) -> bool {
        self.get(name).is_some_and(|d| d.kind == ModeKind::Minor)
    }

    pub fn names(&self, kind: ModeKind) -> Vec<String> {
        self.modes
            .values()
            .filter(|d| d.kind == kind)
            .map(|d| d.name.clone())
            .collect()
    }

    /// The major mode for a file name: by its extension (a mode from a
    /// plugin or init.rhai wins over a built-in one), or for a few files
    /// by their whole name (Cargo.lock, .bashrc).
    pub fn major_for_file(&self, file_name: &str) -> Option<&str> {
        let by_ext = file_name.rsplit_once('.').and_then(|(stem, ext)| {
            let claims = |d: &&ModeDef| {
                d.kind == ModeKind::Major
                    && !stem.is_empty()
                    && d.extensions.iter().any(|e| e == ext)
            };
            let mut found = self.modes.values().filter(claims);
            let first = found.next()?;
            Some(if first.owner.is_some() {
                first
            } else {
                found.find(|d| d.owner.is_some()).unwrap_or(first)
            })
        });
        by_ext.map(|d| d.name.as_str()).or_else(|| {
            crate::syntax::languages()
                .into_iter()
                .find(|l| l.file_names.contains(&file_name))
                .map(|l| l.mode)
        })
    }

    /// Is `mode` `ancestor` or derived from it?
    pub fn derives_from(&self, mode: &str, ancestor: &str) -> bool {
        self.lineage(mode).any(|m| m == ancestor)
    }

    /// `mode`, its parent, and so on.
    pub fn lineage<'a>(&'a self, mode: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let mut next = Some(mode);
        let mut depth = 0;
        std::iter::from_fn(move || {
            let m = next.take()?;
            depth += 1;
            if depth < 32 {
                next = self.get(m).and_then(|d| d.parent.as_deref());
            }
            Some(m)
        })
    }

    /// The grammar for a major mode: its own, or the nearest parent's.
    pub fn language_for(&self, mode: &str) -> Option<&'static crate::syntax::LanguageDef> {
        self.lineage(mode)
            .find_map(crate::syntax::language_for_mode)
    }

    /// The modes a plugin defined.
    pub fn owned_by(&self, owner: &str) -> Vec<ModeDef> {
        self.modes
            .values()
            .filter(|d| d.owner.as_deref() == Some(owner))
            .cloned()
            .collect()
    }

    /// Set what the mode line shows for a minor mode `owner` defined.
    pub fn set_lighter(
        &mut self,
        mode: &str,
        lighter: &str,
        owner: Option<&str>,
    ) -> Result<(), String> {
        match self.modes.get_mut(mode) {
            Some(d) if d.kind == ModeKind::Minor && d.owner.as_deref() == owner => {
                d.lighter = Some(lighter.to_string());
                Ok(())
            }
            Some(_) => Err(format!("{mode} isn't a minor mode this plugin defined")),
            None => Err(format!("unknown mode {mode}")),
        }
    }

    /// The mode line's text for a minor mode (None: nothing).
    pub fn lighter(&self, mode: &str) -> Option<String> {
        match self.modes.get(mode).and_then(|d| d.lighter.clone()) {
            Some(l) if l.is_empty() => None,
            Some(l) => Some(l),
            None => Some(crate::display::mode_display_name(mode)),
        }
    }

    pub fn remove_owned_by(&mut self, owner: &str) {
        self.modes.retain(|_, d| d.owner.as_deref() != Some(owner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_and_find_by_extension() {
        let mut r = ModeRegistry::new();
        assert_eq!(r.major_for_file("main.rs"), Some("rust-mode"));
        assert_eq!(r.major_for_file("notes.md"), Some("markdown-mode"));
        assert_eq!(r.major_for_file("init.rhai"), Some("rhai-mode"));
        assert_eq!(r.major_for_file("notes.txt"), Some("text-mode"));
        assert_eq!(r.major_for_file("Cargo.lock"), Some("toml-mode"));
        assert_eq!(r.major_for_file(".bashrc"), Some("sh-mode"));
        assert_eq!(r.major_for_file("Makefile"), None);
        assert_eq!(r.major_for_file(".rs"), None, "a dotfile has no extension");
        // A plugin's mode wins over the built-in one.
        r.define_major(
            "rustic-mode",
            Some("rust-mode"),
            &["rs".into(), ".rlib".into()],
            Some("rustic"),
        )
        .unwrap();
        assert_eq!(r.major_for_file("main.rs"), Some("rustic-mode"));
        assert_eq!(r.major_for_file("x.rlib"), Some("rustic-mode"));
        // ...and gets the grammar of the mode it derives from.
        assert_eq!(r.language_for("rustic-mode").map(|l| l.name), Some("rust"));
        assert!(r.language_for("text-mode").is_none());
        // Markdown is built in, and gfm-mode (the plugin's) derives from it.
        assert_eq!(
            r.language_for("markdown-mode").map(|l| l.name),
            Some("markdown")
        );
        r.define_major(
            "gfm-mode",
            Some("markdown-mode"),
            &[],
            Some("markdown-mode"),
        )
        .unwrap();
        assert_eq!(r.language_for("gfm-mode").map(|l| l.name), Some("markdown"));
        assert!(r.derives_from("gfm-mode", "text-mode"));
        assert!(r.derives_from("rustic-mode", "prog-mode"));
        assert!(!r.derives_from("help-mode", "prog-mode"));
        assert!(r.is_major("help-mode"));
        assert_eq!(
            r.get("help-mode").unwrap().parent.as_deref(),
            Some("special-mode")
        );
    }

    #[test]
    fn names_are_checked_and_not_stolen() {
        let mut r = ModeRegistry::new();
        assert!(r.define_minor("spell", None).is_err());
        assert!(r.define_minor("spell-mode", Some("a")).is_ok());
        assert!(
            r.define_minor("spell-mode", Some("a")).is_ok(),
            "the owner may redefine"
        );
        assert!(
            r.define_minor("spell-mode", Some("b"))
                .unwrap_err()
                .contains("plugin a")
        );
        assert!(
            r.define_major("help-mode", None, &[], Some("b"))
                .unwrap_err()
                .contains("Forge")
        );
        assert!(
            r.define_major("x-mode", Some("nope-mode"), &[], None)
                .is_err()
        );
        r.remove_owned_by("a");
        assert!(!r.is_minor("spell-mode"));
    }
}
