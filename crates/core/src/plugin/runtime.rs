// SPDX-License-Identifier: MIT OR Apache-2.0
//! The plugin runtime: WebAssembly components run in Wasmtime.
//!
//! A plugin is a component targeting the `forge:editor/plugin` world in
//! `wit/forge-editor.wit`. The host side of that API is generated from
//! the WIT by `bindgen!` and implemented here, against the [`Editor`].
//!
//! The editor is *lent* to a plugin's store for the length of each call
//! (see `PluginHost`), so host functions change buffers and keymaps
//! directly. Things that need the frontend - prompts, echo messages,
//! running other commands - are collected as [`Effect`]s and carried out
//! after the call returns.
//!
//! WASI: both WASI 0.2 (`wasi:*@0.2.x`, what most toolchains produce
//! today) and WASI 0.3 (`wasi:*@0.3.0`) are linked, so a component built
//! for either loads. Wasmtime's 0.3 support is still marked experimental.
//! A plugin gets an empty WASI context: no files, no network, no
//! environment, no arguments. Its stdout and stderr are captured and
//! reported as log lines (a Rust plugin's panic message ends up there).
//! Per-plugin capability grants come in Phase 5.
//!
//! Linking WASI 0.3 makes the store async, so calls into a plugin are
//! driven by a small Tokio runtime owned by [`PluginRuntime`]; to the
//! editor they are still ordinary blocking calls.

use crate::app::{BufferId, Editor};
use crate::buffer::{Buffer, Pos};
use crate::decoration::DecoSpec;
use crate::keymap::{Condition, Lookup, Origin, Scope, parse_keys};
use crate::permissions::{Grants, expand_home};
use crate::process::{ProcessEvent, ProcessId, Processes, Program};
use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wasmtime::component::{
    Component, HasSelf, Instance, Linker, Resource, ResourceTable, TypedFunc,
};
use wasmtime::{Config, Engine, EngineWeak, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::FsPerms;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// What a `buffer` resource handle stands for on the host: its rep is
/// the buffer's id.
pub struct BufferHandle;

wasmtime::component::bindgen!({
    path: "../../wit",
    world: "forge:editor/plugin",
    imports: { default: trappable },
    exports: { default: async },
    with: { "forge:editor/buffers.buffer": BufferHandle },
});

use forge::editor::completion::Selection;
pub use forge::editor::events::Event as PluginEvent;
use forge::editor::events::Kind as EventKind;
pub use forge::editor::log::Level;
use forge::editor::menus::{Item, ItemSelection};
use forge::editor::types::{Position, Span};

/// How much a plugin may write to stdout/stderr before the rest is dropped.
const OUTPUT_LIMIT: usize = 64 * 1024;

/// One line a plugin logged (or wrote to stdout/stderr).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub level: Level,
    pub message: String,
}

/// A question a plugin asked (answered through `handler.on-answer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    Text {
        prompt: String,
        initial: String,
    },
    Completing {
        prompt: String,
        candidates: Vec<String>,
        require_match: bool,
        /// A `completing-read-multiple`-style prompt: the input is split
        /// on [`crate::completion::CRM_SEPARATORS`], each field is
        /// completed and validated, and the answer is the list of fields.
        multiple: bool,
        /// Take over the prompt that is open (minibuffer.completing-read-over):
        /// it is suspended while this runs, and the answer replaces its
        /// input. Nothing to suspend when no prompt is open.
        over: bool,
    },
    YOrN {
        prompt: String,
    },
    /// One key (minibuffer.read-key).
    Key {
        prompt: String,
    },
}

/// Something a plugin asked for that the frontend carries out after the
/// plugin's call returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A popup menu at the pointer (`menus.popup`).
    Popup(Vec<(String, String)>),
    /// Open a folder in Dired (the user chose it from a plugin's page).
    OpenDir(String),
    /// `forge.browse(url)`: show a web address in the browser Forge picks.
    BrowseUrl(String),
    /// Put text on the kill ring (and the clipboard).
    Kill(String),
    /// Run another Rhai file in the same session (`forge.load`).
    LoadRhai(String),
    Message(String),
    RunCommand(String),
    /// A command with a numeric prefix argument (forge.command.run(name, n)).
    RunCommandWithArg(String, i64),
    /// init.rhai ran a command no one has defined yet: run it once a
    /// plugin defines it (plugins load after init.rhai).
    RunCommandLater(String, Option<i64>),
    Ask {
        plugin: String,
        request: u32,
        question: Question,
    },
    /// Keys to handle as if typed (keymap.execute-keys).
    ExecuteKeys(String),
    /// Candidates for a completion at point (completion.deliver, or
    /// completion.deliver-decorated for ones with a margin / deprecation).
    Completions {
        request: u32,
        start: Pos,
        end: Pos,
        candidates: Vec<crate::completion::Candidate>,
    },
    /// A program for init.rhai (`forge.process.run`): spawn it after the
    /// call returns, and answer the Rhai callback on exit.
    Process {
        request: u32,
        program: crate::process::ProcessSpec,
        dir: std::path::PathBuf,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        env: Vec<(String, String)>,
    },
    /// A file operation for init.rhai (`forge.fs`): perform it after the
    /// call returns, and answer the Rhai callback.
    Fs {
        request: u32,
        op: crate::process::FsOp,
    },
    /// A web request for init.rhai (`forge.http.get` / `forge.http.request`):
    /// start it after the call returns, and answer the Rhai callback when the
    /// answer arrives. Asynchronous, like `Effect::Process`.
    Http {
        request: u32,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    },
    /// completion.locate: ask the source of the selected candidate where
    /// it is defined (corfu-info-location). Carried out after the call
    /// returns, so a source can be asked without re-entering the host.
    Locate,
}

pub(crate) const EVENT_KINDS: usize = 5;

pub(crate) fn kind_index(k: EventKind) -> usize {
    match k {
        EventKind::BufferOpened => 0,
        EventKind::BufferKilled => 1,
        EventKind::BufferChanged => 2,
        EventKind::BufferSaved => 3,
        EventKind::ModeChanged => 4,
    }
}

pub(crate) fn event_kind(e: &PluginEvent) -> usize {
    match e {
        PluginEvent::BufferOpened(_) => 0,
        PluginEvent::BufferKilled(_) => 1,
        PluginEvent::BufferChanged(_) => 2,
        PluginEvent::BufferSaved(_) => 3,
        PluginEvent::ModeChanged(_) => 4,
    }
}

/// Per-plugin state that host functions can reach.
pub struct PluginState {
    wasi: WasiCtx,
    table: ResourceTable,
    pub name: String,
    pub logs: Vec<LogLine>,
    /// The editor, while it is lent to this plugin for a call.
    pub(crate) editor: Option<Editor>,
    pub(crate) effects: Vec<Effect>,
    pub(crate) subscriptions: [bool; EVENT_KINDS],
    next_request: u32,
    /// Bindings this plugin made, so unloading can take them back.
    pub(crate) bindings: Vec<(Scope, String, String)>,
    pub(crate) remaps: Vec<(Scope, String, String)>,
    /// Memory limit and friends (see `permissions`).
    limits: StoreLimits,
    /// What it was granted (processes, directories for buffers.visit).
    grants: Grants,
    /// Programs it started (processes.spawn).
    pub(crate) processes: Processes,
    /// Asynchronous requests it made (http.get-async), awaiting answers.
    pub(crate) requests: crate::http::HttpRequests,
    /// Bytes of stderr logged per process (capped).
    stderr_seen: std::collections::BTreeMap<ProcessId, usize>,
    /// Stderr kept per process for `processes.error-output` (capped at
    /// [`ERROR_OUTPUT_LIMIT`]), until its on-exit has run.
    error_output: std::collections::BTreeMap<ProcessId, Vec<u8>>,
    /// The current call runs in the background, on a snapshot.
    pub(crate) background: bool,
    /// Commands registered with register-background.
    pub(crate) background_commands: std::collections::BTreeSet<String>,
    /// Events subscribed with subscribe-background.
    pub(crate) background_events: [bool; EVENT_KINDS],
    /// Buffers whose decorations it set or cleared in this call.
    pub(crate) touched_decorations: std::collections::BTreeSet<BufferId>,
    /// Buffers whose plugin images it set or cleared in this call.
    pub(crate) touched_images: std::collections::BTreeSet<BufferId>,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

type HostResult<T> = wasmtime::Result<T>;

fn pos(p: Position) -> Pos {
    Pos::new(p.line as usize, p.column as usize)
}

/// A project root as the keymaps store it: canonical, so a symlinked path
/// and the real one are one project.
fn canonical(path: &str) -> std::path::PathBuf {
    let path = expand_home(path);
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn position(p: Pos) -> Position {
    Position {
        line: p.line as u32,
        column: p.col as u32,
    }
}

fn span(a: Pos, b: Pos) -> Span {
    Span {
        start: position(a),
        end: position(b),
    }
}

fn location(l: forge::editor::completion::Location) -> crate::completion::Location {
    crate::completion::Location {
        path: l.path.map(std::path::PathBuf::from),
        line: l.line,
        column: l.column,
    }
}

fn wit_location(l: &crate::completion::Location) -> forge::editor::completion::Location {
    forge::editor::completion::Location {
        path: l.path.as_ref().map(|p| p.display().to_string()),
        line: l.line,
        column: l.column,
    }
}

pub(crate) fn wit_selection(s: &crate::completion::Selection) -> Selection {
    Selection {
        text: s.text.clone(),
        annotation: s.annotation.clone(),
        category: s.category.clone(),
        index: s.index,
        total: s.total,
        documentation: s.documentation.clone(),
        location: s.location.as_ref().map(wit_location),
    }
}

impl PluginState {
    fn editor(&mut self) -> HostResult<&mut Editor> {
        self.editor
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("the editor is only available during a call"))
    }

    fn buffer(&mut self, h: &Resource<BufferHandle>) -> HostResult<&mut Buffer> {
        let id = h.rep();
        self.editor()?
            .state
            .get_buffer_mut(BufferId(id))
            .ok_or_else(|| {
                wasmtime::format_err!("buffer {id} was killed (check buffer.alive first)")
            })
    }

    fn scope(s: forge::editor::keymap::Scope) -> Scope {
        match s {
            forge::editor::keymap::Scope::Global => Scope::Global,
            forge::editor::keymap::Scope::Mode(m) => Scope::Mode(m),
            forge::editor::keymap::Scope::Buffer(id) => Scope::Buffer(BufferId(id)),
        }
    }

    /// Err (for a `result`) in a background call.
    fn foreground(&self, what: &str) -> Result<(), String> {
        if self.background {
            Err(format!("{what} isn't available in a background call"))
        } else {
            Ok(())
        }
    }

    /// Trap in a background call (for functions with no `result`).
    fn foreground_only(&self, what: &str) -> HostResult<()> {
        self.foreground(what).map_err(wasmtime::Error::msg)
    }

    /// Bind into a map named by a string rather than by a `scope` case:
    /// a prompt map or a region map. Empty names are refused, since a
    /// binding there could never be reached.
    fn bind_named(
        &mut self,
        name: &str,
        keys: &str,
        command: &str,
        wrap: fn(String) -> Scope,
        what: &str,
        noun: &str,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground(what) {
            return Ok(Err(e));
        }
        if name.is_empty() {
            return Ok(Err(format!("{noun} needs a name")));
        }
        let scope = wrap(name.to_string());
        let Ok(seq) = parse_keys(keys) else {
            return Ok(Err(format!("not valid key notation: {keys}")));
        };
        match self
            .editor()?
            .keymaps
            .bind_seq(&scope, &seq, command, Origin::Package)
        {
            Ok(()) => {
                self.bindings
                    .push((scope, seq.join(" "), command.to_string()));
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    fn ask(&mut self, question: Question) -> u32 {
        self.next_request += 1;
        let request = self.next_request;
        self.effects.push(Effect::Ask {
            plugin: self.name.clone(),
            request,
            question,
        });
        request
    }
}

fn handle(id: BufferId) -> Resource<BufferHandle> {
    Resource::new_own(id.0)
}

// ------------------------------------------------------------------- log

impl forge::editor::log::Host for PluginState {
    fn log(&mut self, level: Level, message: String) -> HostResult<()> {
        log::info!("[plugin {}] {message}", self.name);
        self.logs.push(LogLine { level, message });
        Ok(())
    }
}

impl forge::editor::types::Host for PluginState {}

// --------------------------------------------------------------- buffers

impl forge::editor::buffers::Host for PluginState {}

impl forge::editor::buffers::HostBuffer for PluginState {
    fn current(&mut self) -> HostResult<Resource<BufferHandle>> {
        let id = self
            .editor()?
            .state
            .active_buffer
            .ok_or_else(|| wasmtime::Error::msg("no current buffer"))?;
        Ok(handle(id))
    }

    fn all(&mut self) -> HostResult<Vec<Resource<BufferHandle>>> {
        Ok(self
            .editor()?
            .buffer_ids()
            .into_iter()
            .map(handle)
            .collect())
    }

    fn find(&mut self, name: String) -> HostResult<Option<Resource<BufferHandle>>> {
        Ok(self.editor()?.find_buffer(&name).map(handle))
    }

    fn from_id(&mut self, id: u32) -> HostResult<Option<Resource<BufferHandle>>> {
        Ok(self
            .editor()?
            .state
            .buffers
            .contains_key(&BufferId(id))
            .then(|| handle(BufferId(id))))
    }

    fn create(&mut self, name: String) -> HostResult<Resource<BufferHandle>> {
        self.foreground_only("buffer.create")?;
        Ok(handle(self.editor()?.add_buffer(Buffer::new(name))))
    }

    fn id(&mut self, b: Resource<BufferHandle>) -> HostResult<u32> {
        Ok(b.rep())
    }

    fn alive(&mut self, b: Resource<BufferHandle>) -> HostResult<bool> {
        Ok(self
            .editor()?
            .state
            .buffers
            .contains_key(&BufferId(b.rep())))
    }

    fn name(&mut self, b: Resource<BufferHandle>) -> HostResult<String> {
        Ok(self.buffer(&b)?.name().to_string())
    }

    fn path(&mut self, b: Resource<BufferHandle>) -> HostResult<Option<String>> {
        Ok(self.buffer(&b)?.path().map(|p| p.display().to_string()))
    }

    fn modified(&mut self, b: Resource<BufferHandle>) -> HostResult<bool> {
        Ok(self.buffer(&b)?.is_modified())
    }

    fn version(&mut self, b: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&b)?.version())
    }

    fn text(&mut self, b: Resource<BufferHandle>) -> HostResult<String> {
        Ok(self.buffer(&b)?.get_contents())
    }

    fn text_in(&mut self, b: Resource<BufferHandle>, s: Span) -> HostResult<String> {
        Ok(self.buffer(&b)?.text_range(pos(s.start), pos(s.end)))
    }

    fn line_count(&mut self, b: Resource<BufferHandle>) -> HostResult<u32> {
        Ok(self.buffer(&b)?.line_count() as u32)
    }

    fn line(&mut self, b: Resource<BufferHandle>, line: u32) -> HostResult<String> {
        let buf = self.buffer(&b)?;
        Ok(if (line as usize) < buf.line_count() {
            buf.line(line as usize)
        } else {
            String::new()
        })
    }

    fn point(&mut self, b: Resource<BufferHandle>) -> HostResult<Position> {
        Ok(position(self.buffer(&b)?.point()))
    }

    fn set_point(&mut self, b: Resource<BufferHandle>, p: Position) -> HostResult<()> {
        self.buffer(&b)?.set_point(pos(p));
        Ok(())
    }

    fn mark(&mut self, b: Resource<BufferHandle>) -> HostResult<Option<Position>> {
        Ok(self.buffer(&b)?.mark().map(position))
    }

    fn set_mark(&mut self, b: Resource<BufferHandle>, p: Option<Position>) -> HostResult<()> {
        let buf = self.buffer(&b)?;
        match p {
            Some(p) => buf.push_mark(pos(p), true),
            None => buf.deactivate_mark(),
        }
        Ok(())
    }

    fn region(&mut self, b: Resource<BufferHandle>) -> HostResult<Option<Span>> {
        Ok(self.buffer(&b)?.active_region().map(|(a, z)| span(a, z)))
    }

    fn insert(&mut self, b: Resource<BufferHandle>, text: String) -> HostResult<()> {
        self.buffer(&b)?.as_owner(|b| b.insert_str(&text));
        Ok(())
    }

    fn replace(&mut self, b: Resource<BufferHandle>, s: Span, text: String) -> HostResult<()> {
        self.buffer(&b)?
            .as_owner(|b| b.replace_range(pos(s.start), pos(s.end), &text));
        Ok(())
    }

    fn delete(&mut self, b: Resource<BufferHandle>, s: Span) -> HostResult<String> {
        Ok(self
            .buffer(&b)?
            .as_owner(|b| b.delete_range(pos(s.start), pos(s.end))))
    }

    fn search_forward(
        &mut self,
        b: Resource<BufferHandle>,
        query: String,
        from: Position,
    ) -> HostResult<Option<Span>> {
        Ok(self
            .buffer(&b)?
            .search_forward(&query, pos(from))
            .map(|(a, z)| span(a, z)))
    }

    fn search_backward(
        &mut self,
        b: Resource<BufferHandle>,
        query: String,
        from: Position,
    ) -> HostResult<Option<Span>> {
        Ok(self
            .buffer(&b)?
            .search_backward(&query, pos(from))
            .map(|(a, z)| span(a, z)))
    }

    fn search_regexp(
        &mut self,
        b: Resource<BufferHandle>,
        pattern: String,
        from: Position,
    ) -> HostResult<Result<Option<Span>, String>> {
        let re = match regex::RegexBuilder::new(&pattern).multi_line(true).build() {
            Ok(re) => re,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let buf = self.buffer(&b)?;
        let text = buf.get_contents();
        let start = buf.char_to_byte(buf.pos_to_offset(pos(from)));
        Ok(Ok(re.find_at(&text, start).map(|m| {
            let a = buf.offset_to_pos(buf.byte_to_char(m.start()));
            let z = buf.offset_to_pos(buf.byte_to_char(m.end()));
            span(a, z)
        })))
    }

    fn major_mode(&mut self, b: Resource<BufferHandle>) -> HostResult<String> {
        Ok(self.buffer(&b)?.major_mode().to_string())
    }

    fn set_major_mode(
        &mut self,
        b: Resource<BufferHandle>,
        mode: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("set-major-mode") {
            return Ok(Err(e));
        }
        if !self.editor()?.modes.is_major(&mode) {
            return Ok(Err(format!("unknown major mode {mode}")));
        }
        self.buffer(&b)?.set_major_mode(mode);
        Ok(Ok(()))
    }

    fn minor_modes(&mut self, b: Resource<BufferHandle>) -> HostResult<Vec<String>> {
        Ok(self.buffer(&b)?.minor_modes().to_vec())
    }

    fn enable_minor_mode(
        &mut self,
        b: Resource<BufferHandle>,
        mode: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("enable-minor-mode") {
            return Ok(Err(e));
        }
        if !self.editor()?.modes.is_minor(&mode) {
            return Ok(Err(format!("unknown minor mode {mode}")));
        }
        self.buffer(&b)?.enable_minor_mode(mode);
        Ok(Ok(()))
    }

    fn disable_minor_mode(&mut self, b: Resource<BufferHandle>, mode: String) -> HostResult<()> {
        self.foreground_only("disable-minor-mode")?;
        self.buffer(&b)?.disable_minor_mode(&mode);
        Ok(())
    }

    fn show(&mut self, b: Resource<BufferHandle>) -> HostResult<()> {
        self.foreground_only("buffer.show")?;
        self.buffer(&b)?;
        self.editor()?.show_buffer(BufferId(b.rep()));
        Ok(())
    }

    fn kill(&mut self, b: Resource<BufferHandle>) -> HostResult<()> {
        self.foreground_only("buffer.kill")?;
        self.buffer(&b)?;
        self.editor()?.kill_buffer(BufferId(b.rep()), None);
        Ok(())
    }

    fn set_mode_line_info(
        &mut self,
        b: Resource<BufferHandle>,
        text: Option<String>,
    ) -> HostResult<()> {
        let name = self.name.clone();
        let buf = self.buffer(&b)?;
        match text.filter(|t| !t.is_empty()) {
            Some(t) => {
                buf.mode_line_info.insert(name, t);
            }
            None => {
                buf.mode_line_info.remove(&name);
            }
        }
        Ok(())
    }

    fn mode_line_info(&mut self, b: Resource<BufferHandle>) -> HostResult<Option<String>> {
        let name = self.name.clone();
        Ok(self.buffer(&b)?.mode_line_info.get(&name).cloned())
    }

    fn set_option(
        &mut self,
        b: Resource<BufferHandle>,
        name: String,
        value: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("set-option") {
            return Ok(Err(e));
        }
        Ok(self.buffer(&b)?.set_option(&name, &value))
    }

    fn get_option(
        &mut self,
        b: Resource<BufferHandle>,
        name: String,
    ) -> HostResult<Option<String>> {
        Ok(self.buffer(&b)?.option(&name))
    }

    fn indent_line(&mut self, b: Resource<BufferHandle>, line: u32) -> HostResult<bool> {
        self.buffer(&b)?;
        self.editor()?.sync_language(BufferId(b.rep()));
        Ok(self.buffer(&b)?.indent_line(line as usize))
    }

    fn visit(&mut self, path: String) -> HostResult<Result<Resource<BufferHandle>, String>> {
        if let Err(e) = self.foreground("buffer.visit") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if !p.is_file() {
            return Ok(Err(format!("no file {path}")));
        }
        if self.grants.processes.is_empty() && !self.grants.covers_path(&p) {
            return Ok(Err(format!(
                "{path}: not in a directory this plugin was granted"
            )));
        }
        Ok(self
            .editor()?
            .open_file(&p)
            .map(|(id, _)| handle(id))
            .map_err(|e| format!("{path}: {e}")))
    }

    fn open_path(
        &mut self,
        path: String,
    ) -> HostResult<Result<Option<Resource<BufferHandle>>, String>> {
        if let Err(e) = self.foreground("buffer.open-path") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if p.is_dir() {
            self.effects
                .push(Effect::OpenDir(p.to_string_lossy().into_owned()));
            return Ok(Ok(None));
        }
        if !p.is_file() {
            return Ok(Err(format!("no file {path}")));
        }
        Ok(self
            .editor()?
            .open_file(&p)
            .map(|(id, _)| Some(handle(id)))
            .map_err(|e| format!("{path}: {e}")))
    }

    fn definitions(
        &mut self,
        b: Resource<BufferHandle>,
    ) -> HostResult<Vec<forge::editor::buffers::Definition>> {
        let id = BufferId(b.rep());
        self.buffer(&b)?;
        self.editor()?.sync_language(id);
        Ok(self
            .buffer(&b)?
            .definitions()
            .into_iter()
            .map(|(name, kind, p)| forge::editor::buffers::Definition {
                name,
                kind,
                position: position(p),
            })
            .collect())
    }

    fn drop(&mut self, _b: Resource<BufferHandle>) -> HostResult<()> {
        // Handles are just buffer ids; nothing to free.
        Ok(())
    }
}

// ------------------------------------------------------------ completion

impl forge::editor::completion::Host for PluginState {
    fn set_minibuffer_ui(&mut self, vertical: bool, lines: u32) -> HostResult<()> {
        self.foreground_only("completion.set-minibuffer-ui")?;
        let name = self.name.clone();
        self.editor()?.completion.vertical = vertical.then(|| (lines.clamp(1, 50), name));
        Ok(())
    }

    fn set_minibuffer_cycle(&mut self, cycle: bool) -> HostResult<()> {
        self.foreground_only("completion.set-minibuffer-cycle")?;
        let name = self.name.clone();
        self.editor()?.completion.list_cycle = Some((cycle, name));
        Ok(())
    }

    fn set_matching(&mut self, styles: Vec<String>) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("completion.set-matching") {
            return Ok(Err(e));
        }
        let parsed: Result<Vec<_>, String> = styles
            .iter()
            .map(|s| crate::completion::Style::from_name(s))
            .collect();
        let name = self.name.clone();
        Ok(parsed.map(|p| {
            self.editor
                .as_mut()
                .map(|ed| ed.completion.styles = Some((p, name)))
                .unwrap_or(())
        }))
    }

    fn annotate_category(&mut self, category: String) -> HostResult<()> {
        self.foreground_only("completion.annotate-category")?;
        let name = self.name.clone();
        self.editor()?.completion.annotators.insert(category, name);
        Ok(())
    }

    fn on_selection_changed(&mut self, command: String) -> HostResult<()> {
        self.foreground_only("completion.on-selection-changed")?;
        let name = self.name.clone();
        let ed = self.editor()?;
        if !ed.commands.contains(&command) {
            return Err(wasmtime::Error::msg(format!("no command named {command}")));
        }
        ed.completion.selection_commands.retain(|(p, _)| p != &name);
        ed.completion.selection_commands.push((name, command));
        Ok(())
    }

    fn set_at_point_ui(
        &mut self,
        popup: bool,
        auto: bool,
        delay_ms: u32,
        min_prefix: u32,
        lines: u32,
    ) -> HostResult<()> {
        self.foreground_only("completion.set-at-point-ui")?;
        let name = self.name.clone();
        let a = crate::completion::AtPoint {
            popup,
            auto,
            delay_ms: delay_ms.max(50),
            min_prefix: min_prefix.max(1),
            lines: lines.clamp(1, 50),
        };
        self.editor()?.completion.at_point = Some((a, name));
        Ok(())
    }

    fn set_popup_info(
        &mut self,
        show_documentation: bool,
        show_location: bool,
        delay_ms: u32,
        max_width: u32,
        max_height: u32,
    ) -> HostResult<()> {
        self.foreground_only("completion.set-popup-info")?;
        let name = self.name.clone();
        let info = crate::completion::Info {
            show_documentation,
            show_location,
            delay_ms,
            max_width,
            max_height,
        };
        self.editor()?.completion.info = Some((info, name));
        Ok(())
    }

    fn toggle_popup_info(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::TogglePopupInfo)
    }

    fn scroll_popup_info(&mut self, down: bool) -> HostResult<()> {
        self.control(crate::completion::Control::PopupInfoScroll { down })
    }

    fn popup_info_beginning(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::PopupInfoBeginning)
    }

