// SPDX-License-Identifier: MIT OR Apache-2.0
//! The minibuffer: prompts (C-x C-f, M-x, C-x b, ...), y-or-n questions,
//! and incremental search.

use crate::app::{App, PrefixArg};
use crate::commands;
use crate::help;
use crate::keymap::describe_key_sequence;
use crate::plugins;
use crate::query::{self, ReplaceRequest};
use crate::registers::{self, RegAction};
use forge_core::Lookup;
use forge_core::{Buffer, BufferId, Origin, Pos, Scope};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

/// What a [`Purpose::Thing`] answer should do with the named thing.
#[derive(Debug, Clone, Copy)]
pub enum ThingAction {
    /// beginning-of-thing: move to the thing's start.
    Beginning { arg: i64 },
    /// end-of-thing: move to the thing's end.
    End { arg: i64 },
    /// bounds-of-thing-at-point: show its first and last character.
    Bounds,
    /// thing-at-point: show its text.
    Text,
}

/// What a file-and-directory prompt will do with its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    MakeDirectory,
    MakeEmptyFile,
    /// copy-directory: the directory to copy, then where to.
    CopyFrom,
    CopyTo(String),
    DeleteDirectory,
    /// A directory that isn't empty: "yes" deletes it and all in it.
    DeleteAll(String),
    InsertDirectory,
}

pub enum Purpose {
    FileOp(FileOp),
    /// A password for a remote hop (tether.rs); the answer is kept in memory.
    TetherAuth {
        key: String,
        retry: crate::tether::Retry,
    },
    FindFile {
        other_window: bool,
        other_frame: bool,
        other_tab: bool,
    },
    WriteFile,
    SwitchBuffer {
        default: Option<String>,
        other_window: bool,
        other_frame: bool,
        other_tab: bool,
    },
    KillBuffer {
        default: String,
    },
    GotoLine,
    /// set-fill-column: the new fill column, defaulting to point's
    /// current column when the answer is empty.
    SetFillColumn {
        default: usize,
    },
    /// goto-char: a buffer position (character offset, 1-based).
    GotoChar,
    /// insert-file: insert a file's contents at point.
    InsertFile,
    /// apropos-command / apropos-documentation: a word or regexp.
    Apropos {
        docs: bool,
    },
    /// The prefix argument typed before M-x, for the command it runs.
    ExtendedCommand(Option<PrefixArg>),
    /// M-% and friends: the text to replace...
    ReplaceFrom(ReplaceRequest),
    /// ...and what to replace it with.
    ReplaceTo {
        from: String,
        req: ReplaceRequest,
    },
    /// C-x r t.
    StringRectangle,
    /// C-x r m: the bookmark's name.
    BookmarkName {
        default: String,
    },
    /// Waiting for a register name (one key).
    Register(RegAction),
    /// A plugin's read-string / completing-read.
    PluginAsk {
        plugin: String,
        request: u32,
        candidates: Option<Vec<String>>,
        require_match: bool,
        /// A `completing-read-multiple`-style prompt: fields separated by
        /// [`forge_core::completion::CRM_SEPARATORS`], each completed and
        /// validated, answered with the list of fields.
        multiple: bool,
        /// The question took over the prompt that was open (a plugin
        /// offering that prompt's history): on an answer its input is
        /// replaced, on C-g it is left as it was.
        over: bool,
    },
    /// A plugin's y-or-n.
    PluginYOrN {
        plugin: String,
        request: u32,
    },
    LoadPlugin,
    UnloadPlugin,
    ReloadPlugin,
    /// y / n / C-g for a plugin's permission request.
    PluginGrant,
    /// y / n for "run this project's init.rhai?" (see `project.rs`).
    ProjectGrant {
        root: PathBuf,
    },
    RevokePluginPermissions,
    /// M-: Rhai to evaluate.
    EvalRhai,
    DescribeFunction,
    WhereIs,
    /// global-set-key / local-set-key, after the key sequence was read.
    SetKeyCommand {
        keys: Vec<String>,
        local: bool,
    },
    Confirm(ConfirmAction),
    /// A question answered with one key (a file lock, recovering a file).
    KeyChoice(crate::files::KeyChoice),
    Isearch(Isearch),
    /// Pick one of `candidates` (TAB completes).
    Choose {
        what: Choice,
        candidates: Vec<String>,
    },
    /// M-s o: the regexp to list lines for.
    Occur,
    /// Org sparse tree / occur / drawer-name prompts.
    OrgSparse,
    OrgOccur,
    OrgDrawer,
    /// beginning-of-thing, end-of-thing, bounds-of-thing-at-point or
    /// thing-at-point: which thing to look for.
    Thing(ThingAction),
    /// Every other Org question (see org_cmds.rs).
    OrgAsk(crate::org_cmds::Ask),
    /// M-x compile, grep, project-find-regexp, M-!, M-& (see compile.rs).
    Compile,
    Grep,
    /// M-x grep-find: a whole command line, like M-x grep.
    GrepFind,
    /// M-x lgrep and friends: the pattern, the files, then the
    /// directory.
    GrepSearch {
        /// What kind of search this is.
        kind: crate::grep::GrepKind,
        /// The pattern, once it has been asked for.
        pattern: Option<String>,
        /// The files to search, once they have been asked for.
        files: Option<String>,
        /// The directory to search, once it has been asked for.
        dir: Option<String>,
    },
    ProjectGrep,
    ShellCommand {
        insert: bool,
    },
    AsyncShellCommand,
    /// package-vc-install: `URL[@REV]`.
    PackageVc,
    /// package-manager's substring filter.
    PackageFilter,
    PackageSnapshot(PackageSnapshotAction),
    /// Dired: where to copy or move to, or the directory to make.
    Dired(crate::dired::DiredAction),
    /// A version-control question (a revision, a branch, a directory).
    VcAsk {
        what: crate::vc::Ask,
        candidates: Vec<String>,
    },
    /// insert-char, the emoji commands, iso-transl-set-language (C-x 8).
    CharAsk(crate::iso_transl::Ask),
    /// An input method's name (set-input-method, C-x \\, C-h I).
    InputMethodAsk {
        what: crate::input_method::Ask,
        default: Option<String>,
    },
    /// set-frame-name: the new name for the selected frame.
    SetFrameName {
        default: String,
    },
    /// Image mode: a percentage, scale, frame or file name.
    ImageAsk(crate::image_mode::Ask),
    /// tab-rename: the tab's new name (an empty name clears a custom one).
    RenameTab {
        default: String,
    },
    /// make-frame-on-display / make-frame-on-monitor: which display or
    /// monitor to put the new frame on.
    MakeFrameOnDisplay {
        monitor: bool,
    },
    /// corfu-move-to-minibuffer: a completion session moved from a buffer
    /// into the minibuffer. The candidates are the popup's; a pick replaces
    /// the completed word in `buffer` and is committed as the popup would.
    Transfer {
        /// The buffer the completed word is in.
        buffer: BufferId,
        /// Where the completed word starts (char offset) and how long it
        /// was, at `version`.
        start: usize,
        len: usize,
        version: u64,
        /// The completion's candidates, and their annotations (parallel).
        candidates: Vec<String>,
        notes: Vec<String>,
    },
}

impl Purpose {
    /// The prompt map in charge while this prompt is up. A prompt with no
    /// map of its own falls back to the default one, so naming every
    /// purpose costs nothing until something rebinds it.
    pub fn scope_name(&self) -> &'static str {
        match self {
            Purpose::FindFile { .. } => "find-file",
            Purpose::TetherAuth { .. } => "tether-password",
            Purpose::WriteFile => "write-file",
            Purpose::SwitchBuffer { .. } => "switch-to-buffer",
            Purpose::KillBuffer { .. } => "kill-buffer",
            Purpose::GotoLine => "goto-line",
            Purpose::SetFillColumn { .. } => "set-fill-column",
            Purpose::GotoChar => "goto-char",
            Purpose::InsertFile => "insert-file",
            Purpose::FileOp(_) => "file-op",
            Purpose::Apropos { .. } => "apropos",
            Purpose::ExtendedCommand(_) => "m-x",
            Purpose::ReplaceFrom(_) => "query-replace",
            Purpose::ReplaceTo { .. } => "query-replace-to",
            Purpose::StringRectangle => "string-rectangle",
            Purpose::BookmarkName { .. } => "bookmark-set",
            Purpose::Register(_) => "register",
            Purpose::PluginAsk { .. } => "plugin-ask",
            Purpose::PluginYOrN { .. } => "plugin-y-or-n",
            Purpose::LoadPlugin => "load-plugin",
            Purpose::UnloadPlugin => "unload-plugin",
            Purpose::ReloadPlugin => "reload-plugin",
            Purpose::PluginGrant => "plugin-grant",
            Purpose::ProjectGrant { .. } => "plugin-grant",
            Purpose::RevokePluginPermissions => "revoke-plugin-permissions",
            Purpose::EvalRhai => "eval-rhai",
            Purpose::DescribeFunction => "describe-function",
            Purpose::WhereIs => "where-is",
            Purpose::SetKeyCommand { .. } => "set-key",
            Purpose::Confirm(_) => "confirm",
            Purpose::KeyChoice(_) => "key-choice",
            Purpose::Isearch(_) => "isearch",
            Purpose::Choose { .. } => "choose",
            Purpose::Occur => "occur",
            Purpose::OrgSparse => "org-sparse-tree",
            Purpose::OrgOccur => "org-occur",
            Purpose::OrgDrawer => "org-insert-drawer",
            Purpose::Thing(_) => "thing",
            Purpose::OrgAsk(_) => "org-ask",
            Purpose::Compile => "compile",
            Purpose::Grep => "grep",
            Purpose::GrepFind => "grep-find",
            Purpose::GrepSearch { .. } => "grep-search",
            Purpose::ProjectGrep => "project-grep",
            Purpose::ShellCommand { .. } => "shell-command",
            Purpose::AsyncShellCommand => "async-shell-command",
            Purpose::PackageVc => "package-vc",
            Purpose::PackageFilter => "package-manager-filter",
            Purpose::PackageSnapshot(_) => "package-snapshot",
            Purpose::Dired(_) => "dired",
            Purpose::VcAsk { .. } => "vc-ask",
            Purpose::CharAsk(_) => "char-ask",
            Purpose::InputMethodAsk { .. } => "input-method",
            Purpose::SetFrameName { .. } => "set-frame-name",
            Purpose::ImageAsk(_) => "image-mode",
            Purpose::RenameTab { .. } => "tab-rename",
            Purpose::MakeFrameOnDisplay { .. } => "make-frame-on-display",
            // A moved completion keeps the list's keys (so it can be moved
            // in and picked like any other list).
            Purpose::Transfer { .. } => COMPLETION_LIST,
        }
    }
}

/// The prompt map used while the vertical completion list is open. It
/// replaces the default prompt map for as long as the list is up, so it
/// carries the default's keys as well as the list's.
pub const COMPLETION_LIST: &str = "completion-list";

#[derive(Debug, Clone)]
pub enum PackageSnapshotAction {
    Save,
    DiffFrom,
    DiffTo(String),
    Restore,
}

/// What a [`Purpose::Choose`] answer is for.
pub enum Choice {
    /// recentf-open.
    RecentFile,
    /// imenu: where each candidate is.
    Imenu(Vec<Pos>),
    /// org-goto: where each heading candidate is.
    OrgGoto(Vec<Pos>),
    /// An Org question that picks from a list (see org_cmds.rs).
    Org(crate::org_cmds::Ask),
    /// project-find-file: files relative to this root.
    ProjectFile(PathBuf),
    /// bookmark-jump / bookmark-delete: which bookmark.
    BookmarkJump,
    BookmarkDelete,
    /// describe-symbol: nothing extra (the answer names the symbol).
    DescribeSymbol {
        /// The symbol at point, when the answer is empty.
        default: Option<String>,
    },
    /// select-and-paste: which kill-ring entry, by its number in the
    /// candidate ("3: first line…").
    KillEntry,
    InstallPackage,
    DeletePackage,
    UpgradePackage,
    UpgradeVcPackage,
    GrammarInstall,
    /// describe-plugin: nothing extra (the answer names the plugin).
    DescribePlugin,
    /// select-frame-by-name: the answer is the name of a frame.
    SelectFrame,
    /// tab-switch: the answer is the name of a tab.
    SelectTab,
    /// info-goto-node: the title of a manual node to open.
    GotoNode,
    /// info-menu: one of the current node's children.
    InfoMenu,
    /// info-search: a regexp to find in the manual.
    InfoSearch,
    /// info-index: an index term of the manual.
    InfoIndex,
    /// info-lookup-command: a command name to find the manual node for.
    InfoLookup,
    /// load-theme: which named theme to apply.
    LoadTheme,
}

/// The history kind a prompt records into, for the frontend to mirror into
/// the core editor (minibuffer.history-kind).
pub fn prompt_history_kind(mb: &Minibuffer) -> Option<&'static str> {
    history_kind(&mb.purpose)
}

