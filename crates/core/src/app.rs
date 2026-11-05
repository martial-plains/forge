// SPDX-License-Identifier: MIT OR Apache-2.0
//! Main editor state.

use crate::buffer::Buffer;
use crate::commands::CommandRegistry;
use crate::keymap::{ActiveKeymaps, Condition, Context, Keymaps, OnUnbound, TransientOpts};
use crate::kill_ring::KillRing;
use crate::modes::ModeRegistry;
use crate::plugin::registry::PluginRegistry;
use crate::window::{Direction, FrameId, TabId, WindowConfig, WindowId, Windows};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub u32);

#[derive(Debug, Default, Clone)]
pub struct EditorState {
    pub buffers: HashMap<BufferId, Buffer>,
    pub active_buffer: Option<BufferId>,
    next_id: u32,
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_buffer(&mut self, name: impl Into<String>) -> BufferId {
        self.insert_buffer(Buffer::new(name))
    }

    /// IDs come from a counter, so they stay unique after buffers are killed.
    pub fn insert_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = BufferId(self.next_id);
        self.next_id += 1;
        self.buffers.insert(id, buffer);
        id
    }

    pub fn get_buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }

    pub fn get_buffer_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers.get_mut(&id)
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.active_buffer.and_then(|id| self.buffers.get(&id))
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.active_buffer.and_then(|id| self.buffers.get_mut(&id))
    }
}

/// The editor: buffers, windows, keymaps, the kill ring.
///
/// Invariant: `state.active_buffer` is the selected window's buffer. Use
/// [`Editor::show_buffer`] and the window methods to change either.
#[derive(Debug, Clone)]
pub struct Editor {
    pub state: EditorState,
    /// What `forge.env` reads and writes, and what `forge.process.run`
    /// starts programs with: `process-environment`.
    pub environment: crate::env::Environment,
    /// What the keymap's conditional layers are answered from, refreshed
    /// once per keypress by [`Editor::refresh_key_context`] and read by the
    /// keymap lookup, which can't gather it itself (`Syntax::faces` needs
    /// `&mut`, and the git check walks the filesystem).
    pub key_context: crate::keymap::Context,
    pub windows: Windows,
    pub modes: ModeRegistry,
    pub plugins: PluginRegistry,
    /// What init.rhai set with `forge.plugin.config`: per plugin, a JSON
    /// object the plugin reads with `config.get` when it loads.
    pub plugin_config: std::collections::BTreeMap<String, String>,
    pub commands: CommandRegistry,
    pub kill_ring: KillRing,
    /// C-x r: registers the user saved (text, rectangles, positions).
    pub registers: std::collections::BTreeMap<char, crate::registers::Register>,
    /// C-x ( ... C-x (: the macro being recorded, the last one, the ring.
    pub kmacro: crate::kmacro::Kmacro,
    /// Marks set by `set-mark-command` (C-u) in any buffer, newest first:
    /// what consult-mark jumps through.
    pub global_mark_ring: Vec<(BufferId, usize)>,
    /// Cross-references plugins published (xref.publish), each with its
    /// publisher's name: what consult-xref lists.
    pub xrefs: Vec<(String, crate::xref::Xref)>,
    pub keymaps: Keymaps,
    /// Places, recent files and minibuffer history across sessions.
    pub session: crate::session::Session,
    /// Completion settings plugins chose (vertical lists, popups, sources).
    pub completion: crate::completion::Settings,
    /// The menu bar's menus: the built-in ones, plus what plugins added.
    pub menus: crate::menu::Registry,
    /// The tool bars: the default one and the per-major-mode ones.
    pub tool_bars: crate::toolbar::Registry,
    /// The tool bar's pointer state (which item or modifier button is
    /// under the pointer).
    pub tool_bar: crate::toolbar::ToolBar,
    /// The faces a theme restyled (`forge.theme.set`): both frontends
    /// apply these on top of their built-in styles.
    pub theme: crate::theme::Theme,
    /// Named themes (init.rhai, `~/.config/forge/themes/`, plugins) and
    /// which is applied; see [`crate::theme::Themes`].
    pub themes: crate::theme::Themes,
    /// Org agenda files (`forge.opt.org_agenda_files` and
    /// `~/.config/forge/org-agenda-files`), and whether the file was read.
    pub org_files: Vec<std::path::PathBuf>,
    pub org_files_loaded: bool,
    /// Which menu is open, and where in it.
    pub menu: crate::menu::MenuBar,
    /// The context menu (right-click, S-F10): open or last opened. While
    /// it is open it takes the menu keys instead of the bar.
    pub context_menu: crate::context_menu::ContextMenu,
    /// `context-menu-functions`: which builders fill the context menu,
    /// in order. See [`crate::context_menu::DEFAULT_FUNCTIONS`].
    pub context_menu_functions: Vec<String>,
    /// Scenes plugins draw: see [`crate::canvas`].
    pub canvases: crate::canvas::Canvases,
    /// Minor modes on across every buffer, not just the one selected: the
    /// ones that change the frame rather than the text in it. A buffer can
    /// also list a mode of its own, and a mode counts as on for a buffer
    /// when either list has it -- see [`Editor::minor_mode_enabled`].
    pub global_modes: GlobalModes,
    /// Read .editorconfig files when opening files.
    pub editorconfig: bool,
    /// Open files where they were left (save-place-mode).
    pub save_place: bool,
    /// Keep a list of recently opened files (recentf).
    pub recentf: bool,
    /// How Org files fold on first visit (org-startup-folded): one of
    /// `org::STARTUP_FOLDED_MODES`, "showeverything" by default.
    pub org_startup_folded: String,
    /// Org indent mode on in new Org buffers (org-startup-indented).
    pub org_startup_indented: bool,
    /// Columns per outline level in Org indent mode.
    pub org_indent_per_level: usize,
    /// The candidate under the cursor in whatever list is open (the
    /// vertical minibuffer list, or a popup at point). The frontend keeps
    /// this up to date; `completion.selected` reads it, so a plugin can
    /// describe the selection without owning the list.
    pub selection: Option<crate::completion::Selection>,
    /// The completion popup's session, kept up to date by the frontend so
    /// a plugin can drive it (`completion.current`).
    pub completion_session: Option<crate::completion::Session>,
    /// Requests from plugins to drive the popup (`completion.next` etc.);
    /// the frontend drains them (drained after every key).
    pub completion_control: Vec<crate::completion::Control>,
    /// Transient maps that are up right now, outermost first. A keypads
    /// stack of one; nested keymaps stack. Each entry outranks every other
    /// layer, so the top one is what a key hits first.
    pub transients: Vec<crate::keymap::LiveTransient>,
    /// The prompt map in charge right now, named by purpose ("find-file",
    /// "dired", "m-x", "completion-popup"). None when nothing is asking.
    pub prompt: Option<String>,
    /// Whether a prompt is open, mirroring the frontend's minibuffer so a
    /// plugin can ask (minibuffer.active). It holds no prompt itself.
    pub prompt_open: bool,
    /// The open prompt's history kind ("file", "command", ...), so a plugin
    /// can offer it (minibuffer.history-kind); None when none is asking or
    /// the prompt keeps no history.
    pub prompt_history: Option<String>,
    /// What is typed in the open prompt now (minibuffer.input).
    pub prompt_input: String,
    /// Plugin commands to run once input has stopped for a while
    /// (commands.run-when-idle).
    pub idle_hooks: Vec<IdleHook>,
    /// What the echo area showed when the frontend last looked (for
    /// minibuffer.current-message); the frontend keeps it up to date.
    pub echo_area: Option<String>,
}