    fn popup_info_end(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::PopupInfoEnd)
    }

    fn set_sorting(&mut self, styles: Vec<String>) -> HostResult<()> {
        self.foreground_only("completion.set-sorting")?;
        let name = self.name.clone();
        // Unknown names are dropped, not an error: a plugin written
        // against a newer Forge still loads and gets what this one knows.
        let parsed: Vec<_> = styles
            .iter()
            .filter_map(|s| crate::completion::Sort::from_name(s).ok())
            .collect();
        let ed = self.editor()?;
        ed.completion.sorting = Some((parsed, name));
        Ok(())
    }

    fn selected(&mut self) -> HostResult<Option<Selection>> {
        self.foreground_only("completion.selected")?;
        let Some(s) = self.editor()?.selection.clone() else {
            return Ok(None);
        };
        Ok(Some(wit_selection(&s)))
    }

    fn set_popup_frame(&mut self, min_width: u32, max_width: u32, border: u32) -> HostResult<()> {
        self.foreground_only("completion.set-popup-frame")?;
        let name = self.name.clone();
        let frame = crate::completion::Frame {
            min_width,
            max_width,
            border,
        };
        self.editor()?.completion.frame = Some((frame, name));
        Ok(())
    }

    fn register_source(
        &mut self,
        source: String,
        modes: Vec<String>,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("completion.register-source") {
            return Ok(Err(e));
        }
        let plugin = self.name.clone();
        let ed = self.editor()?;
        if ed
            .completion
            .sources
            .iter()
            .any(|s| s.name == source && s.plugin != plugin)
        {
            return Ok(Err(format!(
                "another plugin has a completion source named {source}"
            )));
        }
        ed.completion.sources.retain(|s| s.name != source);
        ed.completion.sources.push(crate::completion::Source {
            name: source,
            plugin,
            modes,
        });
        Ok(Ok(()))
    }

    fn deliver(
        &mut self,
        request: u32,
        s: Span,
        candidates: Vec<forge::editor::completion::Candidate>,
    ) -> HostResult<()> {
        let candidates = candidates
            .into_iter()
            .map(|c| crate::completion::Candidate::new(c.text, c.annotation))
            .collect();
        self.effects.push(Effect::Completions {
            request,
            start: pos(s.start),
            end: pos(s.end),
            candidates,
        });
        Ok(())
    }

    fn deliver_decorated(
        &mut self,
        request: u32,
        s: Span,
        candidates: Vec<forge::editor::completion::DecoratedCandidate>,
    ) -> HostResult<()> {
        let candidates = candidates
            .into_iter()
            .map(|c| crate::completion::Candidate {
                text: c.text,
                annotation: c.annotation,
                margin: c.margin,
                deprecated: c.deprecated,
                documentation: None,
                location: None,
            })
            .collect();
        self.effects.push(Effect::Completions {
            request,
            start: pos(s.start),
            end: pos(s.end),
            candidates,
        });
        Ok(())
    }

    fn deliver_detailed(
        &mut self,
        request: u32,
        s: Span,
        candidates: Vec<forge::editor::completion::DetailedCandidate>,
    ) -> HostResult<()> {
        let candidates = candidates
            .into_iter()
            .map(|c| crate::completion::Candidate {
                text: c.text,
                annotation: c.annotation,
                margin: c.margin,
                deprecated: c.deprecated,
                documentation: c.documentation,
                location: c.location.map(location),
            })
            .collect();
        self.effects.push(Effect::Completions {
            request,
            start: pos(s.start),
            end: pos(s.end),
            candidates,
        });
        Ok(())
    }

    fn locate(&mut self) -> HostResult<bool> {
        self.foreground_only("completion.locate")?;
        if self.editor()?.selection.is_none() {
            return Ok(false);
        }
        self.effects.push(Effect::Locate);
        Ok(true)
    }

    fn current(&mut self) -> HostResult<Option<forge::editor::completion::SessionInfo>> {
        self.foreground_only("completion.current")?;
        let Some(s) = self.editor()?.completion_session.clone() else {
            return Ok(None);
        };
        Ok(Some(forge::editor::completion::SessionInfo {
            open: s.open,
            popup: s.popup,
            index: s.index,
            total: s.total,
            deselected: s.deselected,
            lines: s.lines,
            input: s.input,
        }))
    }

    fn set_options(&mut self, o: forge::editor::completion::Options) -> HostResult<()> {
        self.foreground_only("completion.set-options")?;
        let name = self.name.clone();
        let policy = crate::completion::Policy {
            cycle: o.cycle,
            preselect_prompt: o.preselect_prompt,
            separator: o.separator,
            quit_at_boundary: crate::completion::Quit::from_name(&o.quit_at_boundary),
            quit_no_match: crate::completion::Quit::from_name(&o.quit_no_match),
            continue_commands: o.continue_commands,
            indexed: o.indexed,
            quick1: o.quick1,
            quick2: o.quick2,
            echo: o.echo,
            echo_delay_ms: o.echo_delay_ms,
            mouse: o.mouse,
            history: o.history,
        };
        self.editor()?.completion.policy = Some((policy, name));
        Ok(())
    }

    fn next(&mut self, n: i32) -> HostResult<()> {
        self.control(crate::completion::Control::Next(n))
    }

    fn previous(&mut self, n: i32) -> HostResult<()> {
        self.control(crate::completion::Control::Previous(n))
    }

    fn first(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::First)
    }

    fn last(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::Last)
    }

    fn scroll(&mut self, down: bool) -> HostResult<()> {
        self.control(crate::completion::Control::Scroll { down })
    }

    fn accept(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::Accept)
    }

    fn expand(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::Expand)
    }

    fn quit(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::Quit)
    }

    fn reset(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::Reset)
    }

    fn accept_row(&mut self, row: u32) -> HostResult<()> {
        self.control(crate::completion::Control::AcceptRow(row))
    }

    fn arm_quick(&mut self, action: String) -> HostResult<()> {
        if let Some(a) = crate::completion::QuickAction::from_name(&action) {
            self.control(crate::completion::Control::ArmQuick(a))?;
        }
        Ok(())
    }

    fn insert_separator(&mut self) -> HostResult<()> {
        self.control(crate::completion::Control::InsertSeparator)
    }
}

impl PluginState {
    /// Queue a request to drive the completion popup; the frontend drains
    /// `editor.completion_control` after the call.
    fn control(&mut self, c: crate::completion::Control) -> HostResult<()> {
        self.foreground_only("completion.control")?;
        self.editor()?.completion_control.push(c);
        Ok(())
    }
}

// ------------------------------------------------------------- processes

// ----------------------------------------------------------------- files

impl PluginState {
    /// The folder this plugin keeps rebuildable files in (`files.cache-dir`).
    fn cache_path(&self) -> std::path::PathBuf {
        let name: String = self
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        crate::config::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("forge"))
            .join("plugin-data")
            .join(name)
    }

    /// Is `path` inside the plugin's own cache folder (and not climbing out of it)?
    fn in_cache(&self, path: &std::path::Path) -> bool {
        path.starts_with(self.cache_path())
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
    }

    /// May this plugin read `path`?
    fn may_read(&self, path: &std::path::Path) -> bool {
        !self.grants.processes.is_empty() || self.in_cache(path) || self.grants.covers_path(path)
    }

    /// May this plugin change `path` on disk?
    fn may_write(&self, path: &std::path::Path) -> Result<(), String> {
        if !self.grants.processes.is_empty() {
            return Ok(());
        }
        if self.in_cache(path) {
            return Ok(());
        }
        // The nearest existing ancestor must be in a writable grant.
        let mut p = path.to_path_buf();
        while !p.exists() {
            match p.parent() {
                Some(q) => p = q.to_path_buf(),
                None => break,
            }
        }
        let p = p.canonicalize().unwrap_or(p);
        let ok = self.grants.directories.iter().any(|d| {
            d.write
                && expand_home(&d.path)
                    .canonicalize()
                    .is_ok_and(|dir| p.starts_with(dir))
        });
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{}: not in a directory this plugin may write",
                path.display()
            ))
        }
    }
}

impl forge::editor::config::Host for PluginState {
    fn get(&mut self) -> HostResult<String> {
        Ok(self
            .editor
            .as_ref()
            .and_then(|e| e.plugin_config.get(&self.name).cloned())
            .unwrap_or_else(|| "{}".into()))
    }
}

/// The error a plugin gets for reading the environment without the grant.
fn environment_denied() -> String {
    "this plugin doesn't have the environment permission (add `environment = true` \
     to its [permissions] and accept it)"
        .to_string()
}

impl forge::editor::environment::Host for PluginState {
    fn get(&mut self, name: String) -> HostResult<Result<Option<String>, String>> {
        if !self.grants.environment {
            return Ok(Err(environment_denied()));
        }
        Ok(Ok(self
            .editor()?
            .environment
            .get(&name)
            .map(str::to_string)))
    }

    fn variables(&mut self) -> HostResult<Result<Vec<(String, String)>, String>> {
        if !self.grants.environment {
            return Ok(Err(environment_denied()));
        }
        Ok(Ok(self.editor()?.environment.all().to_vec()))
    }

    fn system_type(&mut self) -> HostResult<String> {
        Ok(crate::env::system_type().to_string())
    }

    fn display_graphic(&mut self) -> HostResult<bool> {
        Ok(crate::env::display_graphic())
    }

    fn frame_width(&mut self) -> HostResult<u32> {
        let w = self.editor.as_ref().map_or(0, |e| {
            let frame = e.windows.frame();
            let sel = e.windows.selected_id();
            e.windows
                .layout(frame)
                .into_iter()
                .find(|(id, _)| *id == sel)
                .map_or(frame.width, |(_, r)| r.width)
        });
        Ok(u32::from(w))
    }

    fn system_configuration(&mut self) -> HostResult<String> {
        Ok(crate::env::system_configuration().to_string())
    }

    fn system_name(&mut self) -> HostResult<String> {
        Ok(crate::env::system_name())
    }

    fn pid(&mut self) -> HostResult<u32> {
        Ok(crate::env::pid())
    }

    fn null_device(&mut self) -> HostResult<String> {
        Ok(crate::env::null_device().to_string())
    }

    fn path_separator(&mut self) -> HostResult<String> {
        Ok(crate::env::path_separator().to_string())
    }