/// Which history list a prompt uses (M-p / M-n), if any.
fn history_kind(p: &Purpose) -> Option<&'static str> {
    Some(match p {
        // Isearch keeps its strings in the "search" history, so a plugin
        // can offer them (consult-isearch-history).
        Purpose::Isearch(_) => "search",
        Purpose::FindFile { .. }
        | Purpose::WriteFile
        | Purpose::InsertFile
        | Purpose::FileOp(_)
        | Purpose::LoadPlugin
        | Purpose::Dired(_) => "file",
        Purpose::SwitchBuffer { .. } | Purpose::KillBuffer { .. } => "buffer",
        Purpose::ExtendedCommand(_)
        | Purpose::DescribeFunction
        | Purpose::WhereIs
        | Purpose::SetKeyCommand { .. } => "command",
        Purpose::GotoLine | Purpose::GotoChar => "line",
        Purpose::Apropos { .. } => "regexp",
        Purpose::ReplaceFrom(_) | Purpose::ReplaceTo { .. } => "replace",
        Purpose::StringRectangle => "rectangle",
        Purpose::BookmarkName { .. } => "bookmark",
        Purpose::Choose {
            what: Choice::BookmarkJump | Choice::BookmarkDelete,
            ..
        } => "bookmark",
        Purpose::Choose {
            what: Choice::DescribeSymbol { .. },
            ..
        } => "describe",
        Purpose::EvalRhai => "eval",
        Purpose::Occur | Purpose::ProjectGrep | Purpose::OrgSparse | Purpose::OrgOccur => "regexp",
        Purpose::OrgAsk(_) => "org",
        Purpose::Thing(_) => "thing",
        Purpose::Compile => "compile",
        Purpose::Grep => "grep",
        Purpose::GrepFind => "grep",
        Purpose::GrepSearch { .. } => "regexp",
        Purpose::VcAsk { .. } => "vc",
        Purpose::InputMethodAsk { .. } => "input-method",
        Purpose::ShellCommand { .. } | Purpose::AsyncShellCommand => "shell",
        Purpose::Choose {
            what: Choice::ProjectFile(_),
            ..
        } => "project-file",
        Purpose::PluginAsk { .. } => "plugin",
        Purpose::Choose {
            what: Choice::DescribePlugin,
            ..
        } => "plugin",
        Purpose::Choose {
            what: Choice::RecentFile,
            ..
        } => "file",
        Purpose::Choose {
            what: Choice::InfoSearch,
            ..
        } => "regexp",
        Purpose::Choose {
            what: Choice::GotoNode | Choice::InfoMenu | Choice::InfoIndex,
            ..
        } => "manual",
        Purpose::Choose {
            what: Choice::InfoLookup,
            ..
        } => "command",
        Purpose::Choose {
            what: Choice::LoadTheme,
            ..
        } => "theme",
        Purpose::Choose {
            what: Choice::GrammarInstall,
            ..
        } => "grammar",
        _ => return None,
    })
}

pub enum ConfirmAction {
    Quit,
    KillBuffer(BufferId),
    /// Save even though the file changed on disk.
    Save(BufferId),
    /// Save a remote file even though it changed there (tether.rs).
    TetherSave(BufferId),
    Revert(BufferId),
    Recover(BufferId),
    /// Build a package from version control (package_vc.rs).
    BuildPackage,
    /// Apply a reviewed package-index update.
    IndexPackageUpgrade,
    /// Apply a reviewed Git package update.
    GitPackageUpgrade,
    /// Dired: delete these files.
    DiredDelete(Vec<PathBuf>),
    /// A grep about to run: save this buffer, then the ones in `rest`.
    GrepSave {
        buffer: BufferId,
        rest: Vec<BufferId>,
    },
    /// Org Crypt: offering to decrypt after opening a file.
    OrgCryptDecrypt,
    /// Numbered backups past kept-old-versions/kept-new-versions.
    DeleteBackups(Vec<PathBuf>),
    /// Something version control asked (see vc/mod.rs).
    Vc(crate::vc::Confirm),
}

pub struct Isearch {
    forward: bool,
    origin: Pos,
    query: String,
    failing: bool,
    wrapped: bool,
    current: Option<(Pos, Pos)>,
}

pub struct Minibuffer {
    pub prompt: String,
    pub input: String,
    pub purpose: Purpose,
    /// Shown after the input until the next key (e.g. " [No match]").
    pub note: Option<String>,
    /// A passphrase-style prompt: the input shows as bullets and is kept
    /// out of the session history and its file (see `submit`).
    pub secret: bool,
    /// M-p / M-n: how far back in the history (0 = what was typed).
    history_pos: usize,
    /// What was typed before going into the history.
    typed: String,
    /// Where the cursor is in `input`, as (character index, input length
    /// when it was set). A cursor whose length no longer matches belongs
    /// to an older input and counts as the end.
    edit: Option<(usize, usize)>,
    /// The window *Completions* is in, and the buffer it showed before
    /// (None: the window was made for it and goes with the prompt).
    completions: Option<(forge_core::WindowId, Option<forge_core::BufferId>)>,
    /// The candidates for this prompt, filtered by the input (see
    /// [`refresh`]).
    pub list: Option<CandidateList>,
}

/// A row of the vertical list: candidate, annotation, selected.
pub type Row = (String, String, bool);

/// A prompt's candidates, as the vertical list (and TAB) see them.
pub struct CandidateList {
    /// Rebuilt when this changes (the purpose, and the directory for files).
    key: String,
    pub category: &'static str,
    /// Every candidate (for files: the names in the directory).
    all: Vec<String>,
    /// Annotations (from a plugin), parallel to `all`; may be empty.
    notes: Vec<String>,
    /// For files: the directory part of the input, put before a name.
    prefix: String,
    /// Indexes into `all` that match the input, best first.
    pub matches: Vec<usize>,
    pub selected: usize,
    /// The input the matches are for.
    input: String,
}

impl CandidateList {
    /// The candidate at match `i` as it would go in the input.
    pub fn full(&self, i: usize) -> Option<String> {
        self.matches
            .get(i)
            .map(|&k| format!("{}{}", self.prefix, self.all[k]))
    }

    pub fn selected_full(&self) -> Option<String> {
        self.full(self.selected)
    }

    /// Rows to show: (candidate, annotation, selected), `lines` of them
    /// around the selection. For files the candidate is the short name
    /// (`wit/`); the directory stays in the prompt input, traditional, and
    /// the annotation carries the metadata.
    pub fn rows(&self, lines: usize) -> Vec<(String, String, bool)> {
        let lines = lines.max(1);
        let start = self
            .selected
            .saturating_sub(lines - 1)
            .min(self.matches.len().saturating_sub(lines));
        self.matches
            .iter()
            .enumerate()
            .skip(start)
            .take(lines)
            .map(|(i, &k)| {
                (
                    self.all[k].clone(),
                    self.notes.get(k).cloned().unwrap_or_default(),
                    i == self.selected,
                )
            })
            .collect()
    }
}

/// `*Completions*` buffer.
pub const COMPLETIONS: &str = "*Completions*";

/// Lay `items` out in columns that fit `width`, row by row.
fn completion_columns(items: &[String], width: usize) -> String {
    let cell = items.iter().map(|s| s.chars().count()).max().unwrap_or(0) + 2;
    let cols = (width.max(cell) / cell).max(1);
    let mut out = String::new();
    for row in items.chunks(cols) {
        let line: Vec<String> = row.iter().map(|s| format!("{s:<w$}", w = cell)).collect();
        out.push_str(line.concat().trim_end());
        out.push('\n');
    }
    out
}

/// Show the candidates in *Completions*, in another window, without
/// leaving the prompt.
fn show_completions(app: &mut App, mb: &mut Minibuffer, items: &[String]) {
    let width = app.win().width;
    let n = items.len();
    let text = format!(
        "{n} possible completion{}:\n\n{}",
        if n == 1 { "" } else { "s" },
        completion_columns(items, width.max(20))
    );
    let id = match app.editor.find_buffer(COMPLETIONS) {
        Some(id) => id,
        None => app.editor.add_buffer(forge_core::Buffer::new(COMPLETIONS)),
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_read_only(false);
        b.set_contents(&text);
        b.set_major_mode("special-mode");
        b.set_read_only(true);
        b.set_modified(false);
    }
    let before: Vec<(forge_core::WindowId, forge_core::BufferId)> = app
        .editor
        .windows
        .iter()
        .map(|w| (w.id, w.buffer))
        .collect();
    crate::compile::display_buffer(app, id);
    if mb.completions.is_none()
        && let Some(w) = app.editor.windows.iter().find(|w| w.buffer == id)
    {
        let was = before.iter().find(|(i, _)| *i == w.id).map(|(_, b)| *b);
        mb.completions = Some((w.id, was));
    }
    let shown = app
        .editor
        .windows
        .iter()
        .find(|w| w.buffer == id)
        .map(|w| w.id);
    if let Some(w) = shown {
        app.editor.set_window_point(w, 0);
    }
}

/// Take *Completions* away when the prompt is done (or the list isn't
/// needed any more): close the window made for it, or show again what the
/// window had.
fn hide_completions(app: &mut App, mb: &mut Minibuffer) {
    let Some((win, was)) = mb.completions.take() else {
        return;
    };
    let shows = app
        .editor
        .find_buffer(COMPLETIONS)
        .is_some_and(|id| app.editor.windows.get(win).is_some_and(|w| w.buffer == id));
    if !shows {
        return;
    }
    match was {
        Some(prev) if app.editor.state.get_buffer(prev).is_some() => {
            if let Some(w) = app.editor.windows.get_mut(win) {
                w.buffer = prev;
            }
        }
        _ => {
            let _ = app.editor.delete_window(win);
        }
    }
}

/// `?`: list what could complete the input. In a prompt with nothing to
/// complete it is just a question mark.
pub fn cmd_completion_help(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        refresh(app, &mut mb);
        let items: Vec<String> = mb
            .list
            .as_ref()
            .map(|l| l.matches.iter().map(|&k| l.all[k].clone()).collect())
            .unwrap_or_default();
        if mb.list.is_none() {
            mb.insert("?");
        } else if items.is_empty() {
            mb.note = Some(" [No match]".into());
        } else {
            show_completions(app, &mut mb, &items);
        }
        app.minibuffer = Some(mb);
    });
}

/// Letters and digits are the word characters of the minibuffer.
fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

impl Minibuffer {
    fn len(&self) -> usize {
        self.input.chars().count()
    }

    /// The cursor, as a character index into `input`.
    pub fn pos(&self) -> usize {
        let len = self.len();
        self.edit
            .filter(|(_, l)| *l == len)
            .map_or(len, |(c, _)| c.min(len))
    }

    fn set_pos(&mut self, p: usize) {
        let len = self.len();
        self.edit = Some((p.min(len), len));
    }

    fn byte_at(&self, p: usize) -> usize {
        self.input
            .char_indices()
            .nth(p)
            .map_or(self.input.len(), |(i, _)| i)
    }

    /// Insert text at the cursor.
    pub fn insert(&mut self, s: &str) {
        let p = self.pos();
        let b = self.byte_at(p);
        self.input.insert_str(b, s);
        self.set_pos(p + s.chars().count());
    }

    /// Remove characters `from..to` and put the cursor at `from`.
    fn remove(&mut self, from: usize, to: usize) -> String {
        let (a, b) = (self.byte_at(from), self.byte_at(to.max(from)));
        let gone: String = self.input[a..b].to_string();
        self.input.replace_range(a..b, "");
        self.set_pos(from);
        gone
    }

    fn word_forward(&self, from: usize) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = from;
        while i < chars.len() && !is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        i
    }

    fn word_backward(&self, from: usize) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = from.min(chars.len());
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
        i
    }

    /// The prompt and the input up to the cursor: the column it sits in.
    pub fn text_to_cursor(&self) -> String {
        let upto: String = self.input.chars().take(self.pos()).collect();
        let shown = if self.secret {
            "•".repeat(upto.chars().count())
        } else {
            upto
        };
        format!("{}{}", self.prompt, shown)
    }

    pub fn new(prompt: impl Into<String>, purpose: Purpose) -> Self {
        Self {
            prompt: prompt.into(),
            input: String::new(),
            purpose,
            note: None,
            secret: false,
            history_pos: 0,
            typed: String::new(),
            edit: None,
            completions: None,
            list: None,
        }
    }

    /// What the prompt effectively holds, for `minibuffer.input`: the typed
    /// line, or the search string while isearching.
    pub fn current_input(&self) -> String {
        match &self.purpose {
            Purpose::Isearch(s) => s.query.clone(),
            _ => self.input.clone(),
        }
    }

    /// The prompt and input (what the cursor goes after).
    pub fn text(&self) -> String {
        match &self.purpose {
            Purpose::Isearch(s) => {
                let mut label = String::new();
                if s.failing {
                    label.push_str("failing ");
                }
                if s.wrapped {
                    label.push_str("wrapped ");
                }
                label.push_str("I-search");
                if !s.forward {
                    label.push_str(" backward");
                }
                let mut chars = label.chars();
                let first = chars
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default();
                format!("{first}{}: {}", chars.as_str(), s.query)
            }
            _ => {
                let shown = if self.secret {
                    "•".repeat(self.input.chars().count())
                } else {
                    self.input.clone()
                };
                format!("{}{}", self.prompt, shown)
            }
        }
    }

    /// During isearch the cursor stays in the buffer, on the match.
    pub fn cursor_in_echo_area(&self) -> bool {
        !matches!(self.purpose, Purpose::Isearch(_))
    }
}