/// A plugin command run after the editor has been idle for `seconds`.
#[derive(Debug, Clone, PartialEq)]
pub struct IdleHook {
    pub plugin: String,
    pub command: String,
    pub seconds: f64,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            state: EditorState::default(),
            environment: crate::env::Environment::default(),
            key_context: Default::default(),
            // Shows BufferId(0), which is the first buffer created.
            windows: Windows::new(BufferId(0)),
            modes: ModeRegistry::default(),
            plugins: PluginRegistry::default(),
            plugin_config: Default::default(),
            commands: CommandRegistry::default(),
            kill_ring: KillRing::default(),
            registers: std::collections::BTreeMap::new(),
            kmacro: crate::kmacro::Kmacro::default(),
            global_mark_ring: Vec::new(),
            xrefs: Vec::new(),
            keymaps: Keymaps::default(),
            session: crate::session::Session::load(None),
            completion: Default::default(),
            menus: crate::menu::Registry::builtin(),
            tool_bars: crate::toolbar::Registry::builtin(),
            tool_bar: Default::default(),
            menu: Default::default(),
            context_menu: Default::default(),
            canvases: Default::default(),
            context_menu_functions: crate::context_menu::DEFAULT_FUNCTIONS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            theme: Default::default(),
            themes: Default::default(),
            org_files: Vec::new(),
            org_files_loaded: false,
            global_modes: GlobalModes::default(),
            editorconfig: true,
            save_place: true,
            recentf: true,
            org_startup_folded: "showeverything".to_string(),
            org_startup_indented: false,
            org_indent_per_level: 2,
            selection: None,
            completion_session: None,
            completion_control: Vec::new(),
            transients: Vec::new(),
            prompt: None,
            prompt_open: false,
            prompt_history: None,
            prompt_input: String::new(),
            idle_hooks: Vec::new(),
            echo_area: None,
        }
    }
}

/// Minor modes on in every buffer at once.
///
/// minor modes are buffer-local unless a mode says otherwise, and
/// the ones that frame the window -- menu-bar-mode and the like -- say
/// otherwise: switching buffers shouldn't take the frame's furniture with
/// it. Kept apart from [`crate::modes::ModeRegistry`], which is what
/// defines modes and their lighters, not what turns them on.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GlobalModes {
    enabled: Vec<String>,
}

impl GlobalModes {
    /// Is this mode on in every buffer?
    pub fn enabled(&self, mode: &str) -> bool {
        self.enabled.iter().any(|m| m == mode)
    }