    fn invocation_name(&mut self) -> HostResult<String> {
        Ok(crate::env::invocation_name())
    }

    fn parse_path(&mut self, path: String) -> HostResult<Vec<String>> {
        // An empty component means the current directory; WIT has no nil in
        // a list, so it comes back as an empty string, like the Lua API.
        Ok(crate::env::parse_path(&path)
            .into_iter()
            .map(|p| p.unwrap_or_default())
            .collect())
    }
}

impl PluginState {
    /// The entries under `dir` (sorted by path), each with its metadata when
    /// `info` is set. Hidden entries and symlinked directories are skipped.
    fn walk(
        &self,
        dir: &str,
        recursive: bool,
        info: bool,
    ) -> Result<Vec<(String, Option<forge::editor::files::FileInfo>)>, String> {
        let root = expand_home(dir);
        if !self.may_read(&root) {
            return Err(format!("{dir}: not in a directory this plugin was granted"));
        }
        const LIMIT: usize = 1_000_000;
        let ms = |t: std::io::Result<std::time::SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64)
        };
        let mut out = Vec::new();
        let mut todo = vec![root.clone()];
        while let Some(d) = todo.pop() {
            let entries = match std::fs::read_dir(&d) {
                Ok(e) => e,
                Err(e) if d == root => return Err(format!("{dir}: {e}")),
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if out.len() >= LIMIT {
                    break;
                }
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                let mut text = path.display().to_string();
                if is_dir {
                    text.push('/');
                    if recursive {
                        todo.push(path);
                    }
                }
                let meta = if info {
                    entry
                        .metadata()
                        .ok()
                        .map(|m| forge::editor::files::FileInfo {
                            size: m.len(),
                            modified: ms(m.modified()),
                            created: ms(m.created()),
                            directory: m.is_dir(),
                        })
                } else {
                    None
                };
                out.push((text, meta));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

impl forge::editor::files::Host for PluginState {
    fn exists(&mut self, path: String) -> HostResult<bool> {
        Ok(expand_home(&path).exists())
    }

    fn read(&mut self, path: String) -> HostResult<Result<String, String>> {
        let p = expand_home(&path);
        if !self.may_read(&p) {
            return Ok(Err(format!(
                "{path}: not in a directory this plugin was granted"
            )));
        }
        Ok(std::fs::read_to_string(&p).map_err(|e| format!("{path}: {e}")))
    }

    fn create(&mut self, path: String, overwrite: bool) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("files.create") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if let Err(e) = self.may_write(&p) {
            return Ok(Err(e));
        }
        if p.exists() && !overwrite {
            return Ok(Ok(()));
        }
        if let Some(d) = p.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Ok(Err(format!("{}: {e}", d.display())));
        }
        Ok(std::fs::write(&p, "").map_err(|e| format!("{path}: {e}")))
    }

    fn write(&mut self, path: String, data: Vec<u8>) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("files.write") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if let Err(e) = self.may_write(&p) {
            return Ok(Err(e));
        }
        if let Some(d) = p.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Ok(Err(format!("{}: {e}", d.display())));
        }
        Ok(std::fs::write(&p, &data).map_err(|e| format!("{path}: {e}")))
    }

    fn stat(&mut self, path: String) -> HostResult<Option<forge::editor::files::FileInfo>> {
        let p = expand_home(&path);
        if !self.may_read(&p) {
            return Ok(None);
        }
        let Ok(meta) = std::fs::metadata(&p) else {
            return Ok(None);
        };
        let ms = |t: std::io::Result<std::time::SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64)
        };
        Ok(Some(forge::editor::files::FileInfo {
            size: meta.len(),
            modified: ms(meta.modified()),
            created: ms(meta.created()),
            directory: meta.is_dir(),
        }))
    }

    fn list_dir(
        &mut self,
        dir: String,
        recursive: bool,
    ) -> HostResult<Result<Vec<String>, String>> {
        Ok(self
            .walk(&dir, recursive, false)
            .map(|v| v.into_iter().map(|e| e.0).collect()))
    }

    fn list_dir_info(
        &mut self,
        dir: String,
        recursive: bool,
    ) -> HostResult<Result<Vec<forge::editor::files::Entry>, String>> {
        Ok(self.walk(&dir, recursive, true).map(|v| {
            v.into_iter()
                .map(|(path, info)| forge::editor::files::Entry {
                    path,
                    info: info.unwrap_or(forge::editor::files::FileInfo {
                        size: 0,
                        modified: 0,
                        created: 0,
                        directory: false,
                    }),
                })
                .collect()
        }))
    }

    fn cache_dir(&mut self) -> HostResult<String> {
        Ok(self.cache_path().display().to_string())
    }

    fn make_directory(&mut self, path: String, parents: bool) -> HostResult<Result<bool, String>> {
        if let Err(e) = self.foreground("files.make-directory") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if let Err(e) = self.may_write(&p) {
            return Ok(Err(e));
        }
        Ok(crate::fileops::make_directory(&path, parents).map_err(|e| format!("{path}: {e}")))
    }

    fn copy_directory(
        &mut self,
        source: String,
        target: String,
        parents: bool,
        copy_contents: bool,
    ) -> HostResult<Result<String, String>> {
        if let Err(e) = self.foreground("files.copy-directory") {
            return Ok(Err(e));
        }
        let (a, b) = (expand_home(&source), expand_home(&target));
        if !self.may_read(&a) {
            return Ok(Err(format!(
                "{source}: not in a directory this plugin was granted"
            )));
        }
        if let Err(e) = self.may_write(&b) {
            return Ok(Err(e));
        }
        Ok(
            crate::fileops::copy_directory(&source, &target, parents, copy_contents)
                .map_err(|e| format!("{source} -> {target}: {e}")),
        )
    }

    fn directory_files(
        &mut self,
        dir: String,
        full: bool,
        matching: Option<String>,
        count: Option<u32>,
    ) -> HostResult<Result<Vec<String>, String>> {
        if !self.may_read(&expand_home(&dir)) {
            return Ok(Err(format!(
                "{dir}: not in a directory this plugin was granted"
            )));
        }
        let re = match matching.as_deref().map(regex::Regex::new).transpose() {
            Ok(r) => r,
            Err(e) => return Ok(Err(format!("bad regexp: {e}"))),
        };
        Ok(crate::fileops::directory_files(
            &dir,
            full,
            re.as_ref(),
            false,
            count.map(|c| c as usize),
        )
        .map_err(|e| format!("{dir}: {e}")))
    }

    fn directory_files_and_attributes(
        &mut self,
        dir: String,
        full: bool,
        matching: Option<String>,
        count: Option<u32>,
    ) -> HostResult<Result<Vec<forge::editor::files::Entry>, String>> {
        if !self.may_read(&expand_home(&dir)) {
            return Ok(Err(format!(
                "{dir}: not in a directory this plugin was granted"
            )));
        }
        let re = match matching.as_deref().map(regex::Regex::new).transpose() {
            Ok(r) => r,
            Err(e) => return Ok(Err(format!("bad regexp: {e}"))),
        };
        Ok(crate::fileops::directory_files_and_attributes(
            &dir,
            full,
            re.as_ref(),
            false,
            count.map(|c| c as usize),
        )
        .map(|v| {
            v.into_iter()
                .map(|(path, a)| forge::editor::files::Entry {
                    path,
                    info: forge::editor::files::FileInfo {
                        size: a.size,
                        modified: a.modified * 1000,
                        created: 0,
                        directory: a.kind == crate::fileops::Kind::Dir,
                    },
                })
                .collect()
        })
        .map_err(|e| format!("{dir}: {e}")))
    }

    fn directory_empty(&mut self, dir: String) -> HostResult<bool> {
        Ok(self.may_read(&expand_home(&dir)) && crate::fileops::directory_empty_p(&dir))
    }

    fn directory_files_recursively(
        &mut self,
        dir: String,
        regexp: String,
        include_dirs: bool,
    ) -> HostResult<Result<Vec<String>, String>> {
        if !self.may_read(&expand_home(&dir)) {
            return Ok(Err(format!(
                "{dir}: not in a directory this plugin was granted"
            )));
        }
        let re = match regex::Regex::new(&regexp) {
            Ok(r) => r,
            Err(e) => return Ok(Err(format!("bad regexp: {e}"))),
        };
        Ok(Ok(crate::fileops::directory_files_recursively(
            &dir,
            &re,
            include_dirs,
            None,
            false,
        )))
    }

    fn locate_dominating_file(&mut self, file: String, name: String) -> HostResult<Option<String>> {
        if !self.may_read(&expand_home(&file)) {
            return Ok(None);
        }
        Ok(crate::fileops::locate_dominating_name(&file, &name))
    }

    fn in_directory(&mut self, file: String, dir: String) -> HostResult<bool> {
        Ok(self.may_read(&expand_home(&dir)) && crate::fileops::file_in_directory_p(&file, &dir))
    }

    fn expand_wildcards(
        &mut self,
        pattern: String,
        full: bool,
        regexp: bool,
    ) -> HostResult<Result<Vec<String>, String>> {
        let at = crate::fileops::file_name_directory(&pattern).unwrap_or_else(|| "./".into());
        if !self.may_read(&expand_home(&at)) {
            return Ok(Err(format!(
                "{pattern}: not in a directory this plugin was granted"
            )));
        }
        Ok(
            crate::fileops::file_expand_wildcards(&pattern, full, regexp, None).map(|v| {
                v.into_iter()
                    .filter(|p| self.may_read(&expand_home(p)))
                    .collect()
            }),
        )
    }

    fn name_directory(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(crate::fileops::file_name_directory(&name))
    }
    fn name_nondirectory(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::file_name_nondirectory(&name))
    }
    fn name_as_directory(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::file_name_as_directory(&name))
    }
    fn directory_name(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::directory_file_name(&name))
    }
    fn name_extension(&mut self, name: String, period: bool) -> HostResult<Option<String>> {
        Ok(crate::fileops::file_name_extension(&name, period))
    }
    fn name_sans_extension(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::file_name_sans_extension(&name))
    }
    fn name_base(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::file_name_base(&name))
    }
    fn name_with_extension(
        &mut self,
        name: String,
        extension: String,
    ) -> HostResult<Result<String, String>> {
        Ok(crate::fileops::file_name_with_extension(&name, &extension))
    }
    fn name_concat(&mut self, parts: Vec<String>) -> HostResult<String> {
        Ok(crate::fileops::file_name_concat(
            &parts.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
    }
    fn name_absolute(&mut self, name: String) -> HostResult<bool> {
        Ok(crate::fileops::file_name_absolute_p(&name))
    }
    fn name_split(&mut self, name: String) -> HostResult<Vec<String>> {
        Ok(crate::fileops::file_name_split(&name))
    }
    fn name_parent_directory(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(crate::fileops::file_name_parent_directory(&name))
    }
    fn expand_name(&mut self, name: String, dir: Option<String>) -> HostResult<String> {
        Ok(crate::fileops::expand_file_name(&name, dir.as_deref()))
    }
    fn relative_name(&mut self, name: String, dir: Option<String>) -> HostResult<String> {
        Ok(crate::fileops::file_relative_name(&name, dir.as_deref()))
    }
    fn abbreviate_name(&mut self, name: String) -> HostResult<String> {
        Ok(crate::fileops::abbreviate_file_name(&name))
    }

    fn rename(
        &mut self,
        from: String,
        to: String,
        overwrite: bool,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("files.rename") {
            return Ok(Err(e));
        }
        let (a, b) = (expand_home(&from), expand_home(&to));
        if let Err(e) = self.may_write(&a).and_then(|_| self.may_write(&b)) {
            return Ok(Err(e));
        }
        if b.exists() && !overwrite {
            return Ok(Err(format!("{to} exists")));
        }
        if let Some(d) = b.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Ok(Err(format!("{}: {e}", d.display())));
        }
        if let Err(e) = std::fs::rename(&a, &b) {
            return Ok(Err(format!("{from} -> {to}: {e}")));
        }
        // Buffers follow their files (a directory's files too).
        let a = a.canonicalize().unwrap_or(a);
        let b = b.canonicalize().unwrap_or(b);
        let a_abs = if a.is_absolute() {
            a
        } else {
            std::env::current_dir().map(|d| d.join(&a)).unwrap_or(a)
        };
        for buf in self.editor()?.state.buffers.values_mut() {
            let Some(p) = buf.path().map(|p| p.to_path_buf()) else {
                continue;
            };
            if let Ok(rest) = p.strip_prefix(&a_abs) {
                let new = if rest.as_os_str().is_empty() {
                    b.clone()
                } else {
                    b.join(rest)
                };
                buf.set_visited_file(new);
            }
        }
        Ok(Ok(()))
    }

    fn delete(&mut self, path: String, recursive: bool) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("files.delete") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if let Err(e) = self.may_write(&p) {
            return Ok(Err(e));
        }
        let r = if p.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_dir(&p)
            }
        } else {
            std::fs::remove_file(&p)
        };
        Ok(r.map_err(|e| format!("{path}: {e}")))
    }
}

impl forge::editor::processes::Host for PluginState {
    fn spawn(
        &mut self,
        program: String,
        args: Vec<String>,
        dir: Option<String>,
    ) -> HostResult<Result<u32, String>> {
        self.spawn_with(program, args, dir, Vec::new())
    }

    fn spawn_with(
        &mut self,
        program: String,
        args: Vec<String>,
        dir: Option<String>,
        env: Vec<(String, String)>,
    ) -> HostResult<Result<u32, String>> {
        if !self.grants.may_run(&program) {
            return Ok(Err(format!(
                "{program}: this plugin may not run it (the processes permission in its manifest names what it may run)"
            )));
        }
        let dir = match dir {
            Some(d) => expand_home(&d),
            None => {
                let ed = self.editor()?;
                let file = ed
                    .state
                    .active_buffer()
                    .and_then(|b| b.path().map(std::path::Path::to_path_buf));
                let here = file.and_then(|f| f.parent().map(std::path::Path::to_path_buf));
                let here = here
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| ".".into());
                crate::process::project_root(&here).unwrap_or(here)
            }
        };
        if !dir.is_dir() {
            return Ok(Err(format!("no directory {}", dir.display())));
        }
        Ok(self
            .processes
            .spawn(Program::Argv(&program, &args), &dir, true, &env)
            .map(|id| id.0)
            .map_err(|e| format!("{program}: {e}")))
    }

    fn write(&mut self, process: u32, data: Vec<u8>) -> HostResult<Result<(), String>> {
        Ok(self
            .processes
            .write(ProcessId(process), &data)
            .map_err(|e| e.to_string()))
    }

    fn close_input(&mut self, process: u32) -> HostResult<()> {
        self.processes.close_stdin(ProcessId(process));
        Ok(())
    }

    fn kill(&mut self, process: u32) -> HostResult<()> {
        self.processes.kill(ProcessId(process));
        Ok(())
    }

    fn project_root(&mut self, path: String) -> HostResult<Option<String>> {
        Ok(crate::process::project_root(&expand_home(&path)).map(|p| p.display().to_string()))
    }

    fn error_output(&mut self, process: u32) -> HostResult<Vec<u8>> {
        Ok(self
            .error_output
            .get(&ProcessId(process))
            .cloned()
            .unwrap_or_default())
    }
}

// ----------------------------------------------------------- decorations

impl forge::editor::decorations::Host for PluginState {
    fn set(
        &mut self,
        b: Resource<BufferHandle>,
        items: Vec<forge::editor::decorations::Decoration>,
    ) -> HostResult<Result<(), String>> {
        let owner = self.name.clone();
        let buf = self.buffer(&b)?;
        let mut out = Vec::with_capacity(items.len());
        for d in items {
            let face = match crate::decoration::DecoFace::from_name(&d.face) {
                Ok(f) => f,
                Err(e) => return Ok(Err(e)),
            };
            let (a, z) = (
                buf.pos_to_offset(pos(d.span.start)),
                buf.pos_to_offset(pos(d.span.end)),
            );
            out.push(DecoSpec::new(a.min(z), a.max(z), face).message_opt(d.message));
        }
        buf.set_decorations(&owner, out);
        self.touched_decorations.insert(BufferId(b.rep()));
        Ok(Ok(()))
    }

    fn set_regions(
        &mut self,
        b: Resource<BufferHandle>,
        regions: Vec<forge::editor::decorations::Region>,
    ) -> HostResult<Result<(), String>> {
        let owner = self.name.clone();
        if regions.iter().any(|r| r.keymap.is_empty()) {
            return Ok(Err("a region map needs a name".into()));
        }
        let buf = self.buffer(&b)?;
        // A whole set, not an addition: the regions this plugin set last
        // time go, the faces it painted stay.
        buf.clear_decoration_keymaps(&owner);
        for r in regions {
            let (a, z) = (
                buf.pos_to_offset(pos(r.span.start)),
                buf.pos_to_offset(pos(r.span.end)),
            );
            let (start, end) = (a.min(z), a.max(z));
            if start < end {
                buf.set_decoration_keymap(&owner, start, end, &r.keymap);
            }
        }
        self.touched_decorations.insert(BufferId(b.rep()));
        Ok(Ok(()))
    }

    fn set_display(
        &mut self,
        b: Resource<BufferHandle>,
        items: Vec<forge::editor::decorations::Replacement>,
    ) -> HostResult<Result<(), String>> {
        let owner = self.name.clone();
        let buf = self.buffer(&b)?;
        let mut out = Vec::with_capacity(items.len());
        for r in items {
            let face = match r.face.as_deref() {
                Some(name) => match crate::decoration::DecoFace::from_name(name) {
                    Ok(f) => f,
                    Err(e) => return Ok(Err(e)),
                },
                None => crate::decoration::DecoFace::Plain,
            };
            let (a, z) = (
                buf.pos_to_offset(pos(r.span.start)),
                buf.pos_to_offset(pos(r.span.end)),
            );
            let display = match r.text {
                Some(t) => crate::decoration::Display::Text(t),
                None => crate::decoration::Display::Hide,
            };
            out.push(DecoSpec::new(a.min(z), a.max(z), face).display(display));
        }
        buf.set_display(&owner, out);
        self.touched_decorations.insert(BufferId(b.rep()));
        Ok(Ok(()))
    }

    fn clear(&mut self, b: Resource<BufferHandle>) -> HostResult<()> {
        let owner = self.name.clone();
        self.buffer(&b)?.clear_decorations(&owner);
        self.touched_decorations.insert(BufferId(b.rep()));
        Ok(())
    }