/// How many rows the vertical completion list takes above the echo area,
/// or 0 when none is showing. The frame layout uses it so a bottom tool
/// bar sits below the list, where the terminal draws it.
pub fn vertical_height(app: &App) -> u16 {
    let Some(mb) = &app.minibuffer else {
        return 0;
    };
    if !mb.cursor_in_echo_area() {
        return 0;
    }
    let Some(lines) = list_lines(app, mb) else {
        return 0;
    };
    let Some(list) = mb.list.as_ref().filter(|l| !l.matches.is_empty()) else {
        return 0;
    };
    list.rows(lines as usize).len() as u16
}

/// How many rows the vertical list shows, if any: what a plugin set
/// (vertico, corfu-minibuffer-mode), or corfu's default for a completion
/// moved in from the buffer (corfu-move-to-minibuffer), so the list is up
/// even with no plugin listening.
fn list_lines(app: &App, mb: &Minibuffer) -> Option<u32> {
    app.editor
        .completion
        .vertical_lines()
        .or_else(|| matches!(mb.purpose, Purpose::Transfer { .. }).then_some(TRANSFER_LINES))
}

/// Rows the moved completion's list shows (corfu-count).
const TRANSFER_LINES: u32 = 10;

/// Which prompt map is in charge: the vertical completion list while it is
/// open, otherwise the prompt's own purpose.
pub fn active_scope(app: &App, mb: &Minibuffer) -> &'static str {
    if list_lines(app, mb).is_some() && mb.list.as_ref().is_some_and(|l| !l.matches.is_empty()) {
        COMPLETION_LIST
    } else {
        mb.purpose.scope_name()
    }
}

/// The command bound to `key` in prompt map `scope`, if any (a plugin's
/// binding in a prompt, or the isearch map).
fn prompt_command(
    app: &mut App,
    scope: &str,
    key: KeyEvent,
    ctrl: bool,
    alt: bool,
) -> Option<String> {
    app.editor.prompt = Some(scope.to_string());
    let answer = app.editor.active_keymaps_now().lookup_above(
        &[describe(key, ctrl, alt)],
        forge_core::keymap::PRIORITY_PROMPT,
    );
    app.editor.prompt = None;
    match answer {
        Lookup::Command { command, .. } => Some(command),
        _ => None,
    }
}

/// Keys while a prompt is up: look the key up in the prompt map and run
/// what it says. A key the map doesn't bind is typed, which is what makes
/// a prompt read text.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(mut mb) = app.minibuffer.take() else {
        return;
    };
    mb.note = None;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // isearch keeps the cursor in the buffer, on the match, so its keys are
    // its own rather than the prompt's. A plugin may still bind a key in the
    // "isearch" map (consult-isearch-history, on M-r); such a binding wins.
    // Only a map of isearch's own counts: otherwise the default prompt map's
    // RET and C-g would hijack the search.
    if matches!(mb.purpose, Purpose::Isearch(_)) {
        if app.editor.has_prompt_map("isearch")
            && let Some(command) = prompt_command(app, "isearch", key, ctrl, alt)
        {
            app.minibuffer = Some(mb);
            app.run_command(&command);
            return;
        }
        if let Purpose::Isearch(s) = &mut mb.purpose
            && isearch_key(app, s, key)
        {
            app.minibuffer = Some(mb);
        }
        return;
    }

    // Refresh before asking which map is in charge: the list is what makes
    // it the completion map, and it is built here. The prompts that answer
    // to one or two keys never completed, so they don't get a list.
    let completes = !matches!(
        mb.purpose,
        Purpose::Register(_)
            | Purpose::PluginGrant
            | Purpose::PluginYOrN { .. }
            | Purpose::Confirm(_)
            | Purpose::KeyChoice(_)
    );
    if completes && list_lines(app, &mb).is_some() {
        refresh(app, &mut mb);
    }
    let scope = active_scope(app, &mb);
    let found = prompt_command(app, scope, key, ctrl, alt);

    match found {
        Some(command) => {
            // The command works on the prompt, so it has to be back in place
            // before it runs.
            app.minibuffer = Some(mb);
            app.run_command(&command);
        }
        None if let Purpose::Register(action) = mb.purpose => {
            if let KeyCode::Char(c) = key.code
                && !ctrl
                && !alt
            {
                app.message = None;
                registers::run(app, action, c);
            } else {
                app.minibuffer = Some(mb);
            }
        }
        // So does a one-key question.
        None if matches!(mb.purpose, Purpose::KeyChoice(_)) => {
            if let KeyCode::Char(c) = key.code
                && !ctrl
                && !alt
            {
                app.message = None;
                let Purpose::KeyChoice(choice) = &mb.purpose else {
                    unreachable!()
                };
                if !crate::files::answer(app, choice, c) && app.minibuffer.is_none() {
                    app.minibuffer = Some(mb);
                }
            } else {
                app.minibuffer = Some(mb);
            }
        }
        None => match key.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                mb.insert(&c.to_string());
                app.minibuffer = Some(mb);
            }
            _ => app.minibuffer = Some(mb),
        },
    }
}

/// The key notation, for the lookup.
pub fn describe(key: KeyEvent, ctrl: bool, alt: bool) -> String {
    match key.code {
        KeyCode::Char(c) => match ctrl {
            true => format!("C-{c}"),
            false if alt => format!("M-{c}"),
            false => c.to_string(),
        },
        _ => crate::keymap::describe_key(&key, alt).unwrap_or_default(),
    }
}

/// Open a file and show it (C-x C-f, recentf-open).
pub(crate) fn visit_file(app: &mut App, path: &std::path::Path, other_window: bool) {
    visit_file_where(app, path, other_window, false, false);
}

/// [`visit_file`], optionally in another frame (C-x 5 f) or tab (C-x t f).
pub(crate) fn visit_file_where(
    app: &mut App,
    path: &std::path::Path,
    other_window: bool,
    other_frame: bool,
    other_tab: bool,
) {
    if let Some(name) = path.to_str()
        && crate::tether::visit(app, name, other_window, other_frame, other_tab)
    {
        return;
    }
    let followed = follow_vc_symlink(app, path);
    let path = followed.as_deref().unwrap_or(path);
    if other_tab {
        app.editor.make_tab();
    }
    // A directory opens in Dired.
    if path.is_dir() {
        if other_frame {
            app.editor.make_frame();
        } else if other_window {
            crate::windows::pop_to_other_window(app);
        }
        return crate::dired::open(app, path);
    }
    match app.editor.open_file(path) {
        Ok((id, is_new)) => {
            if other_frame {
                crate::frames::show_in_other_frame(app, id);
            } else {
                if other_window {
                    crate::windows::pop_to_other_window(app);
                }
                app.switch_to(id);
            }
            if app.buf_ref().has_newer_auto_save() {
                crate::files::warn_about_auto_save_data(app);
            } else if is_new {
                app.msg("(New file)");
            }
            crate::org_crypt::offer_decrypt(app);
        }
        Err(e) => app.error(format!("Cannot open {}: {e}", path.display())),
    }
}

