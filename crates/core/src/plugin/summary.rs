// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a plugin set up when its init ran: commands, key bindings,
//! remappings, modes and event subscriptions.
//!
//! Forge keeps one summary per plugin in its cache directory. At the next
//! start, if the plugin file and its permissions are exactly the same, the
//! summary stands in for the plugin: its commands, keys and modes exist,
//! but nothing is compiled or run until one of its commands runs or an
//! event it wants happens. That is what makes plugins cheap to have
//! installed.
//!
//! A plugin is only started lazily if its init did nothing else anyone
//! could notice (no messages, questions, commands run, buffers made, and
//! no bindings local to one buffer). Otherwise it always starts eagerly,
//! as does one whose manifest says `startup = true`.

use crate::app::Editor;
use crate::keymap::{Origin, Scope};
use crate::modes::ModeKind;
use crate::permissions::Grants;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Hash of the plugin file, its grants and Forge's version.
    pub key: String,
    /// Init only registered things, so the plugin can start lazily.
    pub lazy: bool,
    #[serde(default)]
    pub commands: Vec<CommandEntry>,
    #[serde(default)]
    pub bindings: Vec<BindingEntry>,
    #[serde(default)]
    pub remaps: Vec<BindingEntry>,
    #[serde(default)]
    pub modes: Vec<ModeEntry>,
    /// Event kinds (indexes, see plugin_runtime::event_kind) it subscribed to.
    #[serde(default)]
    pub events: Vec<usize>,
    /// Completion settings, annotators and sources it set.
    #[serde(default)]
    pub completion: CompletionEntry,
}

/// What a plugin set through the completion interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionEntry {
    #[serde(default)]
    pub vertical: Option<u32>,
    #[serde(default)]
    pub styles: Vec<String>,
    #[serde(default)]
    pub annotates: Vec<String>,
    /// popup, auto, delay-ms, min-prefix, lines
    #[serde(default)]
    pub at_point: Option<(bool, bool, u32, u32, u32)>,
    /// (source, modes)
    #[serde(default)]
    pub sources: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEntry {
    pub name: String,
    pub doc: String,
}