    fn syntax(
        &mut self,
        b: Resource<BufferHandle>,
        s: Span,
    ) -> HostResult<Vec<forge::editor::decorations::Decoration>> {
        self.buffer(&b)?;
        self.editor()?.sync_language(BufferId(b.rep()));
        let buf = self.buffer(&b)?;
        let (a, z) = (
            buf.pos_to_offset(pos(s.start)),
            buf.pos_to_offset(pos(s.end)),
        );
        let (a, z) = (a.min(z), a.max(z));
        let faces = buf.syntax_faces(a..z);
        let mut out = Vec::new();
        let mut i = 0;
        while i < faces.len() {
            let Some(face) = faces[i] else {
                i += 1;
                continue;
            };
            let start = i;
            while i < faces.len() && faces[i] == Some(face) {
                i += 1;
            }
            out.push(forge::editor::decorations::Decoration {
                span: span(buf.offset_to_pos(a + start), buf.offset_to_pos(a + i)),
                face: face.name().to_string(),
                message: None,
            });
        }
        Ok(out)
    }

    fn get(
        &mut self,
        b: Resource<BufferHandle>,
    ) -> HostResult<Vec<forge::editor::decorations::Decoration>> {
        self.buffer(&b)?;
        let buf = self.buffer(&b)?;
        Ok(buf
            .decorations()
            .iter()
            .map(|d| forge::editor::decorations::Decoration {
                span: span(buf.offset_to_pos(d.start), buf.offset_to_pos(d.end)),
                face: d.face.name().to_string(),
                message: d.message.clone(),
            })
            .collect())
    }
}

// ------------------------------------------------------------- images

impl forge::editor::images::Host for PluginState {
    fn set(
        &mut self,
        b: Resource<BufferHandle>,
        items: Vec<forge::editor::images::Image>,
    ) -> HostResult<()> {
        let owner = self.name.clone();
        let buf = self.buffer(&b)?;
        let images: Vec<(usize, std::path::PathBuf)> = items
            .iter()
            .map(|i| (i.line as usize, std::path::PathBuf::from(&i.path)))
            .collect();
        buf.set_plugin_images(&owner, images);
        self.touched_images.insert(BufferId(b.rep()));
        Ok(())
    }

    fn set_centered(
        &mut self,
        b: Resource<BufferHandle>,
        items: Vec<forge::editor::images::Image>,
    ) -> HostResult<()> {
        let owner = self.name.clone();
        let buf = self.buffer(&b)?;
        let images: Vec<(usize, std::path::PathBuf)> = items
            .iter()
            .map(|i| (i.line as usize, std::path::PathBuf::from(&i.path)))
            .collect();
        buf.set_plugin_images_centered(&owner, images);
        self.touched_images.insert(BufferId(b.rep()));
        Ok(())
    }

    fn builtin(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(crate::env::builtin_image(&name).map(|p| p.to_string_lossy().into_owned()))
    }

    fn clear(&mut self, b: Resource<BufferHandle>) -> HostResult<()> {
        let owner = self.name.clone();
        self.buffer(&b)?.clear_plugin_images(&owner);
        // Nothing to redraw beyond this buffer; a page turn drops the rest.
        self.touched_images.insert(BufferId(b.rep()));
        Ok(())
    }
}

// ------------------------------------------------------------- canvas

impl PluginState {
    fn scene(json: &str) -> Result<forge_canvas::Scene, String> {
        forge_canvas::Scene::from_json(json)
    }

    /// The canvas, if this plugin made it (a plugin only drives its own).
    fn canvas_of(&mut self, name: &str) -> Result<&mut crate::canvas::Canvas, String> {
        let owner = self.name.clone();
        let c = self
            .editor
            .as_mut()
            .ok_or_else(|| "no editor".to_string())?
            .canvases
            .get_mut(name)
            .ok_or_else(|| format!("no canvas named {name}"))?;
        if c.owner != owner {
            return Err(format!("canvas {name} belongs to {}", c.owner));
        }
        Ok(c)
    }
}

impl forge::editor::canvas::Host for PluginState {
    fn show(
        &mut self,
        name: String,
        scene: String,
    ) -> HostResult<Result<Resource<BufferHandle>, String>> {
        if let Err(e) = self.foreground("canvas.show") {
            return Ok(Err(e));
        }
        let scene = match Self::scene(&scene) {
            Ok(s) => s,
            Err(e) => return Ok(Err(e)),
        };
        if let Ok(c) = self.canvas_of(&name) {
            c.scene = scene;
            c.dirty = true;
            return Ok(Ok(handle(c.buffer)));
        }
        let owner = self.name.clone();
        if self
            .editor()?
            .canvases
            .get(&name)
            .is_some_and(|c| c.owner != owner)
        {
            return Ok(Err(format!("canvas {name} belongs to another plugin")));
        }
        let id = self.editor()?.canvas_show(&owner, &name, scene);
        Ok(Ok(handle(id)))
    }

    fn show_widgets(
        &mut self,
        name: String,
        tree: String,
        page_width: f32,
    ) -> HostResult<Result<Resource<BufferHandle>, String>> {
        if let Err(e) = self.foreground("canvas.show-widgets") {
            return Ok(Err(e));
        }
        let widget = match forge_canvas::Widget::parse(&tree) {
            Ok(w) => w,
            Err(e) => return Ok(Err(e)),
        };
        let page = if page_width >= 200.0 {
            page_width
        } else {
            720.0
        };
        let scene = forge_canvas::layout_widgets(&widget, page);
        let json = match serde_json::to_string(&scene) {
            Ok(j) => j,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let shown = self.show(name.clone(), json)?;
        if shown.is_ok() {
            if let Ok(c) = self.canvas_of(&name) {
                c.scroll = true;
                c.scroll_w = page;
                c.frame = None;
                c.dirty = true;
            }
        }
        Ok(shown)
    }

    fn display(&mut self, name: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("canvas.display") {
            return Ok(Err(e));
        }
        let id = match self.canvas_of(&name) {
            Ok(c) => c.buffer,
            Err(e) => return Ok(Err(e)),
        };
        self.editor()?.show_buffer(id);
        Ok(Ok(()))
    }

    fn on_event(&mut self, name: String, command: String) -> HostResult<Result<(), String>> {
        Ok(self.canvas_of(&name).map(|c| c.listener = Some(command)))
    }

    fn last_event(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(self
            .canvas_of(&name)
            .ok()
            .and_then(|c| c.last_event.clone()))
    }

    fn set_view(
        &mut self,
        name: String,
        x: f32,
        y: f32,
        zoom: f32,
    ) -> HostResult<Result<(), String>> {
        Ok(self.canvas_of(&name).map(|c| {
            c.view = crate::canvas::View {
                x,
                y,
                zoom: zoom.clamp(crate::canvas::MIN_ZOOM, crate::canvas::MAX_ZOOM),
            };
            c.dirty = true;
        }))
    }

    fn get_view(&mut self, name: String) -> HostResult<Option<(f32, f32, f32)>> {
        Ok(self
            .canvas_of(&name)
            .ok()
            .map(|c| (c.view.x, c.view.y, c.view.zoom)))
    }

    fn set_scroll(
        &mut self,
        name: String,
        yes: bool,
        page_width: f32,
    ) -> HostResult<Result<(), String>> {
        Ok(self.canvas_of(&name).map(|c| {
            c.scroll = yes;
            c.scroll_w = page_width.max(0.0);
            c.frame = None;
            c.dirty = true;
        }))
    }

    fn handle_view_events(&mut self, name: String, yes: bool) -> HostResult<Result<(), String>> {
        Ok(self.canvas_of(&name).map(|c| c.handles_view = yes))
    }

    fn close(&mut self, name: String) -> HostResult<()> {
        self.foreground_only("canvas.close")?;
        if let Ok(c) = self.canvas_of(&name) {
            let id = c.buffer;
            let ed = self.editor()?;
            ed.canvases.remove_buffer(id);
            ed.kill_buffer(id, None);
        }
        Ok(())
    }

    fn names(&mut self) -> HostResult<Vec<String>> {
        let owner = self.name.clone();
        Ok(self
            .editor()?
            .canvases
            .iter()
            .filter(|(_, c)| c.owner == owner)
            .map(|(n, _)| n.clone())
            .collect())
    }

    fn render_png(
        &mut self,
        scene: String,
        path: String,
        scale: f32,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("canvas.render-png") {
            return Ok(Err(e));
        }
        let p = expand_home(&path);
        if let Err(e) = self.may_write(&p) {
            return Ok(Err(e));
        }
        let scene = match Self::scene(&scene) {
            Ok(s) => s,
            Err(e) => return Ok(Err(e)),
        };
        let Some(png) = crate::canvas::render_png(&scene, scale) else {
            return Ok(Err("the scene is empty or too large to draw".into()));
        };
        if let Some(d) = p.parent()
            && let Err(e) = std::fs::create_dir_all(d)
        {
            return Ok(Err(format!("{}: {e}", d.display())));
        }
        Ok(std::fs::write(&p, png).map_err(|e| format!("{path}: {e}")))
    }
}

// ---------------------------------------------------------------- http

impl forge::editor::http::Host for PluginState {
    fn get_async(
        &mut self,
        url: String,
        headers: Vec<(String, String)>,
    ) -> HostResult<Result<u32, String>> {
        if !self.grants.http {
            return Ok(Err(format!(
                "{url}: this plugin may not make web requests (the http permission in its manifest)"
            )));
        }
        Ok(Ok(self.requests.start("GET", &url, headers, None).0))
    }

    fn http_request_async(
        &mut self,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> HostResult<Result<u32, String>> {
        if !self.grants.http {
            return Ok(Err(format!(
                "{url}: this plugin may not make web requests (the http permission in its manifest)"
            )));
        }
        Ok(Ok(self.requests.start(&method, &url, headers, body).0))
    }

    fn cancel(&mut self, request: u32) -> HostResult<()> {
        self.requests.cancel(crate::http::RequestId(request));
        Ok(())
    }
}

// -------------------------------------------------------------- commands

impl forge::editor::commands::Host for PluginState {
    fn register(&mut self, name: String, doc: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("commands.register") {
            return Ok(Err(e));
        }
        let plugin = self.name.clone();
        self.background_commands.remove(&name);
        Ok(self
            .editor()?
            .commands
            .register_plugin(&name, &doc, &plugin))
    }

    fn register_background(&mut self, name: String, doc: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("commands.register-background") {
            return Ok(Err(e));
        }
        let plugin = self.name.clone();
        let r = self
            .editor()?
            .commands
            .register_plugin(&name, &doc, &plugin);
        if r.is_ok() {
            self.background_commands.insert(name);
        }
        Ok(r)
    }

    fn run(&mut self, name: String) -> HostResult<Result<(), String>> {
        if !self.editor()?.commands.contains(&name) {
            return Ok(Err(format!("no command named {name}")));
        }
        self.effects.push(Effect::RunCommand(name));
        Ok(Ok(()))
    }

    fn exists(&mut self, name: String) -> HostResult<bool> {
        Ok(self.editor()?.commands.contains(&name))
    }

    fn doc(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(self.editor()?.commands.get(&name).map(|c| c.doc.clone()))
    }

    fn run_when_idle(&mut self, name: String, seconds: f64) -> HostResult<Result<(), String>> {
        let plugin = self.name.clone();
        let ed = self.editor()?;
        let owned = matches!(
            ed.commands.get(&name).map(|c| &c.source),
            Some(crate::commands::CommandSource::Plugin(p)) if *p == plugin
        );
        if !owned {
            return Ok(Err(format!("{name} isn't a command of plugin {plugin}")));
        }
        ed.idle_hooks
            .retain(|h| !(h.plugin == plugin && h.command == name));
        if seconds > 0.0 && seconds.is_finite() {
            ed.idle_hooks.push(crate::app::IdleHook {
                plugin,
                command: name,
                seconds,
            });
        }
        Ok(Ok(()))
    }