/// `vc-follow-symlinks`: a link to a file under version control opens the
/// file it points to, so edits and VC commands are about the real thing.
fn follow_vc_symlink(app: &mut App, path: &std::path::Path) -> Option<std::path::PathBuf> {
    if !crate::org_flow::setting_flag(app, "vc_follow_symlinks") {
        return None;
    }
    if !std::fs::symlink_metadata(path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    if !real.is_file() {
        return None;
    }
    let versioned = real
        .ancestors()
        .skip(1)
        .any(|d| d.join(".git").exists() || d.join(".hg").exists());
    if !versioned {
        return None;
    }
    app.msg(format!(
        "Symbolic link to version-controlled source file; following link to {}",
        real.display()
    ));
    Some(real)
}

/// C-g in a prompt. A plugin's question is answered with "cancelled",
/// and a project's init file isn't run.
fn cancel(app: &mut App, mut mb: Minibuffer) {
    hide_completions(app, &mut mb);
    app.msg("Quit");
    match mb.purpose {
        Purpose::PluginAsk {
            plugin,
            request,
            over,
            ..
        } => {
            // An over question gives the prompt it took over back, unchanged
            // (an isearch keeps its match highlighted).
            if over && let Some(mut prev) = app.suspended_prompt.take() {
                if let Purpose::Isearch(s) = &mut prev.purpose {
                    app.highlight = s.current;
                }
                app.minibuffer = Some(prev);
            }
            plugins::answer(app, &plugin, request, None);
        }
        Purpose::ProjectGrant { root } => crate::project::answered(app, &root, None),
        Purpose::KeyChoice(choice) => crate::files::cancel(app, &choice),
        Purpose::Confirm(ConfirmAction::IndexPackageUpgrade) => {
            crate::packages::package_upgrade_confirmed(app, false);
        }
        Purpose::Confirm(ConfirmAction::GitPackageUpgrade) => {
            crate::package_vc::upgrade_review_declined(app);
        }
        _ => {}
    }
}

fn confirm(app: &mut App, action: ConfirmAction) {
    match action {
        ConfirmAction::Quit => app.quit = true,
        ConfirmAction::BuildPackage => crate::package_vc::confirmed(app),
        ConfirmAction::IndexPackageUpgrade => {
            crate::packages::package_upgrade_confirmed(app, true);
        }
        ConfirmAction::GitPackageUpgrade => crate::package_vc::reviewed_upgrade(app),
        ConfirmAction::DiredDelete(paths) => crate::dired::delete_confirmed(app, paths),
        ConfirmAction::KillBuffer(id) => app.kill_buffer(id),
        ConfirmAction::DeleteBackups(paths) => {
            forge_core::backup::delete_excess(&paths);
            app.msg(format!(
                "Deleted {} old backup{}",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            ));
        }
        ConfirmAction::Vc(c) => crate::vc::confirmed(app, c),
        ConfirmAction::GrepSave { buffer, rest } => {
            crate::grep::save_confirmed(app, buffer, rest, true);
        }
        ConfirmAction::OrgCryptDecrypt => crate::org_crypt::decrypt_after_confirm(app),
        ConfirmAction::Save(id) if id == app.active_id() => commands::save(app),
        ConfirmAction::TetherSave(id) if id == app.active_id() => {
            crate::tether::save(app, true);
        }
        ConfirmAction::Revert(id) | ConfirmAction::Recover(id) if id == app.active_id() => {
            let revert = matches!(action, ConfirmAction::Revert(_));
            let result = if revert {
                app.buf().revert()
            } else {
                app.buf().recover_auto_save()
            };
            let name = app.buf_ref().name().to_string();
            match (result, revert) {
                (Ok(()), true) => app.msg(format!("Reverted {name}")),
                (Ok(()), false) => app.msg(format!(
                    "Recovered {name}; save it to keep the recovered text"
                )),
                (Err(e), _) => app.error(format!("{name}: {e}")),
            }
        }
        // The buffer changed while the question was up.
        _ => {}
    }
}

fn history_step(app: &App, mb: &mut Minibuffer, by: isize) {
    if mb.secret {
        return;
    }
    let Some(kind) = history_kind(&mb.purpose) else {
        return;
    };
    let items = app.editor.session.history(kind);
    let pos = mb.history_pos as isize + by;
    if pos < 0 || pos as usize > items.len() {
        mb.note = Some(
            if by > 0 {
                " [Beginning of history; no preceding item]"
            } else {
                " [End of history; no default available]"
            }
            .into(),
        );
        return;
    }
    if mb.history_pos == 0 {
        mb.typed = mb.input.clone();
    }
    mb.history_pos = pos as usize;
    mb.input = if pos == 0 {
        mb.typed.clone()
    } else {
        items[items.len() - pos as usize].clone()
    };
}

fn submit(app: &mut App, mut mb: Minibuffer) {
    hide_completions(app, &mut mb);
    let input = mb.input.trim().to_string();
    if !mb.secret
        && let Some(kind) = history_kind(&mb.purpose)
    {
        app.editor.session.add_history(kind, &mb.input);
        if app.savehist {
            let _ = app.editor.session.save();
        }
    }
    match mb.purpose {
        // Not a question with an answer: RET on it loads, like y.
        Purpose::ProjectGrant { root } => crate::project::answered(app, &root, Some(true)),
        Purpose::TetherAuth { key, retry } => crate::tether::answered(app, &key, retry, &input),
        Purpose::FindFile {
            other_window,
            other_frame,
            other_tab,
        } => {
            if input.is_empty() {
                return;
            }
            let path = expand_path(&input);
            let path = if path.is_absolute() {
                path
            } else {
                buffer_directory(app).join(path)
            };
            visit_file_where(app, &path, other_window, other_frame, other_tab);
        }
        Purpose::WriteFile => {
            if input.is_empty() {
                return;
            }
            let path = expand_path(&input);
            if crate::org_crypt::write_file_with_crypt(app, &path) {
                return;
            }
            match app.buf().save_as(&path) {
                Ok(()) => {
                    if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
                        && app.buf_ref().name() != name
                    {
                        let unique = app.editor.unique_name(&name);
                        app.buf().set_name(unique);
                    }
                    app.msg(format!("Wrote {}", path.display()));
                }
                Err(e) => app.msg(format!("Error writing {}: {e}", path.display())),
            }
        }
        Purpose::SwitchBuffer {
            default,
            other_window,
            other_frame,
            other_tab,
        } => {
            let name = if input.is_empty() {
                match default {
                    Some(d) => d,
                    None => return,
                }
            } else {
                input
            };
            let id = match app.editor.find_buffer(&name) {
                Some(id) => id,
                None => app.editor.add_buffer(Buffer::new(name)),
            };
            if other_tab {
                app.editor.make_tab();
            }
            if other_frame {
                crate::frames::show_in_other_frame(app, id);
            } else {
                if other_window {
                    crate::windows::pop_to_other_window(app);
                }
                app.switch_to(id);
            }
        }
        Purpose::KillBuffer { default } => {
            let name = if input.is_empty() { default } else { input };
            match app.editor.find_buffer(&name) {
                None => app.msg(format!("No such buffer: {name}")),
                Some(id) => {
                    let b = app.editor.state.get_buffer(id).expect("found");
                    if b.is_modified() && b.path().is_some() {
                        app.prompt(
                            format!("Buffer {name} modified; kill anyway? (y or n) "),
                            Purpose::Confirm(ConfirmAction::KillBuffer(id)),
                        );
                    } else {
                        app.kill_buffer(id);
                    }
                }
            }
        }
        Purpose::GotoChar => match input.trim().parse::<usize>() {
            Ok(n) if n > 0 => {
                let b = app.buf();
                let p = b.point();
                b.push_mark(p, false);
                let n = (n - 1).min(b.char_count());
                b.set_offset(n);
            }
            _ => app.error("Please enter a buffer position"),
        },
        Purpose::InsertFile => {
            if input.is_empty() {
                return;
            }
            let path = expand_path(&input);
            let path = if path.is_absolute() {
                path
            } else {
                buffer_directory(app).join(path)
            };
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let b = app.buf();
                    let start = b.point();
                    b.push_mark(start, false);
                    b.insert_str(&text);
                    // point stays before the text and the mark is after.
                    let end = b.point();
                    b.push_mark(end, false);
                    b.set_point(start);
                }
                Err(e) => app.error(format!("Cannot read {}: {e}", path.display())),
            }
        }
        Purpose::FileOp(op) => file_op(app, op, &input),
        Purpose::Apropos { docs } => {
            if !input.is_empty() {
                help::apropos(app, &input, docs);
            }
        }
        Purpose::GotoLine => match input.parse::<usize>() {
            Ok(n) if n > 0 => {
                let b = app.buf();
                let p = b.point();
                b.push_mark(p, false);
                b.goto_line(n);
            }
            _ => app.error("Please enter a line number"),
        },
        Purpose::SetFillColumn { default } => {
            let new = if input.is_empty() {
                default
            } else {
                match input.parse::<usize>() {
                    Ok(n) if n > 0 => n,
                    _ => return app.error("Please enter a column number"),
                }
            };
            let old = app.buf_ref().fill_column;
            app.buf().fill_column = new;
            app.msg(format!("Fill column set to {new} (was {old})"));
        }
        Purpose::ExtendedCommand(arg) => {
            if !app.editor.commands.contains(&input) {
                return app.error(format!("[No match] {input}"));
            }
            app.forced_arg = arg;
            app.run_command(&input);
            if app.message.is_none()
                && app.minibuffer.is_none()
                && app.key_reader.is_none()
                && let Some(keys) = app.editor.active_keymaps().where_is(&input).first()
            {
                app.msg(format!("You can run the command '{input}' with {keys}"));
            }
        }
        Purpose::DescribeFunction => {
            if app.editor.commands.contains(&input) {
                help::describe_function(app, &input);
            } else if !input.is_empty() {
                app.error(format!("[No match] {input}"));
            }
        }
        Purpose::WhereIs => {
            if app.editor.commands.contains(&input) {
                let text = help::where_is_message(app, &input);
                app.msg(text);
            } else if !input.is_empty() {
                app.error(format!("[No match] {input}"));
            }
        }
        Purpose::SetKeyCommand { keys, local } => {
            if !app.editor.commands.contains(&input) {
                return app.error(format!("[No match] {input}"));
            }
            let scope = if local {
                Scope::Buffer(app.active_id())
            } else {
                Scope::Global
            };
            let shown = describe_key_sequence(&keys);
            match app
                .editor
                .keymaps
                .bind_seq(&scope, &keys, &input, Origin::User)
            {
                Ok(()) => {
                    let where_ = if local { "in this buffer" } else { "globally" };
                    app.msg(format!("{shown} now runs {input} {where_}"));
                }
                Err(e) => app.msg(e.to_string()),
            }
        }
        Purpose::ReplaceFrom(req) => {
            if input.is_empty() {
                match app.last_replace.clone() {
                    Some((from, to)) => query::start(app, from, to, &req),
                    None => app.error("Nothing to replace"),
                }
                return;
            }
            let what = req.name();
            app.prompt(
                format!("{what} {input} with: "),
                Purpose::ReplaceTo { from: input, req },
            );
        }
        // The replacement may be empty or have spaces at the ends: don't trim it.
        Purpose::ReplaceTo { from, req } => query::start(app, from, mb.input, &req),
        Purpose::StringRectangle => {
            if let Some(m) = app.buf_ref().mark() {
                let p = app.buf_ref().point();
                forge_core::rect::replace_each(app.buf(), m, p, &mb.input);
            }
        }
        Purpose::BookmarkName { default } => {
            let name = if input.is_empty() { default } else { input };
            crate::bookmarks::set(app, &name);
        }
        Purpose::PluginAsk {
            plugin,
            request,
            candidates,
            require_match,
            multiple,
            over,
        } => {
            // Like the other prompts, but the text isn't trimmed.
            let answer = mb.input;
            if multiple {
                // completing-read-multiple: split on commas/tabs (trimming
                // each field) and, with require-match, insist every field
                // is a candidate. The empty input is always allowed.
                let fields: Vec<String> = forge_core::completion::fields(
                    &answer,
                    &forge_core::completion::CRM_SEPARATORS,
                )
                .all()
                .into_iter()
                .map(str::to_string)
                .collect();
                let accepted = fields.is_empty()
                    || !require_match
                    || candidates
                        .as_ref()
                        .is_some_and(|c| fields.iter().all(|f| c.contains(f)));
                if !accepted {
                    app.minibuffer = Some(Minibuffer {
                        note: Some(" [No match]".to_string()),
                        input: answer,
                        prompt: mb.prompt,
                        secret: mb.secret,
                        purpose: Purpose::PluginAsk {
                            plugin,
                            request,
                            candidates,
                            require_match,
                            multiple,
                            over,
                        },
                        history_pos: 0,
                        typed: String::new(),
                        edit: None,
                        completions: None,
                        list: None,
                    });
                    return;
                }
                plugins::answer_list(app, &plugin, request, Some(&fields));
                return;
            }
            if require_match && !candidates.as_ref().is_some_and(|c| c.contains(&answer)) {
                let note = Some(" [No match]".to_string());
                app.minibuffer = Some(Minibuffer {
                    note,
                    input: answer,
                    prompt: mb.prompt,
                    secret: mb.secret,
                    purpose: Purpose::PluginAsk {
                        plugin,
                        request,
                        candidates,
                        require_match,
                        multiple,
                        over,
                    },
                    history_pos: 0,
                    typed: String::new(),
                    edit: None,
                    completions: None,
                    list: None,
                });
                return;
            }
            // An over question hands its answer back to the prompt it took
            // over: restore it with the answer as its input. An isearch
            // resumes with the answer as its search string.
            if over && let Some(mut prev) = app.suspended_prompt.take() {
                match &mut prev.purpose {
                    Purpose::Isearch(s) => {
                        s.query = answer.clone();
                        s.failing = false;
                        s.wrapped = false;
                        s.current = None;
                        let origin = s.origin;
                        isearch_find(app, s, origin);
                    }
                    _ => {
                        prev.input = answer.clone();
                        prev.history_pos = 0;
                        prev.typed = String::new();
                    }
                }
                app.minibuffer = Some(prev);
            }
            plugins::answer(app, &plugin, request, Some(&answer));
        }
        Purpose::LoadPlugin => {
            if !input.is_empty() {
                plugins::load(app, &expand_path(&input));
            }
        }
        Purpose::VcAsk { what, .. } => crate::vc::answered(app, what, mb.input),
        Purpose::CharAsk(ask) => crate::iso_transl::answered(app, ask, &mb.input),
        Purpose::InputMethodAsk { what, default } => {
            crate::input_method::answered(app, what, default, &mb.input)
        }
        // A one-key question has no text to submit: it stays up.
        Purpose::KeyChoice(_) => app.minibuffer = Some(mb),
        Purpose::UnloadPlugin => {
            if !input.is_empty() {
                plugins::unload(app, &input);
            }
        }
        Purpose::ReloadPlugin => {
            if !input.is_empty() {
                plugins::reload(app, &input);
            }
        }
        Purpose::RevokePluginPermissions => {
            if !input.is_empty() {
                plugins::revoke(app, &input);
            }
        }
        Purpose::EvalRhai => {
            if !input.is_empty() {
                plugins::eval_rhai(app, &mb.input);
            }
        }
        Purpose::Occur => {
            if !input.is_empty() {
                crate::builtins::occur(app, &mb.input);
            }
        }
        Purpose::OrgSparse => {
            if !input.is_empty() {
                crate::builtins::org_sparse_tree(app, &mb.input);
            }
        }
        Purpose::OrgOccur => {
            if !input.is_empty() {
                crate::builtins::org_occur(app, &mb.input);
            }
        }
        Purpose::OrgDrawer => {
            crate::builtins::org_insert_drawer(app, &mb.input);
        }
        Purpose::OrgAsk(ask) => crate::org_cmds::answered(app, ask, &mb.input),
        Purpose::Thing(action) => {
            let name = input.trim();
            let Some(thing) = forge_core::treesit::Thing::from_name(name) else {
                app.error(format!("Unknown thing: {name}"));
                return;
            };
            let offset = app.buf_ref().point_offset();
            match action {
                ThingAction::Beginning { arg } => match forge_core::treesit::beginning_of_thing(
                    app.buf(),
                    offset,
                    arg,
                    thing,
                    forge_core::treesit::Tactic::Nested,
                ) {
                    Some(to) => app.buf().set_offset(to),
                    None => app.error(format!("No {} to move to", thing.name())),
                },
                ThingAction::End { arg } => match forge_core::treesit::end_of_thing(
                    app.buf(),
                    offset,
                    arg,
                    thing,
                    forge_core::treesit::Tactic::Nested,
                ) {
                    Some(to) => app.buf().set_offset(to),
                    None => app.error(format!("No {} to move to", thing.name())),
                },
                ThingAction::Bounds => {
                    match forge_core::treesit::thing_at(app.buf(), offset, thing) {
                        Some(range) => app.msg(format!("{}..{}", range.start, range.end)),
                        None => app.error(format!("No {} at point", thing.name())),
                    }
                }
                ThingAction::Text => {
                    match forge_core::treesit::thing_at(app.buf(), offset, thing) {
                        Some(range) => {
                            let text = app.buf_ref().text_offsets(range.start, range.end);
                            app.msg(text);
                        }
                        None => app.error(format!("No {} at point", thing.name())),
                    }
                }
            }
        }
        Purpose::Choose { what, candidates } => match what {
            Choice::RecentFile => {
                if !input.is_empty() {
                    visit_file(app, &expand_path(&input), false);
                }
            }
            Choice::Imenu(places) => match candidates.iter().position(|c| *c == input) {
                Some(i) => {
                    let b = app.buf();
                    let p = b.point();
                    b.push_mark(p, false);
                    b.set_point(places[i]);
                }
                None => app.error(format!("No definition {input}")),
            },
            Choice::Org(ask) => crate::org_cmds::chosen(app, ask, &mb.input, &candidates),
            Choice::OrgGoto(places) => match candidates.iter().position(|c| *c == input) {
                Some(i) => {
                    let b = app.buf();
                    let p = b.point();
                    b.push_mark(p, false);
                    b.set_point(places[i]);
                    b.reveal(b.point_offset());
                }
                None => app.error(format!("No heading {input}")),
            },
            Choice::InstallPackage => {
                if candidates.contains(&input) {
                    crate::packages::install_named(app, &input);
                } else if !input.is_empty() {
                    app.error(format!("No package {input}"));
                }
            }
            Choice::UpgradeVcPackage => {
                if candidates.contains(&input) {
                    crate::package_vc::upgrade(app, &input);
                }
            }
            Choice::UpgradePackage => {
                if candidates.contains(&input) {
                    crate::packages::upgrade_named(app, &input);
                }
            }
            Choice::GrammarInstall => {
                if candidates.contains(&input) {
                    crate::grammar_install::install_named(app, &input);
                } else if !input.is_empty() {
                    app.error(format!("No grammar recipe for {input}"));
                }
            }
            Choice::DescribePlugin => {
                if !input.is_empty() {
                    crate::plugins::describe(app, &input);
                }
            }
            Choice::DeletePackage => {
                if candidates.contains(&input) {
                    crate::packages::delete(app, &input);
                } else if !input.is_empty() {
                    app.error(format!("{input} isn't installed"));
                }
            }
            Choice::ProjectFile(root) => {
                if !input.is_empty() {
                    crate::compile::visit_project_file(app, &root, &candidates, &input);
                }
            }
            Choice::BookmarkJump => {
                if !input.is_empty() {
                    crate::bookmarks::jump(app, &input);
                }
            }
            Choice::BookmarkDelete => {
                if !input.is_empty() {
                    crate::bookmarks::delete(app, &input);
                }
            }
            Choice::DescribeSymbol { default } => match (input.is_empty(), default) {
                (true, Some(name)) if !name.is_empty() => crate::help::describe_symbol(app, &name),
                (true, _) => app.error("No symbol here"),
                (false, _) => crate::help::describe_symbol(app, &input),
            },
            Choice::KillEntry => {
                // Candidates are "N: first line…"; the number is what
                // makes them unique when kills repeat.
                let index = input
                    .split(':')
                    .next()
                    .and_then(|n| n.parse::<usize>().ok());
                match index {
                    Some(n) => crate::commands::insert_kill(app, n.saturating_sub(1)),
                    None => app.error(format!("No kill {input}")),
                }
            }
            Choice::SelectFrame => {
                if !input.is_empty() {
                    crate::frames::select_by_name(app, &input);
                }
            }
            Choice::SelectTab => {
                if !input.is_empty() {
                    crate::tabs::select_by_name(app, &input);
                }
            }
            Choice::GotoNode => crate::info::goto(app, &input),
            Choice::InfoMenu => crate::info::menu(app, &input),
            Choice::InfoSearch => crate::info::search(app, &input),
            Choice::InfoIndex => crate::info::index(app, &input),
            Choice::InfoLookup => crate::info::lookup(app, &input),
            Choice::LoadTheme => match app.editor.load_theme(&input) {
                Ok(()) => app.msg(format!("Loaded theme: {input}")),
                Err(e) => app.error(e),
            },
        },
        p @ (Purpose::Compile
        | Purpose::ProjectGrep
        | Purpose::ShellCommand { .. }
        | Purpose::AsyncShellCommand) => crate::compile::answered(app, &p, &input),
        p @ (Purpose::Grep | Purpose::GrepFind | Purpose::GrepSearch { .. }) => {
            crate::grep::answered(app, &p, &input);
        }
        Purpose::Dired(action) => crate::dired::answered(app, action, &input),
        Purpose::PackageVc => {
            if !input.trim().is_empty() {
                crate::package_vc::install_spec(app, &input);
            }
        }
        Purpose::PackageFilter => crate::packages::filter_manager(app, &input),
        Purpose::PackageSnapshot(action) => {
            crate::packages::answer_snapshot_prompt(app, action, &input)
        }
        Purpose::ImageAsk(ask) => crate::image_mode::answer(app, ask, &input),
        Purpose::SetFrameName { default } => {
            let name = if input.is_empty() { default } else { input };
            crate::frames::rename_selected(app, &name);
        }
        Purpose::RenameTab { default } => {
            let _ = default;
            crate::tabs::rename_selected(app, &input);
        }
        Purpose::MakeFrameOnDisplay { monitor } => {
            crate::frames::make_frame_on_display(app, monitor, &input);
        }
        // corfu-move-to-minibuffer: put the picked candidate where the
        // word was, then complete it as the popup would have.
        Purpose::Transfer {
            buffer,
            start,
            len,
            version,
            ..
        } => {
            if input.is_empty() {
                return;
            }
            if let Some(b) = app.editor.state.get_buffer_mut(buffer) {
                let s = b.transform_offset(start, version);
                let e = b.transform_offset(start + len, version);
                b.replace_offsets(s, e, &input);
                b.set_offset(s + input.chars().count());
            }
            crate::capf::commit(app, &input, "exact");
        }
        Purpose::Confirm(_)
        | Purpose::Isearch(_)
        | Purpose::Register(_)
        | Purpose::PluginYOrN { .. }
        | Purpose::PluginGrant => {}
    }
}

