// SPDX-License-Identifier: MIT OR Apache-2.0
//! Loaded plugins and every call into them.
//!
//! For each call the [`Editor`] is lent to the plugin's store (moved in,
//! then moved back), so host functions can change buffers and keymaps
//! directly while the rest of Forge keeps plain ownership of the editor.
//! What a call leaves for the frontend comes back as an [`Outcome`].
//!
//! Events: after each command the frontend calls [`PluginHost::notice_changes`],
//! which compares the editor with what it saw last time and tells
//! subscribed plugins what happened. `buffer-changed` waits for idle time
//! (`idle = true`), so plugins never run on every keystroke. Changes that
//! plugins make while handling events are not announced again, which
//! keeps two plugins from ping-ponging.

#![deny(clippy::unwrap_used, clippy::expect_used)]

use crate::app::{BufferId, Editor};
use crate::events::EventTracker;
use crate::keymap::Scope;
use crate::permissions::{Grants, Manifest};
use crate::plugin::runtime::{
    CallResult, EVENT_KINDS, Effect, Inspection, LoadedPlugin, LogLine, PluginEvent, PluginMonitor,
    PluginRuntime, event_kind, wit_selection,
};
use crate::plugin::summary::{self, Summary};
use anyhow::Result;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

/// A call waiting for a plugin that is busy in the background.
enum Queued {
    Command(String, Option<i64>),
    Event(PluginEvent),
    Answer(u32, Option<String>),
}

/// What a background call hands back.
struct Done {
    plugin: LoadedPlugin,
    snapshot: Editor,
    base: crate::background::Base,
    result: CallResult,
}

/// A plugin running a background call on another thread.
struct Away {
    rx: mpsc::Receiver<Done>,
    queue: VecDeque<Queued>,
    /// Its subscriptions when it left (events for it are queued).
    subscriptions: [bool; EVENT_KINDS],
    /// Unload it when it's back.
    unload: bool,
    /// The key bindings and remappings it had made, so they can be taken
    /// back if it crashes and its store is lost with it.
    bindings: Vec<(Scope, String, String)>,
    remaps: Vec<(Scope, String, String)>,
}

/// What calls into plugins left for the frontend to do or show.
#[derive(Debug, Default)]
pub struct Outcome {
    pub effects: Vec<Effect>,
    /// (plugin, line) in the order they were logged.
    pub logs: Vec<(String, LogLine)>,
    /// A plugin returned an error or crashed.
    pub errors: Vec<String>,
    /// Plugins unloaded because they crashed.
    pub unloaded: Vec<String>,
}

impl Outcome {
    fn merge(&mut self, other: Outcome) {
        self.effects.extend(other.effects);
        self.logs.extend(other.logs);
        self.errors.extend(other.errors);
        self.unloaded.extend(other.unloaded);
    }
}

/// A plugin file read from disk, with its manifest (next to it or
/// embedded in it). Nothing is compiled yet.
pub struct PluginFile {
    pub path: PathBuf,
    pub name: String,
    pub manifest: Option<Manifest>,
    bytes: Vec<u8>,
}

impl PluginFile {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let manifest = Manifest::for_component(path, &bytes).map_err(anyhow::Error::msg)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "plugin".into());
        let name = manifest
            .as_ref()
            .and_then(|m| m.name.clone())
            .unwrap_or(stem);
        Ok(Self {
            path: path.to_path_buf(),
            name,
            manifest,
            bytes,
        })
    }

    /// What the manifest asks for.
    pub fn requested(&self) -> Grants {
        self.manifest
            .as_ref()
            .map(|m| m.permissions.clone())
            .unwrap_or_default()
    }
}

/// A plugin file, compiled and looked at but not yet running.
pub struct PluginRequest {
    pub inspection: Inspection,
    pub manifest: Option<Manifest>,
    /// The file it came from and the digest of its bytes.
    source: Option<(PathBuf, String)>,
}

impl PluginRequest {
    pub fn name(&self) -> &str {
        &self.inspection.name
    }

    /// What the manifest asks for.
    pub fn requested(&self) -> Grants {
        self.manifest
            .as_ref()
            .map(|m| m.permissions.clone())
            .unwrap_or_default()
    }
}

/// When a plugin file was last changed, to notice rebuilds.
type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Where a plugin was loaded from.
struct Source {
    path: PathBuf,
    loaded: Option<Stamp>,
    /// A change seen at the last poll; reloaded once it stops changing.
    changing: Option<Stamp>,
}

/// A plugin that hasn't started: stand-ins from its summary are
/// registered, and it starts when one of its commands runs or an event it
/// subscribed to happens.
struct Dormant {
    grants: Grants,
    summary: Summary,
    /// Modes that start it once one becomes a buffer's (a declaration's
    /// `on.modes`, or the modes its summary recorded).
    wake_modes: Vec<String>,
    /// File extensions that start it when such a file is opened (a
    /// declaration's `on.extensions`, or the summary's modes' extensions).
    wake_extensions: Vec<String>,
}

impl Dormant {
    /// Does this buffer bring the plugin out of dormancy? On its major mode
    /// (or one it derives from), one of its minor modes, or the file's
    /// extension.
    fn woken_by(&self, editor: &Editor, id: BufferId) -> bool {
        let Some(b) = editor.state.get_buffer(id) else {
            return false;
        };
        if self
            .wake_modes
            .iter()
            .any(|m| editor.modes.derives_from(b.major_mode(), m) || b.minor_modes().contains(m))
        {
            return true;
        }
        b.path()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .is_some_and(|ext| self.wake_extensions.iter().any(|e| e == ext))
    }
}

/// The modes a summary says the plugin defines, and their extensions.
fn summary_wake(summary: &Summary) -> (Vec<String>, Vec<String>) {
    let modes = summary.modes.iter().map(|m| m.name.clone()).collect();
    let extensions = summary
        .modes
        .iter()
        .flat_map(|m| m.extensions.iter().cloned())
        .collect();
    (modes, extensions)
}