    fn names(&mut self) -> HostResult<Vec<String>> {
        Ok(self
            .editor()?
            .commands
            .names()
            .map(str::to_string)
            .collect())
    }
}

// ---------------------------------------------------------------- keymap

impl forge::editor::keymap::Host for PluginState {
    fn bind(
        &mut self,
        scope: forge::editor::keymap::Scope,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("keymap.bind") {
            return Ok(Err(e));
        }
        let scope = Self::scope(scope);
        match self
            .editor()?
            .keymaps
            .bind(&scope, &keys, &command, Origin::Package)
        {
            Ok(seq) => {
                self.bindings.push((scope, seq.join(" "), command));
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    fn unbind(
        &mut self,
        scope: forge::editor::keymap::Scope,
        keys: String,
    ) -> HostResult<Result<bool, String>> {
        if let Err(e) = self.foreground("keymap.unbind") {
            return Ok(Err(e));
        }
        let scope = Self::scope(scope);
        Ok(self
            .editor()?
            .keymaps
            .unbind(&scope, &keys)
            .map_err(|e| e.to_string()))
    }

    fn remap(
        &mut self,
        scope: forge::editor::keymap::Scope,
        from: String,
        to: String,
    ) -> HostResult<()> {
        self.foreground_only("keymap.remap")?;
        let scope = Self::scope(scope);
        self.editor()?.keymaps.remap(&scope, &from, &to);
        self.remaps.push((scope, from, to));
        Ok(())
    }

    fn lookup(&mut self, keys: String) -> HostResult<Option<String>> {
        let Ok(seq) = parse_keys(&keys) else {
            return Ok(None);
        };
        Ok(match self.editor()?.active_keymaps().lookup(&seq) {
            Lookup::Command { command, .. } => Some(command),
            _ => None,
        })
    }

    fn is_prefix(&mut self, keys: String) -> HostResult<bool> {
        let Ok(seq) = parse_keys(&keys) else {
            return Ok(false);
        };
        Ok(matches!(
            self.editor()?.active_keymaps().lookup(&seq),
            Lookup::Prefix
        ))
    }

    fn execute_keys(&mut self, keys: String) -> HostResult<Result<(), String>> {
        if let Err(e) = parse_keys(&keys) {
            return Ok(Err(e.to_string()));
        }
        self.effects.push(Effect::ExecuteKeys(keys));
        Ok(Ok(()))
    }

    fn where_is(&mut self, command: String) -> HostResult<Vec<String>> {
        Ok(self.editor()?.active_keymaps().where_is(&command))
    }

    fn set_priority(
        &mut self,
        scope: forge::editor::keymap::Scope,
        priority: i32,
    ) -> HostResult<i32> {
        if self.foreground("keymap.set-priority").is_err() {
            // Nothing has changed, but the previous priority is still the
            // answer the caller asked for.
            return Ok(self.editor()?.keymaps.priority(&Self::scope(scope)));
        }
        let scope = Self::scope(scope);
        let previous = self.editor()?.keymaps.priority(&scope);
        self.editor()?.keymaps.set_priority(&scope, priority);
        Ok(previous)
    }

    fn reset_priority(&mut self, scope: forge::editor::keymap::Scope) -> HostResult<bool> {
        self.foreground_only("keymap.reset-priority")?;
        let scope = Self::scope(scope);
        Ok(self.editor()?.keymaps.reset_priority(&scope))
    }

    fn bind_transient(
        &mut self,
        name: String,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("keymap.bind-transient") {
            return Ok(Err(e));
        }
        if name.is_empty() {
            return Ok(Err("a transient map needs a name".into()));
        }
        let scope = Scope::Transient(name);
        let Ok(seq) = parse_keys(&keys) else {
            return Ok(Err(format!("not valid key notation: {keys}")));
        };
        match self
            .editor()?
            .keymaps
            .bind_seq(&scope, &seq, &command, Origin::Package)
        {
            Ok(()) => {
                self.bindings.push((scope, seq.join(" "), command));
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    fn bind_layer(
        &mut self,
        name: String,
        when: String,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("keymap.bind-layer") {
            return Ok(Err(e));
        }
        if name.is_empty() {
            return Ok(Err("a conditional layer needs a name".into()));
        }
        let condition = match Condition::parse(&when) {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        let scope = Scope::Layer(name.clone());
        let Ok(seq) = parse_keys(&keys) else {
            return Ok(Err(format!("not valid key notation: {keys}")));
        };
        // The condition goes on before the binding, so a rejected binding
        // can't leave a layer that is up with nothing in it.
        self.editor()?.keymaps.add_layer(&name, condition);
        match self
            .editor()?
            .keymaps
            .bind_seq(&scope, &seq, &command, Origin::Package)
        {
            Ok(()) => {
                self.bindings.push((scope, seq.join(" "), command));
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    fn remove_layer(&mut self, name: String) -> HostResult<Result<bool, String>> {
        if let Err(e) = self.foreground("keymap.remove-layer") {
            return Ok(Err(e));
        }
        let editor = self.editor()?;
        editor.keymaps.remove_layer(&name);
        self.bindings
            .retain(|(s, _, _)| !matches!(s, Scope::Layer(n) if *n == name));
        Ok(Ok(true))
    }

    fn bind_prompt(
        &mut self,
        name: String,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        self.bind_named(
            &name,
            &keys,
            &command,
            Scope::Prompt,
            "keymap.bind-prompt",
            "a prompt map",
        )
    }

    fn bind_region(
        &mut self,
        name: String,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        self.bind_named(
            &name,
            &keys,
            &command,
            Scope::Region,
            "keymap.bind-region",
            "a region map",
        )
    }

    fn bind_project(
        &mut self,
        root: String,
        keys: String,
        command: String,
    ) -> HostResult<Result<(), String>> {
        // A path, not a name, so it isn't refused for being empty; an
        // empty one simply has no file under it.
        let Ok(seq) = parse_keys(&keys) else {
            return Ok(Err(format!("not valid key notation: {keys}")));
        };
        if let Err(e) = self.foreground("keymap.bind-project") {
            return Ok(Err(e));
        }
        let scope = Scope::Project(canonical(&root));
        match self
            .editor()?
            .keymaps
            .bind_seq(&scope, &seq, &command, Origin::Package)
        {
            Ok(()) => {
                self.bindings
                    .push((scope, seq.join(" "), command.to_string()));
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    fn remove_project(&mut self, root: String) -> HostResult<Result<bool, String>> {
        if let Err(e) = self.foreground("keymap.remove-project") {
            return Ok(Err(e));
        }
        let scope = Scope::Project(canonical(&root));
        self.bindings.retain(|(s, _, _)| *s != scope);
        Ok(Ok(self.editor()?.keymaps.remove_project(&canonical(&root))))
    }

    fn remove_region(&mut self, name: String) -> HostResult<Result<bool, String>> {
        if let Err(e) = self.foreground("keymap.remove-region") {
            return Ok(Err(e));
        }
        let scope = Scope::Region(name.clone());
        self.bindings.retain(|(s, _, _)| *s != scope);
        Ok(Ok(self.editor()?.keymaps.remove_region(&name)))
    }

    fn push_transient(
        &mut self,
        name: String,
        opts: Vec<forge::editor::keymap::TransientOpts>,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("keymap.push-transient") {
            return Ok(Err(e));
        }
        if name.is_empty() {
            return Ok(Err("a transient map needs a name".into()));
        }
        let mut parsed = crate::keymap::TransientOpts::default();
        for opt in opts {
            match opt {
                forge::editor::keymap::TransientOpts::OneShot => parsed.one_shot = true,
                forge::editor::keymap::TransientOpts::OnUnbound(p) => {
                    parsed.on_unbound = Some(match p {
                        forge::editor::keymap::OnUnbound::Stay => crate::keymap::OnUnbound::Stay,
                        forge::editor::keymap::OnUnbound::Swallow => {
                            crate::keymap::OnUnbound::Swallow
                        }
                        forge::editor::keymap::OnUnbound::Exit => crate::keymap::OnUnbound::Exit,
                    })
                }
                forge::editor::keymap::TransientOpts::Timeout(secs) => {
                    parsed.timeout = Some(parsed.timeout.unwrap_or(0.0).max(secs))
                }
            }
        }
        self.editor()?.push_transient(&name, parsed);
        Ok(Ok(()))
    }

    fn pop_transient(&mut self, name: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("keymap.pop-transient") {
            return Ok(Err(e));
        }
        if self.editor()?.pop_transient(&name) {
            Ok(Ok(()))
        } else {
            Ok(Err(format!("no transient map called {name} is up")))
        }
    }

    fn transients(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.live_transients())
    }

    fn bindings(
        &mut self,
        scope: forge::editor::keymap::Scope,
    ) -> HostResult<Vec<(String, String)>> {
        let scope = Self::scope(scope);
        Ok(self
            .editor()?
            .keymaps
            .get(&scope)
            .map(|m| {
                m.bindings()
                    .map(|(k, c)| (k.to_string(), c.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }
}

// ---------------------------------------------------------------- events

impl forge::editor::events::Host for PluginState {
    fn subscribe(&mut self, kind: EventKind) -> HostResult<()> {
        self.foreground_only("events.subscribe")?;
        self.subscriptions[kind_index(kind)] = true;
        self.background_events[kind_index(kind)] = false;
        Ok(())
    }

    fn unsubscribe(&mut self, kind: EventKind) -> HostResult<()> {
        self.foreground_only("events.unsubscribe")?;
        self.subscriptions[kind_index(kind)] = false;
        self.background_events[kind_index(kind)] = false;
        Ok(())
    }

    fn subscribe_background(&mut self, kind: EventKind) -> HostResult<()> {
        self.foreground_only("events.subscribe-background")?;
        self.subscriptions[kind_index(kind)] = true;
        self.background_events[kind_index(kind)] = true;
        Ok(())
    }
}

// ------------------------------------------------------------ minibuffer

impl forge::editor::minibuffer::Host for PluginState {
    fn read_string(&mut self, prompt: String, initial: String) -> HostResult<u32> {
        Ok(self.ask(Question::Text { prompt, initial }))
    }

    fn completing_read(
        &mut self,
        prompt: String,
        candidates: Vec<String>,
        require_match: bool,
    ) -> HostResult<u32> {
        Ok(self.ask(Question::Completing {
            prompt,
            candidates,
            require_match,
            multiple: false,
            over: false,
        }))
    }

    fn completing_read_over(
        &mut self,
        prompt: String,
        candidates: Vec<String>,
        require_match: bool,
    ) -> HostResult<u32> {
        Ok(self.ask(Question::Completing {
            prompt,
            candidates,
            require_match,
            multiple: false,
            over: true,
        }))
    }

    fn active(&mut self) -> HostResult<bool> {
        Ok(self.editor()?.prompt_open)
    }

    fn history_kind(&mut self) -> HostResult<Option<String>> {
        Ok(self.editor()?.prompt_history.clone())
    }

    fn input(&mut self) -> HostResult<String> {
        Ok(self.editor()?.prompt_input.clone())
    }

    fn on_input_changed(&mut self, command: String) -> HostResult<()> {
        self.foreground_only("minibuffer.on-input-changed")?;
        let name = self.name.clone();
        let ed = self.editor()?;
        if !ed.commands.contains(&command) {
            return Err(wasmtime::Error::msg(format!("no command named {command}")));
        }
        ed.completion
            .minibuffer_input_commands
            .retain(|(plugin, _)| plugin != &name);
        ed.completion
            .minibuffer_input_commands
            .push((name, command));
        Ok(())
    }

    fn on_selection_changed(&mut self, command: String) -> HostResult<()> {
        self.foreground_only("minibuffer.on-selection-changed")?;
        let name = self.name.clone();
        let ed = self.editor()?;
        if !ed.commands.contains(&command) {
            return Err(wasmtime::Error::msg(format!("no command named {command}")));
        }
        ed.completion
            .minibuffer_selection_commands
            .retain(|(plugin, _)| plugin != &name);
        ed.completion
            .minibuffer_selection_commands
            .push((name, command));
        Ok(())
    }

    fn y_or_n(&mut self, prompt: String) -> HostResult<u32> {
        Ok(self.ask(Question::YOrN { prompt }))
    }

    fn read_key(&mut self, prompt: String) -> HostResult<u32> {
        Ok(self.ask(Question::Key { prompt }))
    }

    fn message(&mut self, text: String) -> HostResult<()> {
        self.effects.push(Effect::Message(text));
        Ok(())
    }

    fn current_message(&mut self) -> HostResult<Option<String>> {
        // A message this call already asked for counts too.
        if let Some(Effect::Message(m)) = self
            .effects
            .iter()
            .rev()
            .find(|e| matches!(e, Effect::Message(_)))
        {
            return Ok(Some(m.clone()));
        }
        Ok(self.editor()?.echo_area.clone())
    }
}

// ----------------------------------------------------------------- modes

/// The menu bar, for plugins (see the `menus` interface).
impl forge::editor::menus::Host for PluginState {
    fn define(&mut self, title: String, items: Vec<Item>) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.define") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let items = items
            .into_iter()
            .map(|i| crate::menu::Item::new(i.label, i.command));
        Ok(self.editor()?.menus.define(&title, items.collect(), &owner))
    }

    fn popup(&mut self, items: Vec<Item>) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.popup") {
            return Ok(Err(e));
        }
        self.effects.push(Effect::Popup(
            items.into_iter().map(|i| (i.label, i.command)).collect(),
        ));
        Ok(Ok(()))
    }

    fn define_for_mode(
        &mut self,
        title: String,
        items: Vec<Item>,
        mode: String,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.define-for-mode") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let items = items
            .into_iter()
            .map(|i| crate::menu::Item::new(i.label, i.command));
        Ok(self.editor()?.menus.define_when(
            &title,
            items.collect(),
            &owner,
            crate::menu::When::MajorMode(mode),
        ))
    }

    fn extend(&mut self, title: String, items: Vec<Item>) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.extend") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let items = items
            .into_iter()
            .map(|i| crate::menu::Item::new(i.label, i.command));
        Ok(self.editor()?.menus.extend(&title, items.collect(), &owner))
    }

    fn extend_submenu(
        &mut self,
        menu: String,
        label: String,
        items: Vec<Item>,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.extend-submenu") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let items = items
            .into_iter()
            .map(|i| crate::menu::Item::new(i.label, i.command));
        Ok(self
            .editor()?
            .menus
            .extend_submenu(&menu, &label, items.collect(), &owner))
    }

    fn remove(&mut self, title: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("menus.remove") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        Ok(self.editor()?.menus.remove(&title, &owner))
    }

    fn set_enabled(&mut self, enabled: bool) -> HostResult<()> {
        self.foreground_only("menus.set-enabled")?;
        let mode = crate::modes::MENU_BAR_MODE;
        let ed = self.editor()?;
        // The selected window's buffer, the way every other per-buffer
        // setting works.
        let id = ed.windows.selected().buffer;
        let b = ed
            .state
            .get_buffer_mut(id)
            .expect("the selected window's buffer is still open");
        if enabled {
            b.enable_minor_mode(mode);
        } else {
            b.disable_minor_mode(mode);
        }
        Ok(())
    }

    fn enabled(&mut self) -> HostResult<bool> {
        self.foreground_only("menus.enabled")?;
        let ed = self.editor()?;
        let id = ed.windows.selected().buffer;
        Ok(ed
            .state
            .get_buffer(id)
            .is_some_and(|b| b.minor_mode_enabled(crate::modes::MENU_BAR_MODE)))
    }

    fn open(&mut self, title: String) -> HostResult<()> {
        self.foreground_only("menus.open")?;
        let ed = self.editor()?;
        if title.is_empty() {
            ed.menu.close();
            return Ok(());
        }
        match ed.menus.index_of(&title) {
            Some(i) => {
                ed.menu.open = Some(i);
                ed.menu.item = 0;
                ed.menu.hover = Some(i);
            }
            // A message in the echo area, like a command that names
            // something that isn't there.
            None => self
                .effects
                .push(Effect::Message(format!("there is no {title} menu"))),
        }
        Ok(())
    }

    fn close(&mut self) -> HostResult<()> {
        self.foreground_only("menus.close")?;
        self.editor()?.menu.close();
        Ok(())
    }

    fn open_title(&mut self) -> HostResult<Option<String>> {
        self.foreground_only("menus.open-title")?;
        let ed = self.editor()?;
        Ok(ed
            .menu
            .open
            .and_then(|i| ed.menus.menu(i))
            .map(|m| m.title.clone()))
    }

    fn choose(&mut self) -> HostResult<bool> {
        self.foreground_only("menus.choose")?;
        let ed = self.editor()?;
        let Some(item) = ed.menu.selected(&ed.menus) else {
            return Ok(false);
        };
        let command = item.command.clone();
        ed.menu.close();
        // Run it the way commands.run does: the editor's command table
        // lives above this crate, so ask the frontend to call it.
        self.effects.push(Effect::RunCommand(command));
        Ok(true)
    }

    fn selection(&mut self) -> HostResult<Option<ItemSelection>> {
        self.foreground_only("menus.selection")?;
        let ed = self.editor()?;
        let Some(menu_index) = ed.menu.open else {
            return Ok(None);
        };
        let Some(menu) = ed.menus.menu(menu_index) else {
            return Ok(None);
        };
        let Some(item) = ed.menu.selected(&ed.menus) else {
            return Ok(None);
        };
        Ok(Some(ItemSelection {
            menu: menu.title.clone(),
            label: item.label.clone(),
            command: item.command.clone(),
            index: ed.menu.item as u32 + 1,
            total: menu.items.len() as u32,
            // The same answer the menu draws from, so a plugin describing
            // the selection says the same thing the greyed-out item does.
            available: ed.commands.contains(&item.command)
                && item.enabled.holds(&crate::menu::MenuState::of(ed)),
        }))
    }
}

impl forge::editor::modes::Host for PluginState {
    fn define_major_mode(
        &mut self,
        name: String,
        parent: Option<String>,
        extensions: Vec<String>,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("modes.define-major-mode") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let ed = self.editor()?;
        if let Err(e) = ed
            .modes
            .define_major(&name, parent.as_deref(), &extensions, Some(&owner))
        {
            return Ok(Err(e));
        }
        if let Some(p) = &parent {
            ed.keymaps.set_mode_parent(&name, p);
        }
        Ok(Ok(()))
    }

    fn set_lighter(&mut self, mode: String, lighter: String) -> HostResult<Result<(), String>> {
        let owner = self.name.clone();
        Ok(self
            .editor()?
            .modes
            .set_lighter(&mode, &lighter, Some(&owner)))
    }

    fn define_minor_mode(&mut self, name: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("modes.define-minor-mode") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        Ok(self.editor()?.modes.define_minor(&name, Some(&owner)))
    }

    fn major_modes(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.modes.names(crate::modes::ModeKind::Major))
    }

    fn minor_modes(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.modes.names(crate::modes::ModeKind::Minor))
    }

    fn major_mode_parent(&mut self, mode: String) -> HostResult<Option<String>> {
        Ok(self
            .editor()?
            .modes
            .get(&mode)
            .and_then(|d| d.parent.clone()))
    }
}

// -------------------------------------------------------------- kill-ring

impl forge::editor::kill_ring::Host for PluginState {
    fn length(&mut self) -> HostResult<u32> {
        Ok(self.editor()?.kill_ring.len() as u32)
    }

    fn entries(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.kill_ring.entries())
    }

    fn current(&mut self) -> HostResult<Option<String>> {
        Ok(self.editor()?.kill_ring.current().map(str::to_string))
    }

    fn get(&mut self, index: u32) -> HostResult<Option<String>> {
        let ring = &self.editor()?.kill_ring;
        if ring.is_empty() {
            return Ok(None);
        }
        Ok(ring.get(index as usize % ring.len()).map(str::to_string))
    }

    fn rotate(&mut self) -> HostResult<Option<String>> {
        Ok(self.editor()?.kill_ring.rotate().map(str::to_string))
    }

    fn select(&mut self, index: u32) -> HostResult<()> {
        self.editor()?.kill_ring.select(index as usize);
        Ok(())
    }

    fn push(&mut self, text: String) -> HostResult<()> {
        self.foreground_only("kill-ring.push")?;
        self.effects.push(Effect::Kill(text));
        Ok(())
    }
}

// --------------------------------------------------------------- session

impl forge::editor::session::Host for PluginState {
    fn history(&mut self, kind: String) -> HostResult<Vec<String>> {
        let mut items: Vec<String> = self.editor()?.session.history(&kind).to_vec();
        items.reverse();
        Ok(items)
    }

    fn bookmark_names(&mut self) -> HostResult<Vec<String>> {
        let mut names = self.editor()?.session.bookmark_names();
        names.reverse();
        Ok(names)
    }

    fn bookmark(
        &mut self,
        name: String,
    ) -> HostResult<Option<forge::editor::session::BookmarkInfo>> {
        Ok(self
            .editor()?
            .session
            .bookmark(&name)
            .map(|b| forge::editor::session::BookmarkInfo {
                name: b.name,
                path: b.path,
                offset: b.offset as u32,
            }))
    }

    fn recent_files(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.session.recent_files())
    }
}

// ------------------------------------------------------------- registers

impl forge::editor::registers::Host for PluginState {
    fn length(&mut self) -> HostResult<u32> {
        Ok(self.editor()?.registers.len() as u32)
    }

    fn names(&mut self) -> HostResult<Vec<String>> {
        Ok(self
            .editor()?
            .registers
            .keys()
            .map(|c| c.to_string())
            .collect())
    }

    fn describe(&mut self, name: String) -> HostResult<Option<String>> {
        let Some(c) = one_char(&name) else {
            return Ok(None);
        };
        Ok(self
            .editor()?
            .registers
            .get(&c)
            .map(crate::registers::describe))
    }

    fn text(&mut self, name: String) -> HostResult<Option<String>> {
        let Some(c) = one_char(&name) else {
            return Ok(None);
        };
        Ok(self
            .editor()?
            .registers
            .get(&c)
            .and_then(crate::registers::as_text))
    }

    fn position(&mut self, name: String) -> HostResult<Option<forge::editor::marks::PositionMark>> {
        let Some(c) = one_char(&name) else {
            return Ok(None);
        };
        let ed = self.editor()?;
        let Some((id, offset)) = ed.registers.get(&c).and_then(|r| match r {
            crate::registers::Register::Position { buffer, offset, .. } => Some((*buffer, *offset)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let p = ed
            .state
            .get_buffer(id)
            .map(|b| b.offset_to_pos(offset))
            .unwrap_or_default();
        Ok(Some(position_mark(id, offset, p)))
    }
}

/// A register name is one character. None when `name` isn't exactly one.
fn one_char(name: &str) -> Option<char> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

// --------------------------------------------------------------- kmacro

impl forge::editor::kmacro::Host for PluginState {
    fn length(&mut self) -> HostResult<u32> {
        let k = &self.editor()?.kmacro;
        Ok(k.last.is_some() as u32 + k.ring.len() as u32)
    }

    fn last(&mut self) -> HostResult<Option<Vec<String>>> {
        Ok(self.editor()?.kmacro.last.clone())
    }

    fn entries(&mut self) -> HostResult<Vec<Vec<String>>> {
        Ok(self.editor()?.kmacro.entries())
    }

    fn select(&mut self, index: u32) -> HostResult<()> {
        self.editor()?.kmacro.select(index as usize);
        Ok(())
    }
}

// ----------------------------------------------------------------- marks

impl forge::editor::marks::Host for PluginState {
    fn buffer_marks(&mut self) -> HostResult<Vec<forge::editor::marks::PositionMark>> {
        let ed = self.editor()?;
        let Some(id) = ed.state.active_buffer else {
            return Ok(Vec::new());
        };
        let Some(buf) = ed.state.get_buffer(id) else {
            return Ok(Vec::new());
        };
        Ok(buf
            .mark_ring()
            .into_iter()
            .map(|(offset, p)| position_mark(id, offset, p))
            .collect())
    }

    fn global_marks(&mut self) -> HostResult<Vec<forge::editor::marks::PositionMark>> {
        let ed = self.editor()?;
        let marks = ed.global_mark_ring.clone();
        Ok(marks
            .into_iter()
            .filter(|(id, _)| ed.state.get_buffer(*id).is_some())
            .map(|(id, offset)| {
                let p = ed
                    .state
                    .get_buffer(id)
                    .map(|b| b.offset_to_pos(offset))
                    .unwrap_or_default();
                position_mark(id, offset, p)
            })
            .collect())
    }

    fn pop_mark(&mut self) -> HostResult<()> {
        if let Some(buf) = self.editor()?.state.active_buffer_mut() {
            buf.pop_mark();
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ xref

impl forge::editor::xref::Host for PluginState {
    fn publish(&mut self, items: Vec<forge::editor::xref::Reference>) -> HostResult<()> {
        let owner = self.name.clone();
        let ed = self.editor()?;
        ed.xrefs.retain(|(o, _)| o != &owner);
        ed.xrefs.extend(items.into_iter().map(|r| {
            (
                owner.clone(),
                crate::xref::Xref {
                    name: r.name,
                    kind: r.kind,
                    path: r.path,
                    line: r.line,
                    column: r.column,
                },
            )
        }));
        Ok(())
    }

    fn all(&mut self) -> HostResult<Vec<forge::editor::xref::Reference>> {
        Ok(self
            .editor()?
            .xrefs
            .iter()
            .map(|(_, x)| forge::editor::xref::Reference {
                name: x.name.clone(),
                kind: x.kind.clone(),
                path: x.path.clone(),
                line: x.line,
                column: x.column,
            })
            .collect())
    }
}

impl forge::editor::theme::Host for PluginState {
    fn define(
        &mut self,
        name: String,
        description: String,
        faces: Vec<(String, forge::editor::theme::FaceStyle)>,
    ) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("theme.define") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        let mut list = Vec::with_capacity(faces.len());
        for (face, style) in faces {
            match face_style(style) {
                Ok(style) => list.push((face, style)),
                Err(e) => return Ok(Err(e)),
            }
        }
        Ok(self.editor()?.themes.define(
            &owner,
            &name,
            crate::theme::NamedTheme {
                description,
                faces: list,
            },
        ))
    }

    fn undefine(&mut self, name: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("theme.undefine") {
            return Ok(Err(e));
        }
        let owner = self.name.clone();
        if !self.editor()?.themes.is_owned_by(&name, &owner) {
            return Ok(Err(format!(
                "no theme named {name:?} defined by this plugin"
            )));
        }
        self.editor()?.themes.undefine(&name);
        Ok(Ok(()))
    }

    fn names(&mut self) -> HostResult<Vec<String>> {
        Ok(self.editor()?.themes.names())
    }

    fn describe(&mut self, name: String) -> HostResult<Option<String>> {
        Ok(self.editor()?.themes.description(&name).map(str::to_string))
    }

    fn current(&mut self) -> HostResult<Option<String>> {
        Ok(self.editor()?.themes.current().map(str::to_string))
    }

    fn load(&mut self, name: String) -> HostResult<Result<(), String>> {
        if let Err(e) = self.foreground("theme.load") {
            return Ok(Err(e));
        }
        Ok(self.editor()?.load_theme(&name))
    }

    fn disable(&mut self) -> HostResult<()> {
        self.foreground_only("theme.disable")?;
        self.editor()?.disable_theme();
        Ok(())
    }

    fn palette(&mut self) -> HostResult<Vec<(String, String)>> {
        Ok(self.editor()?.canvas_palette().to_pairs())
    }
}

impl forge::editor::org::Host for PluginState {
    fn files(&mut self) -> HostResult<Vec<String>> {
        Ok(self
            .editor()?
            .org_agenda_files()
            .iter()
            .map(|p| p.display().to_string())
            .collect())
    }

    fn span(
        &mut self,
        days: u32,
        start: Option<String>,
        log: bool,
    ) -> HostResult<Result<Vec<forge::editor::org::AgendaEntry>, String>> {
        let start = match start {
            Some(s) => match crate::org_time::Date::parse_iso(&s) {
                Some(d) => Some(d),
                None => return Ok(Err(format!("bad date {s:?} (want YYYY-MM-DD)"))),
            },
            None => None,
        };
        self.with_agenda("org.span", move |sources, ids, names| {
            let (today, _) = crate::org_time::now_local();
            let from = start.unwrap_or(today);
            let opts = crate::org_agenda::Options {
                log,
                ..Default::default()
            };
            let entries = crate::org_agenda::collect_span(
                sources,
                from,
                i64::from(days.max(1)),
                today,
                &opts,
            );
            Ok(entries.iter().map(|e| wit_entry(e, ids, names)).collect())
        })
    }

    fn todos(
        &mut self,
        keyword: Option<String>,
    ) -> HostResult<Result<Vec<forge::editor::org::AgendaEntry>, String>> {
        self.with_agenda("org.todos", move |sources, ids, names| {
            let opts = crate::org_agenda::Options::default();
            let entries = crate::org_agenda::collect_todos(sources, keyword.as_deref(), &opts);
            Ok(entries.iter().map(|e| wit_entry(e, ids, names)).collect())
        })
    }

    fn matching(
        &mut self,
        matcher: String,
        todo_only: bool,
    ) -> HostResult<Result<Vec<forge::editor::org::AgendaEntry>, String>> {
        self.with_agenda("org.matching", move |sources, ids, names| {
            let opts = crate::org_agenda::Options::default();
            let entries = crate::org_agenda::collect_match(sources, &matcher, todo_only, &opts)?;
            Ok(entries.iter().map(|e| wit_entry(e, ids, names)).collect())
        })
    }

    fn search(
        &mut self,
        query: String,
    ) -> HostResult<Result<Vec<forge::editor::org::AgendaEntry>, String>> {
        self.with_agenda("org.search", move |sources, ids, names| {
            let opts = crate::org_agenda::Options::default();
            let entries = crate::org_agenda::collect_search(sources, &query, &opts)?;
            Ok(entries.iter().map(|e| wit_entry(e, ids, names)).collect())
        })
    }

    fn stuck(&mut self) -> HostResult<Result<Vec<forge::editor::org::AgendaEntry>, String>> {
        self.with_agenda("org.stuck", |sources, ids, names| {
            let opts = crate::org_agenda::Options::default();
            let entries = crate::org_agenda::collect_stuck(sources, &opts);
            Ok(entries.iter().map(|e| wit_entry(e, ids, names)).collect())
        })
    }
}

/// A WIT face style as the editor's own, its colors parsed now.
fn face_style(s: forge::editor::theme::FaceStyle) -> Result<crate::theme::FaceStyle, String> {
    fn color(c: Option<String>) -> Result<Option<crate::theme::Rgb>, String> {
        c.map(|c| crate::theme::parse_color(&c)).transpose()
    }
    Ok(crate::theme::FaceStyle {
        fg: color(s.fg)?,
        bg: color(s.bg)?,
        bold: s.bold,
        italic: s.italic,
        underline: s.underline,
    })
}

impl PluginState {
    /// Build the agenda sources from the editor's agenda buffers, then run
    /// `f` over them. It opens the agenda files, so it needs the foreground.
    fn with_agenda<T>(
        &mut self,
        what: &str,
        f: impl FnOnce(&[crate::org_agenda::Source], &[BufferId], &[String]) -> Result<T, String>,
    ) -> HostResult<Result<T, String>> {
        if let Err(e) = self.foreground(what) {
            return Ok(Err(e));
        }
        let ids = self.editor()?.org_agenda_buffers();
        let ed = self.editor()?;
        let mut source_ids = Vec::new();
        let mut names = Vec::new();
        for id in ids {
            if let Some(b) = ed.state.get_buffer(id) {
                source_ids.push(id);
                names.push(
                    b.path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| b.name().to_string()),
                );
            }
        }
        let sources: Vec<crate::org_agenda::Source> = source_ids
            .iter()
            .zip(names.iter())
            .filter_map(|(id, name)| {
                ed.state
                    .get_buffer(*id)
                    .map(|b| crate::org_agenda::Source { name, buf: b })
            })
            .collect();
        Ok(f(&sources, &source_ids, &names))
    }
}

/// A collected agenda entry as the WIT record, resolving its source.
fn wit_entry(
    e: &crate::org_agenda::Entry,
    ids: &[BufferId],
    names: &[String],
) -> forge::editor::org::AgendaEntry {
    use crate::org_agenda::Kind;
    use forge::editor::org::AgendaKind;
    forge::editor::org::AgendaEntry {
        buffer: ids.get(e.source).map_or(0, |id| id.0),
        file: names.get(e.source).cloned().unwrap_or_default(),
        line: e.line as u32,
        heading: e.heading.clone(),
        title: e.title.clone(),
        category: e.category.clone(),
        keyword: e.keyword.clone(),
        done: e.done,
        tags: e.tags.clone(),
        priority: e.priority.map(|c| c.to_string()),
        effort: e.effort,
        kind: match e.kind {
            Kind::Scheduled => AgendaKind::Scheduled,
            Kind::Deadline => AgendaKind::Deadline,
            Kind::Timestamp => AgendaKind::Timestamp,
            Kind::Closed => AgendaKind::Closed,
            Kind::Clocked => AgendaKind::Clocked,
            Kind::Todo => AgendaKind::Todo,
            Kind::Match => AgendaKind::Match,
            Kind::Search => AgendaKind::Search,
            Kind::Stuck => AgendaKind::Stuck,
        },
        date: e.date.map(|d| d.iso()),
        time: e.time.map(|(h, m)| format!("{h:02}:{m:02}")),
        label: e.label.clone(),
    }
}

fn position_mark(id: BufferId, offset: usize, p: Pos) -> forge::editor::marks::PositionMark {
    forge::editor::marks::PositionMark {
        buffer: id.0,
        offset: offset as u32,
        position: forge::editor::types::Position {
            line: p.line as u32,
            column: p.col as u32,
        },
    }
}

// --------------------------------------------------------------- runtime

/// How often the engine's epoch advances (the granularity of time limits).
pub const TICK: Duration = Duration::from_millis(10);
/// Longest a single call into a plugin may run before it is stopped.
pub const CALL_TIME_LIMIT: Duration = Duration::from_secs(5);
/// The same for background calls (commands.register-background).
pub const BACKGROUND_TIME_LIMIT: Duration = Duration::from_secs(60);

/// Interfaces Forge links for plugins. Anything else a component imports
/// still loads, but calling it traps (see [`LoadedPlugin::unsupported`]).
const SUPPORTED: &[&str] = &[
    "forge:editor/",
    "wasi:cli/",
    "wasi:io/",
    "wasi:clocks/",
    "wasi:random/",
    "wasi:filesystem/",
    "wasi:sockets/",
];

pub fn is_supported_import(name: &str) -> bool {
    SUPPORTED.iter().any(|p| name.starts_with(p))
}

/// Which WASI a component was built against, judged by its imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasiVersion {
    /// Imports no WASI at all.
    None,
    P2,
    P3,
}

/// A thread-safe view of plugin calls, for a watchdog on another thread:
/// who is running and for how long, and a way to stop them.
#[derive(Clone)]
pub struct PluginMonitor {
    inner: Arc<MonitorInner>,
}

struct MonitorInner {
    engine: Engine,
    busy: Mutex<Option<(String, Instant)>>,
    cancelled: AtomicBool,
}

impl PluginMonitor {
    /// The plugin running right now and for how long.
    pub fn busy(&self) -> Option<(String, Duration)> {
        self.inner
            .busy
            .lock()
            .expect("monitor lock")
            .as_ref()
            .map(|(n, t)| (n.clone(), t.elapsed()))
    }

    /// Stop the running call (C-g). Safe to call from any thread.
    pub fn interrupt(&self) {
        if self.busy().is_none() {
            return;
        }
        self.inner.cancelled.store(true, Ordering::SeqCst);
        // Jump the epoch past any deadline a call can have.
        let ticks = CALL_TIME_LIMIT.as_millis() / TICK.as_millis() + 2;
        for _ in 0..ticks.max(1000) {
            self.inner.engine.increment_epoch();
        }
    }

    fn start(&self, name: &str) {
        self.inner.cancelled.store(false, Ordering::SeqCst);
        *self.inner.busy.lock().expect("monitor lock") = Some((name.to_string(), Instant::now()));
    }

    /// Returns whether the call was interrupted by [`PluginMonitor::interrupt`].
    fn finish(&self) -> bool {
        *self.inner.busy.lock().expect("monitor lock") = None;
        self.inner.cancelled.swap(false, Ordering::SeqCst)
    }
}

/// Advance the engine's epoch every [`TICK`] until the engine is gone.
fn start_ticker(engine: EngineWeak) {
    std::thread::Builder::new()
        .name("forge-plugin-ticker".into())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                match engine.upgrade() {
                    Some(e) => e.increment_epoch(),
                    None => break,
                }
            }
        })
        .expect("spawn the plugin ticker");
}

pub struct PluginRuntime {
    engine: Engine,
    linker: Linker<PluginState>,
    executor: Arc<tokio::runtime::Runtime>,
    monitor: PluginMonitor,
    call_time_limit: Duration,
}

/// A compiled component, looked at before anything in it runs.
pub struct Inspection {
    pub name: String,
    component: Component,
    /// Everything the component imports ("wasi:cli/stderr@0.2.3", ...).
    pub imports: Vec<String>,
    /// Imports Forge doesn't provide: calling them traps.
    pub unsupported: Vec<String>,
    pub wasi: WasiVersion,
}

/// A plugin's process-handler export (world plugin-with-processes).
struct ProcessHandler {
    on_output: TypedFunc<(u32, Vec<u8>), ()>,
    on_exit: TypedFunc<(u32, Option<i32>), ()>,
}

/// A plugin's http-handler export (world plugin-with-http). Since 0.1.27.
struct HttpHandler {
    on_response: TypedFunc<(u32, Result<forge::editor::http::Response, String>), ()>,
}

/// A plugin's completion-handler export (world plugin-with-completion).
struct CompletionHandler {
    annotate: TypedFunc<(String, Vec<String>), (Vec<String>,)>,
    complete: TypedFunc<(u32, String, u32, Position), ()>,
    /// The exit callback (since 0.1.16). Optional: a plugin built against
    /// an older interface doesn't export it.
    commit: Option<TypedFunc<(String, String), ()>>,
    /// The location callback (since 0.1.17). Optional.
    resolve_location: Option<TypedFunc<(Selection,), ()>>,
}

/// Stderr lines a process may log before the rest is dropped.
const STDERR_LOG_LIMIT: usize = 16 * 1024;

/// Stderr a process keeps for `processes.error-output`.
const ERROR_OUTPUT_LIMIT: usize = 64 * 1024;

pub struct LoadedPlugin {
    pub(crate) store: Store<PluginState>,
    bindings: Plugin,
    process_handler: Option<ProcessHandler>,
    completion_handler: Option<CompletionHandler>,
    http_handler: Option<HttpHandler>,
    executor: Arc<tokio::runtime::Runtime>,
    monitor: PluginMonitor,
    call_time_limit: Duration,
    stdout: MemoryOutputPipe,
    stderr: MemoryOutputPipe,
    /// Bytes of stdout / stderr already moved into the log.
    output_seen: [usize; 2],
    pub imports: Vec<String>,
    pub unsupported: Vec<String>,
    pub wasi: WasiVersion,
    /// What this plugin was allowed when it was loaded.
    pub grants: Grants,
}

/// A call into a plugin either returns (maybe with the plugin's own error
/// message) or traps (a crash: the plugin should be unloaded).
pub type CallResult = std::result::Result<std::result::Result<(), String>, anyhow::Error>;

impl LoadedPlugin {
    pub fn name(&self) -> &str {
        &self.store.data().name
    }

    pub fn logs(&self) -> &[LogLine] {
        &self.store.data().logs
    }

    /// Just the messages, for simple display.
    pub fn messages(&self) -> Vec<&str> {
        self.logs().iter().map(|l| l.message.as_str()).collect()
    }

    pub fn subscribed(&self, kind: usize) -> bool {
        self.store.data().subscriptions[kind]
    }

    /// Key bindings it made: (scope, keys, command).
    pub fn bindings(&self) -> &[(Scope, String, String)] {
        &self.store.data().bindings
    }

    /// Remappings it made: (scope, command, replacement).
    pub fn remaps(&self) -> &[(Scope, String, String)] {
        &self.store.data().remaps
    }

    /// Run the plugin's `init` export.
    pub fn init(&mut self) -> Result<()> {
        match self.call_init() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(msg)) => Err(anyhow!("plugin init failed: {msg}")),
            Err(trap) => Err(trap.context("plugin crashed during init")),
        }
    }

    /// Before a call: set its time limit and tell the monitor (unless it
    /// runs in the background: nobody waits for those).
    fn begin(&mut self) {
        let background = self.store.data().background;
        let limit = if background {
            BACKGROUND_TIME_LIMIT
        } else {
            self.call_time_limit
        };
        let ticks = (limit.as_millis() / TICK.as_millis()).max(1) as u64;
        self.store.set_epoch_deadline(ticks);
        self.store.epoch_deadline_trap();
        self.store.data_mut().touched_decorations.clear();
        self.store.data_mut().touched_images.clear();
        if !background {
            let name = self.name().to_string();
            self.monitor.start(&name);
        }
    }

    /// After a call: collect output and explain a trap.
    fn end(&mut self, r: wasmtime::Result<std::result::Result<(), String>>) -> CallResult {
        let cancelled = if self.store.data().background {
            false
        } else {
            self.monitor.finish()
        };
        self.collect_output();
        r.map_err(|e| self.explain_trap(e, cancelled))
    }

    fn explain_trap(&self, e: wasmtime::Error, cancelled: bool) -> anyhow::Error {
        let interrupted = e.downcast_ref::<Trap>() == Some(&Trap::Interrupt);
        let text = format!("{e:#}");
        let e = anyhow::Error::from(e);
        let limit = if self.store.data().background {
            BACKGROUND_TIME_LIMIT
        } else {
            self.call_time_limit
        };
        if interrupted && cancelled {
            e.context("stopped with C-g")
        } else if interrupted {
            e.context(format!(
                "stopped: still running after {} seconds",
                limit.as_secs_f32()
            ))
        } else if let Some(i) = self.unsupported.iter().find(|i| text.contains(i.as_str())) {
            e.context(format!(
                "it called `{i}`, which Forge doesn't provide to plugins"
            ))
        } else {
            e
        }
    }

    /// Does this command run in the background?
    pub fn is_background_command(&self, name: &str) -> bool {
        self.store.data().background_commands.contains(name)
    }

    /// Does this event's handler run in the background?
    pub fn is_background_event(&self, kind: usize) -> bool {
        self.store.data().background_events[kind]
    }

    /// Calls from now on run in the background (on another thread, with
    /// their own executor) or not.
    pub(crate) fn set_background(&mut self, background: bool) -> Result<()> {
        self.store.data_mut().background = background;
        if background {
            self.executor = Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?,
            );
        }
        Ok(())
    }

    pub(crate) fn call_init(&mut self) -> CallResult {
        self.begin();
        let r = self
            .executor
            .clone()
            .block_on(self.bindings.call_init(&mut self.store));
        self.end(r)
    }

    pub(crate) fn call_run_command(&mut self, name: &str, prefix: Option<i64>) -> CallResult {
        self.begin();
        let h = self.bindings.forge_editor_handler();
        let r = self
            .executor
            .clone()
            .block_on(h.call_run_command(&mut self.store, name, prefix));
        self.end(r)
    }

    pub(crate) fn call_on_event(&mut self, e: PluginEvent) -> CallResult {
        self.begin();
        let h = self.bindings.forge_editor_handler();
        let r = self
            .executor
            .clone()
            .block_on(h.call_on_event(&mut self.store, e));
        self.end(r.map(Ok))
    }

    pub(crate) fn call_on_answer(&mut self, request: u32, answer: Option<&str>) -> CallResult {
        self.begin();
        let h = self.bindings.forge_editor_handler();
        let r = self
            .executor
            .clone()
            .block_on(h.call_on_answer(&mut self.store, request, answer));
        self.end(r.map(Ok))
    }

    /// Does it have processes running?
    pub fn has_processes(&self) -> bool {
        self.store.data().processes.any_running()
    }

    /// Answers to its asynchronous HTTP requests, ready since last time.
    pub(crate) fn poll_http_events(&mut self) -> Vec<crate::http::HttpEvent> {
        let st = self.store.data_mut();
        st.requests.poll()
    }

    pub fn has_http(&self) -> bool {
        self.store.data().requests.any_running()
    }

    /// Output and exits of its processes since last time. Stderr goes to
    /// its log (debug level, up to a limit per process).
    pub(crate) fn poll_processes(&mut self) -> Vec<ProcessEvent> {
        let st = self.store.data_mut();
        let mut out = Vec::new();
        for ev in st.processes.poll() {
            match ev {
                ProcessEvent::Stderr(id, text) => {
                    let kept = st.error_output.entry(id).or_default();
                    let room = ERROR_OUTPUT_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&text.as_bytes()[..text.len().min(room)]);
                    let seen = st.stderr_seen.entry(id).or_default();
                    if *seen < STDERR_LOG_LIMIT {
                        *seen += text.len();
                        for line in text.lines().filter(|l| !l.trim().is_empty()) {
                            st.logs.push(LogLine {
                                level: Level::Debug,
                                message: line.to_string(),
                            });
                        }
                    }
                }
                ProcessEvent::Exited(id, _) => {
                    st.stderr_seen.remove(&id);
                    out.push(ev);
                }
                ev => out.push(ev),
            }
        }
        out
    }

    pub fn has_http_handler(&self) -> bool {
        self.http_handler.is_some()
    }

    /// Does it export completion-handler?
    pub fn provides_completion(&self) -> bool {
        self.completion_handler.is_some()
    }

    /// Ask for annotations (one per candidate; missing ones are "").
    pub(crate) fn call_annotate(
        &mut self,
        category: &str,
        candidates: &[String],
    ) -> (CallResult, Vec<String>) {
        let Some(f) = self.completion_handler.as_ref().map(|h| h.annotate) else {
            return (Ok(Ok(())), Vec::new());
        };
        self.begin();
        let exec = self.executor.clone();
        let r = exec
            .block_on(f.call_async(&mut self.store, (category.to_string(), candidates.to_vec())));
        let (r, out) = match r {
            Ok((v,)) => (Ok(Ok(())), v),
            Err(e) => (Err(e), Vec::new()),
        };
        (self.end(r), out)
    }

    pub(crate) fn call_complete(
        &mut self,
        request: u32,
        source: &str,
        buffer: u32,
        point: Pos,
    ) -> CallResult {
        let Some(f) = self.completion_handler.as_ref().map(|h| h.complete) else {
            return Ok(Ok(()));
        };
        self.begin();
        let exec = self.executor.clone();
        let r = exec.block_on(f.call_async(
            &mut self.store,
            (request, source.to_string(), buffer, position(point)),
        ));
        self.end(r.map(Ok))
    }

    /// Tell plugin `plugin` that `candidate` was committed with `status`
    /// (corfu's `:exit-function`). A plugin without the callback is fine.
    pub(crate) fn call_commit(&mut self, candidate: &str, status: &str) {
        let Some(f) = self.completion_handler.as_ref().and_then(|h| h.commit) else {
            return;
        };
        self.begin();
        let exec = self.executor.clone();
        let r = exec
            .block_on(f.call_async(&mut self.store, (candidate.to_string(), status.to_string())));
        let _ = self.end(r.map(Ok));
    }

    /// Where the selected candidate is defined (corfu-info-location).
    /// A plugin without the callback is fine.
    pub(crate) fn call_resolve_location(&mut self, selection: Selection) {
        let Some(f) = self
            .completion_handler
            .as_ref()
            .and_then(|h| h.resolve_location)
        else {
            return;
        };
        self.begin();
        let exec = self.executor.clone();
        let r = exec.block_on(f.call_async(&mut self.store, (selection,)));
        let _ = self.end(r.map(Ok));
    }

    pub(crate) fn call_process_event(&mut self, ev: ProcessEvent) -> CallResult {
        let exited = match ev {
            ProcessEvent::Exited(id, _) => Some(id),
            _ => None,
        };
        let r = self.dispatch_process_event(ev);
        // Its stderr was there for on-exit; nothing asks for it after.
        if let Some(id) = exited {
            self.store.data_mut().error_output.remove(&id);
        }
        r
    }

    /// Deliver a finished HTTP request to the plugin's http-handler.
    pub(crate) fn call_http_event(&mut self, ev: crate::http::HttpEvent) -> CallResult {
        let Some(h) = self.http_handler.as_ref() else {
            return Ok(Ok(()));
        };
        let f = h.on_response;
        self.begin();
        let exec = self.executor.clone();
        let answer = match ev.answer {
            Ok(r) => Ok(forge::editor::http::Response {
                status: r.status,
                headers: r.headers,
                body: r.body,
            }),
            Err(e) => Err(e),
        };
        let r = exec.block_on(f.call_async(&mut self.store, (ev.id.0, answer)));
        self.end(r.map(Ok))
    }

    fn dispatch_process_event(&mut self, ev: ProcessEvent) -> CallResult {
        let Some(h) = self.process_handler.as_ref() else {
            return Ok(Ok(()));
        };
        let (on_output, on_exit) = (h.on_output, h.on_exit);
        self.begin();
        let exec = self.executor.clone();
        let r = match ev {
            ProcessEvent::Output(id, text) => {
                exec.block_on(on_output.call_async(&mut self.store, (id.0, text.into_bytes())))
            }
            ProcessEvent::Exited(id, code) => {
                exec.block_on(on_exit.call_async(&mut self.store, (id.0, code)))
            }
            ProcessEvent::Stderr(..) => Ok(()),
        };
        self.end(r.map(Ok))
    }

    /// Move what the plugin printed since last time into its log.
    fn collect_output(&mut self) {
        let pipes = [
            (&self.stdout, Level::Info, 0),
            (&self.stderr, Level::Error, 1),
        ];
        for (pipe, level, i) in pipes {
            let bytes = pipe.contents();
            let new = &bytes[self.output_seen[i].min(bytes.len())..];
            self.output_seen[i] = bytes.len();
            let logs = &mut self.store.data_mut().logs;
            for line in String::from_utf8_lossy(new)
                .lines()
                .filter(|l| !l.trim().is_empty())
            {
                logs.push(LogLine {
                    level,
                    message: line.to_string(),
                });
            }
        }
    }
}

/// The WASI context a plugin runs with: captured stdio, plus exactly the
/// directories and network it was granted. Problems (a granted directory
/// that's gone) come back as warnings.
fn wasi_ctx(
    grants: &Grants,
    stdout: &MemoryOutputPipe,
    stderr: &MemoryOutputPipe,
) -> (WasiCtx, Vec<String>) {
    let mut b = WasiCtxBuilder::new();
    b.stdout(stdout.clone()).stderr(stderr.clone());
    let mut warnings = Vec::new();
    for d in &grants.directories {
        let host = expand_home(&d.path);
        let host = match host.canonicalize() {
            Ok(h) if h.is_dir() => h,
            _ => {
                warnings.push(format!(
                    "granted directory {} doesn't exist; skipped",
                    d.path
                ));
                continue;
            }
        };
        // The plugin sees the directory under its real path.
        let guest = host.display().to_string();
        let perms = if d.write {
            FsPerms::ReadWrite
        } else {
            FsPerms::ReadOnly
        };
        if let Err(e) = b.preopened_dir(&host, &guest, perms) {
            warnings.push(format!("can't open {}: {e}", host.display()));
        }
    }
    if grants.network {
        b.inherit_network();
        b.allow_ip_name_lookup(true);
    }
    (b.build(), warnings)
}

impl PluginRuntime {
    pub fn new() -> Result<Self> {
        Self::with_cache(None)
    }

    /// With a directory to keep compiled plugins in, each is only
    /// compiled once (until it or Forge changes). A cache that can't be
    /// set up is skipped: it's only a speed-up.
    pub fn with_cache(dir: Option<&std::path::Path>) -> Result<Self> {
        let mut config = Config::new();
        if let Some(dir) = dir {
            let mut cc = wasmtime::CacheConfig::new();
            cc.with_directory(dir);
            if let Ok(cache) = wasmtime::Cache::new(cc) {
                config.cache(Some(cache));
            }
        }
        config.wasm_component_model(true);
        // WASI 0.3 is built on the component model's async support.
        config.wasm_component_model_async(true);
        // Time limits: calls trap once the epoch passes their deadline.
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;
        start_ticker(engine.weak());

        let mut linker: Linker<PluginState> = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(anyhow::Error::from)
            .context("linking WASI 0.2")?;
        wasmtime_wasi::p3::add_to_linker(&mut linker)
            .map_err(anyhow::Error::from)
            .context("linking WASI 0.3")?;
        Plugin::add_to_linker::<PluginState, HasSelf<PluginState>>(&mut linker, |s| s)?;

        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let monitor = PluginMonitor {
            inner: Arc::new(MonitorInner {
                engine: engine.clone(),
                busy: Mutex::new(None),
                cancelled: AtomicBool::new(false),
            }),
        };
        Ok(Self {
            engine,
            linker,
            executor: Arc::new(executor),
            monitor,
            call_time_limit: CALL_TIME_LIMIT,
        })
    }

    pub fn monitor(&self) -> PluginMonitor {
        self.monitor.clone()
    }

    /// Change the per-call time limit (tests use a short one).
    pub fn set_call_time_limit(&mut self, limit: Duration) {
        self.call_time_limit = limit;
    }

    /// Compile a component and look at what it imports. Nothing runs.
    pub fn inspect(&self, name: &str, bytes: &[u8]) -> Result<Inspection> {
        if is_core_module(bytes) {
            bail!(
                "this is a core WebAssembly module, not a component; Forge plugins are \
                 components now: build with `cargo build --target wasm32-wasip2` \
                 (see plugins/hello)"
            );
        }
        let component = Component::new(&self.engine, bytes)?;
        let imports: Vec<String> = component
            .component_type()
            .imports(&self.engine)
            .map(|(name, _)| name.to_string())
            .collect();
        let wasi = if imports
            .iter()
            .any(|i| i.starts_with("wasi:") && i.contains("@0.3"))
        {
            WasiVersion::P3
        } else if imports.iter().any(|i| i.starts_with("wasi:")) {
            WasiVersion::P2
        } else {
            WasiVersion::None
        };
        let unsupported = imports
            .iter()
            .filter(|i| !is_supported_import(i))
            .cloned()
            .collect();
        Ok(Inspection {
            name: name.to_string(),
            component,
            imports,
            unsupported,
            wasi,
        })
    }

    /// Instantiate an inspected component with the given grants. This runs
    /// no plugin code beyond the component's own start-up; call
    /// [`LoadedPlugin::init`] (with the editor lent: see `PluginHost`).
    pub fn instantiate(&self, inspection: Inspection, grants: &Grants) -> Result<LoadedPlugin> {
        let Inspection {
            name,
            component,
            imports,
            unsupported,
            wasi,
        } = inspection;
        // Everything the plugin can use must link. Imports Forge doesn't
        // provide at all (wasi:http, ...) become traps, except that a
        // missing forge:editor function is an error now (see the
        // compatibility rules in docs/plugin-api.md).
        let pre = match self.linker.instantiate_pre(&component) {
            Ok(pre) => pre,
            Err(e) => {
                let text = format!("{e:#}");
                if unsupported.is_empty() || text.contains("forge:editor") {
                    return Err(explain_link_error(e.into(), &imports));
                }
                let mut linker = self.linker.clone();
                linker.define_unknown_imports_as_traps(&component)?;
                linker
                    .instantiate_pre(&component)
                    .map_err(|e| explain_link_error(e.into(), &imports))?
            }
        };
        let pre = PluginPre::new(pre)?;

        let stdout = MemoryOutputPipe::new(OUTPUT_LIMIT);
        let stderr = MemoryOutputPipe::new(OUTPUT_LIMIT);
        let (wasi_ctx, warnings) = wasi_ctx(grants, &stdout, &stderr);
        let limits = StoreLimitsBuilder::new()
            .memory_size(grants.memory_mb() as usize * 1024 * 1024)
            .instances(1000)
            .tables(1000)
            .memories(100)
            .build();
        let state = PluginState {
            wasi: wasi_ctx,
            table: ResourceTable::new(),
            name: name.clone(),
            logs: warnings
                .into_iter()
                .map(|message| LogLine {
                    level: Level::Warn,
                    message,
                })
                .collect(),
            editor: None,
            effects: Vec::new(),
            subscriptions: [false; EVENT_KINDS],
            next_request: 0,
            bindings: Vec::new(),
            remaps: Vec::new(),
            limits,
            grants: grants.clone(),
            processes: Processes::new(),
            requests: crate::http::HttpRequests::new(),
            stderr_seen: Default::default(),
            error_output: Default::default(),
            background: false,
            background_commands: Default::default(),
            background_events: [false; EVENT_KINDS],
            touched_decorations: Default::default(),
            touched_images: Default::default(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        // Start-up code runs under the same time limit as calls.
        let ticks = (self.call_time_limit.as_millis() / TICK.as_millis()).max(1) as u64;
        store.set_epoch_deadline(ticks);
        store.epoch_deadline_trap();

        let instance: Instance = self
            .executor
            .block_on(pre.instance_pre().instantiate_async(&mut store))
            .map_err(|e| explain_link_error(e.into(), &imports))?;
        let bindings = Plugin::new(&mut store, &instance)?;
        let process_handler = process_handler(&mut store, &instance)?;
        let completion_handler = completion_handler(&mut store, &instance)?;
        let http_handler = http_handler(&mut store, &instance)?;
        Ok(LoadedPlugin {
            store,
            bindings,
            process_handler,
            completion_handler,
            http_handler,
            executor: self.executor.clone(),
            monitor: self.monitor.clone(),
            call_time_limit: self.call_time_limit,
            stdout,
            stderr,
            output_seen: [0; 2],
            imports,
            unsupported,
            wasi,
            grants: grants.clone(),
        })
    }

    /// Inspect and instantiate with no grants (tests and simple tools).
    pub fn load(&self, name: &str, bytes: &[u8]) -> Result<LoadedPlugin> {
        self.instantiate(self.inspect(name, bytes)?, &Grants::default())
    }

    /// Load a component from a file with no grants; the plugin is named
    /// after the file ("spell.wasm" is "spell").
    pub fn load_plugin_from_file(&self, path: impl AsRef<Path>) -> Result<LoadedPlugin> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "plugin".into());
        self.load(&name, &bytes)
            .with_context(|| format!("loading {}", path.display()))
    }
}

/// The plugin's `forge:editor/process-handler@0.1.x` export, if it has one.
fn process_handler(
    store: &mut Store<PluginState>,
    instance: &Instance,
) -> Result<Option<ProcessHandler>> {
    let names: Vec<String> = instance
        .get_export(&mut *store, None, "forge:editor/process-handler@0.1.2")
        .map(|_| vec!["forge:editor/process-handler@0.1.2".to_string()])
        .unwrap_or_default();
    // Later 0.1.x plugins export a newer version of the same interface.
    let name = names.into_iter().next().or_else(|| {
        (3..20)
            .map(|v| format!("forge:editor/process-handler@0.1.{v}"))
            .find(|n| instance.get_export(&mut *store, None, n).is_some())
    });
    let Some(name) = name else { return Ok(None) };
    let iface = instance
        .get_export_index(&mut *store, None, &name)
        .expect("just found");
    let func = |store: &mut Store<PluginState>, f: &str| {
        instance
            .get_export_index(&mut *store, Some(&iface), f)
            .with_context(|| format!("{name} has no {f}"))
    };
    let out = func(store, "on-output")?;
    let exit = func(store, "on-exit")?;
    Ok(Some(ProcessHandler {
        on_output: instance
            .get_typed_func(&mut *store, out)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("{name}.on-output"))?,
        on_exit: instance
            .get_typed_func(&mut *store, exit)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("{name}.on-exit"))?,
    }))
}

/// The export `iface` at the newest 0.1.x from `since` on, if any.
fn find_export(
    store: &mut Store<PluginState>,
    instance: &Instance,
    iface: &str,
    since: u32,
) -> Option<String> {
    (since..30)
        .rev()
        .map(|v| format!("forge:editor/{iface}@0.1.{v}"))
        .find(|n| instance.get_export(&mut *store, None, n).is_some())
}

/// The plugin's `forge:editor/http-handler@0.1.x` export, if any.
fn http_handler(
    store: &mut Store<PluginState>,
    instance: &Instance,
) -> Result<Option<HttpHandler>> {
    let Some(name) = find_export(store, instance, "http-handler", 27) else {
        return Ok(None);
    };
    let iface = instance
        .get_export_index(&mut *store, None, &name)
        .expect("just found");
    let func = |store: &mut Store<PluginState>, f: &str| {
        instance
            .get_export_index(&mut *store, Some(&iface), f)
            .with_context(|| format!("{name} has no {f}"))
    };
    let on_response = func(store, "on-response")?;
    Ok(Some(HttpHandler {
        on_response: instance
            .get_typed_func(&mut *store, on_response)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("{name}.on-response"))?,
    }))
}

/// The plugin's `forge:editor/completion-handler@0.1.x` export, if any.
fn completion_handler(
    store: &mut Store<PluginState>,
    instance: &Instance,
) -> Result<Option<CompletionHandler>> {
    let Some(name) = find_export(store, instance, "completion-handler", 3) else {
        return Ok(None);
    };
    let iface = instance
        .get_export_index(&mut *store, None, &name)
        .expect("just found");
    let func = |store: &mut Store<PluginState>, f: &str| {
        instance
            .get_export_index(&mut *store, Some(&iface), f)
            .with_context(|| format!("{name} has no {f}"))
    };
    let annotate = func(store, "annotate")?;
    let complete = func(store, "complete")?;
    // The exit and location callbacks are optional, so a plugin from
    // before 0.1.16 / 0.1.17 loads.
    let commit = func(store, "commit")
        .ok()
        .and_then(|f| instance.get_typed_func(&mut *store, f).ok());
    let resolve_location = func(store, "resolve-location")
        .ok()
        .and_then(|f| instance.get_typed_func(&mut *store, f).ok());
    Ok(Some(CompletionHandler {
        annotate: instance
            .get_typed_func(&mut *store, annotate)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("{name}.annotate"))?,
        complete: instance
            .get_typed_func(&mut *store, complete)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("{name}.complete"))?,
        commit,
        resolve_location,
    }))
}

/// A binary core module has version 1 in its header; a component has a
/// different "layer" there.
fn is_core_module(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[0..4] == b"\0asm" && bytes[4..8] == [1, 0, 0, 0]
}

/// Point at the import that couldn't be satisfied.
fn explain_link_error(e: anyhow::Error, imports: &[String]) -> anyhow::Error {
    let text = format!("{e:#}");
    match imports.iter().find(|i| text.contains(i.as_str())) {
        Some(i) => e.context(format!(
            "the plugin imports `{i}`, which Forge doesn't provide"
        )),
        None => e,
    }
}

#[cfg(test)]
mod tests {
    //! These build real components the way `cargo component` and
    //! `wasm-component-ld` do: a core module (written in WAT here, so no
    //! wasm Rust target is needed) plus the world's WIT, encoded with
    //! `wit-component`.