// ------------------------------------------------------------ completion

fn complete(app: &mut App, mb: &mut Minibuffer) {
    refresh(app, mb);
    let Some(list) = mb.list.as_ref() else { return };
    let input = mb.input.clone();
    let mut candidates: Vec<String> = (0..list.matches.len())
        .filter_map(|i| list.full(i))
        .collect();
    // Files: skip backups and build leftovers while anything else fits.
    if list.category == "file" {
        let kept: Vec<String> = candidates
            .iter()
            .filter(|c| !ignored_extension(c))
            .cloned()
            .collect();
        if !kept.is_empty() {
            candidates = kept;
        }
    }

    match candidates.len() {
        0 => mb.note = Some(" [No match]".into()),
        1 => {
            hide_completions(app, mb);
            mb.input = candidates[0].clone();
            if !mb.input.ends_with('/') {
                mb.note = Some(" [Sole completion]".into());
            }
        }
        _ => {
            let common = common_prefix(&candidates);
            if common.chars().count() > input.chars().count() {
                hide_completions(app, mb);
                mb.input = common;
            } else {
                if candidates.contains(&input) {
                    mb.note = Some(" [Complete, but not unique]".into());
                }
                // No progress to make: show what the choices are, in
                // *Completions*.
                let shown: Vec<String> = mb
                    .list
                    .as_ref()
                    .map(|l| l.matches.iter().map(|&k| l.all[k].clone()).collect())
                    .unwrap_or_default();
                show_completions(app, mb, &shown);
            }
        }
    }
}

/// What a prompt completes from: (cache key, category, candidates, the
/// part of the input they're matched against, prefix put back in front).
fn source(
    app: &App,
    mb: &Minibuffer,
) -> Option<(String, &'static str, Vec<String>, String, String)> {
    let input = mb.input.clone();
    let whole = |key: &str, cat: &'static str, list: Vec<String>| {
        Some((key.to_string(), cat, list, input.clone(), String::new()))
    };
    match &mb.purpose {
        Purpose::ExtendedCommand(_)
        | Purpose::DescribeFunction
        | Purpose::WhereIs
        | Purpose::SetKeyCommand { .. } => {
            whole("command", "command", commands::visible_names(app))
        }
        Purpose::SwitchBuffer { .. } | Purpose::KillBuffer { .. } => {
            // Most recent first would be nicer; other buffers before this one.
            let active = app.active_id();
            let mut ids = app.editor.buffer_ids();
            ids.sort_by_key(|id| *id == active);
            let names = ids
                .into_iter()
                .filter_map(|id| app.editor.state.get_buffer(id))
                .map(|b| b.name().to_string())
                .collect();
            whole("buffer", "buffer", names)
        }
        Purpose::FindFile { .. }
        | Purpose::WriteFile
        | Purpose::InsertFile
        | Purpose::FileOp(
            FileOp::MakeDirectory
            | FileOp::MakeEmptyFile
            | FileOp::CopyFrom
            | FileOp::CopyTo(_)
            | FileOp::DeleteDirectory
            | FileOp::InsertDirectory,
        )
        | Purpose::LoadPlugin
        | Purpose::Dired(_) => {
            let (dir_part, name) = match input.rfind('/') {
                Some(i) => (&input[..=i], &input[i + 1..]),
                None => ("", input.as_str()),
            };
            let is_find = matches!(&mb.purpose, Purpose::FindFile { .. });
            if is_find && input == "~" {
                return Some((
                    "file:~".to_string(),
                    "file",
                    vec!["~/".to_string()],
                    "~".to_string(),
                    String::new(),
                ));
            }
            let buf_dir = buffer_directory(app);
            let base = is_find.then_some(buf_dir.as_path());
            // partial-completion on the directories: `/u/l/b` for
            // `/usr/local/bin/`. One expansion replaces the typed part;
            // several are listed together by their whole path.
            let mut dir_part = dir_part.to_string();
            let mut spread: Option<Vec<String>> = None;
            if !dir_part.is_empty() && !dir_exists(&dir_part, base) {
                let mut exp = partial_dirs(&dir_part, base);
                match exp.len() {
                    0 => {}
                    1 => dir_part = exp.remove(0),
                    _ => spread = Some(exp),
                }
            }
            if let Some(dirs) = spread {
                // Filtered here on the last part of each name: the list
                // is rebuilt on every key, so nothing is left to match.
                use forge_core::completion::Style;
                let mut all = Vec::new();
                for d in &dirs {
                    all.extend(file_candidates_in(d, base).into_iter().filter(|c| {
                        let last = c.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                        name.is_empty()
                            || Style::Prefix.matches(name, last)
                            || Style::Partial.matches(name, last)
                            || Style::Orderless.matches(name, last)
                    }));
                }
                return Some((
                    format!("file:{input}"),
                    "file",
                    all,
                    String::new(),
                    String::new(),
                ));
            }
            let files = file_candidates_in(&dir_part, base);
            let mut names: Vec<String> = files
                .into_iter()
                .map(|f| f[dir_part.len()..].to_string())
                .collect();
            // Like ido's ".": pick it to open the directory itself in Dired.
            // And "../", which Forge lists in every directory.
            let opens_dir = matches!(&mb.purpose, Purpose::FindFile { .. } | Purpose::Dired(_));
            if !dir_part.is_empty() || !input.is_empty() || opens_dir {
                if !names.iter().any(|n| n == "../") {
                    names.insert(0, "../".to_string());
                }
                if opens_dir && !names.iter().any(|n| n == "./") {
                    names.insert(0, "./".to_string());
                }
            }
            let dir_part = dir_part.as_str();
            // The directory stays in the prompt input, traditional; the
            // candidates are short names with file metadata annotations.
            Some((
                format!("file:{dir_part}"),
                "file",
                names,
                name.to_string(),
                dir_part.to_string(),
            ))
        }
        Purpose::VcAsk { candidates, .. } if !candidates.is_empty() => {
            whole("vc", "vc", candidates.clone())
        }
        Purpose::InputMethodAsk { what, .. } => {
            let key = format!("im:{}", crate::input_method::list_key(what));
            let all = if mb.list.as_ref().is_some_and(|l| l.key == key) {
                Vec::new()
            } else {
                crate::input_method::candidates()
            };
            whole(&key, "input-method", all)
        }
        Purpose::CharAsk(ask) => {
            // Some lists are big (every Unicode name): built once per
            // prompt, not for every key.
            let key = format!("char:{}", crate::iso_transl::list_key(ask));
            let all = if mb.list.as_ref().is_some_and(|l| l.key == key) {
                Vec::new()
            } else {
                crate::iso_transl::candidates(app, ask)
            };
            whole(&key, crate::iso_transl::category(ask), all)
        }
        Purpose::UnloadPlugin => whole("unload", "plugin", plugins::loaded_names(app)),
        Purpose::ReloadPlugin => whole("reload", "plugin", plugins::reloadable_names(app)),
        Purpose::RevokePluginPermissions => whole("revoke", "plugin", plugins::granted_names(app)),
        Purpose::PluginAsk {
            candidates: Some(c),
            plugin,
            multiple: true,
            ..
        } => {
            let _ = plugin;
            // completing-read-multiple: match only the field being typed,
            // keeping the fields already typed in front (so the list
            // rebuilds when that part changes, like the file directory).
            let f = forge_core::completion::fields(&input, &forge_core::completion::CRM_SEPARATORS);
            Some((
                format!("question:{}", f.prefix),
                "candidate",
                c.clone(),
                f.field.to_string(),
                f.prefix.to_string(),
            ))
        }
        Purpose::PluginAsk {
            candidates: Some(c),
            plugin,
            ..
        } => {
            let _ = plugin;
            whole("question", "candidate", c.clone())
        }
        Purpose::Choose { what, candidates } => {
            let cat = match what {
                Choice::RecentFile => "file",
                Choice::Imenu(_) | Choice::OrgGoto(_) => "symbol",
                Choice::Org(_) => "org",
                Choice::ProjectFile(_) => "project-file",
                Choice::BookmarkJump | Choice::BookmarkDelete => "bookmark",
                Choice::DescribeSymbol { .. } => "symbol",
                Choice::KillEntry => "kill",
                Choice::InstallPackage
                | Choice::DeletePackage
                | Choice::UpgradePackage
                | Choice::UpgradeVcPackage => "package",
                Choice::GrammarInstall => "grammar",
                Choice::DescribePlugin => "plugin",
                Choice::SelectFrame => "frame",
                Choice::SelectTab => "tab",
                Choice::GotoNode | Choice::InfoMenu | Choice::InfoSearch | Choice::InfoIndex => {
                    "manual"
                }
                Choice::InfoLookup => "command",
                Choice::LoadTheme => "theme",
            };
            whole("choose", cat, candidates.clone())
        }
        Purpose::Thing(_) => whole(
            "thing",
            "thing",
            forge_core::treesit::Thing::ALL
                .iter()
                .map(|t| t.name().to_string())
                .collect(),
        ),
        Purpose::Transfer { candidates, .. } => whole("transfer", "point", candidates.clone()),
        _ => None,
    }
}