    pub fn enable(&mut self, mode: impl Into<String>) {
        let mode = mode.into();
        if !self.enabled(&mode) {
            self.enabled.push(mode);
        }
    }

    /// Turn it off. False if it wasn't on.
    pub fn disable(&mut self, mode: &str) -> bool {
        match self.enabled.iter().position(|m| m == mode) {
            Some(i) => {
                self.enabled.remove(i);
                true
            }
            None => false,
        }
    }

    /// The ones that are on, in the order they were turned on.
    pub fn enabled_modes(&self) -> &[String] {
        &self.enabled
    }
}

impl Editor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Is `mode` on for the selected buffer?
    ///
    /// True when it's a global mode, or a mode of this buffer's own.
    pub fn minor_mode_enabled(&self, mode: &str) -> bool {
        self.global_modes.enabled(mode)
            || self
                .state
                .active_buffer()
                .is_some_and(|b| b.minor_mode_enabled(mode))
    }

    /// Creates a buffer and shows it in the selected window.
    pub fn create_buffer(&mut self, name: impl Into<String>) -> BufferId {
        let id = self.state.create_buffer(name);
        self.show_buffer(id);
        id
    }

    // ------------------------------------------------------------ windows

    /// Show a buffer in the selected window. Point is the buffer's own
    /// point (where it was last left).
    pub fn show_buffer(&mut self, id: BufferId) {
        self.windows.selected_mut().set_buffer(id);
        self.state.active_buffer = Some(id);
    }

    /// Where a window's point is right now.
    pub fn window_point(&self, id: WindowId) -> usize {
        let Some(w) = self.windows.get(id) else {
            return 0;
        };
        let Some(b) = self.state.get_buffer(w.buffer) else {
            return 0;
        };
        if id == self.windows.selected_id() {
            b.point_offset()
        } else {
            b.transform_offset(w.saved_point.0, w.saved_point.1)
        }
    }

    /// Move a window's point (the buffer's point if it's the selected
    /// window).
    pub fn set_window_point(&mut self, id: WindowId, offset: usize) {
        let Some(buf) = self.windows.get(id).map(|w| w.buffer) else {
            return;
        };
        let Some(b) = self.state.buffers.get_mut(&buf) else {
            return;
        };
        if id == self.windows.selected_id() {
            b.set_offset(offset);
        } else {
            let saved = (offset.min(b.char_count()), b.version());
            if let Some(w) = self.windows.get_mut(id) {
                w.saved_point = saved;
            }
        }
    }

    /// Select another window: the old one remembers its point, the new
    /// one's point becomes its buffer's point.
    pub fn select_window(&mut self, id: WindowId) {
        let old = self.windows.selected_id();
        if old == id || self.windows.get(id).is_none() {
            return;
        }
        if let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        let point = self.window_point(id);
        self.windows.set_selected(id);
        let buf = self.windows.selected().buffer;
        self.state.active_buffer = Some(buf);
        if let Some(b) = self.state.get_buffer_mut(buf) {
            b.set_offset(point);
        }
    }

    /// Put a saved window arrangement back (winner-undo). The selected
    /// window's point becomes its buffer's point, as in `select_window`.
    pub fn restore_windows(&mut self, cfg: &WindowConfig) {
        if let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        let fallback = self.windows.selected().buffer;
        let state = &self.state;
        self.windows
            .restore(cfg, |b| state.get_buffer(b).is_some(), fallback);
        let id = self.windows.selected_id();
        let point = self.window_point(id);
        let buf = self.windows.selected().buffer;
        self.state.active_buffer = Some(buf);
        if let Some(b) = self.state.get_buffer_mut(buf) {
            b.set_offset(point);
        }
    }

    fn current_point(&self) -> (usize, u64) {
        self.state
            .active_buffer()
            .map(|b| (b.point_offset(), b.version()))
            .unwrap_or((0, 0))
    }

    /// C-x 2 / C-x 3. Returns the new window.
    pub fn split_window(&mut self, dir: Direction) -> Result<WindowId, String> {
        let point = self.current_point();
        self.windows.split(dir, point)
    }

    /// C-x 0 on the selected window: the next window gets selected.
    pub fn delete_window(&mut self, id: WindowId) -> Result<(), String> {
        if id == self.windows.selected_id() && self.windows.count() > 1 {
            let next = self.windows.nth_from_selected(1);
            self.select_window(next);
        }
        self.windows.delete(id)
    }

    /// C-x 1.
    pub fn delete_other_windows(&mut self) {
        self.windows.delete_others();
    }

    // ------------------------------------------------------------ frames

    /// C-x 5 o: make `id` the selected frame. The old frame's selected
    /// window remembers its point; the new frame's selected window's
    /// point becomes its buffer's point.
    pub fn select_frame(&mut self, id: FrameId) {
        if id == self.windows.selected_frame_id() || self.windows.frame_name(id).is_none() {
            return;
        }
        // Save where the outgoing frame's selected window had point.
        if let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        self.windows.select_frame(id);
        let (buf, point) = {
            let w = self.windows.selected();
            let point = self
                .state
                .get_buffer(w.buffer)
                .map_or(0, |b| b.transform_offset(w.saved_point.0, w.saved_point.1));
            (w.buffer, point)
        };
        self.state.active_buffer = Some(buf);
        if let Some(b) = self.state.get_buffer_mut(buf) {
            b.set_offset(point);
        }
    }

    /// Cycle to the frame `n` steps away (C-x 5 o).
    pub fn other_frame(&mut self, n: isize) {
        let next = self.windows.other_frame(n);
        self.select_frame(next);
    }

    /// C-x 5 1: keep only the selected frame.
    pub fn delete_other_frames(&mut self) {
        self.windows.delete_other_frames();
    }

    /// C-x 5 2: a new frame, selected, showing the current buffer. The new
    /// frame's point starts where the current one's is.
    pub fn make_frame(&mut self) -> FrameId {
        let buffer = self.windows.selected().buffer;
        let point = self.current_point();
        let name = self.unique_frame_name();
        let id = self.windows.make_frame(name, buffer, point);
        self.select_frame(id);
        id
    }

    /// A frame name "F1", "F2", ... that no frame uses.
    pub fn unique_frame_name(&self) -> String {
        let used: Vec<&str> = self
            .windows
            .frame_ids()
            .into_iter()
            .filter_map(|id| self.windows.frame_name(id))
            .collect();
        (1..)
            .map(|n| format!("F{n}"))
            .find(|name| !used.contains(&name.as_str()))
            .expect("some name is free")
    }

    /// C-x 5 0: delete a frame (never the last one). If it was selected,
    /// the point moves to whichever frame takes over.
    pub fn delete_frame(&mut self, id: FrameId) -> Result<(), String> {
        let was_selected = id == self.windows.selected_frame_id();
        if was_selected && let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        self.windows.delete_frame(id)?;
        if was_selected {
            let (buf, point) = {
                let w = self.windows.selected();
                let point = self
                    .state
                    .get_buffer(w.buffer)
                    .map_or(0, |b| b.transform_offset(w.saved_point.0, w.saved_point.1));
                (w.buffer, point)
            };
            self.state.active_buffer = Some(buf);
            if let Some(b) = self.state.get_buffer_mut(buf) {
                b.set_offset(point);
            }
        }
        Ok(())
    }

    /// C-x 5 u: bring back a frame deleted with `delete-frame` (1 is the
    /// most recently deleted). It becomes the selected frame, at the same
    /// point it had when it was deleted.
    pub fn undelete_frame(&mut self, arg: usize) -> Option<FrameId> {
        let id = self.windows.undelete_frame(arg)?;
        self.select_frame(id);
        Some(id)
    }

    /// How many frames `undelete-frame` could bring back.
    pub fn deleted_frame_count(&self) -> usize {
        self.windows.deleted_frame_count()
    }

    // ------------------------------------------------------------ tabs

    /// C-x t o: make `id` the selected tab of the selected frame. The old
    /// tab's selected window remembers its point; the new tab's selected
    /// window's point becomes its buffer's point. The same dance as
    /// [`Self::select_frame`], one level down.
    pub fn select_tab(&mut self, id: TabId) {
        if id == self.windows.selected_tab_id() || self.windows.tab_name(id).is_none() {
            return;
        }
        // Save where the outgoing tab's selected window had point.
        if let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        self.windows.select_tab(id);
        let (buf, point) = {
            let w = self.windows.selected();
            let point = self
                .state
                .get_buffer(w.buffer)
                .map_or(0, |b| b.transform_offset(w.saved_point.0, w.saved_point.1));
            (w.buffer, point)
        };
        self.state.active_buffer = Some(buf);
        if let Some(b) = self.state.get_buffer_mut(buf) {
            b.set_offset(point);
        }
    }

    /// Cycle to the tab `n` steps away (C-x t o / C-x t O).
    pub fn other_tab(&mut self, n: isize) {
        let next = self.windows.other_tab(n);
        self.select_tab(next);
    }

    /// C-x t 2: a new tab, selected, holding a copy of the current tab's
    /// window configuration, named after the buffers it shows.
    pub fn make_tab(&mut self) -> TabId {
        if let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        let name = self.current_tab_default_name();
        let id = self.windows.clone_current_tab(name);
        self.select_tab(id);
        id
    }

    /// The name a new tab starts with: the buffers the current tab shows,
    /// like default `tab-bar-tab-name-function`.
    pub fn current_tab_default_name(&self) -> String {
        let names: Vec<String> = self
            .windows
            .tab_window_buffers(self.windows.selected_tab_id())
            .into_iter()
            .filter_map(|id| self.state.get_buffer(id).map(|b| b.name().to_string()))
            .collect();
        if names.is_empty() {
            "???".to_string()
        } else {
            names.join(" ")
        }
    }

    /// C-x t 0: close a tab (never the sole one). If it was selected, the
    /// point moves to whichever tab takes over.
    pub fn delete_tab(&mut self, id: TabId) -> Result<(), String> {
        let was_selected = id == self.windows.selected_tab_id();
        if was_selected && let Some(b) = self.state.active_buffer() {
            let saved = (b.point_offset(), b.version());
            self.windows.selected_mut().saved_point = saved;
        }
        self.windows.delete_tab(id)?;
        if was_selected {
            let (buf, point) = {
                let w = self.windows.selected();
                let point = self
                    .state
                    .get_buffer(w.buffer)
                    .map_or(0, |b| b.transform_offset(w.saved_point.0, w.saved_point.1));
                (w.buffer, point)
            };
            self.state.active_buffer = Some(buf);
            if let Some(b) = self.state.get_buffer_mut(buf) {
                b.set_offset(point);
            }
        }
        Ok(())
    }

    /// C-x t r: rename the tab (an empty name brings the derived one back).
    pub fn rename_tab(&mut self, id: TabId, name: &str) -> bool {
        self.windows.set_tab_name(id, name)
    }

    /// C-x t m: move the selected tab in the order.
    pub fn move_tab(&mut self, n: isize) {
        self.windows.move_tab(n);
    }

    /// Drag a tab of `frame` to place `to` in its order.
    pub fn move_tab_to(&mut self, frame: FrameId, tab: TabId, to: usize) -> bool {
        self.windows.move_tab_to(frame, tab, to)
    }

    /// Bring back a tab deleted with `delete-tab` (1 is the most recently
    /// deleted). It becomes the selected tab, in its frame (selecting that
    /// frame if it wasn't the selected one).
    pub fn undelete_tab(&mut self, arg: usize) -> Option<(FrameId, TabId)> {
        let (fid, tid) = self.windows.undelete_tab(arg)?;
        self.select_frame(fid);
        self.select_tab(tid);
        Some((fid, tid))
    }

    /// How many tabs `tab-undo` could bring back.
    pub fn deleted_tab_count(&self) -> usize {
        self.windows.deleted_tab_count()
    }

    /// Adds a buffer (renaming it "name<2>" etc. if the name is taken).
    /// Does not make it active.
    pub fn add_buffer(&mut self, mut buffer: Buffer) -> BufferId {
        let name = self.unique_name(buffer.name());
        buffer.set_name(name);
        self.state.insert_buffer(buffer)
    }

    pub fn unique_name(&self, base: &str) -> String {
        if self.find_buffer(base).is_none() {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}<{n}>"))
            .find(|name| self.find_buffer(name).is_none())
            .expect("some suffix is free")
    }

    pub fn find_buffer(&self, name: &str) -> Option<BufferId> {
        self.state
            .buffers
            .iter()
            .find(|(_, b)| b.name() == name)
            .map(|(id, _)| *id)
    }

    pub fn find_buffer_by_path(&self, path: &Path) -> Option<BufferId> {
        let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.state
            .buffers
            .iter()
            .find(|(_, b)| {
                let Some(p) = b.path() else { return false };
                let have = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
                have == wanted
            })
            .map(|(id, _)| *id)
    }

    /// Buffer IDs in creation order.
    /// The names of the buffers the Buffers menu lists: every one but the
    /// hidden ones (names that start with a space), the selected one first.
    pub fn menu_buffer_names(&self) -> Vec<String> {
        let here = self.state.active_buffer;
        let mut ids = self.buffer_ids();
        ids.sort_by_key(|id| Some(*id) != here);
        ids.into_iter()
            .filter_map(|id| self.state.get_buffer(id))
            .map(|b| b.name().to_string())
            .filter(|n| !n.starts_with(' '))
            .collect()
    }

    /// Bring the menus that list live things up to date: call before one
    /// is shown.
    pub fn refresh_menus(&mut self) {
        let names = self.menu_buffer_names();
        self.menus.set_buffers(&names);
    }

    pub fn buffer_ids(&self) -> Vec<BufferId> {
        let mut ids: Vec<_> = self.state.buffers.keys().copied().collect();
        ids.sort();
        ids
    }

    /// The keymaps that apply in the active buffer, highest priority first.
    /// Transients that have been up longer than they asked for are dropped
    /// first, so a timeout doesn't need a timer to fire.
    ///
    /// The conditional layers are answered from [`Editor::key_context`],
    /// which the key dispatch refreshes once per keypress. It is refreshed
    /// here too, so a lookup that didn't come from a key (a command run
    /// from M-x, say) still sees the current state.
    pub fn active_keymaps(&mut self) -> ActiveKeymaps<'_> {
        self.drop_expired_transients();
        self.refresh_key_context();
        self.active_keymaps_now()
    }

    /// The same maps, without dropping anything or re-reading the buffer:
    /// for reading `where-is` or a mode's bindings, where popping a map on
    /// the way would be a surprise.
    pub fn active_keymaps_now(&self) -> ActiveKeymaps<'_> {
        let id = self
            .state
            .active_buffer
            .expect("there is always an active buffer");
        let b = self.state.get_buffer(id).expect("active buffer exists");
        self.keymaps.active(
            id,
            b.major_mode(),
            b.minor_modes(),
            &self.transients,
            self.prompt.as_deref(),
            &self.key_context,
        )
    }

    /// Whether a prompt has a keymap of its own (see
    /// [`Keymaps::prompt_map_exists`]).
    pub fn has_prompt_map(&self, name: &str) -> bool {
        self.keymaps.prompt_map_exists(name)
    }

    /// Fill in [`Editor::key_context`] for the active buffer: everything the
    /// conditional layers ask about, gathered once.
    ///
    /// The syntax face is the character *before* point, which is the one a
    /// cursor sits in as a comment or a string, and it costs a range of one
    /// character rather than the whole buffer. With no conditional layers
    /// that ask for the syntax or the git check, neither is computed.
    pub fn refresh_key_context(&mut self) {
        let Some(b) = self.state.active_buffer_mut() else {
            self.key_context = Context::default();
            return;
        };
        let wants_syntax = self
            .keymaps
            .any_layer_needs(|c| matches!(c, Condition::InComment | Condition::InString));
        let wants_git = self
            .keymaps
            .any_layer_needs(|c| matches!(c, Condition::InGitRepo));
        // Where the cursor is: inside the comment or string it is in, not
        // the one starting at the next character.
        let before = b.point_offset().saturating_sub(1);
        let face = if wants_syntax && b.point_offset() > 0 {
            b.syntax_faces(before..before.saturating_add(1))
                .first()
                .copied()
                .flatten()
        } else {
            None
        };
        let in_git_repo = wants_git && b.path().is_some_and(crate::process::in_git_repo);
        // What the text point is in brings its own keys, so the region
        // maps under point are part of every lookup. Skipped when no
        // region map has bindings, which is the usual case.
        let region_maps = if !self.keymaps.has_regions() {
            Vec::new()
        } else {
            b.decoration_keymaps_at(b.point_offset())
                .into_iter()
                .map(str::to_string)
                .collect()
        };
        // A project's bindings apply to every file in it. The root is
        // picked by prefix, so this costs nothing but a walk of the
        // maps that exist, and is skipped entirely when none do.
        let project = if self.keymaps.has_projects() {
            b.path()
                .and_then(|p| self.keymaps.project_for(p))
                .map(|(root, _)| root.clone())
        } else {
            None
        };
        self.key_context = Context {
            region_active: b.mark_active(),
            read_only: b.read_only(),
            in_git_repo,
            face_at_point: face,
            region_maps,
            project,
        };
    }

    // ------------------------------------------------- transient maps

    /// Put a transient map on top of every other layer, under `opts`.
    /// Pushing the same name again makes it the outermost one.
    pub fn push_transient(&mut self, name: &str, opts: TransientOpts) {
        // Outermost first: drop any older copy of this name, then push.
        self.transients.retain(|t| t.name != name);
        self.transients.push(opts.to_live(name.to_string()));
    }

    /// Take a transient map down. Returns whether it was up.
    pub fn pop_transient(&mut self, name: &str) -> bool {
        let before = self.transients.len();
        self.transients.retain(|t| t.name != name);
        before != self.transients.len()
    }

    /// Take down every transient map.
    pub fn pop_all_transients(&mut self) {
        self.transients.clear();
    }

    /// The names of the transient maps that are up, outermost first.
    pub fn live_transients(&self) -> Vec<String> {
        self.transients.iter().map(|t| t.name.clone()).collect()
    }

    fn drop_expired_transients(&mut self) {
        self.transients.retain(|t| !t.expired());
    }

    /// The policy of the outermost transient map, if any is up.
    pub fn transient_policy(&self) -> OnUnbound {
        self.transients
            .last()
            .map(|t| t.on_unbound)
            .unwrap_or(OnUnbound::Stay)
    }

    /// Apply the named theme `name`: clear the restyles and set the ones
    /// it defines. Applied themes are what both frontends draw from.
    pub fn load_theme(&mut self, name: &str) -> Result<(), String> {
        let faces = self
            .themes
            .get(name)
            .map(|t| t.faces.clone())
            .ok_or_else(|| format!("no theme named {name:?}"))?;
        self.theme.clear();
        for (face, style) in faces {
            self.theme.set(&face, style)?;
        }
        self.themes.set_current(Some(name.to_string()));
        Ok(())
    }

    /// Forget the applied theme's restyles (the built-in palette shows).
    pub fn disable_theme(&mut self) {
        self.theme.clear();
        self.themes.set_current(None);
    }

    /// The configured Org agenda files, reading
    /// `~/.config/forge/org-agenda-files` once (comments and blanks are
    /// skipped, `~` is expanded).
    pub fn org_agenda_files(&mut self) -> Vec<std::path::PathBuf> {
        if !self.org_files_loaded {
            self.org_files_loaded = true;
            let path = crate::config::config_dir().map(|d| d.join("org-agenda-files"));
            if let Some(path) = path
                && let Ok(text) = std::fs::read_to_string(path)
            {
                for line in text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                {
                    let f = crate::permissions::expand_home(line);
                    if !self.org_files.contains(&f) {
                        self.org_files.push(f);
                    }
                }
            }
        }
        let mut out = Vec::new();
        for f in &self.org_files {
            for g in org_files_in(f) {
                if !out.contains(&g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// Add an agenda file (at the end), keeping it unique.
    pub fn add_org_agenda_file(&mut self, file: std::path::PathBuf) {
        if !self.org_files.contains(&file) {
            self.org_files.push(file);
        }
    }

    /// The agenda files as buffers: open the configured ones, or fall back
    /// to the Org file buffers open now.
    pub fn org_agenda_buffers(&mut self) -> Vec<BufferId> {
        let files = self.org_agenda_files();
        let mut out = Vec::new();
        if files.is_empty() {
            for id in self.buffer_ids() {
                if let Some(b) = self.state.get_buffer(id)
                    && b.major_mode() == crate::org::MODE
                    && b.path().is_some()
                {
                    out.push(id);
                }
            }
            return out;
        }
        for f in files {
            if let Ok((id, _)) = self.open_file(&f)
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out
    }

    /// Open a file (or return the buffer already visiting it).
    /// The bool is true when the file doesn't exist yet.
    pub fn open_file(&mut self, path: impl AsRef<Path>) -> io::Result<(BufferId, bool)> {
        let path = absolute(path.as_ref());
        if let Some(id) = self.find_buffer_by_path(&path) {
            return Ok((id, false));
        }
        let is_new = !path.exists();
        let mut buffer = Buffer::from_file(&path)?;
        // The major mode comes from the file's name (usually its
        // extension), else a script's #! line.
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // notes.org.gpg is an Org file: the mode comes from the name
        // without .gpg.
        let mode_name = crate::epa::name_without_gpg(&file_name).unwrap_or(&file_name);
        let mode = self
            .modes
            .major_for_file(mode_name)
            .map(str::to_string)
            .or_else(|| {
                let head = (0..buffer.line_count().min(4))
                    .map(|i| buffer.line(i).to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.modes.major_for_content(&head)
            });
        if let Some(mode) = mode {
            buffer.set_major_mode(mode);
        }
        // An image file gets Image mode's buffer state; the editor fills
        // in the rendered picture.
        if buffer.major_mode() == crate::buffer::IMAGE_MODE {
            buffer.image = Some(crate::buffer::ImageState::default());
        }
        // First visit to an Org file: its #+STARTUP keywords fold it and
        // set indent mode, over the configured defaults.
        if buffer.major_mode() == crate::org::MODE {
            buffer.apply_org_startup(&self.org_startup_folded, self.org_startup_indented);
        }
        if self.editorconfig {
            crate::editorconfig::apply(&mut buffer, &path);
        }
        let key = path.display().to_string();
        if self.save_place
            && let Some(offset) = self.session.place(&key)
        {
            buffer.set_offset(offset.min(buffer.char_count()));
        }
        if self.recentf && !is_new {
            self.session.add_recent(&key);
            let _ = self.session.save();
        }
        let id = self.add_buffer(buffer);
        self.sync_language(id);
        Ok((id, is_new))
    }

    /// save-place: remember where point is in a file buffer (called when
    /// it's killed, and for every buffer when Forge quits).
    pub fn remember_place(&mut self, id: BufferId) {
        if !self.save_place {
            return;
        }
        let Some(b) = self.state.get_buffer(id) else {
            return;
        };
        let Some(path) = b.path().map(|p| p.display().to_string()) else {
            return;
        };
        let offset = if self.state.active_buffer == Some(id) {
            b.point_offset()
        } else {
            self.window_point_of(id)
        };
        self.session.remember_place(&path, offset);
    }

    /// Before quitting: remember every file's place and save the session.
    pub fn save_session(&mut self) {
        for id in self.buffer_ids() {
            self.remember_place(id);
        }
        let _ = self.session.save();
    }

    /// Point of a buffer as its window shows it (its own point if no
    /// window shows it).
    fn window_point_of(&self, id: BufferId) -> usize {
        match self.windows.iter().find(|w| w.buffer == id) {
            Some(w) => self.window_point(w.id),
            None => self.state.get_buffer(id).map_or(0, |b| b.point_offset()),
        }
    }

    /// Give a buffer the grammar of its major mode (or of the mode that
    /// derives from). Cheap when nothing changed; redisplay calls it.
    pub fn sync_language(&mut self, id: BufferId) {
        let Some(b) = self.state.buffers.get_mut(&id) else {
            return;
        };
        let lang = self.modes.language_for(b.major_mode());
        b.set_language(lang);
    }

    /// Re-indent a line of the active buffer by its major mode's rules.
    /// False if the mode has none.
    pub fn indent_line(&mut self, line: usize) -> bool {
        let id = self
            .state
            .active_buffer
            .expect("there is always an active buffer");
        self.sync_language(id);
        self.state
            .active_buffer_mut()
            .expect("active")
            .indent_line(line)
    }

    /// Remove a buffer. Windows showing it switch to `fallback` if that
    /// is still alive, else the newest remaining buffer (a fresh
    /// *scratch* if it was the last one).
    pub fn kill_buffer(&mut self, id: BufferId, fallback: Option<BufferId>) {
        self.remember_place(id);
        let _ = self.session.save();
        self.state.buffers.remove(&id);
        self.keymaps.remove_buffer(id);
        let replacement = match fallback.filter(|f| self.state.buffers.contains_key(f)) {
            Some(f) => f,
            None => match self.buffer_ids().last().copied() {
                Some(next) => next,
                None => {
                    let id = self.state.create_buffer("*scratch*");
                    if let Some(b) = self.state.get_buffer_mut(id) {
                        b.set_major_mode(crate::rhai::INTERACTION_MODE);
                    }
                    id
                }
            },
        };
        for w in self.windows.all_windows_mut() {
            w.forget_buffer(id);
            if w.buffer == id {
                w.set_buffer(replacement);
                w.saved_point = (0, 0);
            }
        }
        self.state.active_buffer = Some(self.windows.selected().buffer);
    }
}

fn absolute(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    if path.exists() {
        path.canonicalize().unwrap_or(path)
    } else {
        path
    }
}

/// The Org files a setting names: a file is itself; a directory stands for
/// every `.org` file under it, however deeply nested (what
/// `(directory-files-recursively DIR "\\.org\\'")` gives), sorted.
/// Hidden entries are skipped. A path that doesn't exist yet stays as it is,
/// so it can be created.
pub fn org_files_in(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if depth < 16 {
                    walk(&p, out, depth + 1);
                }
            } else if p.extension().is_some_and(|x| x == "org") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(path, &mut out, 0);
    out
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    fn point(ed: &Editor, id: BufferId) -> usize {
        ed.state
            .get_buffer(id)
            .expect("buffer exists")
            .point_offset()
    }

    #[test]
    fn frames_share_buffers_and_remember_their_own_point() {
        let mut ed = Editor::new();
        let a = ed.create_buffer("a");
        let b = ed.state.get_buffer_mut(a).expect("buffer");
        b.set_contents("hello world");
        b.set_offset(3);

        let f2 = ed.make_frame();
        assert_eq!(ed.windows.frame_count(), 2);
        assert_eq!(
            ed.windows.selected_frame_id(),
            f2,
            "a new frame is selected"
        );
        assert_eq!(point(&ed, a), 3, "the new frame starts at the same point");

        ed.state.get_buffer_mut(a).expect("buffer").set_offset(7);

        ed.select_frame(FrameId(0));
        assert_eq!(point(&ed, a), 3);
        ed.select_frame(f2);
        assert_eq!(point(&ed, a), 7);

        // The last frame is never deleted.
        assert!(ed.delete_frame(FrameId(0)).is_ok());
        assert_eq!(ed.windows.frame_count(), 1);
        assert!(ed.delete_frame(ed.windows.selected_frame_id()).is_err());
    }

    #[test]
    fn killing_a_buffer_heals_windows_in_every_frame() {
        let mut ed = Editor::new();
        let a = ed.create_buffer("a");
        let b = ed.create_buffer("b");
        ed.windows.selected_mut().set_buffer(a);
        ed.state.active_buffer = Some(a);
        ed.make_frame();
        // Frame 2 shows a too; leave a third buffer around to fall back on.
        ed.state.active_buffer = Some(b);
        ed.windows.selected_mut().set_buffer(b);

        ed.kill_buffer(a, Some(b));
        for w in ed.windows.all_windows_mut() {
            assert_ne!(w.buffer, a, "no window is left showing a killed buffer");
        }
    }

    #[test]
    fn undelete_brings_back_the_same_frame() {
        let mut ed = Editor::new();
        let a = ed.create_buffer("a");
        ed.state
            .get_buffer_mut(a)
            .expect("buffer")
            .set_contents("hello");
        let f1 = ed.windows.selected_frame_id();
        let f2 = ed.make_frame();
        assert_eq!(ed.windows.selected_frame_id(), f2);
        assert_eq!(ed.deleted_frame_count(), 0);

        ed.delete_frame(f1).expect("delete f1");
        assert_eq!(ed.windows.frame_count(), 1);
        assert_eq!(ed.deleted_frame_count(), 1);
        assert_eq!(ed.undelete_frame(1), Some(f1));
        assert_eq!(
            ed.windows.selected_frame_id(),
            f1,
            "the frame is selected again"
        );
        assert_eq!(ed.windows.frame_count(), 2);

        // A deleted frame's windows are healed when their buffer is killed,
        // so undelete never brings back a dangling buffer.
        ed.select_frame(f2);
        ed.delete_frame(f1).expect("delete f1 again");
        ed.kill_buffer(a, None);
        let back = ed.undelete_frame(1).expect("undelete");
        let w = ed.windows.frame_selected_id(back);
        assert_ne!(ed.windows.get(w).unwrap().buffer, a);
    }

    #[test]
    fn agenda_directories_stand_for_the_org_files_under_them() {
        let dir = std::env::temp_dir().join(format!("forge-agenda-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/deep")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for f in [
            "top.org",
            "a/one.org",
            "a/deep/two.org",
            "a/notes.txt",
            ".hidden/x.org",
        ] {
            std::fs::write(dir.join(f), "* x\n").unwrap();
        }
        let names: Vec<String> = org_files_in(&dir)
            .iter()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["a/deep/two.org", "a/one.org", "top.org"]);
        let missing = dir.join("new.org");
        assert_eq!(org_files_in(&missing), vec![missing]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