    use super::*;
    use wit_component::{ComponentEncoder, StringEncoding};
    use wit_parser::Resolve;

    /// WASI packages as a plugin toolchain would see them (just the
    /// function the tests call).
    const WASI_RANDOM_P2: &str = "package wasi:random@0.2.0;
        interface random { get-random-u64: func() -> u64; }";
    const WASI_RANDOM_P3: &str = "package wasi:random@0.3.0;
        interface random { get-random-u64: func() -> u64; }";

    /// Encode `wat` as a component of `world` (in `extra_wit`, which may
    /// `include forge:editor/plugin`).
    fn component(wat: &str, deps: &[&str], world_wit: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/wit-0.1.0/forge-editor.wit"
        );
        resolve.push_file(root).unwrap();
        for (i, d) in deps.iter().enumerate() {
            resolve.push_str(format!("dep{i}.wit"), d).unwrap();
        }
        let pkg = resolve.push_str("test.wit", world_wit).unwrap();
        let world = resolve.select_world(&[pkg], None).unwrap();
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

    /// The handler exports every plugin must have, doing nothing.
    const HANDLER_STUBS: &str = r#"
        ;; A bump allocator: the host copies strings into guest memory with it.
        (global $heap (mut i32) (i32.const 4096))
        (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
          (local $p i32)
          (local.set $p (i32.and
            (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
            (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
          (global.set $heap (i32.add (local.get $p) (local.get 3)))
          (local.get $p))
        (func (export "forge:editor/handler@0.1.0#run-command") (param i32 i32 i32 i64) (result i32)
          (i32.store8 (i32.const 0) (i32.const 0)) (i32.const 0))
        (func (export "forge:editor/handler@0.1.0#on-event") (param i32 i32))
        (func (export "forge:editor/handler@0.1.0#on-answer") (param i32 i32 i32 i32))"#;

    const PLAIN_WORLD: &str = "package test:plugin; world t { include forge:editor/plugin@0.1.0; }";

    /// A plugin whose init logs one line and returns ok. `extra` is
    /// spliced into the module (more imports, code before the log).
    fn hello(extra_imports: &str, before: &str) -> String {
        format!(
            r#"(module
                (import "forge:editor/log@0.1.0" "log" (func $log (param i32 i32 i32)))
                {extra_imports}
                (memory (export "memory") 1)
                (data (i32.const 100) "hello from a component")
                {HANDLER_STUBS}
                (func (export "init") (result i32)
                  {before}
                  ;; log(info, "hello from a component")
                  (call $log (i32.const 1) (i32.const 100) (i32.const 22))
                  ;; result<_, string>: tag 0 = ok, in the return area at 0
                  (i32.store8 (i32.const 0) (i32.const 0))
                  (i32.const 0)))"#
        )
    }

    #[test]
    fn loads_a_component_and_runs_init() {
        let rt = PluginRuntime::new().unwrap();
        let bytes = component(&hello("", ""), &[], PLAIN_WORLD);
        let mut p = rt.load("t", &bytes).unwrap();
        assert_eq!(p.wasi, WasiVersion::None);
        // The handler's types come from events and minibuffer, so those are imported too.
        assert!(p.imports.contains(&"forge:editor/log@0.1.0".to_string()));
        assert!(
            !p.imports
                .iter()
                .any(|i| i.starts_with("forge:editor/buffers"))
        );
        p.init().unwrap();
        assert_eq!(
            p.logs(),
            [LogLine {
                level: Level::Info,
                message: "hello from a component".into()
            }]
        );
    }

    #[test]
    fn init_errors_and_traps_are_reported() {
        let rt = PluginRuntime::new().unwrap();
        let failing = r#"(module
            (import "forge:editor/log@0.1.0" "log" (func (param i32 i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 100) "no config found")
            HANDLERS
            (func (export "init") (result i32)
              ;; result<_, string>: tag 1 = err, then (ptr, len) at offsets 4 and 8
              (i32.store8 (i32.const 0) (i32.const 1))
              (i32.store (i32.const 4) (i32.const 100))
              (i32.store (i32.const 8) (i32.const 15))
              (i32.const 0)))"#;
        let failing = failing.replace("HANDLERS", HANDLER_STUBS);
        let mut p = rt
            .load("t", &component(&failing, &[], PLAIN_WORLD))
            .unwrap();
        assert_eq!(
            p.init().unwrap_err().to_string(),
            "plugin init failed: no config found"
        );

        let trapping = hello("", "unreachable");
        let mut p = rt
            .load("t", &component(&trapping, &[], PLAIN_WORLD))
            .unwrap();
        let err = format!("{:#}", p.init().unwrap_err());
        assert!(err.starts_with("plugin crashed during init"), "{err}");
    }

    #[test]
    fn wasi_0_2_components_still_load() {
        let rt = PluginRuntime::new().unwrap();
        let world = "package test:plugin; world t {
            include forge:editor/plugin@0.1.0;
            import wasi:random/random@0.2.0;
        }";
        let wat = hello(
            r#"(import "wasi:random/random@0.2.0" "get-random-u64" (func $rand (result i64)))"#,
            "(drop (call $rand))",
        );
        let mut p = rt
            .load("t", &component(&wat, &[WASI_RANDOM_P2], world))
            .unwrap();
        assert_eq!(p.wasi, WasiVersion::P2);
        // Imported as 0.2.0, served by the host's newer 0.2.x (semver-compatible).
        p.init().unwrap();
        assert_eq!(p.messages(), ["hello from a component"]);
    }

    #[test]
    fn wasi_0_3_components_load() {
        let rt = PluginRuntime::new().unwrap();
        let world = "package test:plugin; world t {
            include forge:editor/plugin@0.1.0;
            import wasi:random/random@0.3.0;
        }";
        let wat = hello(
            r#"(import "wasi:random/random@0.3.0" "get-random-u64" (func $rand (result i64)))"#,
            "(drop (call $rand))",
        );
        let mut p = rt
            .load("t", &component(&wat, &[WASI_RANDOM_P3], world))
            .unwrap();
        assert_eq!(p.wasi, WasiVersion::P3);
        p.init().unwrap();
        assert_eq!(p.messages(), ["hello from a component"]);
    }

    #[test]
    fn unknown_imports_load_but_trap_when_called() {
        let rt = PluginRuntime::new().unwrap();
        let world = "package test:plugin;
            interface gadget { poke: func(); }
            world t { include forge:editor/plugin@0.1.0; import gadget; }";
        let wat = hello(
            r#"(import "test:plugin/gadget" "poke" (func $poke))"#,
            "(call $poke)",
        );
        let mut p = rt.load("t", &component(&wat, &[], world)).unwrap();
        assert_eq!(p.unsupported, ["test:plugin/gadget"]);
        let err = format!("{:#}", p.init().unwrap_err());
        assert!(
            err.contains("it called `test:plugin/gadget`, which Forge doesn't provide to plugins"),
            "{err}"
        );

        let core = wat::parse_str("(module)").unwrap();
        let err = rt.load("t", &core).err().expect("should fail").to_string();
        assert!(
            err.contains("core WebAssembly module, not a component"),
            "{err}"
        );
    }

    // ------------------------------------------------ permissions (Phase 5)

    const WASI_DEPS: [&str; 4] = [
        include_str!("../../../../tests/fixtures/wasi-0.2/io.wit"),
        include_str!("../../../../tests/fixtures/wasi-0.2/clocks.wit"),
        include_str!("../../../../tests/fixtures/wasi-0.2/filesystem.wit"),
        include_str!("../../../../tests/fixtures/wasi-0.2/sockets.wit"),
    ];

    /// A plugin whose init runs `body` (which may call $log) and returns ok.
    fn plugin_with(imports: &str, body: &str) -> String {
        format!(
            r#"(module
                (import "forge:editor/log@0.1.0" "log" (func $log (param i32 i32 i32)))
                {imports}
                (memory (export "memory") 1)
                (data (i32.const 100) "no directories")  ;; 14
                (data (i32.const 120) "localhost")       ;; 9
                (data (i32.const 140) "allowed")         ;; 7
                (data (i32.const 150) "denied")          ;; 6
                (data (i32.const 160) "grown")           ;; 5
                {HANDLER_STUBS}
                (func (export "init") (result i32)
                  (local $el i32)
                  {body}
                  (i32.store8 (i32.const 0) (i32.const 0))
                  (i32.const 0)))"#
        )
    }