/// Bring the prompt's candidate list up to date with the input: rebuilt
/// (and annotated, if a plugin annotates the category) when the source
/// changed, re-filtered when the input did.
pub fn refresh(app: &mut App, mb: &mut Minibuffer) {
    let Some((key, category, all, needle, prefix)) = source(app, mb) else {
        mb.list = None;
        return;
    };
    let rebuild = mb.list.as_ref().is_none_or(|l| l.key != key);
    if rebuild {
        let mut notes = Vec::new();
        if category == "file" {
            // File metadata comes from the host (a plugin can't stat the
            // files): permissions, size and age, Forge marginalia-style.
            let buf_dir = buffer_directory(app);
            notes = file_notes(&all, &prefix, &mb.purpose, &buf_dir);
        } else if category == "input-method"
            && !app.editor.completion.annotators.contains_key(category)
        {
            notes = crate::input_method::notes(&all);
        } else if matches!(category, "unicode-name" | "emoji")
            && !app.editor.completion.annotators.contains_key(category)
        {
            notes = crate::iso_transl::notes(category, &all);
        } else if app.editor.completion.annotators.contains_key(category)
            && let Some(mut h) = app.plugin_host.take()
        {
            let (n, out) = h.annotate(&mut app.editor, category, &all);
            app.plugin_host = Some(h);
            notes = n;
            plugins::apply(app, out);
        } else if let Purpose::Transfer { notes: carried, .. } = &mb.purpose {
            // The popup's annotations travel with the moved completion.
            notes = carried.clone();
        }
        mb.list = Some(CandidateList {
            key,
            category,
            all,
            notes,
            prefix,
            matches: Vec::new(),
            selected: 0,
            input: "\u{0}".into(),
        });
    }
    let styles = app.editor.completion.styles().to_vec();
    let sorting = app.editor.completion.sorting().to_vec();
    // Vertico sorts by how often (and how recently) you picked something,
    // so the history of this prompt's category feeds the sort.
    let hist: Vec<String> = app.editor.session.history(category).to_vec();
    let rank = |c: &str| hist.iter().position(|h| h == c);
    let list = mb.list.as_mut().expect("just made");
    if list.input != needle {
        list.matches =
            forge_core::completion::filter_with(&styles, &sorting, &needle, &list.all, &rank)
                .into_iter()
                // Each match is a reference into `all`: its index is
                // where it sits (a search per match would be quadratic,
                // and insert-char lists every Unicode name).
                .map(|c| {
                    let base = list.all.as_ptr() as usize;
                    let i = (c as *const String as usize - base) / std::mem::size_of::<String>();
                    debug_assert!(std::ptr::eq(&list.all[i], c));
                    i
                })
                .collect();
        list.selected = 0;
        list.input = needle;
    }
}

/// The vertical list under the prompt, if vertical completion is on:
/// rows of (candidate, annotation, selected), and "N/M" counts.
/// The candidate under the cursor in the vertical list, for a plugin
/// that wants to describe it. None when the list isn't showing.
pub fn selected(app: &App) -> Option<forge_core::completion::Selection> {
    let mb = app
        .minibuffer
        .as_ref()
        .filter(|mb| mb.cursor_in_echo_area())?;
    let list = mb.list.as_ref().filter(|l| !l.matches.is_empty())?;
    let &k = list.matches.get(list.selected)?;
    Some(forge_core::completion::Selection {
        text: list.all[k].clone(),
        annotation: list.notes.get(k).cloned().filter(|n| !n.is_empty()),
        category: list.category.to_string(),
        index: list.selected as u32 + 1,
        total: list.matches.len() as u32,
        documentation: None,
        location: None,
    })
}

pub fn vertical_view(app: &mut App) -> Option<(Vec<Row>, String)> {
    let lines = app.minibuffer.as_ref().and_then(|mb| list_lines(app, mb))? as usize;
    let mut mb = app.minibuffer.take()?;
    if !mb.cursor_in_echo_area() {
        app.minibuffer = Some(mb);
        return None;
    }
    refresh(app, &mut mb);
    let view = mb.list.as_ref().map(|l| {
        let count = if l.matches.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", l.selected + 1, l.matches.len())
        };
        (l.rows(lines), count)
    });
    app.minibuffer = Some(mb);
    view
}

fn file_candidates_in(input: &str, default_dir: Option<&std::path::Path>) -> Vec<String> {
    if let Some(remote) = crate::tether::complete(input) {
        return remote;
    }
    if input == "~" {
        return vec!["~/".to_string()];
    }
    let (dir_part, prefix) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    };
    let entered = expand_path(dir_part);
    let dir = if dir_part.is_empty() {
        default_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else if entered.is_absolute() {
        entered
    } else if let Some(base) = default_dir {
        base.join(entered)
    } else {
        entered
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) {
                return None;
            }
            let slash = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{dir_part}{name}{slash}"))
        })
        .collect();
    out.sort();
    out
}

/// Does the directory part of a file prompt name a directory?
fn dir_exists(dir_part: &str, base: Option<&Path>) -> bool {
    let p = expand_path(dir_part);
    match base {
        Some(b) if !p.is_absolute() => b.join(p).is_dir(),
        _ => p.is_dir(),
    }
}

/// `completion-ignored-extensions`: names TAB skips while something else
/// matches.
const IGNORED_EXTENSIONS: &[&str] = &[
    "~", ".o", ".so", ".a", ".bin", ".lbin", ".ln", ".blg", ".bbl", ".elc", ".lof", ".glo", ".idx",
    ".lot", ".pyc", ".pyo", ".class", ".fasl", ".swp", ".orig", ".rej", ".svn/", ".hg/", ".git/",
    ".bzr/", "CVS/", ".dll", ".exe",
];

fn ignored_extension(name: &str) -> bool {
    IGNORED_EXTENSIONS.iter().any(|e| name.ends_with(e))
}

/// The directories a partial path stands for: each component is taken as
/// is when it exists, else as the start of a word in each name that fits
/// (`/u/l/` is `/usr/local/` and `/usr/lib/`).
fn partial_dirs(dir_part: &str, base: Option<&Path>) -> Vec<String> {
    let typed = shadowed(dir_part);
    let (mut cur, rest): (Vec<String>, &str) = if let Some(r) = typed.strip_prefix('/') {
        (vec!["/".to_string()], r)
    } else if let Some(r) = typed.strip_prefix("~/") {
        (vec!["~/".to_string()], r)
    } else {
        (vec![String::new()], typed)
    };
    for comp in rest.split('/').filter(|c| !c.is_empty()) {
        let mut next = Vec::new();
        for dir in &cur {
            if comp == "." || comp == ".." {
                next.push(format!("{dir}{comp}/"));
                continue;
            }
            let real = expand_path(dir);
            let real = match base {
                Some(b) if !real.is_absolute() => b.join(real),
                _ => real,
            };
            if real.join(comp).is_dir() {
                next.push(format!("{dir}{comp}/"));
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&real) else {
                continue;
            };
            for e in entries.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir()
                    && (forge_core::completion::Style::Partial.matches(comp, &name)
                        || forge_core::completion::Style::Prefix.matches(comp, &name))
                {
                    next.push(format!("{dir}{name}/"));
                }
            }
            if next.len() > 50 {
                next.truncate(50);
            }
        }
        if next.is_empty() {
            return next;
        }
        next.sort();
        next.dedup();
        cur = next;
    }
    cur
}

/// A GUI started from the Dock or Finder has `/` as its directory. Forge
/// then starts in the home directory, and so does Forge.
pub fn start_in_home_from_gui() {
    let rootless = std::env::current_dir().map_or(true, |d| d == Path::new("/"));
    if rootless && let Some(home) = std::env::var_os("HOME").filter(|h| Path::new(h).is_dir()) {
        let _ = std::env::set_current_dir(home);
    }
}

fn buffer_directory(app: &App) -> PathBuf {
    // A Tether mirror: relative names mean the remote directory.
    if let Some(dir) = app
        .buf_ref()
        .path()
        .and_then(|p| p.parent())
        .and_then(crate::tether::from_mirror)
    {
        return PathBuf::from(dir.display());
    }
    app.buf_ref()
        .path()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `/Users/ada/x` -> `~/x`, for the find-file prompt's starting input.
pub(crate) fn tilde_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = home.to_string_lossy().into_owned();
        if s == home {
            return "~".to_string();
        }
        if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    for home in ["/home/", "/Users/"] {
        if let Some(rest) = s.strip_prefix(home)
            && let Some(i) = rest.find('/')
        {
            return format!("~{}", &rest[i..]);
        }
    }
    match s.strip_prefix("/root") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => s,
    }
}

/// What C-x C-f starts with: the buffer's directory, traditional, with
/// `~` for home and a trailing slash.
pub(crate) fn initial_find_file_input(app: &App) -> String {
    let dir = buffer_directory(app);
    if dir.as_os_str() == "." {
        return String::new();
    }
    let mut s = tilde_path(&dir);
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Detailed sizes for file annotations: `96`, `103k`.
fn human_size(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    const UNITS: [&str; 5] = ["k", "M", "G", "T", "P"];
    let (mut v, mut i) = (n as f64 / 1000.0, 0);
    while v >= 999.95 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v < 9.95 {
        format!("{v:.1}{}", UNITS[i])
    } else {
        format!("{v:.0}{}", UNITS[i])
    }
}

#[cfg(unix)]
fn file_mode_string(m: &std::fs::Metadata) -> String {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let ft = m.file_type();
    let kind = if ft.is_dir() {
        'd'
    } else if ft.is_symlink() {
        'l'
    } else if ft.is_block_device() {
        'b'
    } else if ft.is_char_device() {
        'c'
    } else if ft.is_fifo() {
        'p'
    } else if ft.is_socket() {
        's'
    } else {
        '-'
    };
    let mode = m.permissions().mode();
    let bit = |b: u32, c: char| if mode & b != 0 { c } else { '-' };
    [
        kind,
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        bit(0o100, 'x'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        bit(0o010, 'x'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        bit(0o001, 'x'),
    ]
    .iter()
    .collect()
}

#[cfg(not(unix))]
fn file_mode_string(m: &std::fs::Metadata) -> String {
    let kind = if m.is_dir() {
        'd'
    } else if m.file_type().is_symlink() {
        'l'
    } else {
        '-'
    };
    let w = if m.permissions().readonly() { '-' } else { 'w' };
    format!("{kind}r{w}xr{w}xr{w}x")
}

/// `13h 8m ago`, `1d 6h ago`: marginalia-style age for file annotations.
fn relative_age(mtime: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let secs = now
        .duration_since(mtime)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        .max(0) as u64;
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h {}m ago", hours, mins % 60);
    }
    format!("{}d {}h ago", hours / 24, hours % 24)
}

/// Marginalia-style annotations for file candidates: permissions, size
/// and age (`drwxr-xr-x 96 13h 8m ago`), resolved against the prompt's
/// directory. The host provides these; a plugin can't stat the files.
fn file_notes(all: &[String], prefix: &str, purpose: &Purpose, buf_dir: &Path) -> Vec<String> {
    let base: PathBuf = match purpose {
        Purpose::FindFile { .. } => {
            let entered = expand_path(prefix);
            if prefix.is_empty() {
                buf_dir.to_path_buf()
            } else if entered.is_absolute() {
                entered
            } else {
                buf_dir.join(entered)
            }
        }
        _ => {
            let entered = expand_path(prefix);
            if prefix.is_empty() {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            } else if entered.is_absolute() {
                entered
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(entered)
            }
        }
    };
    let now = std::time::SystemTime::now();
    all.iter()
        .map(|name| {
            if name == "./" {
                return String::new();
            }
            let path = base.join(name.trim_end_matches('/'));
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                return String::new();
            };
            let mode = file_mode_string(&meta);
            let size = human_size(meta.len());
            let age = meta
                .modified()
                .map(|t| relative_age(t, now))
                .unwrap_or_default();
            match age.is_empty() {
                true => format!("{mode} {size:>5}"),
                false => format!("{mode} {size:>5}  {age}"),
            }
        })
        .collect()
}

fn common_prefix(items: &[String]) -> String {
    let mut prefix: Vec<char> = items[0].chars().collect();
    for item in &items[1..] {
        let n = prefix
            .iter()
            .zip(item.chars())
            .take_while(|(a, b)| **a == *b)
            .count();
        prefix.truncate(n);
    }
    prefix.into_iter().collect()
}

/// `file-name-shadow-mode`: in `/a/b//c` and `/a/b/~/c` everything before
/// the last `//` or `/~/` is ignored, so the rest names the file.
pub(crate) fn shadowed(input: &str) -> &str {
    let mut start = 0;
    if let Some(i) = input.rfind("//") {
        start = start.max(i + 1);
    }
    if let Some(i) = input.rfind("/~/") {
        start = start.max(i + 1);
    }
    &input[start..]
}

pub(crate) fn expand_path(input: &str) -> PathBuf {
    let input = shadowed(input);
    if (input == "~" || input.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(input.trim_start_matches('~').trim_start_matches('/'));
    }
    #[cfg(windows)]
    {
        let input = input
            .strip_prefix("/\\\\?\\")
            .or_else(|| {
                let bytes = input.as_bytes();
                (bytes.len() >= 3
                    && bytes[0] == b'/'
                    && bytes[1].is_ascii_alphabetic()
                    && bytes[2] == b':')
                    .then(|| &input[1..])
            })
            .unwrap_or(input);
        PathBuf::from(input.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    PathBuf::from(input)
}

// ------------------------------------------------------ incremental search

pub fn start_isearch(app: &mut App, forward: bool) {
    let origin = app.buf_ref().point();
    let state = Isearch {
        forward,
        origin,
        query: String::new(),
        failing: false,
        wrapped: false,
        current: None,
    };
    app.minibuffer = Some(Minibuffer::new("", Purpose::Isearch(state)));
}

/// Returns true to keep searching.
fn isearch_key(app: &mut App, s: &mut Isearch, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('s') if ctrl && !alt => {
            isearch_repeat(app, s, true);
            true
        }
        KeyCode::Char('r') if ctrl && !alt => {
            isearch_repeat(app, s, false);
            true
        }
        KeyCode::Char('g') if ctrl => {
            // Abort: go back to where the search started.
            app.buf().set_point(s.origin);
            app.highlight = None;
            app.msg("Quit");
            false
        }
        KeyCode::Enter => {
            isearch_done(app, s);
            false
        }
        KeyCode::Char('%') if alt && !s.query.is_empty() => {
            // M-% during a search: query-replace the search string from the match.
            let from = s.query.clone();
            if let Some((a, _)) = s.current {
                app.buf().set_point(a);
            }
            isearch_done(app, s);
            let req = ReplaceRequest {
                regexp: false,
                delimited: false,
                query: true,
                region: None,
            };
            app.prompt(
                format!("Query replace {from} with: "),
                Purpose::ReplaceTo { from, req },
            );
            false
        }
        KeyCode::Backspace => {
            s.query.pop();
            s.current = None;
            s.wrapped = false;
            isearch_find(app, s, s.origin);
            true
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            s.query.push(c);
            let from = match s.current {
                Some((a, b)) => {
                    if s.forward {
                        a
                    } else {
                        b
                    }
                }
                None => s.origin,
            };
            isearch_find(app, s, from);
            true
        }
        _ => {
            // Any other key ends the search and then does its normal job.
            isearch_done(app, s);
            app.dispatch_key(key);
            false
        }
    }
}

fn isearch_repeat(app: &mut App, s: &mut Isearch, forward: bool) {
    if s.query.is_empty() {
        // C-s C-s reuses the previous search string.
        s.forward = forward;
        if app.last_isearch.is_empty() {
            return;
        }
        s.query = app.last_isearch.clone();
        isearch_find(app, s, s.origin);
        return;
    }
    let from = if s.forward != forward {
        s.forward = forward;
        app.buf_ref().point()
    } else if s.failing {
        s.wrapped = true;
        if forward {
            app.buf_ref().start()
        } else {
            app.buf_ref().end()
        }
    } else {
        app.buf_ref().point()
    };
    isearch_find(app, s, from);
}

fn isearch_find(app: &mut App, s: &mut Isearch, from: Pos) {
    if s.query.is_empty() {
        s.failing = false;
        s.current = None;
        app.highlight = None;
        app.buf().set_point(s.origin);
        return;
    }
    let found = if s.forward {
        app.buf_ref().search_forward(&s.query, from)
    } else {
        app.buf_ref().search_backward(&s.query, from)
    };
    match found {
        Some((a, b)) => {
            s.failing = false;
            s.current = Some((a, b));
            app.highlight = Some((a, b));
            app.buf().set_point(if s.forward { b } else { a });
        }
        None => {
            s.failing = true;
            app.bell = true;
        }
    }
}

fn isearch_done(app: &mut App, s: &Isearch) {
    app.highlight = None;
    if !s.query.is_empty() {
        app.last_isearch = s.query.clone();
        // Remember a search that found something, for consult-isearch-history
        // (M-r during isearch) and C-s C-s in a later session.
        if !s.failing {
            app.editor.session.add_history("search", &s.query);
            if app.savehist {
                let _ = app.editor.session.save();
            }
        }
    }
    if app.buf_ref().point() != s.origin {
        app.buf().push_mark(s.origin, false);
        app.msg("Mark saved where search started");
    }
}

// -------------------------------------------------- prompt key commands

/// Take the prompt out and hand it back, so a command can work on it. The
/// prompt goes back before the command runs, which is what lets a command
/// open another one.
fn with_prompt(app: &mut App, f: impl FnOnce(&mut App, Minibuffer)) -> bool {
    let Some(mb) = app.minibuffer.take() else {
        return false;
    };
    f(app, mb);
    true
}

/// C-g and ESC: give up on the prompt. A plugin that asked something is
/// told the answer is cancelled, rather than left waiting.
pub fn cmd_abort(app: &mut App) {
    let Some(mb) = app.minibuffer.take() else {
        return;
    };
    match mb.purpose {
        Purpose::PluginGrant => {
            app.msg("Quit");
            plugins::answer_grant(app, None);
        }
        Purpose::PluginYOrN { plugin, request } => {
            app.msg("Quit");
            plugins::answer(app, &plugin, request, None);
        }
        _ => cancel(app, mb),
    }
}

/// RET and C-j: answer the prompt.
pub fn cmd_exit(app: &mut App) {
    with_prompt(app, submit);
}

/// M-RET: answer with the text as typed, ignoring the candidate under the
/// cursor.
pub fn cmd_exit_with_input(app: &mut App) {
    with_prompt(app, submit);
}

/// TAB: complete against the candidates.
pub fn cmd_complete(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        complete(app, &mut mb);
        app.minibuffer = Some(mb);
    });
}