/// A declaration's names, plus what the plugin's manifest says it
/// provides, without duplicates.
fn merge(declared: &[String], manifest: Option<&Vec<String>>) -> Vec<String> {
    let mut out = declared.to_vec();
    for name in manifest.into_iter().flatten() {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// Take back everything a plugin registered: its commands, modes, key
/// bindings, menus, decorations and cross-references. A binding is only
/// unbound if it is still the one the plugin made.
fn cleanup_plugin(
    editor: &mut Editor,
    name: &str,
    bindings: &[(Scope, String, String)],
    remaps: &[(Scope, String, String)],
) {
    for (scope, keys, command) in bindings {
        if editor.keymaps.get(scope).and_then(|m| m.get(keys)) == Some(command.as_str()) {
            let _ = editor.keymaps.unbind(scope, keys);
        }
    }
    for (scope, from, _) in remaps {
        editor.keymaps.unremap(scope, from);
    }
    editor.commands.remove_plugin(name);
    editor.modes.remove_owned_by(name);
    editor.menus.remove_plugin(name);
    editor.completion.remove_plugin(name);
    if editor.themes.remove_owner(name) {
        editor.theme.clear();
    }
    editor.idle_hooks.retain(|h| h.plugin != name);
    editor.xrefs.retain(|(owner, _)| owner.as_str() != name);
    for b in editor.state.buffers.values_mut() {
        b.clear_decorations(name);
        b.clear_plugin_images(name);
        b.mode_line_info.remove(name);
    }
}

pub struct PluginHost {
    runtime: PluginRuntime,
    plugins: BTreeMap<String, LoadedPlugin>,
    dormant: BTreeMap<String, Dormant>,
    sources: BTreeMap<String, Source>,
    tracker: EventTracker,
    /// The tracker has seen the editor (events are relative to that).
    tracking: bool,
    /// Where plugin summaries are kept (None: no lazy starting).
    summaries: Option<PathBuf>,
    /// Plugins running a background call.
    away: BTreeMap<String, Away>,
}

impl PluginHost {
    pub fn new() -> Result<Self> {
        Self::with_cache_dir(None)
    }

    /// With a cache directory, compiled plugins are kept in `DIR/wasmtime`
    /// and summaries in `DIR/plugins`, so unchanged plugins start fast or
    /// lazily. The directory is only a cache: deleting it is always safe.
    pub fn with_cache_dir(dir: Option<&Path>) -> Result<Self> {
        Ok(Self {
            runtime: PluginRuntime::with_cache(dir.map(|d| d.join("wasmtime")).as_deref())?,
            plugins: BTreeMap::new(),
            dormant: BTreeMap::new(),
            sources: BTreeMap::new(),
            tracker: EventTracker::default(),
            tracking: false,
            summaries: dir.map(|d| d.join("plugins")),
            away: BTreeMap::new(),
        })
    }

    /// Every plugin, started or not.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let mut all: Vec<&str> = self
            .plugins
            .keys()
            .chain(self.dormant.keys())
            .chain(self.away.keys())
            .map(String::as_str)
            .collect();
        all.sort();
        all.into_iter()
    }

    /// A started plugin.
    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    /// Loaded but not started yet (see [`PluginHost::load_dormant`]).
    pub fn is_dormant(&self, name: &str) -> bool {
        self.dormant.contains_key(name)
    }

    /// The grants a not-yet-started plugin will get.
    pub fn dormant_grants(&self, name: &str) -> Option<&Grants> {
        self.dormant.get(name).map(|d| &d.grants)
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty() && self.dormant.is_empty() && self.away.is_empty()
    }

    /// The file a plugin was loaded from.
    pub fn source(&self, name: &str) -> Option<&Path> {
        self.sources.get(name).map(|s| s.path.as_path())
    }

    /// Some plugin came from a file (so there's something to watch).
    pub fn watching_files(&self) -> bool {
        !self.sources.is_empty()
    }

    /// Plugin files that were rebuilt since they were loaded, as (name,
    /// path). Call about once a second: a file must look the same at two
    /// calls in a row, so a half-written file isn't loaded. Each change is
    /// reported once.
    pub fn changed_files(&mut self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        for (name, s) in &mut self.sources {
            let now = stamp(&s.path);
            if now.is_none() || now == s.loaded {
                s.changing = None;
            } else if now == s.changing {
                s.loaded = now;
                s.changing = None;
                out.push((name.clone(), s.path.clone()));
            } else {
                s.changing = now;
            }
        }
        out
    }

    /// Compile a plugin file without running anything: the first step of
    /// loading, so the frontend can show what the plugin wants and ask.
    pub fn inspect(&self, file: PluginFile) -> Result<PluginRequest> {
        let path = file.path.clone();
        let inspection = self
            .runtime
            .inspect(&file.name, &file.bytes)
            .map_err(|e| e.context(format!("loading {}", path.display())))?;
        let digest = summary::digest(&file.bytes);
        Ok(PluginRequest {
            inspection,
            manifest: file.manifest,
            source: Some((path, digest)),
        })
    }

    /// Read and compile a plugin file (see [`PluginFile::read`]).
    pub fn inspect_file(&self, path: &Path) -> Result<PluginRequest> {
        self.inspect(PluginFile::read(path)?)
    }

    /// Does this component use a `forge:editor` function this Forge does
    /// not have? The component is compiled and type-checked against the
    /// linker; no plugin code runs.
    pub fn compatibility_problem(&self, name: &str, bytes: &[u8]) -> Result<Option<String>> {
        let inspection = self.runtime.inspect(name, bytes)?;
        Ok(self.runtime.compatibility_problem(&inspection))
    }

    fn start_tracking(&mut self, editor: &Editor) {
        if !self.tracking {
            // Start watching from here; existing buffers aren't "opened".
            self.tracker.scan(editor, true);
            self.tracking = true;
        }
    }

    /// Load a plugin without starting it, if its summary from last time
    /// matches (same file, same grants) and says that's safe. `grants`
    /// must cover what it asks for. Otherwise hands the file back, to be
    /// loaded normally.
    pub fn load_dormant(
        &mut self,
        editor: &mut Editor,
        file: PluginFile,
        grants: &Grants,
    ) -> std::result::Result<(), Box<PluginFile>> {
        let startup = file.manifest.as_ref().is_some_and(|m| m.startup);
        let Some(dir) = self
            .summaries
            .as_ref()
            .filter(|_| !startup && grants.missing(&file.requested()).is_empty())
        else {
            return Err(Box::new(file));
        };
        let key = summary::key(&summary::digest(&file.bytes), grants);
        let Some(summary) = Summary::load(dir, &file.name).filter(|s| s.lazy && s.key == key)
        else {
            return Err(Box::new(file));
        };
        let name = file.name.clone();
        self.unload(editor, &name);
        self.start_tracking(editor);
        summary.install(editor, &name);
        let (wake_modes, wake_extensions) = summary_wake(&summary);
        self.dormant.insert(
            name.clone(),
            Dormant {
                grants: grants.clone(),
                summary,
                wake_modes,
                wake_extensions,
            },
        );
        self.sources.insert(
            name,
            Source {
                loaded: stamp(&file.path),
                path: file.path,
                changing: None,
            },
        );
        Ok(())
    }

    /// Load a plugin without starting it, registering stand-ins only for
    /// the commands a declaration names (`use_package`'s `on`): it
    /// starts when one of them runs, one of its modes becomes a buffer's,
    /// or a file with one of its extensions is opened. Unlike
    /// [`PluginHost::load_dormant`] this doesn't need a summary from a
    /// previous run — the triggers come from the declaration, and from the
    /// plugin's own manifest (`commands`/`modes`/`extensions`).
    pub fn load_lazy(
        &mut self,
        editor: &mut Editor,
        file: PluginFile,
        grants: &Grants,
        commands: &[String],
        modes: &[String],
        extensions: &[String],
    ) -> Result<()> {
        let name = file.name.clone();
        let key = summary::key(&summary::digest(&file.bytes), grants);
        let m = file.manifest.as_ref();
        let manifest_commands = m.map(Manifest::command_names);
        let commands = merge(commands, manifest_commands.as_ref());
        let wake_modes = merge(modes, m.map(|m| &m.modes));
        let wake_extensions = merge(extensions, m.map(|m| &m.extensions));
        let summary = Summary {
            key,
            lazy: true,
            commands: commands
                .iter()
                .map(|c| summary::CommandEntry {
                    name: c.clone(),
                    doc: String::new(),
                })
                .collect(),
            ..Default::default()
        };
        self.unload(editor, &name);
        self.start_tracking(editor);
        summary.install(editor, &name);
        self.dormant.insert(
            name.clone(),
            Dormant {
                grants: grants.clone(),
                summary,
                wake_modes,
                wake_extensions,
            },
        );
        self.sources.insert(
            name,
            Source {
                loaded: stamp(&file.path),
                path: file.path,
                changing: None,
            },
        );
        Ok(())
    }

    /// Start a dormant plugin: replace its stand-ins with the real thing.
    fn activate(&mut self, editor: &mut Editor, name: &str, out: &mut Outcome) -> bool {
        let Some(d) = self.dormant.remove(name) else {
            return self.plugins.contains_key(name);
        };
        d.summary.uninstall(editor, name);
        let Some(path) = self.sources.get(name).map(|s| s.path.clone()) else {
            return false;
        };
        let request = match self.inspect_file(&path) {
            Ok(r) => r,
            Err(e) => {
                out.errors.push(format!("plugin {name}: {e:#}"));
                return false;
            }
        };
        let (r, more) = self.load_request(editor, request, &d.grants);
        out.merge(more);
        match r {
            Ok(()) => true,
            Err(e) => {
                out.errors.push(format!("{e:#}"));
                false
            }
        }
    }

    /// Instantiate an inspected plugin with `grants` and run its init.
    /// Loading a name that is already loaded replaces it.
    pub fn load_request(
        &mut self,
        editor: &mut Editor,
        request: PluginRequest,
        grants: &Grants,
    ) -> (Result<()>, Outcome) {
        let mut out = Outcome::default();
        let name = request.inspection.name.clone();
        self.unload(editor, &name);
        // Remember the file even if loading fails: a fixed rebuild then
        // loads by itself (see changed_files).
        if let Some((path, _)) = &request.source {
            self.sources.insert(
                name.clone(),
                Source {
                    loaded: stamp(path),
                    path: path.clone(),
                    changing: None,
                },
            );
        }
        let source = self.sources.get(&name).map(|s| (s.path.clone(), s.loaded));
        let plugin = match self.runtime.instantiate(request.inspection, grants) {
            Ok(p) => p,
            Err(e) => return (Err(e), out),
        };
        self.start_tracking(editor);
        self.plugins.insert(name.clone(), plugin);
        let buffers = editor.buffer_ids().len();
        let r = self.call(editor, &name, &mut out, |p| p.call_init());
        if let (Some(Ok(())), Some((_, digest))) = (&r, &request.source) {
            // Only registering things: next time it can start lazily.
            let quiet = out.effects.is_empty()
                && out.errors.is_empty()
                && editor.buffer_ids().len() == buffers;
            self.save_summary(editor, &name, summary::key(digest, grants), quiet);
        }
        let result = match r {
            Some(Ok(())) => Ok(()),
            Some(Err(msg)) => {
                self.unload(editor, &name);
                Err(anyhow::anyhow!("plugin {name}: init failed: {msg}"))
            }
            None => Err(anyhow::anyhow!(
                out.errors
                    .pop()
                    .unwrap_or_else(|| format!("plugin {name} crashed"))
            )),
        };
        if let Some((path, loaded)) = source {
            self.sources.entry(name).or_insert(Source {
                path,
                loaded,
                changing: None,
            });
        }
        (result, out)
    }

    /// Load a file with the given grants (no asking): inspect + load_request.
    pub fn load_file(
        &mut self,
        editor: &mut Editor,
        path: &Path,
        grants: &Grants,
    ) -> (Result<String>, Outcome) {
        match self.inspect_file(path) {
            Ok(req) => {
                let name = req.inspection.name.clone();
                let (r, out) = self.load_request(editor, req, grants);
                (r.map(|()| name), out)
            }
            Err(e) => (Err(e), Outcome::default()),
        }
    }

    fn save_summary(&self, editor: &Editor, name: &str, key: String, lazy: bool) {
        let (Some(dir), Some(p)) = (&self.summaries, self.plugins.get(name)) else {
            return;
        };
        let st = p.store.data();
        let events = (0..EVENT_KINDS).filter(|&k| st.subscriptions[k]).collect();
        let summary = Summary::record(editor, name, key, &st.bindings, &st.remaps, events, lazy);
        // Only a cache: failing to write it just means no lazy start.
        let _ = summary.save(dir, name);
    }

    /// Load from bytes with no grants (tests and tools).
    pub fn load(&mut self, editor: &mut Editor, name: &str, bytes: &[u8]) -> (Result<()>, Outcome) {
        self.load_with(editor, name, bytes, &Grants::default())
    }

    pub fn load_with(
        &mut self,
        editor: &mut Editor,
        name: &str,
        bytes: &[u8],
        grants: &Grants,
    ) -> (Result<()>, Outcome) {
        match self.runtime.inspect(name, bytes) {
            Ok(inspection) => self.load_request(
                editor,
                PluginRequest {
                    inspection,
                    manifest: None,
                    source: None,
                },
                grants,
            ),
            Err(e) => (Err(e), Outcome::default()),
        }
    }

    /// Watch and interrupt plugin calls from another thread.
    pub fn monitor(&self) -> PluginMonitor {
        self.runtime.monitor()
    }

    /// Shorter or longer per-call time limit (applies to plugins loaded after).
    pub fn set_call_time_limit(&mut self, limit: std::time::Duration) {
        self.runtime.set_call_time_limit(limit);
    }

    /// Remove a plugin and everything it registered: commands, modes,
    /// key bindings and remappings it made (unless something else has
    /// replaced them since).
    pub fn unload(&mut self, editor: &mut Editor, name: &str) -> bool {
        if let Some(a) = self.away.get_mut(name) {
            // Busy in the background: unloaded when it's back.
            a.unload = true;
            a.queue.clear();
            return true;
        }
        if let Some(d) = self.dormant.remove(name) {
            d.summary.uninstall(editor, name);
            self.sources.remove(name);
            return true;
        }
        let Some(mut p) = self.plugins.remove(name) else {
            return false;
        };
        self.sources.remove(name);
        let (bindings, remaps) = {
            let st = p.store.data_mut();
            (
                std::mem::take(&mut st.bindings),
                std::mem::take(&mut st.remaps),
            )
        };
        cleanup_plugin(editor, name, &bindings, &remaps);
        true
    }

    /// Lend the editor to `name` for one call, then collect what it left.
    /// Returns None (and unloads the plugin) if it crashed; Some(Err) if
    /// it returned an error.
    fn call(
        &mut self,
        editor: &mut Editor,
        name: &str,
        out: &mut Outcome,
        f: impl FnOnce(&mut LoadedPlugin) -> CallResult,
    ) -> Option<std::result::Result<(), String>> {
        let p = self.plugins.get_mut(name)?;
        p.store.data_mut().editor = Some(std::mem::take(editor));
        let r = f(p);
        let st = p.store.data_mut();
        *editor = st.editor.take().unwrap_or_default();
        out.effects.append(&mut st.effects);
        out.logs
            .extend(st.logs.drain(..).map(|l| (name.to_string(), l)));
        match r {
            Ok(Ok(())) => Some(Ok(())),
            Ok(Err(msg)) => {
                out.errors.push(format!("{name}: {msg}"));
                Some(Err(msg))
            }
            Err(trap) => {
                out.errors
                    .push(format!("plugin {name} crashed and was unloaded: {trap:#}"));
                self.unload(editor, name);
                out.unloaded.push(name.to_string());
                None
            }
        }
    }

    /// Run a plugin command (the frontend checked `editor.commands`).
    pub fn run_command(
        &mut self,
        editor: &mut Editor,
        command: &str,
        prefix: Option<i64>,
    ) -> Outcome {
        let mut out = Outcome::default();
        match editor.commands.plugin_for(command).map(str::to_string) {
            Some(plugin) => {
                if self.away.contains_key(&plugin) || self.activate(editor, &plugin, &mut out) {
                    self.dispatch(
                        editor,
                        &plugin,
                        Queued::Command(command.to_string(), prefix),
                        &mut out,
                    );
                }
            }
            None => out.errors.push(format!("{command} isn't a plugin command")),
        }
        out
    }

    /// The user answered (or cancelled: None) a plugin's question.
    pub fn answer(
        &mut self,
        editor: &mut Editor,
        plugin: &str,
        request: u32,
        answer: Option<&str>,
    ) -> Outcome {
        let mut out = Outcome::default();
        self.dispatch(
            editor,
            plugin,
            Queued::Answer(request, answer.map(String::from)),
            &mut out,
        );
        out
    }

    /// Make a call: in the foreground, in the background (commands and
    /// events the plugin asked for that way), or later if the plugin is
    /// busy in the background.
    fn dispatch(&mut self, editor: &mut Editor, name: &str, call: Queued, out: &mut Outcome) {
        if let Some(a) = self.away.get_mut(name) {
            if !a.unload {
                a.queue.push_back(call);
            }
            return;
        }
        let Some(p) = self.plugins.get(name) else {
            return;
        };
        let background = match &call {
            Queued::Command(c, _) => p.is_background_command(c),
            Queued::Event(e) => p.is_background_event(event_kind(e)),
            Queued::Answer(..) => false,
        };
        if background {
            if let Err(e) = self.start_background(editor, name, call) {
                out.errors
                    .push(format!("{name}: can't run in the background: {e:#}"));
            }
            return;
        }
        match call {
            Queued::Command(c, prefix) => {
                self.call(editor, name, out, |p| p.call_run_command(&c, prefix))
            }
            Queued::Event(e) => self.call(editor, name, out, |p| p.call_on_event(e)),
            Queued::Answer(r, a) => {
                self.call(editor, name, out, |p| p.call_on_answer(r, a.as_deref()))
            }
        };
    }

    /// Send a plugin to a worker thread with a snapshot of the editor.
    fn start_background(&mut self, editor: &Editor, name: &str, call: Queued) -> Result<()> {
        let Some(mut plugin) = self.plugins.remove(name) else {
            anyhow::bail!("plugin {name} is not loaded");
        };
        if let Err(e) = plugin.set_background(true) {
            self.plugins.insert(name.to_string(), plugin);
            return Err(e);
        }
        let subscriptions = plugin.store.data().subscriptions;
        // Kept so the keys it bound can be taken back if it crashes and
        // its store is lost with it.
        let bindings = plugin.store.data().bindings.clone();
        let remaps = plugin.store.data().remaps.clone();
        let snapshot = editor.clone();
        let base = crate::background::base_of(&snapshot);
        let (tx, rx) = mpsc::channel();
        // The plugin goes into the thread through a slot, so a thread that
        // never starts leaves it here to be put back.
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(plugin)));
        let thread_slot = slot.clone();
        let handle = std::thread::Builder::new()
            .name(format!("forge-plugin-{name}"))
            .spawn(move || {
                // A poisoned lock still holds the plugin, so take it out
                // of it rather than losing the plugin.
                let plugin = thread_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let Some(mut plugin) = plugin else {
                    return;
                };
                plugin.store.data_mut().editor = Some(snapshot);
                let result = match call {
                    Queued::Command(c, prefix) => plugin.call_run_command(&c, prefix),
                    Queued::Event(e) => plugin.call_on_event(e),
                    Queued::Answer(r, a) => plugin.call_on_answer(r, a.as_deref()),
                };
                let snapshot = plugin.store.data_mut().editor.take().unwrap_or_default();
                let _ = tx.send(Done {
                    plugin,
                    snapshot,
                    base,
                    result,
                });
            });
        if let Err(e) = handle {
            // The thread never started, so the plugin is still here: put it
            // back, so its commands and keys are not left behind.
            if let Some(p) = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                self.plugins.insert(name.to_string(), p);
            }
            return Err(e.into());
        }
        self.away.insert(
            name.to_string(),
            Away {
                rx,
                queue: VecDeque::new(),
                subscriptions,
                unload: false,
                bindings: bindings.clone(),
                remaps: remaps.clone(),
            },
        );
        Ok(())
    }

    /// A plugin is busy in the background.
    pub fn background_running(&self) -> bool {
        !self.away.is_empty()
    }

    /// Plugins busy in the background.
    pub fn away_names(&self) -> impl Iterator<Item = &str> {
        self.away.keys().map(String::as_str)
    }

    /// Is `name` busy in the background?
    pub fn is_away(&self, name: &str) -> bool {
        self.away.contains_key(name)
    }

    /// Merge in finished background calls (call when idle), then make the
    /// calls that waited for them.
    pub fn poll_background(&mut self, editor: &mut Editor) -> Outcome {
        let mut out = Outcome::default();
        let names: Vec<String> = self.away.keys().cloned().collect();
        for name in names {
            let got = self.away[&name].rx.try_recv();
            let done = match got {
                Ok(d) => d,
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The worker thread died, so the plugin's store went
                    // with it: take back everything it had registered.
                    let Some(away) = self.away.remove(&name) else {
                        continue;
                    };
                    self.sources.remove(&name);
                    cleanup_plugin(editor, &name, &away.bindings, &away.remaps);
                    out.errors.push(format!(
                        "plugin {name} crashed in the background and was unloaded"
                    ));
                    out.unloaded.push(name);
                    continue;
                }
            };
            let Some(away) = self.away.remove(&name) else {
                continue;
            };
            let Done {
                mut plugin,
                snapshot,
                base,
                result,
            } = done;
            let _ = plugin.set_background(false);
            let st = plugin.store.data_mut();
            let touched = std::mem::take(&mut st.touched_decorations);
            let touched_images = std::mem::take(&mut st.touched_images);
            out.effects.append(&mut st.effects);
            out.logs
                .extend(st.logs.drain(..).map(|l| (name.clone(), l)));
            self.plugins.insert(name.clone(), plugin);
            match result {
                Ok(r) => {
                    for problem in crate::background::merge(
                        editor,
                        &snapshot,
                        &base,
                        &name,
                        &touched,
                        &touched_images,
                    ) {
                        out.errors.push(problem);
                    }
                    if let Err(msg) = r {
                        out.errors.push(format!("{name}: {msg}"));
                    }
                }
                Err(trap) => {
                    out.errors
                        .push(format!("plugin {name} crashed and was unloaded: {trap:#}"));
                    self.unload(editor, &name);
                    out.unloaded.push(name);
                    continue;
                }
            }
            if away.unload {
                self.unload(editor, &name);
                continue;
            }
            let mut queue = away.queue;
            while let Some(call) = queue.pop_front() {
                self.dispatch(editor, &name, call, &mut out);
                if let Some(a) = self.away.get_mut(&name) {
                    // Off to the background again: the rest wait for that.
                    a.queue.append(&mut queue);
                    break;
                }
            }
        }
        out
    }

    /// Tell subscribed plugins what changed since last time. `idle`: the
    /// editor has been idle, so deliver buffer-changed too.
    pub fn notice_changes(&mut self, editor: &mut Editor, idle: bool) -> Outcome {
        let mut out = Outcome::default();
        if self.is_empty() {
            return out;
        }
        let mut events = self.tracker.scan(editor, false);
        if idle {
            events.extend(self.tracker.take_changed());
        }
        // A dormant plugin declared (or recorded) that it wants a mode or a
        // file extension: start it if any buffer matches. This considers
        // every buffer, not just this round's events, so a declared plugin
        // still starts for a file named on the command line (opened before
        // the plugin was loaded, so it produced no buffer-opened event).
        let waking: Vec<String> = self
            .dormant
            .iter()
            .filter(|(_, d)| {
                editor
                    .buffer_ids()
                    .into_iter()
                    .any(|id| d.woken_by(editor, id))
            })
            .map(|(n, _)| n.clone())
            .collect();
        for name in waking {
            self.activate(editor, &name, &mut out);
        }
        if events.is_empty() {
            return out;
        }
        for e in events {
            let waking: Vec<String> = self
                .dormant
                .iter()
                .filter(|(_, d)| d.summary.events.contains(&event_kind(&e)))
                .map(|(n, _)| n.clone())
                .collect();
            for name in waking {
                self.activate(editor, &name, &mut out);
            }
            let mut names: Vec<String> = self
                .plugins
                .iter()
                .filter(|(_, p)| p.subscribed(event_kind(&e)))
                .map(|(n, _)| n.clone())
                .collect();
            names.extend(
                self.away
                    .iter()
                    .filter(|(_, a)| a.subscriptions[event_kind(&e)])
                    .map(|(n, _)| n.clone()),
            );
            for name in names {
                let mut one = Outcome::default();
                self.dispatch(editor, &name, Queued::Event(e), &mut one);
                out.merge(one);
            }
        }
        // Whatever the handlers changed isn't announced again.
        self.tracker.scan(editor, true);
        out
    }

    /// Never report events for this buffer.
    pub fn ignore_buffer(&mut self, id: BufferId) {
        self.tracker.ignored.insert(id);
        self.tracker.snaps.remove(&id);
        self.tracker.changed.remove(&id);
    }

    /// Put plugins' key bindings and remappings back after the keymaps were
    /// reset (reloading init.rhai). User bindings go on top afterwards.
    pub fn reapply_bindings(&self, editor: &mut Editor) {
        for p in self.plugins.values() {
            let st = p.store.data();
            for (scope, keys, command) in &st.bindings {
                let seq: Vec<String> = keys.split(' ').map(String::from).collect();
                let _ =
                    editor
                        .keymaps
                        .bind_seq(scope, &seq, command, crate::keymap::Origin::Package);
            }
            for (scope, from, to) in &st.remaps {
                editor.keymaps.remap(scope, from, to);
            }
        }
    }

    /// Some plugin has a process running.
    pub fn processes_running(&self) -> bool {
        self.plugins.values().any(LoadedPlugin::has_processes)
    }

    /// Deliver processes' output and exits to the plugins that started
    /// them (called when idle, like buffer-changed).
    pub fn poll_processes(&mut self, editor: &mut Editor) -> Outcome {
        let mut out = Outcome::default();
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|(_, p)| p.has_processes())
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            let Some(p) = self.plugins.get_mut(&name) else {
                continue;
            };
            let events = p.poll_processes();
            let logs: Vec<LogLine> = std::mem::take(&mut p.store.data_mut().logs);
            out.logs.extend(logs.into_iter().map(|l| (name.clone(), l)));
            for ev in events {
                if self
                    .call(editor, &name, &mut out, |p| p.call_process_event(ev))
                    .is_none()
                {
                    break; // crashed and unloaded
                }
            }
        }
        out
    }

    /// Some plugin is waiting on an asynchronous HTTP request.
    pub fn http_running(&self) -> bool {
        self.plugins.values().any(LoadedPlugin::has_http)
    }

    /// Deliver answers to asynchronous HTTP requests to the plugins that
    /// made them (called when idle, like `poll_processes`).
    pub fn poll_http(&mut self, editor: &mut Editor) -> Outcome {
        let mut out = Outcome::default();
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|(_, p)| p.has_http())
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            let Some(p) = self.plugins.get_mut(&name) else {
                continue;
            };
            let events = p.poll_http_events();
            for ev in events {
                if self
                    .call(editor, &name, &mut out, |p| p.call_http_event(ev))
                    .is_none()
                {
                    break; // crashed and unloaded
                }
            }
        }
        out
    }

    /// Annotations for a completion list from the plugin that annotates
    /// `category` (empty if none, or it failed).
    pub fn annotate(
        &mut self,
        editor: &mut Editor,
        category: &str,
        candidates: &[String],
    ) -> (Vec<String>, Outcome) {
        let mut out = Outcome::default();
        let Some(plugin) = editor.completion.annotators.get(category).cloned() else {
            return (Vec::new(), out);
        };
        if self.away.contains_key(&plugin) || !self.activate(editor, &plugin, &mut out) {
            return (Vec::new(), out);
        }
        let mut notes = Vec::new();
        self.call(editor, &plugin, &mut out, |p| {
            let (r, n) = p.call_annotate(category, candidates);
            notes = n;
            r
        });
        notes.resize(candidates.len(), String::new());
        (notes, out)
    }

    /// Group names for a completion list from the plugin that groups
    /// `category` (empty if none, or it failed).
    pub fn group(
        &mut self,
        editor: &mut Editor,
        category: &str,
        candidates: &[String],
    ) -> (Vec<String>, Outcome) {
        let mut out = Outcome::default();
        let Some(plugin) = editor.completion.groupers.get(category).cloned() else {
            return (Vec::new(), out);
        };
        if self.away.contains_key(&plugin) || !self.activate(editor, &plugin, &mut out) {
            return (Vec::new(), out);
        }
        let mut groups = Vec::new();
        self.call(editor, &plugin, &mut out, |p| {
            let (r, g) = p.call_group(category, candidates);
            groups = g;
            r
        });
        groups.resize(candidates.len(), String::new());
        (groups, out)
    }

    /// Ask a completion source for candidates at `point` in `buffer`
    /// (they come back as Effect::Completions, now or later).
    pub fn complete(
        &mut self,
        editor: &mut Editor,
        source: &str,
        request: u32,
        buffer: BufferId,
        point: crate::Pos,
    ) -> Outcome {
        let mut out = Outcome::default();
        let Some(plugin) = editor
            .completion
            .sources
            .iter()
            .find(|s| s.name == source)
            .map(|s| s.plugin.clone())
        else {
            return out;
        };
        if self.away.contains_key(&plugin) || !self.activate(editor, &plugin, &mut out) {
            return out;
        }
        self.call(editor, &plugin, &mut out, |p| {
            p.call_complete(request, source, buffer.0, point)
        });
        out
    }

    /// Tell every loaded completion provider that `candidate` was committed
    /// with `status` (the completion-handler.commit exit callback).
    pub fn commit(&mut self, editor: &mut Editor, candidate: &str, status: &str) -> Outcome {
        let mut out = Outcome::default();
        let providers: Vec<String> = self
            .plugins
            .iter()
            .filter(|(_, p)| p.provides_completion())
            .map(|(n, _)| n.clone())
            .collect();
        for name in providers {
            if self.away.contains_key(&name) || !self.activate(editor, &name, &mut out) {
                continue;
            }
            self.call(editor, &name, &mut out, |p| {
                p.call_commit(candidate, status);
                Ok(Ok(()))
            });
        }
        out
    }

    /// Ask the loaded completion sources where the selected candidate is
    /// defined (corfu-info-location). A source answers in its own time.
    pub fn locate(&mut self, editor: &mut Editor) -> Outcome {
        let mut out = Outcome::default();
        let Some(selection) = editor.selection.clone() else {
            return out;
        };
        let providers: Vec<String> = self
            .plugins
            .iter()
            .filter(|(_, p)| p.provides_completion())
            .map(|(n, _)| n.clone())
            .collect();
        let selection = wit_selection(&selection);
        for name in providers {
            if self.away.contains_key(&name) || !self.activate(editor, &name, &mut out) {
                continue;
            }
            let selection = selection.clone();
            self.call(editor, &name, &mut out, |p| {
                p.call_resolve_location(selection);
                Ok(Ok(()))
            });
        }
        out
    }

    /// Buffer-changed events are waiting for idle time.
    pub fn changes_pending(&self) -> bool {
        !self.tracker.changed.is_empty()
            && (self.plugins.values().any(|p| p.subscribed(2))
                || self.dormant.values().any(|d| d.summary.events.contains(&2))
                || self.away.values().any(|a| a.subscriptions[2]))
    }
}