    fn logged(p: &LoadedPlugin) -> Vec<String> {
        p.messages().into_iter().map(String::from).collect()
    }

    #[test]
    fn plugins_see_only_granted_directories() {
        let world = "package test:plugin; world t {
            include forge:editor/plugin@0.1.0;
            import wasi:filesystem/preopens@0.2.12;
        }";
        let wat = plugin_with(
            r#"(import "wasi:filesystem/preopens@0.2.12" "get-directories" (func $dirs (param i32)))"#,
            r#"(call $dirs (i32.const 32))
               (if (i32.eqz (i32.load (i32.const 36)))
                 (then (call $log (i32.const 1) (i32.const 100) (i32.const 14)))
                 (else
                   ;; first element: (descriptor, path ptr, path len)
                   (local.set $el (i32.load (i32.const 32)))
                   (call $log (i32.const 1)
                     (i32.load (i32.add (local.get $el) (i32.const 4)))
                     (i32.load (i32.add (local.get $el) (i32.const 8))))))"#,
        );
        let bytes = component(&wat, &WASI_DEPS, world);
        let rt = PluginRuntime::new().unwrap();

        let mut p = rt.load("fs", &bytes).unwrap();
        p.init().unwrap();
        assert_eq!(logged(&p), ["no directories"]);