/// TAB while the vertical list is up: put the candidate under the cursor
/// into the input, without leaving the prompt.
pub fn cmd_insert_candidate(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        refresh(app, &mut mb);
        if let Some(c) = mb.list.as_ref().and_then(|l| l.selected_full()) {
            mb.input = c;
        }
        app.minibuffer = Some(mb);
    });
}

pub fn cmd_delete_backward(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        let p = mb.pos();
        if p > 0 {
            mb.remove(p - 1, p);
        }
        app.minibuffer = Some(mb);
    });
}

/// The editing keys of a prompt, as minibuffer has them. `what`
/// names the command without its `minibuffer-` prefix.
pub fn cmd_edit(app: &mut App, what: &str) {
    with_prompt(app, |app, mut mb| {
        let (p, len) = (mb.pos(), mb.len());
        match what {
            "beginning-of-line" => mb.set_pos(0),
            "end-of-line" => mb.set_pos(len),
            "forward-char" => mb.set_pos((p + 1).min(len)),
            "backward-char" => mb.set_pos(p.saturating_sub(1)),
            "forward-word" => {
                let to = mb.word_forward(p);
                mb.set_pos(to);
            }
            "backward-word" => {
                let to = mb.word_backward(p);
                mb.set_pos(to);
            }
            "delete-char" => {
                if p < len {
                    mb.remove(p, p + 1);
                }
            }
            "kill-word" => {
                let to = mb.word_forward(p);
                let gone = mb.remove(p, to);
                app.kill_ring_save(gone);
            }
            "backward-kill-word" => {
                let from = mb.word_backward(p);
                let gone = mb.remove(from, p);
                app.kill_ring_save(gone);
            }
            "kill-line" => {
                let gone = mb.remove(p, len);
                app.kill_ring_save(gone);
            }
            "kill-whole-line" => {
                let gone = mb.remove(0, len);
                app.kill_ring_save(gone);
            }
            "yank" => {
                if let Some(text) = app.editor.kill_ring.current().map(str::to_string) {
                    mb.insert(&text.replace('\n', " "));
                }
            }
            "transpose-chars" => {
                // At the end, swap the last two; else the one before the
                // cursor with the one under it, and move past both.
                let at = if p >= len { len.saturating_sub(1) } else { p };
                if at >= 1 && len >= 2 {
                    let mut chars: Vec<char> = mb.input.chars().collect();
                    chars.swap(at - 1, at);
                    mb.input = chars.into_iter().collect();
                    mb.set_pos(at + 1);
                }
            }
            "upcase-word" | "downcase-word" | "capitalize-word" => {
                let to = mb.word_forward(p);
                let word: String = mb.input.chars().skip(p).take(to - p).collect();
                let changed = match what {
                    "upcase-word" => word.to_uppercase(),
                    "downcase-word" => word.to_lowercase(),
                    _ => {
                        let mut seen = false;
                        word.chars()
                            .map(|c| {
                                if c.is_alphanumeric() && !seen {
                                    seen = true;
                                    c.to_uppercase().next().unwrap_or(c)
                                } else {
                                    c.to_lowercase().next().unwrap_or(c)
                                }
                            })
                            .collect()
                    }
                };
                mb.remove(p, to);
                mb.insert(&changed);
            }
            _ => {}
        }
        app.minibuffer = Some(mb);
    });
}

pub fn cmd_history_prev(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        history_step(app, &mut mb, 1);
        app.minibuffer = Some(mb);
    });
}

pub fn cmd_history_next(app: &mut App) {
    with_prompt(app, |app, mut mb| {
        history_step(app, &mut mb, -1);
        app.minibuffer = Some(mb);
    });
}

/// Move the highlight in the vertical list, without leaving the prompt.
/// `wrap` sends it around the ends (vertico-cycle) instead of stopping.
fn list_step(app: &mut App, by: isize, wrap: bool) {
    with_prompt(app, |app, mut mb| {
        refresh(app, &mut mb);
        let n = mb.list.as_ref().map_or(0, |l| l.matches.len());
        if n > 0
            && let Some(list) = mb.list.as_mut()
        {
            let to = list.selected as isize + by;
            list.selected = if wrap {
                to.rem_euclid(n as isize) as usize
            } else {
                to.clamp(0, n as isize - 1) as usize
            };
        }
        app.minibuffer = Some(mb);
    });
}

/// Move the highlight by `by` candidates, wrapping when vertico-cycle is
/// on (vertico-next / -previous, and completion.next at a prompt).
pub fn cmd_list_step(app: &mut App, by: isize) {
    let wrap = app.editor.completion.list_cycle();
    list_step(app, by, wrap);
}

pub fn cmd_list_next(app: &mut App) {
    cmd_list_step(app, 1);
}

pub fn cmd_list_prev(app: &mut App) {
    cmd_list_step(app, -1);
}

pub fn cmd_list_next_page(app: &mut App) {
    list_step(app, 10, false);
}

pub fn cmd_list_prev_page(app: &mut App) {
    list_step(app, -10, false);
}

/// Highlight candidate number `pick(n)` (n = how many match).
fn list_select(app: &mut App, pick: impl Fn(usize) -> usize) {
    with_prompt(app, |app, mut mb| {
        refresh(app, &mut mb);
        let n = mb.list.as_ref().map_or(0, |l| l.matches.len());
        if n > 0
            && let Some(list) = mb.list.as_mut()
        {
            list.selected = pick(n);
        }
        app.minibuffer = Some(mb);
    });
}

/// M-< and M-> in the list (corfu-map, vertico-first / -last): jump to the
/// first or the last candidate.
pub fn cmd_list_first(app: &mut App) {
    list_select(app, |_| 0);
}

pub fn cmd_list_last(app: &mut App) {
    list_select(app, |n| n.saturating_sub(1));
}

/// RET while the vertical list is up: take the candidate, unless the input
/// is already an exact match and nothing is highlighted.
pub fn cmd_exit_completion(app: &mut App) {
    let Some(mut mb) = app.minibuffer.take() else {
        return;
    };
    refresh(app, &mut mb);
    let take = mb.list.as_ref().is_some_and(|list| {
        let exact = list
            .all
            .iter()
            .any(|c| format!("{}{c}", list.prefix) == mb.input);
        !exact || list.selected > 0
    });
    if take && let Some(c) = mb.list.as_ref().and_then(|l| l.selected_full()) {
        mb.input = c;
    }
    // A directory: go into it rather than open it, except where a
    // directory is an answer ("./", and Dired's prompts).
    let descend = mb.list.as_ref().is_some_and(|l| l.category == "file")
        && mb.input.ends_with('/')
        && !mb.input.ends_with("/./")
        && !matches!(mb.purpose, Purpose::Dired(_));
    if descend {
        app.minibuffer = Some(mb);
        return;
    }
    submit(app, mb);
}

/// y for the yes/no prompts: a confirmation, a plugin's question, a grant.
pub fn cmd_yes(app: &mut App) {
    let Some(mb) = app.minibuffer.take() else {
        return;
    };
    match mb.purpose {
        Purpose::Confirm(action) => {
            let build = matches!(action, ConfirmAction::BuildPackage);
            confirm(app, action);
            if build && crate::package_vc::confirming(app) {
                crate::package_vc::declined(app);
            }
        }
        Purpose::PluginGrant => plugins::answer_grant(app, Some(true)),
        Purpose::ProjectGrant { root } => crate::project::answered(app, &root, Some(true)),
        Purpose::PluginYOrN { plugin, request } => {
            plugins::answer(app, &plugin, request, Some("y"))
        }
        _ => app.minibuffer = Some(mb),
    }
    // Another question may have been waiting for this one (a project's
    // init.rhai, after a file lock).
    crate::project::load_if_granted(app);
}

/// n, where y is the answer.
pub fn cmd_no(app: &mut App) {
    let Some(mb) = app.minibuffer.take() else {
        return;
    };
    match mb.purpose {
        Purpose::PluginGrant => plugins::answer_grant(app, Some(false)),
        Purpose::ProjectGrant { root } => crate::project::answered(app, &root, Some(false)),
        Purpose::PluginYOrN { plugin, request } => {
            plugins::answer(app, &plugin, request, Some("n"))
        }
        // Any other confirmation: "no" just closes the question, except
        // for a build, where it has to let the queue know it was declined.
        Purpose::Confirm(ConfirmAction::BuildPackage) => {
            if crate::package_vc::confirming(app) {
                crate::package_vc::declined(app);
            }
        }
        Purpose::Confirm(ConfirmAction::IndexPackageUpgrade) => {
            crate::packages::package_upgrade_confirmed(app, false);
        }
        Purpose::Confirm(ConfirmAction::GitPackageUpgrade) => {
            crate::package_vc::upgrade_review_declined(app);
        }
        Purpose::Confirm(ConfirmAction::Vc(c)) => crate::vc::declined(app, c),
        Purpose::Confirm(ConfirmAction::GrepSave { buffer, rest }) => {
            crate::grep::save_confirmed(app, buffer, rest, false);
        }
        Purpose::Confirm(_) => {}
        _ => app.minibuffer = Some(mb),
    }
    crate::project::load_if_granted(app);
}