/// A binding or remapping. `mode` empty means the global keymap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingEntry {
    #[serde(default)]
    pub mode: String,
    pub keys: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeEntry {
    pub name: String,
    pub major: bool,
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub lighter: Option<String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A plugin file's digest.
pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The cache key: changes whenever the plugin (its `digest`), what it's
/// allowed, or Forge itself changes.
pub fn key(digest: &str, grants: &Grants) -> String {
    let mut h = Sha256::new();
    h.update(crate::VERSION.as_bytes());
    h.update([0]);
    // The grants are part of the key, so they have to be in it even if
    // they cannot be written out as TOML: the debug form still says what
    // they are.
    h.update(
        toml::to_string(grants)
            .unwrap_or_else(|_| format!("{grants:?}"))
            .as_bytes(),
    );
    h.update([0]);
    h.update(digest.as_bytes());
    hex(&h.finalize())
}

fn scope_of(mode: &str) -> Scope {
    if mode.is_empty() {
        Scope::Global
    } else {
        Scope::Mode(mode.to_string())
    }
}

impl Summary {
    /// Where `name`'s summary lives in `dir`.
    pub fn path(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{name}.toml"))
    }

    pub fn load(dir: &Path, name: &str) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(dir, name)).ok()?;
        toml::from_str(&text).ok()
    }

    pub fn save(&self, dir: &Path, name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves half a summary.
        let path = Self::path(dir, name);
        crate::fsutil::write_atomic(&path, text)
    }

    /// Read what plugin `name` has registered in `editor`.
    /// `bindings`/`remaps` are its own records (scope, keys, command).
    pub fn record(
        editor: &Editor,
        name: &str,
        key: String,
        bindings: &[(Scope, String, String)],
        remaps: &[(Scope, String, String)],
        events: Vec<usize>,
        lazy: bool,
    ) -> Self {
        let mut lazy = lazy;
        let mut entries = |list: &[(Scope, String, String)]| -> Vec<BindingEntry> {
            list.iter()
                .filter_map(|(scope, keys, command)| {
                    match scope {
                        Scope::Global => Some(String::new()),
                        Scope::Mode(m) => Some(m.clone()),
                        // Buffers don't survive a restart: start eagerly.
                        Scope::Buffer(_) => {
                            lazy = false;
                            None
                        }
                        // A transient map is only up while something pushed
                        // it: rebuild it when that something loads.
                        Scope::Transient(_) => {
                            lazy = false;
                            None
                        }
                        // A prompt map only matters while something is
                        // asking, so it is not a startup binding either.
                        Scope::Prompt(_) => {
                            lazy = false;
                            None
                        }
                        // A conditional layer is rebuilt by the plugin that
                        // set its condition, not from here: the condition
                        // isn't a mode, and a summary has nowhere to put it.
                        Scope::Layer(_) => {
                            lazy = false;
                            None
                        }
                        // A region map comes up with the text that names
                        // it, which is the plugin's own decorations: those
                        // are set again when it loads, so this is a
                        // startup binding like a mode's.
                        Scope::Region(m) => Some(m.clone()),
                        // A project map comes from the project's own
                        // init.rhai, which loads when a file inside it is
                        // opened: not a startup binding like a mode's.
                        Scope::Project(_) => {
                            lazy = false;
                            None
                        }
                    }
                    .map(|mode| BindingEntry {
                        mode,
                        keys: keys.clone(),
                        command: command.clone(),
                    })
                })
                .collect()
        };
        let bindings = entries(bindings);
        let remaps = entries(remaps);
        let commands = editor
            .commands
            .names()
            .filter(|c| editor.commands.plugin_for(c) == Some(name))
            .map(|c| CommandEntry {
                name: c.to_string(),
                doc: editor
                    .commands
                    .get(c)
                    .map(|i| i.doc.clone())
                    .unwrap_or_default(),
            })
            .collect();
        let modes = editor
            .modes
            .owned_by(name)
            .into_iter()
            .map(|d| ModeEntry {
                major: d.kind == ModeKind::Major,
                parent: d.parent.unwrap_or_default(),
                extensions: d.extensions,
                name: d.name,
                lighter: d.lighter,
            })
            .collect();
        let c = &editor.completion;
        let mine = |owner: &String| owner == name;
        let completion = CompletionEntry {
            vertical: c.vertical.as_ref().filter(|v| mine(&v.1)).map(|v| v.0),
            styles: c
                .styles
                .as_ref()
                .filter(|v| mine(&v.1))
                .map(|v| v.0.iter().map(|s| s.name().to_string()).collect())
                .unwrap_or_default(),
            annotates: c
                .annotators
                .iter()
                .filter(|(_, p)| mine(p))
                .map(|(k, _)| k.clone())
                .collect(),
            at_point: c
                .at_point
                .as_ref()
                .filter(|v| mine(&v.1))
                .map(|(a, _)| (a.popup, a.auto, a.delay_ms, a.min_prefix, a.lines)),
            sources: c
                .sources
                .iter()
                .filter(|s| mine(&s.plugin))
                .map(|s| (s.name.clone(), s.modes.clone()))
                .collect(),
        };
        Summary {
            key,
            lazy,
            commands,
            bindings,
            remaps,
            modes,
            events,
            completion,
        }
    }

    /// Register stand-ins for everything the plugin set up. Commands run
    /// through the plugin host, which starts the plugin first.
    pub fn install(&self, editor: &mut Editor, name: &str) {
        // Parents first: a plugin's major mode may derive from another of its own.
        let mut modes: Vec<&ModeEntry> = self.modes.iter().collect();
        modes.sort_by_key(|m| self.modes.iter().any(|o| o.name == m.parent));
        for m in modes {
            let _ = if m.major {
                let parent = (!m.parent.is_empty()).then_some(m.parent.as_str());
                editor
                    .modes
                    .define_major(&m.name, parent, &m.extensions, Some(name))
            } else {
                editor.modes.define_minor(&m.name, Some(name))
            };
            if let Some(l) = &m.lighter {
                let _ = editor.modes.set_lighter(&m.name, l, Some(name));
            }
        }
        let c = &self.completion;
        let owner = name.to_string();
        if let Some(v) = c.vertical {
            editor.completion.vertical = Some((v, owner.clone()));
        }
        if !c.styles.is_empty() {
            let styles = c
                .styles
                .iter()
                .filter_map(|s| crate::completion::Style::from_name(s).ok())
                .collect();
            editor.completion.styles = Some((styles, owner.clone()));
        }
        for cat in &c.annotates {
            editor
                .completion
                .annotators
                .insert(cat.clone(), owner.clone());
        }
        if let Some((popup, auto, delay_ms, min_prefix, lines)) = c.at_point {
            let a = crate::completion::AtPoint {
                popup,
                auto,
                delay_ms,
                min_prefix,
                lines,
            };
            editor.completion.at_point = Some((a, owner.clone()));
        }
        for (source, modes) in &c.sources {
            editor.completion.sources.push(crate::completion::Source {
                name: source.clone(),
                plugin: owner.clone(),
                modes: modes.clone(),
            });
        }
        for c in &self.commands {
            let _ = editor.commands.register_plugin(&c.name, &c.doc, name);
        }
        for b in &self.bindings {
            let seq: Vec<String> = b.keys.split(' ').map(String::from).collect();
            let _ = editor
                .keymaps
                .bind_seq(&scope_of(&b.mode), &seq, &b.command, Origin::Package);
        }
        for r in &self.remaps {
            editor
                .keymaps
                .remap(&scope_of(&r.mode), &r.keys, &r.command);
        }
    }

    /// Take the stand-ins away again (before the real plugin starts, or
    /// when it's unloaded without ever starting).
    pub fn uninstall(&self, editor: &mut Editor, name: &str) {
        for b in &self.bindings {
            let scope = scope_of(&b.mode);
            if editor.keymaps.get(&scope).and_then(|m| m.get(&b.keys)) == Some(b.command.as_str()) {
                let _ = editor.keymaps.unbind(&scope, &b.keys);
            }
        }
        for r in &self.remaps {
            editor.keymaps.unremap(&scope_of(&r.mode), &r.keys);
        }
        editor.commands.remove_plugin(name);
        editor.modes.remove_owned_by(name);
        editor.menus.remove_plugin(name);
        editor.completion.remove_plugin(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_depends_on_bytes_and_grants() {
        let g = Grants::default();
        let net = Grants {
            network: true,
            ..Grants::default()
        };
        let (a, b) = (digest(b"abc"), digest(b"abd"));
        assert_eq!(key(&a, &g), key(&a, &g));
        assert_ne!(key(&a, &g), key(&b, &g));
        assert_ne!(key(&a, &g), key(&a, &net));
    }

    #[test]
    fn install_and_uninstall_round_trip() {
        let s = Summary {
            key: "k".into(),
            lazy: true,
            commands: vec![CommandEntry {
                name: "spell-check".into(),
                doc: "Check spelling.".into(),
            }],
            bindings: vec![BindingEntry {
                mode: String::new(),
                keys: "<f7>".into(),
                command: "spell-check".into(),
            }],
            remaps: vec![],
            modes: vec![ModeEntry {
                name: "spell-mode".into(),
                major: false,
                parent: String::new(),
                extensions: vec![],
                lighter: None,
            }],
            events: vec![0],
            completion: CompletionEntry {
                vertical: Some(8),
                annotates: vec!["command".into()],
                ..Default::default()
            },
        };
        let dir = std::env::temp_dir().join(format!("forge-summary-{}", std::process::id()));
        s.save(&dir, "spell").unwrap();
        assert_eq!(Summary::load(&dir, "spell").as_ref(), Some(&s));
        std::fs::remove_dir_all(&dir).unwrap();

        let mut ed = Editor::new();
        s.install(&mut ed, "spell");
        assert_eq!(ed.commands.plugin_for("spell-check"), Some("spell"));
        assert_eq!(
            ed.keymaps.get(&Scope::Global).and_then(|m| m.get("<f7>")),
            Some("spell-check")
        );
        assert!(ed.modes.is_minor("spell-mode"));
        assert_eq!(ed.completion.vertical_lines(), Some(8));
        assert_eq!(
            ed.completion.annotators.get("command").map(String::as_str),
            Some("spell")
        );
        s.uninstall(&mut ed, "spell");
        assert_eq!(ed.completion.vertical_lines(), None);
        assert!(ed.commands.plugin_for("spell-check").is_none());
        assert!(
            ed.keymaps
                .get(&Scope::Global)
                .and_then(|m| m.get("<f7>"))
                .is_none()
        );
        assert!(!ed.modes.is_minor("spell-mode"));
    }
}