        let dir = std::env::temp_dir().join(format!("forge-grant-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let grants = Grants {
            directories: vec![crate::permissions::DirGrant {
                path: dir.display().to_string(),
                write: false,
                why: None,
            }],
            ..Default::default()
        };
        let mut p = rt
            .instantiate(rt.inspect("fs", &bytes).unwrap(), &grants)
            .unwrap();
        p.init().unwrap();
        assert_eq!(
            logged(&p),
            [dir.canonicalize().unwrap().display().to_string()]
        );

        // A granted directory that doesn't exist is skipped with a warning.
        let gone = Grants {
            directories: vec![crate::permissions::DirGrant {
                path: "/no/such/dir".into(),
                write: false,
                why: None,
            }],
            ..Default::default()
        };
        let mut p = rt
            .instantiate(rt.inspect("fs", &bytes).unwrap(), &gone)
            .unwrap();
        p.init().unwrap();
        assert_eq!(
            logged(&p),
            [
                "granted directory /no/such/dir doesn't exist; skipped",
                "no directories"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn network_needs_a_grant() {
        let world = "package test:plugin; world t {
            include forge:editor/plugin@0.1.0;
            import wasi:sockets/instance-network@0.2.12;
            import wasi:sockets/ip-name-lookup@0.2.12;
        }";
        let wat = plugin_with(
            r#"(import "wasi:sockets/instance-network@0.2.12" "instance-network" (func $net (result i32)))
               (import "wasi:sockets/ip-name-lookup@0.2.12" "resolve-addresses" (func $resolve (param i32 i32 i32 i32)))"#,
            r#";; resolve-addresses(network, "localhost") -> result at 48 (tag 0 = ok)
               (call $resolve (call $net) (i32.const 120) (i32.const 9) (i32.const 48))
               (if (i32.eqz (i32.load8_u (i32.const 48)))
                 (then (call $log (i32.const 1) (i32.const 140) (i32.const 7)))
                 (else (call $log (i32.const 1) (i32.const 150) (i32.const 6))))"#,
        );
        let bytes = component(&wat, &WASI_DEPS, world);
        let rt = PluginRuntime::new().unwrap();
        let mut p = rt.load("net", &bytes).unwrap();
        p.init().unwrap();
        assert_eq!(logged(&p), ["denied"]);

        let grants = Grants {
            network: true,
            ..Default::default()
        };
        let mut p = rt
            .instantiate(rt.inspect("net", &bytes).unwrap(), &grants)
            .unwrap();
        p.init().unwrap();
        assert_eq!(logged(&p), ["allowed"]);
    }

    #[test]
    fn memory_is_limited_unless_granted() {
        // 3000 pages is ~190 MB: over the 128 MB default.
        let wat = plugin_with(
            "",
            r#"(if (i32.eq (memory.grow (i32.const 3000)) (i32.const -1))
                 (then (call $log (i32.const 1) (i32.const 150) (i32.const 6)))
                 (else (call $log (i32.const 1) (i32.const 160) (i32.const 5))))"#,
        );
        let bytes = component(&wat, &[], PLAIN_WORLD);
        let rt = PluginRuntime::new().unwrap();
        let mut p = rt.load("mem", &bytes).unwrap();
        p.init().unwrap();
        assert_eq!(logged(&p), ["denied"]);

        let grants = Grants {
            memory_mb: Some(512),
            ..Default::default()
        };
        let mut p = rt
            .instantiate(rt.inspect("mem", &bytes).unwrap(), &grants)
            .unwrap();
        p.init().unwrap();
        assert_eq!(logged(&p), ["grown"]);
    }

    /// Encode against a stand-in "newer" forge:editor package.
    fn component_against(package: &str, world: &str, wat: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        resolve.push_str("newer.wit", package).unwrap();
        let pkg = resolve.push_str("t.wit", world).unwrap();
        let w = resolve.select_world(&[pkg], None).unwrap();
        let mut module = wat::parse_str(wat).unwrap();
        wit_component::embed_component_metadata(&mut module, &resolve, w, StringEncoding::UTF8)
            .unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
    }

    #[test]
    fn plugins_built_against_a_newer_0_1_x() {
        // The world's other interfaces and exports, as in the real WIT.
        let rest = include_str!("../../../../wit/forge-editor.wit")
            .split_once("interface types")
            .map(|(_, r)| format!("interface types{r}"))
            .unwrap();
        // Without the worlds that include `plugin`.
        let rest = rest
            .split("/// A plugin that runs processes")
            .next()
            .unwrap()
            .replace("interface log {", "interface unused-log {")
            .replace("world plugin {", "world unused {");
        let rt = PluginRuntime::new().unwrap();
        // Expects a `log.flush` this host doesn't provide.
        let newer = format!("package forge:editor@0.1.12;\n{rest}\n")
            + r#"interface log {
                enum level { debug, info, warn, error }
                log: func(level: level, message: string);
                flush: func();
            }"#;

        // Uses only what 0.1.0 has: loads (semver-compatible import).
        let world = "package test:plugin; world t {
            import forge:editor/log@0.1.12;
            export forge:editor/handler@0.1.12;
            export init: func() -> result<_, string>;
        }";
        let wat = hello("", "").replace("@0.1.0", "@0.1.12");
        let mut p = rt
            .load("t", &component_against(&newer, world, &wat))
            .unwrap();
        p.init().unwrap();

        // Uses the new function: refused, naming what's missing.
        let wat = hello(
            r#"(import "forge:editor/log@0.1.12" "flush" (func $flush))"#,
            "(call $flush)",
        )
        .replace("@0.1.0", "@0.1.12");
        let err = format!(
            "{:#}",
            rt.load("t", &component_against(&newer, world, &wat))
                .err()
                .expect("should fail")
        );
        assert!(err.contains("flush"), "{err}");
    }

    #[test]
    fn a_selections_location_survives_the_wit_round_trip() {
        let sel = crate::completion::Selection {
            text: "alpha".into(),
            annotation: None,
            category: "point".into(),
            index: 2,
            total: 5,
            documentation: Some("The first letter.".into()),
            location: Some(crate::completion::Location {
                path: Some(std::path::PathBuf::from("/src/foo.rs")),
                line: 3,
                column: 7,
            }),
        };
        let wit = wit_selection(&sel);
        assert_eq!(
            wit.location.as_ref().and_then(|l| l.path.as_deref()),
            Some("/src/foo.rs")
        );
        // And back, as deliver-detailed reads one in (deliver_detailed maps
        // the WIT record with `location`).
        let back = wit.location.map(location).unwrap();
        assert_eq!(
            back.path.as_deref(),
            Some(std::path::Path::new("/src/foo.rs"))
        );
        assert_eq!((back.line, back.column), (3, 7));
    }
}