#[cfg(test)]
pub(crate) fn buffer_directory_for_test(app: &App) -> PathBuf {
    buffer_directory(app)
}

/// Where a typed file name points: `~` expanded, a relative name taken from
/// the buffer's directory.
fn typed_path(app: &App, input: &str) -> String {
    let p = expand_path(input);
    let p = if p.is_absolute() {
        p
    } else {
        buffer_directory(app).join(p)
    };
    p.display().to_string()
}

/// The answer to make-directory, make-empty-file, copy-directory,
/// delete-directory and insert-directory.
fn file_op(app: &mut App, op: FileOp, input: &str) {
    use forge_core::fileops as fo;
    let input = input.trim();
    if input.is_empty() {
        return;
    }
    match op {
        FileOp::MakeDirectory => {
            let at = typed_path(app, input);
            match fo::make_directory(&at, true) {
                Ok(true) => app.msg(format!("Made directory {at}")),
                Ok(false) => app.msg(format!("{at} already exists")),
                Err(e) => app.error(format!("Cannot make directory {at}: {e}")),
            }
        }
        FileOp::MakeEmptyFile => {
            let at = typed_path(app, input);
            match fo::make_empty_file(&at, true) {
                Ok(()) => app.msg(format!("Made empty file {at}")),
                Err(e) => app.error(format!("Cannot make {at}: {e}")),
            }
        }
        FileOp::CopyFrom => {
            let from = typed_path(app, input);
            if !std::path::Path::new(&from).is_dir() {
                app.error(format!("{from} is not a directory"));
                return;
            }
            app.prompt(
                format!("Copy directory {from} to: "),
                Purpose::FileOp(FileOp::CopyTo(from)),
            );
        }
        FileOp::CopyTo(from) => {
            let to = typed_path(app, input);
            match fo::copy_directory(&from, &to, true, false) {
                Ok(wrote) => app.msg(format!("Copied {from} to {wrote}")),
                Err(e) => app.error(format!("Cannot copy {from}: {e}")),
            }
        }
        FileOp::DeleteDirectory => {
            let at = typed_path(app, input);
            if !std::path::Path::new(&at).is_dir() {
                app.error(format!("{at} is not a directory"));
            } else if fo::directory_empty_p(&at) {
                match fo::delete_directory(&at, false) {
                    Ok(()) => app.msg(format!("Deleted {at}")),
                    Err(e) => app.error(format!("Cannot delete {at}: {e}")),
                }
            } else {
                app.prompt(
                    format!("{at} is not empty; delete it and everything in it? (yes or no) "),
                    Purpose::FileOp(FileOp::DeleteAll(at)),
                );
            }
        }
        FileOp::DeleteAll(at) => {
            if input != "yes" {
                app.msg("Not deleted".to_string());
                return;
            }
            match fo::delete_directory(&at, true) {
                Ok(()) => app.msg(format!("Deleted {at}")),
                Err(e) => app.error(format!("Cannot delete {at}: {e}")),
            }
        }
        FileOp::InsertDirectory => {
            let at = typed_path(app, input);
            let wildcard = input.contains(['*', '?', '[']);
            let full = std::path::Path::new(&at).is_dir();
            match fo::directory_listing(&at, &["-al".to_string()], wildcard, full) {
                Ok(text) => {
                    let b = app.buf();
                    let start = b.point();
                    b.push_mark(start, false);
                    b.insert_str(&text);
                    let end = b.point();
                    b.push_mark(end, false);
                    b.set_point(start);
                }
                Err(e) => app.error(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_file() -> Purpose {
        Purpose::FindFile {
            other_window: false,
            other_frame: false,
            other_tab: false,
        }
    }

    fn over_question() -> forge_core::Question {
        forge_core::Question::Completing {
            prompt: "History: ".into(),
            candidates: vec!["a".into(), "b".into()],
            require_match: false,
            multiple: false,
            over: true,
        }
    }

    fn pending(over: bool) -> plugins::PendingQuestion {
        plugins::PendingQuestion {
            plugin: "test".into(),
            request: 1,
            question: if over {
                over_question()
            } else {
                forge_core::Question::Completing {
                    prompt: "Other: ".into(),
                    candidates: Vec::new(),
                    require_match: false,
                    multiple: false,
                    over: false,
                }
            },
        }
    }

    #[test]
    fn over_question_suspends_and_restores_the_prompt() {
        let mut app = App::new(forge_core::Editor::default());
        app.prompt("Find file: ", find_file());
        app.minibuffer.as_mut().unwrap().input = "old".to_string();
        app.pending_questions.push_back(pending(true));

        plugins::show_next_question(&mut app);
        assert!(app.suspended_prompt.is_some(), "the prompt was suspended");
        assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "History: ");

        let mut mb = app.minibuffer.take().unwrap();
        mb.input = "b".to_string();
        submit(&mut app, mb);

        let restored = app.minibuffer.as_ref().expect("the prompt came back");
        assert_eq!(restored.prompt, "Find file: ");
        assert_eq!(restored.input, "b", "the pick replaced its input");
    }

    #[test]
    fn plain_question_waits_for_a_busy_prompt() {
        let mut app = App::new(forge_core::Editor::default());
        app.prompt("Find file: ", find_file());
        app.pending_questions.push_back(pending(false));

        plugins::show_next_question(&mut app);
        assert!(app.suspended_prompt.is_none());
        assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "Find file: ");
        assert_eq!(app.pending_questions.len(), 1, "still waiting its turn");
    }

    #[test]
    fn mx_offers_mode_commands_only_in_their_mode() {
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);

        let names = |app: &mut App| {
            app.prompt("M-x ", Purpose::ExtendedCommand(None));
            let mb = app.minibuffer.as_ref().unwrap();
            source(app, mb).unwrap().2
        };

        let outside = names(&mut app);
        assert!(outside.contains(&"webkit".to_string()));
        assert!(outside.contains(&"webkit-browse-url".to_string()));
        assert!(!outside.contains(&"webkit-back".to_string()));
        assert!(!outside.contains(&"dired-summary".to_string()));

        app.buf().set_major_mode("dired-mode");
        let dired = names(&mut app);
        assert!(dired.contains(&"dired-summary".to_string()));
        assert!(!dired.contains(&"webkit-back".to_string()));

        assert!(crate::webkit::answer(
            &mut app,
            &crate::org_cmds::Ask::WebGo,
            "example.com"
        ));
        let page = names(&mut app);
        assert!(page.contains(&"webkit-back".to_string()));
        assert!(!page.contains(&"dired-summary".to_string()));
    }

    #[test]
    fn over_question_resumes_an_isearch_with_the_pick() {
        let mut app = App::new(forge_core::Editor::default());
        app.editor.create_buffer("x");
        start_isearch(&mut app, true);
        app.pending_questions.push_back(pending(true));

        plugins::show_next_question(&mut app);
        assert!(app.suspended_prompt.is_some(), "the search was suspended");
        assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "History: ");

        let mut mb = app.minibuffer.take().unwrap();
        mb.input = "beta".to_string();
        submit(&mut app, mb);

        let restored = app.minibuffer.as_ref().expect("the search came back");
        match &restored.purpose {
            Purpose::Isearch(s) => {
                assert_eq!(s.query, "beta", "the pick became the search string")
            }
            _ => panic!("expected the search to resume"),
        }
    }

    fn prompt_app(tag: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("forge-mb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["usr/local/bin", "usr/lib", "usr/local/share", "home"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["notes.txt", "notes.txt~", "Readme.md"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(forge_core::Buffer::new("scratch"));
        app.editor.show_buffer(id);
        let dir = std::fs::canonicalize(dir).unwrap();
        (app, dir)
    }

    fn matches_for(app: &mut App, input: &str) -> Vec<String> {
        let mut mb = Minibuffer::new("Find file: ", find_file());
        mb.input = input.to_string();
        refresh(app, &mut mb);
        let list = mb.list.as_ref().expect("a file list");
        (0..list.matches.len())
            .filter_map(|i| list.full(i))
            .collect()
    }

    #[test]
    fn the_prompt_edits() {
        let mut mb = Minibuffer::new("P: ", find_file());
        mb.input = "foo-bar baz".into();
        assert_eq!(mb.pos(), 11, "the cursor starts at the end");
        mb.set_pos(0);
        mb.insert("x");
        assert_eq!(mb.input, "xfoo-bar baz");
        assert_eq!(mb.pos(), 1);
        let to = mb.word_forward(1);
        assert_eq!(to, 4);
        assert_eq!(mb.word_backward(12), 9);
        assert_eq!(mb.remove(1, 4), "foo");
        assert_eq!(mb.input, "x-bar baz");
        assert_eq!(mb.text_to_cursor(), "P: x");
        // Text added from outside puts the cursor back at the end.
        mb.input.push_str("!!");
        assert_eq!(mb.pos(), mb.input.chars().count());
    }

    #[test]
    fn keys_move_and_kill_in_the_prompt() {
        let (mut app, _dir) = prompt_app("keys");
        let mut mb = Minibuffer::new("Find file: ", find_file());
        mb.input = "one two three".into();
        app.minibuffer = Some(mb);
        for cmd in ["beginning-of-line", "forward-word", "kill-word"] {
            cmd_edit(&mut app, cmd);
        }
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "one three");
        cmd_edit(&mut app, "end-of-line");
        cmd_edit(&mut app, "backward-kill-word");
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "one ");
        cmd_edit(&mut app, "yank");
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "one three");
        cmd_edit(&mut app, "beginning-of-line");
        cmd_edit(&mut app, "kill-line");
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "");
        cmd_edit(&mut app, "yank");
        cmd_edit(&mut app, "transpose-chars");
        assert_eq!(
            app.minibuffer.as_ref().unwrap().input,
            "one threee"[..0].to_string() + "one thre" + "e" + ""
        );
    }

    #[test]
    fn file_prompts_shadow_expand_and_list_dot_dirs() {
        let (mut app, dir) = prompt_app("files");
        let d = dir.display().to_string();
        // Partial completion of directory names.
        let m = matches_for(&mut app, &format!("{d}/u/l/b"));
        assert!(m.iter().any(|c| c.ends_with("usr/local/bin/")), "{m:?}");
        // Several expansions are listed by full path.
        let m = matches_for(&mut app, &format!("{d}/u/l/"));
        assert!(
            m.iter().any(|c| c.ends_with("usr/local/bin/"))
                && m.iter().any(|c| c.ends_with("usr/local/share/")),
            "{m:?}"
        );
        // ../ is always offered, ./ opens the directory.
        let m = matches_for(&mut app, &format!("{d}/"));
        assert!(
            m.contains(&format!("{d}/../")) && m.contains(&format!("{d}/./")),
            "{m:?}"
        );
        // Case does not matter when typing lower case.
        let m = matches_for(&mut app, &format!("{d}/readme"));
        assert!(m.iter().any(|c| c.ends_with("Readme.md")), "{m:?}");
        // // and ~/ shadow what came before.
        assert_eq!(shadowed("/a/b//c/d"), "/c/d");
        assert_eq!(shadowed("/a/b/~/c"), "~/c");
        assert_eq!(expand_path("/ignored//tmp"), PathBuf::from("/tmp"));
        let m = matches_for(&mut app, &format!("/nowhere//{d}/not"));
        assert!(m.iter().any(|c| c.ends_with("notes.txt")), "{m:?}");
    }

    #[test]
    fn tab_skips_ignored_extensions() {
        let (mut app, dir) = prompt_app("tab");
        let d = dir.display().to_string();
        let mut mb = Minibuffer::new("Find file: ", find_file());
        mb.input = format!("{d}/note");
        complete(&mut app, &mut mb);
        assert_eq!(mb.input, format!("{d}/notes.txt"), "the backup is skipped");
    }

    #[test]
    fn completions_buffer_shows_and_goes_away() {
        let (mut app, dir) = prompt_app("cmpl");
        let d = dir.display().to_string();
        let windows = |app: &App| app.editor.windows.iter().count();
        let before = windows(&app);
        let mut mb = Minibuffer::new("Find file: ", find_file());
        mb.input = format!("{d}/");
        complete(&mut app, &mut mb);
        let id = app
            .editor
            .find_buffer(COMPLETIONS)
            .expect("a *Completions* buffer");
        let text = app.editor.state.get_buffer(id).unwrap().get_contents();
        assert!(
            text.contains("possible completions:") && text.contains("notes.txt"),
            "{text}"
        );
        assert_eq!(windows(&app), before + 1, "it got a window of its own");
        hide_completions(&mut app, &mut mb);
        assert_eq!(windows(&app), before, "and loses it with the prompt");
    }

    #[test]
    fn question_mark_inserts_where_nothing_completes() {
        let (mut app, _dir) = prompt_app("qm");
        app.minibuffer = Some(Minibuffer::new(
            "Name: ",
            Purpose::RenameTab {
                default: String::new(),
            },
        ));
        cmd_completion_help(&mut app);
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "?");
    }

    #[test]
    #[cfg(unix)]
    fn the_gui_starts_in_home_when_launched_from_root() {
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir("/").unwrap();
        start_in_home_from_gui();
        let now = std::env::current_dir().unwrap();
        let home = std::fs::canonicalize(std::env::var_os("HOME").unwrap()).unwrap();
        std::env::set_current_dir(old).unwrap();
        assert_eq!(now, home);
    }
}