#[cfg(test)]
mod tests {
    //! A whole plugin, written in WAT against the canonical ABI, driven
    //! through PluginHost with a real Editor.

    use super::*;
    use crate::Pos;
    use crate::keymap::{Lookup, Scope};
    use crate::plugin::runtime::{Level, Question};
    use wit_component::{ComponentEncoder, StringEncoding};
    use wit_parser::Resolve;

    fn component(wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/wit-0.1.0/forge-editor.wit"
        );
        let pkg = resolve.push_file(root).unwrap();
        let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    /// A component for WIT 0.1.1 (decorations, options), frozen in
    /// tests/fixtures like 0.1.0.
    fn component_current(wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/wit-0.1.1/forge-editor.wit"
            ))
            .unwrap();
        let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    /// A component for WIT 0.1.2 (background calls), frozen in
    /// tests/fixtures.
    fn component_now(wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/wit-0.1.2/forge-editor.wit"
            ))
            .unwrap();
        let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    #[test]
    fn background_commands_run_on_a_snapshot_and_merge_back() {
        let mut ed = editor();
        let id = ed.create_buffer("t");
        ed.state.get_buffer_mut(id).unwrap().set_contents("hello\n");
        ed.show_buffer(id);
        let mut host = PluginHost::new().unwrap();
        host.load(
            &mut ed,
            "bg",
            &component_now(include_str!("../../../../tests/fixtures/bg-plugin.wat")),
        )
        .0
        .unwrap();

        let out = host.run_command(&mut ed, "bg-insert", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(host.is_away("bg") && host.background_running());
        assert_eq!(host.away_names().collect::<Vec<_>>(), ["bg"]);
        // Another call waits its turn; the user keeps typing.
        assert!(host.run_command(&mut ed, "bg-bind", None).errors.is_empty());
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .replace_offsets(6, 6, "world");

        let mut logs = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while host.background_running() && std::time::Instant::now() < deadline {
            let out = host.poll_background(&mut ed);
            assert!(out.errors.is_empty(), "{:?}", out.errors);
            logs.extend(out.logs.into_iter().map(|(_, l)| l.message));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!host.background_running());
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.get_contents(), "[bg]hello\nworld");
        let d = &b.decorations()[0];
        assert_eq!((d.owner.as_str(), d.start, d.end), ("bg", 0, 4));
        assert_eq!(logs[0], "done");
        assert!(
            logs[1].contains("keymap.bind isn't available in a background call"),
            "{logs:?}"
        );
        assert!(ed.keymaps.global.get("<f6>").is_none());

        // Unloading while it's away waits for it to come back.
        host.run_command(&mut ed, "bg-insert", None);
        assert!(host.unload(&mut ed, "bg"));
        while host.background_running() {
            host.poll_background(&mut ed);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!ed.commands.contains("bg-insert"));
    }

    #[test]
    fn a_plugin_reads_its_config() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load(
            &mut ed,
            "cfg",
            &component_0_1_19(include_str!("../../../../tests/fixtures/config-plugin.wat")),
        )
        .0
        .unwrap();
        let out = host.run_command(&mut ed, "config-run", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(messages(&out), ["{}"], "no config: an empty object");

        ed.plugin_config
            .insert("cfg".into(), r#"{"star":["a"]}"#.into());
        ed.plugin_config.insert("other".into(), r#"{"x":1}"#.into());
        let out = host.run_command(&mut ed, "config-run", None);
        assert_eq!(
            messages(&out),
            [r#"{"star":["a"]}"#],
            "its own, not another plugin's"
        );
    }

    #[test]
    fn region_maps_from_a_plugin() {
        let mut ed = editor();
        let id = ed.create_buffer("x.rs");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_contents("hello world\n");
        ed.show_buffer(id);
        let mut host = PluginHost::new().unwrap();
        host.load(
            &mut ed,
            "region",
            &component_0_1_19(include_str!("../../../../tests/fixtures/region-plugin.wat")),
        )
        .0
        .unwrap();

        let out = host.run_command(&mut ed, "region-run", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(messages(&out), ["bound"]);
        // The plugin's region is on the first five characters, so a key
        // bound in it applies there and nowhere else.
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.decoration_keymaps_at(2), ["test-word"]);
        let scope = Scope::Region("test-word".into());
        assert_eq!(
            ed.keymaps.get(&scope).and_then(|m| m.get("<mouse-1>")),
            Some("kill-line")
        );
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_point(crate::Pos::new(0, 2));
        let maps = ed.active_keymaps();
        assert_eq!(
            maps.lookup(&["<mouse-1>".to_string()]),
            Lookup::Command {
                command: "kill-line".into(),
                scope: scope.clone(),
                remapped_from: None,
            },
            "the click resolves in the region's own map"
        );
        drop(maps);
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_point(crate::Pos::new(0, 7));
        assert_eq!(
            ed.active_keymaps().lookup(&["<mouse-1>".to_string()]),
            Lookup::Undefined,
            "outside the text, nothing is bound for a click"
        );

        // A key that isn't key notation is refused, and nothing changed.
        let out = host.run_command(&mut ed, "region-oops", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(
            messages(&out)[0].contains("not valid key notation"),
            "{:?}",
            out.logs
        );
        // Setting the regions again replaces them rather than adding to
        // them, and leaves a face the plugin painted where it was.
        let out = host.run_command(&mut ed, "region-run", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(messages(&out), ["bound"]);
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.decorations().len(), 1, "not one per call");
        assert_eq!(b.decoration_keymaps_at(2), ["test-word"]);

        // Unloading takes the regions and the bindings away with it.
        assert!(host.unload(&mut ed, "region"));
        let b = ed.state.get_buffer(id).unwrap();
        assert!(b.decorations().is_empty());
        assert!(
            ed.keymaps
                .get(&scope)
                .and_then(|m| m.get("<mouse-1>"))
                .is_none(),
            "and so is the binding in it"
        );
    }

    /// A component for the WIT this build ships, frozen in
    /// tests/fixtures like the older ones.
    #[allow(dead_code)]
    fn component_latest(wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../wit/forge-editor.wit"
            ))
            .unwrap();
        let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    /// A component for WIT 0.1.19, frozen in tests/fixtures: it must
    /// still load in a newer Forge (semver-compatible imports).
    fn component_0_1_19(wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let pkg = resolve
            .push_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/wit-0.1.19/forge-editor.wit"
            ))
            .unwrap();
        let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    #[test]
    fn options_indentation_and_decorations_from_a_plugin() {
        let mut ed = editor();
        let id = ed.create_buffer("x.rs");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_contents("fn a() {\nx;\n}\n");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_major_mode("rust-mode");
        ed.show_buffer(id);
        let mut host = PluginHost::new().unwrap();
        // Built against 0.1.1 while the demo plugin (other tests) is 0.1.0:
        // both load in a 0.1.2 Forge.
        host.load(
            &mut ed,
            "deco",
            &component_current(include_str!("../../../../tests/fixtures/deco-plugin.wat")),
        )
        .0
        .unwrap();
        host.load(&mut ed, "demo", &component(DEMO)).0.unwrap();

        let out = host.run_command(&mut ed, "deco-run", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(messages(&out), ["indented", "keyword"]);
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.option("line-numbers").as_deref(), Some("relative"));
        assert_eq!(b.line(1), "    x;");
        let d = &b.decorations()[0];
        assert_eq!(
            (d.owner.as_str(), d.start, d.end, d.face),
            ("deco", 0, 2, crate::decoration::DecoFace::Error)
        );
        assert_eq!(b.decoration_message_at(1), Some("bad"));

        let out = host.run_command(&mut ed, "deco-bad-face", None);
        assert!(
            messages(&out)[0].contains("unknown face \"red\""),
            "{:?}",
            out.logs
        );
        assert_eq!(
            ed.state.get_buffer(id).unwrap().decorations().len(),
            1,
            "a failed set changes nothing"
        );

        // Unloading takes its decorations away.
        host.unload(&mut ed, "deco");
        assert!(ed.state.get_buffer(id).unwrap().decorations().is_empty());
    }

    #[test]
    fn environment_reads_need_a_grant_but_system_facts_dont() {
        // Built against the current WIT, whose `plugin` world imports
        // `forge:editor/environment`. init reads a variable (gated) and a
        // system fact (not gated), logging each result.
        let wat = r#"(module
            (import "forge:editor/log@0.1.19" "log" (func $log (param i32 i32 i32)))
            (import "forge:editor/environment@0.1.19" "get" (func $get (param i32 i32 i32)))
            (import "forge:editor/environment@0.1.19" "system-type" (func $sys (param i32)))
            (global $heap (mut i32) (i32.const 4096))
            (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $p i32)
              (local.set $p (i32.and
                (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
                (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
              (global.set $heap (i32.add (local.get $p) (local.get 3)))
              (local.get $p))
            (memory (export "memory") 1)
            (data (i32.const 100) "PATH")
            (data (i32.const 140) "allowed")
            (data (i32.const 150) "denied")
            (data (i32.const 160) "system-ok")
            (data (i32.const 170) "system-bad")
            (func (export "forge:editor/handler@0.1.19#run-command") (param i32 i32 i32 i64) (result i32)
              (i32.store8 (i32.const 0) (i32.const 0)) (i32.const 0))
            (func (export "forge:editor/handler@0.1.19#on-event") (param i32 i32))
            (func (export "forge:editor/handler@0.1.19#on-answer") (param i32 i32 i32 i32))
            (func (export "init") (result i32)
              ;; get("PATH") -> result<option<string>, string>, ret area at 300
              (call $get (i32.const 100) (i32.const 4) (i32.const 300))
              (if (i32.eqz (i32.load8_u (i32.const 300)))
                (then (call $log (i32.const 1) (i32.const 140) (i32.const 7)))
                (else (call $log (i32.const 1) (i32.const 150) (i32.const 6))))
              ;; system-type() -> string, ret area at 320: (ptr, len)
              (call $sys (i32.const 320))
              (if (i32.gt_u (i32.load (i32.const 324)) (i32.const 0))
                (then (call $log (i32.const 1) (i32.const 160) (i32.const 9)))
                (else (call $log (i32.const 1) (i32.const 170) (i32.const 10))))
              (i32.store8 (i32.const 0) (i32.const 0))
              (i32.const 0)))"#;

        // No grant: reading a variable is refused; the system fact works.
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        let (r, out) = host.load(&mut ed, "env", &component_0_1_19(wat));
        r.unwrap();
        assert_eq!(messages(&out), ["denied", "system-ok"]);

        // Granted: the same read succeeds (PATH is set in the test env).
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        let grants = Grants {
            environment: true,
            ..Default::default()
        };
        let (r, out) = host.load_with(&mut ed, "env", &component_0_1_19(wat), &grants);
        r.unwrap();
        assert_eq!(messages(&out), ["allowed", "system-ok"]);
    }

    /// "demo": registers demo-greet (asks for a name, then inserts
    /// "Hi, NAME" into the current buffer), binds it to <f5>, defines
    /// demo-mode, subscribes to buffer-opened and buffer-changed and logs
    /// each event's name.
    const DEMO: &str = include_str!("../../../../tests/fixtures/demo-plugin.wat");

    fn editor() -> Editor {
        let mut ed = Editor::new();
        ed.create_buffer("*scratch*");
        ed
    }

    fn messages(out: &Outcome) -> Vec<&str> {
        out.logs.iter().map(|(_, l)| l.message.as_str()).collect()
    }

    #[test]
    fn a_plugin_registers_binds_asks_and_edits() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        let (r, out) = host.load(&mut ed, "demo", &component(DEMO));
        r.unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);

        assert_eq!(ed.commands.plugin_for("demo-greet"), Some("demo"));
        assert_eq!(ed.commands.get("demo-greet").unwrap().doc, "Greet someone.");
        let f5 = vec!["<f5>".to_string()];
        assert!(
            matches!(ed.active_keymaps().lookup(&f5), Lookup::Command { command, .. } if command == "demo-greet")
        );
        assert!(ed.modes.is_minor("demo-mode"));

        // Running the command asks a question (after the call)...
        let out = host.run_command(&mut ed, "demo-greet", Some(4));
        let (plugin, request) = match &out.effects[..] {
            [
                Effect::Ask {
                    plugin,
                    request,
                    question: Question::Text { prompt, .. },
                },
            ] => {
                assert_eq!(prompt, "Name: ");
                (plugin.clone(), *request)
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(plugin, "demo");

        let out = host.answer(&mut ed, "demo", request, Some("Ada"));
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let b = ed.state.active_buffer().unwrap();
        assert_eq!(b.get_contents(), "Hi, Ada");
        assert_eq!(b.point(), Pos::new(0, 7));
        host.answer(&mut ed, "demo", request, None);
        assert_eq!(ed.state.active_buffer().unwrap().get_contents(), "Hi, Ada");
    }

    #[test]
    fn events_are_delivered_and_buffer_changed_waits_for_idle() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load(&mut ed, "demo", &component(DEMO)).0.unwrap();

        ed.state.active_buffer_mut().unwrap().insert_str("x");
        let other = ed.add_buffer(crate::Buffer::new("other"));
        let out = host.notice_changes(&mut ed, false);
        assert_eq!(messages(&out), ["buffer-opened"], "changed waits for idle");
        assert!(host.changes_pending());
        let out = host.notice_changes(&mut ed, true);
        assert_eq!(messages(&out), ["buffer-changed"]);
        assert!(!host.changes_pending());
        assert!(
            out.logs
                .iter()
                .all(|(p, l)| p == "demo" && l.level == Level::Info)
        );

        ed.kill_buffer(other, None);
        assert!(host.notice_changes(&mut ed, true).logs.is_empty());
    }

    #[test]
    fn unloading_takes_back_everything() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load(&mut ed, "demo", &component(DEMO)).0.unwrap();
        assert!(host.unload(&mut ed, "demo"));
        assert!(!ed.commands.contains("demo-greet"));
        assert!(!ed.modes.is_minor("demo-mode"));
        assert!(
            ed.keymaps
                .get(&Scope::Global)
                .unwrap()
                .get("<f5>")
                .is_none()
        );
        // Loading it again works (same names are free again).
        host.load(&mut ed, "demo", &component(DEMO)).0.unwrap();
        assert!(ed.commands.contains("demo-greet"));
    }

    #[test]
    fn a_plugin_can_hang_a_submenu_off_edit() {
        // A guest against this build's WIT, calling menus.extend-submenu
        // with one item (a list of one record through the canonical ABI).
        let wat = r#"(module
      (import "forge:editor/menus@0.1.19" "extend-submenu" (func $sub (param i32 i32 i32 i32 i32 i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "Edit")
      (data (i32.const 110) "Spell")
      (data (i32.const 120) "Check Spelling")
      (data (i32.const 140) "spell-check-buffer")
      (global $heap (mut i32) (i32.const 4096))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p))
      (func (export "init") (result i32)
        (i32.store (i32.const 200) (i32.const 120))
        (i32.store (i32.const 204) (i32.const 14))
        (i32.store (i32.const 208) (i32.const 140))
        (i32.store (i32.const 212) (i32.const 18))
        (call $sub (i32.const 100) (i32.const 4)
                      (i32.const 110) (i32.const 5)
                      (i32.const 200) (i32.const 1)
                      (i32.const 16))
        (i32.const 0))
      (func (export "forge:editor/handler@0.1.19#run-command") (param i32 i32 i32 i64) (result i32) (i32.const 16))
      (func (export "forge:editor/handler@0.1.19#on-event") (param i32 i32))
      (func (export "forge:editor/handler@0.1.19#on-answer") (param i32 i32 i32 i32))
    )"#;
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        let (r, out) = host.load(&mut ed, "spell", &component_0_1_19(wat));
        r.unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let edit = ed.menus.index_of("Edit").expect("Edit is in the bar");
        let spell = ed
            .menus
            .menu(edit)
            .unwrap()
            .items
            .iter()
            .find(|i| i.label == "Spell")
            .expect("Spell is in Edit");
        assert!(spell.has_submenu());
        assert_eq!(spell.owner.as_deref(), Some("spell"));
        assert_eq!(
            spell
                .submenu
                .iter()
                .map(|c| c.command.as_str())
                .collect::<Vec<_>>(),
            ["spell-check-buffer"]
        );
        // Unloading takes the submenu back out again.
        assert!(host.unload(&mut ed, "spell"));
        assert!(
            ed.menus
                .menu(edit)
                .unwrap()
                .items
                .iter()
                .all(|i| i.label != "Spell"),
            "the empty header went away"
        );
    }

    #[test]
    fn a_crashing_plugin_is_unloaded() {
        let crashy = DEMO.replace(
            "(drop (call $read (i32.const 150) (i32.const 6) (i32.const 0) (i32.const 0)))",
            "unreachable",
        );
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load(&mut ed, "demo", &component(&crashy)).0.unwrap();
        let out = host.run_command(&mut ed, "demo-greet", None);
        assert_eq!(out.unloaded, ["demo"]);
        assert!(
            out.errors[0].starts_with("plugin demo crashed and was unloaded"),
            "{:?}",
            out.errors
        );
        assert!(!ed.commands.contains("demo-greet"));
        assert!(host.is_empty());
        assert_eq!(ed.state.active_buffer().unwrap().name(), "*scratch*");
    }

    #[test]
    fn plugins_cannot_take_user_keys_or_builtin_names() {
        // Binds C-c a (5 bytes) instead of <f5>.
        let greedy = DEMO
            .replace(
                r#"(data (i32.const 140) "<f5>")"#,
                r#"(data (i32.const 140) "C-c a")"#,
            )
            .replace(
                "(i32.const 140) (i32.const 4)",
                "(i32.const 140) (i32.const 5)",
            );
        let mut ed = editor();
        ed.commands.register_builtin("demo-greet", "Taken.");
        let mut host = PluginHost::new().unwrap();
        host.load(&mut ed, "demo", &component(&greedy)).0.unwrap();
        // Both calls returned errors to the plugin (which ignores them).
        assert_eq!(ed.commands.plugin_for("demo-greet"), None);
        assert!(
            ed.keymaps
                .get(&Scope::Global)
                .unwrap()
                .get("C-c a")
                .is_none()
        );
    }

    /// DEMO, but demo-greet loops forever.
    fn stuck() -> String {
        DEMO.replace(
            "(drop (call $read (i32.const 150) (i32.const 6) (i32.const 0) (i32.const 0)))",
            "(loop (br 0))",
        )
    }

    #[test]
    fn a_stuck_plugin_is_stopped_by_the_time_limit() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.set_call_time_limit(std::time::Duration::from_millis(200));
        host.load(&mut ed, "demo", &component(&stuck())).0.unwrap();
        let t = std::time::Instant::now();
        let out = host.run_command(&mut ed, "demo-greet", None);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(3),
            "{:?}",
            t.elapsed()
        );
        assert!(
            out.errors[0].contains("stopped: still running after 0.2 seconds"),
            "{:?}",
            out.errors
        );
        assert_eq!(out.unloaded, ["demo"]);
        assert_eq!(
            ed.state.active_buffer().unwrap().name(),
            "*scratch*",
            "the editor came back"
        );
    }

    #[test]
    fn a_watchdog_can_interrupt_a_call_from_another_thread() {
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load(&mut ed, "demo", &component(&stuck())).0.unwrap();
        let monitor = host.monitor();
        assert!(monitor.busy().is_none());
        let watchdog = std::thread::spawn(move || {
            // Wait for the call to start, then press "C-g".
            let busy = loop {
                if let Some(b) = monitor.busy() {
                    break b;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            std::thread::sleep(std::time::Duration::from_millis(50));
            monitor.interrupt();
            busy.0
        });
        let out = host.run_command(&mut ed, "demo-greet", None);
        assert_eq!(watchdog.join().unwrap(), "demo");
        assert!(
            out.errors[0].contains("stopped with C-g"),
            "{:?}",
            out.errors
        );
        assert!(host.monitor().busy().is_none());
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Append a custom section to a component (allowed anywhere).
    fn with_custom_section(mut bytes: Vec<u8>, name: &str, data: &[u8]) -> Vec<u8> {
        fn leb(mut n: usize, out: &mut Vec<u8>) {
            loop {
                let b = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        let mut body = Vec::new();
        leb(name.len(), &mut body);
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(data);
        bytes.push(0);
        leb(body.len(), &mut bytes);
        bytes.extend(body);
        bytes
    }

    #[test]
    fn an_unchanged_plugin_starts_lazily_from_its_summary() {
        let dir = tempdir("lazy");
        let cache = dir.join("cache");
        let wasm = dir.join("demo.wasm");
        std::fs::write(&wasm, component(DEMO)).unwrap();

        // First run: loads normally and leaves a summary behind.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        let (r, _) = host.load_file(&mut ed, &wasm, &Grants::default());
        assert_eq!(r.unwrap(), "demo");
        let summary = Summary::load(&cache.join("plugins"), "demo").expect("summary saved");
        assert!(summary.lazy);
        assert_eq!(summary.events, [0, 2]);
        assert!(cache.join("wasmtime").is_dir(), "compiled code is cached");

        // Different grants: the summary doesn't apply.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        let net = Grants {
            network: true,
            ..Grants::default()
        };
        assert!(
            host.load_dormant(&mut ed, PluginFile::read(&wasm).unwrap(), &net)
                .is_err()
        );

        // Next run: stand-ins only, nothing started.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        assert!(
            host.load_dormant(
                &mut ed,
                PluginFile::read(&wasm).unwrap(),
                &Grants::default()
            )
            .is_ok()
        );
        assert!(host.is_dormant("demo") && host.get("demo").is_none());
        assert_eq!(host.names().collect::<Vec<_>>(), ["demo"]);
        assert_eq!(ed.commands.get("demo-greet").unwrap().doc, "Greet someone.");
        let f5 = vec!["<f5>".to_string()];
        assert!(
            matches!(ed.active_keymaps().lookup(&f5), Lookup::Command { command, .. } if command == "demo-greet")
        );
        assert!(ed.modes.is_minor("demo-mode"));

        // Running its command starts it, then runs the command.
        let out = host.run_command(&mut ed, "demo-greet", None);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(
            matches!(&out.effects[..], [Effect::Ask { .. }]),
            "{:?}",
            out.effects
        );
        assert!(!host.is_dormant("demo") && host.get("demo").is_some());
        assert_eq!(ed.commands.plugin_for("demo-greet"), Some("demo"));

        // An event it subscribed to starts it too.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        host.load_dormant(
            &mut ed,
            PluginFile::read(&wasm).unwrap(),
            &Grants::default(),
        )
        .ok()
        .unwrap();
        assert!(host.notice_changes(&mut ed, true).logs.is_empty());
        ed.create_buffer("notes");
        let out = host.notice_changes(&mut ed, false);
        assert!(host.get("demo").is_some());
        assert!(messages(&out).contains(&"buffer-opened"), "{:?}", out.logs);

        // Unloading a dormant plugin takes its stand-ins away.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        host.load_dormant(
            &mut ed,
            PluginFile::read(&wasm).unwrap(),
            &Grants::default(),
        )
        .ok()
        .unwrap();
        assert!(host.unload(&mut ed, "demo"));
        assert!(ed.commands.get("demo-greet").is_none() && !ed.modes.is_minor("demo-mode"));
        assert!(host.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_declaration_wakes_a_dormant_plugin_on_a_mode_or_extension() {
        let dir = tempdir("wake");
        let wasm = dir.join("demo.wasm");
        std::fs::write(&wasm, component(DEMO)).unwrap();
        let file = || PluginFile::read(&wasm).unwrap();

        // Declared with on.modes: dormant until a buffer enters that mode.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&dir.join("cache"))).unwrap();
        host.load_lazy(
            &mut ed,
            file(),
            &Grants::default(),
            &[],
            &["rust-mode".to_string()],
            &[],
        )
        .unwrap();
        assert!(host.is_dormant("demo"));
        // An unrelated buffer leaves it dormant.
        ed.create_buffer("notes");
        host.notice_changes(&mut ed, false);
        assert!(host.is_dormant("demo"), "no matching mode yet");
        // A buffer in that mode starts it.
        let id = ed.create_buffer("code");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_major_mode("rust-mode");
        host.notice_changes(&mut ed, false);
        assert!(host.get("demo").is_some(), "the mode started it");

        // Declared with on.extensions: dormant until such a file is opened.
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&dir.join("cache"))).unwrap();
        host.load_lazy(
            &mut ed,
            file(),
            &Grants::default(),
            &[],
            &[],
            &["foo".to_string()],
        )
        .unwrap();
        ed.create_buffer("plain");
        host.notice_changes(&mut ed, false);
        assert!(host.is_dormant("demo"), "the extension doesn't match");
        let path = dir.join("note.foo");
        std::fs::write(&path, "hello").unwrap();
        ed.open_file(&path).unwrap();
        host.notice_changes(&mut ed, false);
        assert!(host.get("demo").is_some(), "the extension started it");

        // A file already open when the declaration is resolved (a file
        // named on the command line) still wakes it, with no event.
        let mut ed = editor();
        std::fs::write(&path, "hello").unwrap();
        ed.open_file(&path).unwrap();
        let mut host = PluginHost::with_cache_dir(Some(&dir.join("cache"))).unwrap();
        host.load_lazy(
            &mut ed,
            file(),
            &Grants::default(),
            &[],
            &[],
            &["foo".to_string()],
        )
        .unwrap();
        assert!(host.is_dormant("demo"));
        host.notice_changes(&mut ed, false);
        assert!(host.get("demo").is_some(), "the open file started it");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn embedded_manifests_name_plugins_and_can_ask_for_startup() {
        let dir = tempdir("embedded");
        let wasm = dir.join("x.wasm");
        let manifest = b"name = \"greeter\"\nstartup = true\n[permissions]\nnetwork = true\n";
        std::fs::write(
            &wasm,
            with_custom_section(component(DEMO), "forge-manifest", manifest),
        )
        .unwrap();
        let file = PluginFile::read(&wasm).unwrap();
        assert_eq!(file.name, "greeter");
        assert!(file.requested().network);

        // The manifest next to it wins.
        std::fs::write(dir.join("x.toml"), "name = \"sidecar\"\n").unwrap();
        assert_eq!(PluginFile::read(&wasm).unwrap().name, "sidecar");
        std::fs::remove_file(dir.join("x.toml")).unwrap();

        // It loads (wasmtime ignores the section), and startup = true
        // means it never starts lazily.
        let cache = dir.join("cache");
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        let net = Grants {
            network: true,
            ..Grants::default()
        };
        assert_eq!(host.load_file(&mut ed, &wasm, &net).0.unwrap(), "greeter");
        let mut ed = editor();
        let mut host = PluginHost::with_cache_dir(Some(&cache)).unwrap();
        assert!(
            host.load_dormant(&mut ed, PluginFile::read(&wasm).unwrap(), &net)
                .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_manifest_inside_the_core_module_is_found() {
        // Where the Rust SDK's link_section puts it.
        let wat = DEMO.replacen(
            "(module",
            "(module\n(@custom \"forge-manifest\" \"name = 'inner'\")",
            1,
        );
        assert_ne!(wat, DEMO);
        let m = Manifest::embedded(&component(&wat))
            .unwrap()
            .expect("found");
        assert_eq!(m.name.as_deref(), Some("inner"));
        assert!(Manifest::embedded(&component(DEMO)).unwrap().is_none());
    }

    #[test]
    fn rebuilt_plugin_files_are_noticed_once_they_settle() {
        let dir = tempdir("reload");
        let wasm = dir.join("demo.wasm");
        let bytes = component(DEMO);
        std::fs::write(&wasm, &bytes).unwrap();
        let mut ed = editor();
        let mut host = PluginHost::new().unwrap();
        host.load_file(&mut ed, &wasm, &Grants::default())
            .0
            .unwrap();
        assert_eq!(host.source("demo"), Some(wasm.as_path()));
        assert!(host.changed_files().is_empty());

        // A rebuild (here: the same code with one more custom section).
        std::fs::write(&wasm, with_custom_section(bytes, "note", b"v2")).unwrap();
        assert!(
            host.changed_files().is_empty(),
            "wait until it stops changing"
        );
        assert_eq!(host.changed_files(), [("demo".to_string(), wasm.clone())]);
        assert!(host.changed_files().is_empty(), "reported once");

        host.unload(&mut ed, "demo");
        assert!(host.source("demo").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
