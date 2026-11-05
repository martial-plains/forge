// SPDX-License-Identifier: MIT OR Apache-2.0
//! Text buffer: a rope of text, point (cursor), mark, undo history, the
//! edit journal, and the file it visits.
//!
//! Positions are (line, column) with columns counted in characters, or
//! plain character offsets. Point moves by grapheme cluster, so an emoji
//! or an accented letter built from two code points is one step.
//!
//! Every change goes through one function, `Buffer::apply`, which
//! records it twice: in the undo history (grouped into steps) and in the
//! edit journal (a numbered list of [`Edit`]s). Anything that remembers a
//! position in the buffer - another window's point, a register, the mark
//! - can carry it across later edits with [`Buffer::transform_offset`].

#![deny(clippy::unwrap_used, clippy::expect_used)]

use crate::width;
use ropey::Rope;
use std::cmp::min;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

/// How many older marks a buffer's mark ring keeps.
pub const MARK_RING_LIMIT: usize = 16;

/// `mark-ring-max`: how many entries a buffer's mark ring keeps. The
/// editor sets it from `forge.opt.mark_ring_max`.
static MARK_RING_MAX: AtomicUsize = AtomicUsize::new(MARK_RING_LIMIT);

/// Set how many entries a buffer's mark ring keeps (`forge.opt.mark_ring_max`).
pub fn set_mark_ring_max(n: usize) {
    MARK_RING_MAX.store(n, Ordering::Relaxed);
}

/// How many entries a buffer's mark ring keeps (`forge.opt.mark_ring_max`).
pub fn mark_ring_max() -> usize {
    MARK_RING_MAX.load(Ordering::Relaxed)
}

/// A position in a buffer. Ordered by line, then column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// One change: at char offset `at`, `deleted` was replaced by `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub deleted: String,
    pub inserted: String,
}

impl Edit {
    pub(crate) fn deleted_len(&self) -> usize {
        self.deleted.chars().count()
    }

    fn inserted_len(&self) -> usize {
        self.inserted.chars().count()
    }

    fn inverse(&self) -> Edit {
        Edit {
            at: self.at,
            deleted: self.inserted.clone(),
            inserted: self.deleted.clone(),
        }
    }

    /// Where a remembered offset ends up after this edit. Text inserted
    /// exactly at the offset goes after it; an
    /// offset inside deleted text moves to where the deletion was.
    pub fn transform(&self, offset: usize) -> usize {
        let end = self.at + self.deleted_len();
        if offset <= self.at {
            offset
        } else if offset >= end {
            offset - self.deleted_len() + self.inserted_len()
        } else {
            self.at
        }
    }
}

/// The major mode of a buffer nobody has given another one.
pub const DEFAULT_MAJOR_MODE: &str = "fundamental-mode";
pub const DEFAULT_FILL_COLUMN: usize = 70;

/// The major mode for visiting image files (see the "Viewing Image Files"
/// manual node). A buffer whose file name ends in an image extension gets
/// it automatically.
pub const IMAGE_MODE: &str = "image-mode";

/// How an Image-mode buffer shows its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageDisplay {
    /// The picture, drawn by a frontend that can.
    #[default]
    Image,
    /// The file's underlying text (raw bytes, lossy-decoded).
    Text,

    /// The file's bytes as hex.
    Hex,
}

/// A transform applied to an Image-mode picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageTransform {
    /// Percent of the original size: 100 is the original.
    pub percent: f64,
    /// Rotation in degrees clockwise (0, 90, 180, 270).
    pub rotation: u16,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Default for ImageTransform {
    fn default() -> Self {
        Self {
            percent: 100.0,
            rotation: 0,
            flip_h: false,
            flip_v: false,
        }
    }
}

/// Image-mode's buffer-local state. The rendered picture lives in a temp
/// file the frontend draws over the window.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageState {
    pub display: ImageDisplay,
    pub transform: ImageTransform,
    /// The transformed render to draw (None until rendered).
    pub rendered: Option<PathBuf>,
    /// The original picture's size, if it could be read.
    pub size: (u32, u32),
    /// The buffer's text when not showing hex (restored when leaving Hex).
    pub raw: Option<String>,
    /// Animation: the current frame (0-based), how many there are, and
    /// whether it plays.
    pub frame: usize,
    pub frame_count: usize,
    pub animating: bool,
    /// Animation speed: 1.0 is normal.
    pub speed: f64,
    /// Whether the animation loops.
    pub looping: bool,
    /// A plugin's canvas rather than a picture file: shown the same way,
    /// in whatever major mode the plugin gave the buffer.
    pub canvas: bool,
}

impl Default for ImageState {
    fn default() -> Self {
        Self {
            display: ImageDisplay::Image,
            transform: ImageTransform::default(),
            rendered: None,
            size: (0, 0),
            raw: None,
            frame: 0,
            frame_count: 1,
            animating: false,
            speed: 1.0,
            looping: false,
            canvas: false,
        }
    }
}

/// Undo steps kept per buffer.
const UNDO_LIMIT: usize = 1000;
/// consecutive typed characters undo together in groups of 20.
const INSERT_GROUP_LIMIT: usize = 20;
/// Journal entries kept. Positions older than this are clamped instead.
const JOURNAL_LIMIT: usize = 10_000;
/// A buffer that shrinks below 3/4 of its size at the last save, when
/// that was more than this many characters, stops auto-saving until it is
/// saved for real ("has shrunk a lot; auto save disabled").
pub const AUTO_SAVE_SHRINK_MIN: usize = 5000;

#[derive(Clone, Debug)]
struct UndoGroup {
    edits: Vec<Edit>,
    point_before: usize,
    point_after: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKind {
    /// Typed characters (counts how many).
    Typing(usize),
    /// Everything one command did.
    Command,
}

/// Numbered edits. `version` is the number of edits ever applied; the
/// journal keeps the last [`JOURNAL_LIMIT`].
#[derive(Clone, Debug, Default)]
struct Journal {
    /// Version of the first entry in `edits`.
    base: u64,
    edits: Vec<Edit>,
}

impl Journal {
    fn version(&self) -> u64 {
        self.base + self.edits.len() as u64
    }

    fn push(&mut self, e: Edit) {
        self.edits.push(e);
        if self.edits.len() > JOURNAL_LIMIT * 2 {
            self.edits.drain(..JOURNAL_LIMIT);
            self.base += JOURNAL_LIMIT as u64;
        }
    }

    /// Forget everything (the text was replaced wholesale).
    fn reset(&mut self) {
        self.base = self.version() + 1;
        self.edits.clear();
    }

    fn since(&self, version: u64) -> Option<&[Edit]> {
        if version < self.base || version > self.version() {
            return None;
        }
        Some(&self.edits[(version - self.base) as usize..])
    }
}

/// What the file looked like on disk when we last read or wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskStamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &Path) -> Option<DiskStamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(DiskStamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// One value in a display specification: a number, string, symbol, bool
/// or nil.
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayValue {
    Nil,
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(String),
    Symbol(String),
    /// A width in pixels: `(N)`.
    Pixels(f64),
    /// A font size step: `(+ N)` and `(- N)`.
    Step(i64),
}

/// One display specification, e.g. `image` or `height`. `name` is the CAR;
/// `args` are the rest, as alternating `:key value` pairs or positional
/// values.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySpec {
    pub name: String,
    pub args: Vec<DisplayValue>,
}

impl DisplaySpec {
    /// The value of the named argument (e.g. `:file`), if present.
    pub fn arg(&self, key: &str) -> Option<&DisplayValue> {
        self.args
            .windows(2)
            .find(|w| matches!(&w[0], DisplayValue::Symbol(s) if s == key))
            .map(|w| &w[1])
    }
}

/// A `display` text property's value: a list of display specifications
/// applied in parallel to the text they cover.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DisplayProps(pub Vec<DisplaySpec>);

/// The `display` text property over a range of text (see the "The display
/// Property" manual node).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRange {
    pub start: usize,
    pub end: usize,
    pub props: Vec<DisplaySpec>,
}

/// An overlay: a range with its own properties, which take precedence over
/// the buffer's text property at the same position.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub id: usize,
    pub start: usize,
    pub end: usize,
    pub display: DisplayProps,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    pub(crate) text: Rope,
    /// Char offset of point.
    point: usize,
    /// Display column that C-n / C-p try to return to across short lines.
    goal_col: Option<usize>,
    name: String,
    modified: bool,
    mark: Option<usize>,
    mark_active: bool,
    /// Older marks this buffer's mark visited, newest first (C-u C-SPC
    /// walks back through them; consult-mark lists them).
    mark_ring: Vec<usize>,
    path: Option<PathBuf>,
    /// The `.#name` lock, held while the buffer is modified (filelock.rs).
    file_lock: Option<crate::filelock::FileLock>,
    /// Someone else held the lock when the buffer was first modified.
    lock_conflict: Option<crate::filelock::LockOwner>,
    /// ...and the user hasn't been asked about it yet.
    lock_question: bool,
    /// The user chose to edit without the lock (`p`); asked again after
    /// the next save.
    lock_declined: bool,
    /// This buffer's own `create-lockfiles`, if it has one (else the
    /// global setting in filelock.rs).
    lockfiles: Option<bool>,
    /// Undo depth at which the text is what's on disk (None: no longer
    /// reachable by undo). Undoing back to it clears the modified flag.
    saved_depth: Option<usize>,
    crlf: bool,
    /// Visiting a `.gpg` file: who it is encrypted to (see [`crate::epa`]).
    epa: Option<crate::epa::Epa>,
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    /// The undo group new edits join, if any.
    open_group: Option<GroupKind>,
    journal: Journal,
    /// Major mode name, e.g. "fundamental-mode". Picks the major-mode keymap.
    major_mode: String,
    /// Enabled minor modes, most recently enabled first (that is also
    /// their keymap priority).
    minor_modes: Vec<String>,
    /// M-q wraps lines at this display column.
    pub fill_column: usize,
    disk: Option<DiskStamp>,
    /// A backup (file~) was made this session.
    backed_up: bool,
    /// No backups for this buffer's file: version control sets it for registered files.
    pub backup_inhibited: bool,
    /// Numbered backups past what's kept, for the editor to offer to
    /// delete.
    excess_backups: Vec<PathBuf>,
    /// Journal version at the last save or auto-save.
    auto_saved_version: u64,
    /// Characters at the last save or auto-save; None while auto-saving
    /// is off because the buffer shrank a lot.
    save_length: Option<usize>,
    /// How many times the buffer has been saved (for buffer-saved events).
    save_count: u64,
    /// Syntax tree, if the major mode has a grammar (see buffer_ext.rs).
    pub(crate) syntax: SyntaxSlot,
    pub(crate) decorations: Vec<crate::decoration::Decoration>,
    /// Ranges of text that point somewhere (see links.rs).
    pub(crate) links: Vec<crate::links::LinkSpan>,
    /// Hidden lines (see fold.rs).
    pub(crate) folds: Vec<crate::fold::Fold>,
    pub options: crate::decoration::BufferOptions,
    /// Org's display-only changes (shrunk tables, link previews, table header line).
    pub org_display: crate::org_display::OrgDisplay,
    /// `text-scale-adjust`: steps of text size above (or below) normal.
    pub text_scale: i32,
    /// Inline pictures by buffer line, for frontends that draw them (the
    /// text browser fills this from its image cache; Org fills it from
    /// link previews through `org_line_image`).
    pub inline_images: std::collections::BTreeMap<usize, std::path::PathBuf>,
    /// Pictures a plugin pinned to a line (`images.set`), by line then by
    /// plugin, so one plugin's pictures don't clobber another's and they
    /// go when the plugin is unloaded.
    pub plugin_images:
        std::collections::BTreeMap<usize, std::collections::BTreeMap<String, std::path::PathBuf>>,
    /// Lines whose plugin picture is centred in the window (`images.set-centered`),
    /// with the plugins that asked.
    pub plugin_centered: std::collections::BTreeMap<usize, std::collections::BTreeSet<String>>,
    /// Image-mode's state, for a buffer visiting an image file.
    pub image: Option<ImageState>,
    /// The `display` text property, by range (see the "The display
    /// Property" manual node).
    pub display: Vec<DisplayRange>,
    /// Overlays, whose properties win over the text property.
    pub overlay: Vec<Overlay>,
    /// The next overlay id.
    pub(crate) next_overlay: usize,
    /// Editing is refused: the buffer is a listing (dired), or a caller
    /// marked it. Answers the `read-only` keymap condition.
    read_only: bool,
    /// The `read-only` buffer option: edits are refused (and remembered
    /// in `refused`) unless they come from the buffer's owner (see
    /// [`Buffer::as_owner`]).
    locked: bool,
    /// Inside [`Buffer::as_owner`]: a locked buffer takes edits.
    owner_editing: bool,
    /// An edit was refused since [`Buffer::take_refused`] last asked.
    refused: bool,
    /// Text from this offset to the end is read-only to commands (a
    /// commit buffer's summary of its files). Text typed right before it
    /// goes before it.
    protected_from: Option<usize>,
    /// Text before this offset is read-only to commands (a shell's output
    /// above its input line). Text typed right at it stays after it, in
    /// the editable part. The owner ([`Buffer::as_owner`]) can still
    /// change it.
    protected_head: Option<usize>,
    /// What the mode line shows for version control (`Git-main`), as
    /// buffer-local `vc-mode`.
    pub vc_mode: Option<String>,
    /// What plugins show in the mode line for this buffer, by plugin
    /// (buffers.set-mode-line-info).
    pub mode_line_info: std::collections::BTreeMap<String, String>,
    /// The title of the input method on in this buffer, shown at the
    /// start of the mode line as current-input-method-title is
    /// (`\\U:` for TeX, `1<U:` for latin-postfix).
    pub input_method_title: Option<String>,
    /// Org Crypt's decrypt cache (plaintext hashes and armors per
    /// heading); hash-guarded, so stale line numbers only miss.
    pub crypt_cache: crate::org_crypt::crypto::CryptCache,
}

/// A buffer's syntax tree. Copying a buffer doesn't copy it (it's rebuilt
/// on demand).
#[derive(Default)]
pub(crate) struct SyntaxSlot(pub(crate) Option<Box<crate::syntax::Syntax>>);

impl Clone for SyntaxSlot {
    fn clone(&self) -> Self {
        Self(
            self.0
                .as_ref()
                .and_then(|s| crate::syntax::Syntax::new(s.language()).map(Box::new)),
        )
    }
}

impl std::fmt::Debug for SyntaxSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Characters that make up a "word" for M-f, M-b, M-d, etc.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn chars_eq(a: char, b: char, fold_case: bool) -> bool {
    a == b || (fold_case && a.to_lowercase().eq(b.to_lowercase()))
}

impl Buffer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            text: Rope::new(),
            point: 0,
            goal_col: None,
            name: name.into(),
            modified: false,
            mark: None,
            mark_active: false,
            mark_ring: Vec::new(),
            path: None,
            file_lock: None,
            lock_conflict: None,
            lock_question: false,
            lock_declined: false,
            lockfiles: None,
            saved_depth: Some(0),
            crlf: false,
            epa: None,
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: None,
            journal: Journal::default(),
            major_mode: DEFAULT_MAJOR_MODE.to_string(),
            read_only: false,
            locked: false,
            owner_editing: false,
            refused: false,
            protected_from: None,
            protected_head: None,
            vc_mode: None,
            mode_line_info: Default::default(),
            input_method_title: None,
            crypt_cache: Default::default(),
            minor_modes: Vec::new(),
            fill_column: DEFAULT_FILL_COLUMN,
            disk: None,
            backed_up: false,
            backup_inhibited: false,
            excess_backups: Vec::new(),
            auto_saved_version: 0,
            save_length: Some(0),
            save_count: 0,
            syntax: SyntaxSlot::default(),
            decorations: Vec::new(),
            links: Vec::new(),
            folds: Vec::new(),
            options: Default::default(),
            org_display: Default::default(),
            text_scale: 0,
            inline_images: Default::default(),
            plugin_images: Default::default(),
            plugin_centered: Default::default(),
            image: None,
            display: Vec::new(),
            overlay: Vec::new(),
            next_overlay: 1,
        }
    }

    // ---------------------------------------------------------------- files

    /// Open a file. A path that doesn't exist yet gives an empty buffer
    /// visiting it (like "(New file)").
    pub fn from_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut buf = Buffer::new(name);
        // No lock yet:, a file is locked when its buffer is
        // first modified, not when it is visited.
        // Read bytes, not a string: an image (or any binary) file is shown
        // as raw text in Image mode rather than failing to visit.
        // A .gpg file is decrypted into the buffer and encrypted on save.
        let read = if crate::epa::is_encrypted_name(path) && path.exists() {
            crate::epa::decrypt(path).map(|(bytes, epa)| {
                buf.epa = Some(epa);
                bytes
            })
        } else {
            if crate::epa::is_encrypted_name(path) {
                buf.epa = Some(crate::epa::Epa::default());
            }
            std::fs::read(path)
        };
        match read {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                buf.crlf = text.contains("\r\n");
                buf.set_contents(&text);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        buf.path = Some(path.to_path_buf());
        buf.disk = stamp(path);
        Ok(buf)
    }

    fn file_text(&self) -> String {
        self.crlf_text(&self.text.to_string())
    }

    /// Buffer text is LF-only; put the file's CRLF line endings back.
    /// Any CRLF already in `text` is flattened first so a buffer that
    /// somehow holds one (a pasted block, say) never doubles to `\r\r\n`.
    fn crlf_text(&self, text: &str) -> String {
        if self.crlf {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text.to_string()
        }
    }

    /// Write the buffer to its file. The first save in a session copies
    /// the old file to `file~` first, and a successful save
    /// removes the auto-save file.
    pub fn save(&mut self) -> io::Result<()> {
        self.before_save();
        let text = self.file_text();
        self.write_save(&text)
    }

    /// Save `text` (LF line endings) to the visited file, with the backup
    /// and saved-state bookkeeping of [`Buffer::save`]. Powers Org Crypt, which
    /// writes encrypted text while the buffer keeps showing the plaintext.
    pub fn save_text_override(&mut self, text: &str) -> io::Result<()> {
        self.before_save();
        let text = self.crlf_text(text);
        self.write_save(&text)
    }

    fn write_save(&mut self, text: &str) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::other("buffer is not visiting a file"))?;
        self.write_save_to(&path, text)
    }

    /// The actual write, to an explicit path (so `save-as` can try the
    /// new file before committing the buffer to it). The backup, save
    /// count, saved state and disk stamp only happen once the write
    /// succeeded.
    fn write_save_to(&mut self, path: &Path, text: &str) -> io::Result<()> {
        if !self.backed_up && !self.backup_inhibited && path.is_file() {
            // A failed backup shouldn't stop the save.
            if let Ok(made) = crate::backup::make_backup(path) {
                self.excess_backups = made.excess;
            }
            self.backed_up = true;
        }
        if crate::epa::is_encrypted_name(path) {
            let epa = self.epa.clone().unwrap_or_default();
            let data = crate::epa::encrypt(text.as_bytes(), &epa)?;
            crate::fsutil::write_atomic(path, data)?;
            self.epa.get_or_insert(epa);
        } else {
            crate::fsutil::write_atomic(path, text)?;
            self.epa = None;
        }
        self.save_count += 1;
        self.mark_saved();
        self.disk = stamp(path);
        self.discard_auto_save();
        Ok(())
    }

    /// Excess numbered backups the last save left for the user to decide
    /// about. Asking clears them.
    pub fn take_excess_backups(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.excess_backups)
    }

    /// The text is now what's on disk: unmodified (which releases the
    /// lock), undo can come back here, and auto-saving starts afresh.
    fn mark_saved(&mut self) {
        self.break_undo_group();
        self.saved_depth = Some(self.undo.len());
        self.save_length = Some(self.char_count());
        self.set_modified(false);
    }

    pub fn save_as(&mut self, path: impl Into<PathBuf>) -> io::Result<()> {
        let path = path.into();
        self.save_as_text(path, None)
    }

    /// [`Buffer::save_as`], but what lands on disk is `text` (LF line endings)
    /// instead of the buffer, as with [`Buffer::save_text_override`]. Powers Org
    /// Crypt's "write encrypted" for `C-x C-w`.
    pub fn save_as_text_override(
        &mut self,
        path: impl Into<PathBuf>,
        text: &str,
    ) -> io::Result<()> {
        let path = path.into();
        self.save_as_text(path, Some(text))
    }

    fn save_as_text(&mut self, path: PathBuf, text: Option<&str>) -> io::Result<()> {
        let changing = self.path.as_deref() != Some(path.as_path());
        if changing
            // Writing over a file someone else is editing would lose their
            // work when they save; Forge asks here, and so do we (by
            // refusing, so the user can steal the lock or pick another name).
            && crate::filelock::create_lockfiles()
            && let Some(owner) = crate::filelock::other_owner(&path)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is locked by {}", path.display(), owner.describe()),
            ));
        }
        self.before_save();
        let body = match text {
            Some(t) => self.crlf_text(t),
            None => self.file_text(),
        };
        // Try the write before retargeting the buffer: a failed save-as
        // (a missing directory, no permission) leaves it visiting the old
        // file, not a path nothing was written to.
        let was_backed_up = std::mem::replace(&mut self.backed_up, false);
        match self.write_save_to(&path, &body) {
            Ok(()) => {
                if changing {
                    // The old file's lock is no business of ours any more.
                    self.file_lock = None;
                    self.lock_conflict = None;
                    self.lock_question = false;
                    self.lock_declined = false;
                }
                self.path = Some(path);
                Ok(())
            }
            Err(e) => {
                self.backed_up = was_backed_up;
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------ locks

    /// Lock the file now that the buffer has its first unsaved change.
    fn lock_on_first_change(&mut self) {
        let enabled = self
            .lockfiles
            .unwrap_or_else(crate::filelock::create_lockfiles);
        if self.file_lock.is_some() || self.lock_declined || !enabled {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        match crate::filelock::acquire(&path, false) {
            Ok(crate::filelock::Attempt::Acquired(lock)) => {
                self.file_lock = Some(lock);
                self.lock_conflict = None;
            }
            Ok(crate::filelock::Attempt::Locked(owner)) => {
                self.lock_conflict = Some(owner);
                self.lock_question = true;
            }
            // Can't make one (a read-only directory, say): edit anyway,
            //.
            Err(_) => {}
        }
    }

    /// Unmodified again: let go of the lock (and forget any conflict).
    fn unlock(&mut self) {
        self.file_lock = None;
        self.lock_conflict = None;
        self.lock_question = false;
        self.lock_declined = false;
    }

    /// Lock files for this buffer only; None follows the global setting.
    pub fn set_lockfiles(&mut self, on: Option<bool>) {
        self.lockfiles = on;
        if on == Some(false) {
            self.file_lock = None;
        }
    }

    /// Does this buffer hold its file's lock?
    pub fn holds_file_lock(&self) -> bool {
        self.file_lock.is_some()
    }

    /// Who else holds the lock, if the buffer's first change found one
    /// and the user hasn't stolen it.
    pub fn lock_conflict(&self) -> Option<&crate::filelock::LockOwner> {
        self.lock_conflict.as_ref()
    }

    /// A lock conflict the user hasn't been asked about yet. Asking clears it.
    pub fn take_lock_question(&mut self) -> Option<crate::filelock::LockOwner> {
        if !std::mem::take(&mut self.lock_question) {
            return None;
        }
        self.lock_conflict.clone()
    }

    /// `s`: take the lock from its owner.
    pub fn steal_file_lock(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::other("buffer is not visiting a file"))?;
        match crate::filelock::acquire(&path, true)? {
            crate::filelock::Attempt::Acquired(lock) => {
                self.file_lock = Some(lock);
                self.lock_conflict = None;
                self.lock_question = false;
                self.lock_declined = false;
                Ok(())
            }
            crate::filelock::Attempt::Locked(owner) => {
                let who = owner.describe();
                self.lock_conflict = Some(owner);
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("lock was taken by {who}"),
                ))
            }
        }
    }

    /// `p`: edit without the lock (until the buffer is next unmodified).
    pub fn ignore_file_lock(&mut self) {
        self.lock_conflict = None;
        self.lock_question = false;
        self.lock_declined = true;
    }

    /// Visit `path` instead (the file was renamed on disk): Forge's
    /// set-visited-file-name. The buffer's name follows the file's; a lock
    /// on the old name is let go and taken on the new one if modified.
    pub fn set_visited_file(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if let Some(n) = path.file_name() {
            self.name = n.to_string_lossy().into_owned();
        }
        self.path = Some(path.clone());
        self.disk = stamp(&path);
        self.file_lock = None;
        self.lock_conflict = None;
        if self.modified {
            self.lock_on_first_change();
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Is editing refused? This only marks the buffer: the commands that
    /// refuse to edit a read-only buffer still ask what kind it is.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, yes: bool) {
        self.read_only = yes;
    }

    /// Lock (or unlock) the text against commands: the `read-only` buffer
    /// option. A locked buffer is also read-only (the keymap condition).
    pub fn set_locked(&mut self, yes: bool) {
        self.locked = yes;
        self.read_only = yes;
    }

    /// Is the text locked against commands?
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Run `f` as the buffer's owner (a plugin rewriting its listing):
    /// a locked buffer takes the edits, and they don't count as unsaved
    /// changes (a listing is never "modified").
    pub fn as_owner<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = std::mem::replace(&mut self.owner_editing, true);
        let modified = self.modified;
        let r = f(self);
        self.owner_editing = was;
        if self.locked {
            self.modified = modified;
        }
        r
    }

    /// Was an edit refused since the last call? Clears the flag.
    pub fn take_refused(&mut self) -> bool {
        std::mem::take(&mut self.refused)
    }

    /// Would an edit be refused now? Records the refusal if so.
    fn refuse_edit(&mut self) -> bool {
        if self.locked && !self.owner_editing {
            self.refused = true;
            true
        } else {
            false
        }
    }

    /// Would replacing `del` chars at `at` be refused (a locked buffer, a
    /// protected head, or the protected tail)? Records the refusal if so.
    fn refuse_edit_at(&mut self, at: usize, del: usize) -> bool {
        if self.refuse_edit() {
            return true;
        }
        if matches!(self.protected_head, Some(h) if !self.owner_editing && at < h) {
            self.refused = true;
            return true;
        }
        match self.protected_from {
            Some(p) if !self.owner_editing && (at > p || at + del > p) => {
                self.refused = true;
                true
            }
            _ => false,
        }
    }

    /// Make the text from `offset` to the end read-only to commands (None:
    /// none of it). The owner ([`Buffer::as_owner`]) can still change it.
    pub fn set_protected_tail(&mut self, offset: Option<usize>) {
        self.protected_from = offset.map(|o| o.min(self.char_count()));
    }

    /// Where the protected tail starts, if there is one.
    pub fn protected_tail(&self) -> Option<usize> {
        self.protected_from
    }

    /// Make the text before `offset` read-only to commands (None: none of
    /// it). The owner ([`Buffer::as_owner`]) can still change it.
    pub fn set_protected_head(&mut self, offset: Option<usize>) {
        self.protected_head = offset.map(|o| o.min(self.char_count()));
    }

    /// Where the protected head ends, if there is one.
    pub fn protected_head(&self) -> Option<usize> {
        self.protected_head
    }

    /// The file changed on disk since we last read or wrote it.
    pub fn changed_on_disk(&self) -> bool {
        match (&self.path, self.disk) {
            (Some(p), Some(old)) => stamp(p) != Some(old),
            // We never saw it on disk; it appearing counts as a change.
            (Some(p), None) => p.exists(),
            _ => false,
        }
    }

    /// Reload the file, replacing the text as one undoable step. Point
    /// stays on the same line and column where possible.
    pub fn revert(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::other("buffer is not visiting a file"))?;
        let read = if crate::epa::is_encrypted_name(&path) && path.exists() {
            crate::epa::decrypt(&path).map(|(bytes, epa)| {
                self.epa = Some(epa);
                String::from_utf8_lossy(&bytes).into_owned()
            })
        } else {
            std::fs::read_to_string(&path)
        };
        let text = match read {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        self.crlf = text.contains("\r\n");
        let text = if self.crlf {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        let p = self.point();
        self.break_undo_group();
        if self.text != text.as_str() {
            let end = self.char_count();
            self.edit_offsets(0, end, &text, GroupKind::Command);
        }
        self.break_undo_group();
        self.set_point(p);
        self.mark_saved();
        self.disk = stamp(&path);
        self.auto_saved_version = self.version();
        self.discard_auto_save();
        self.crypt_cache.clear();
        Ok(())
    }

    /// Drop the `#name#` file and forget that anything was auto-saved.
    /// Called whenever the text it holds stops being the only copy.
    fn discard_auto_save(&mut self) {
        self.auto_saved_version = self.version();
        if let Some(a) = self.auto_save_path() {
            let _ = std::fs::remove_file(a);
        }
    }

    /// `#name#` next to the file, or in no-littering's `var/auto-save/`
    /// when `forge.opt.no_littering` is on (except for temporary files).
    pub fn auto_save_path(&self) -> Option<PathBuf> {
        let p = self.path.as_ref()?;
        let backup = crate::backup::config();
        if backup.no_littering
            && let Some(a) = crate::no_littering::auto_save_path(p, &backup.temporary_dirs)
        {
            return Some(a);
        }
        let name = p.file_name()?.to_string_lossy();
        Some(p.with_file_name(format!("#{name}#")))
    }

    /// Modified since the last save or auto-save (and auto-saving isn't
    /// off because the buffer shrank a lot).
    pub fn needs_auto_save(&self) -> bool {
        self.path.is_some()
            && self.modified
            && self.save_length.is_some()
            && self.version() != self.auto_saved_version
    }

    /// The buffer has shrunk below 3/4 of what it was at the last save or
    /// auto-save, from more than [`AUTO_SAVE_SHRINK_MIN`] characters: a
    /// big deletion that was probably a mistake, so auto-saving it would
    /// overwrite the copy that could undo it.
    pub fn shrunk_a_lot(&self) -> bool {
        self.save_length
            .is_some_and(|l| l > AUTO_SAVE_SHRINK_MIN && 4 * self.char_count() < 3 * l)
    }

    /// Stop auto-saving until the next real save.
    pub fn disable_auto_save(&mut self) {
        self.save_length = None;
    }

    /// Is auto-saving on for this buffer? (Off after a big deletion, until
    /// the buffer is saved.)
    pub fn auto_save_enabled(&self) -> bool {
        self.save_length.is_some()
    }

    /// Write the text to `#name#`. The file gets the visited file's
    /// permissions plus owner read/write, so a private
    /// file's auto-save is private too. Refuses when the buffer shows
    /// decrypted Org Crypt text or a visited `.gpg` file's text, which must
    /// never reach disk unencrypted.
    pub fn auto_save(&mut self) -> io::Result<()> {
        if !self.crypt_cache.is_empty() || self.epa.is_some() {
            return Ok(());
        }
        let Some(a) = self.auto_save_path() else {
            return Ok(());
        };
        if let Some(dir) = a.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_with_mode(&a, self.file_text().as_bytes(), self.auto_save_mode())?;
        self.auto_saved_version = self.version();
        self.save_length = Some(self.char_count());
        Ok(())
    }

    #[cfg(unix)]
    fn auto_save_mode(&self) -> Option<u32> {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(self.path.as_ref()?).ok()?;
        Some((m.permissions().mode() | 0o600) & 0o777)
    }

    #[cfg(not(unix))]
    fn auto_save_mode(&self) -> Option<u32> {
        None
    }

    /// Has the buffer been auto-saved since it was last read or saved
    ///?
    pub fn recently_auto_saved(&self) -> bool {
        self.modified
            && self.auto_saved_version == self.version()
            && self.auto_save_path().is_some_and(|a| a.exists())
    }

    /// There is an auto-save file at least as new as the file itself.
    /// Saving deletes the auto-save file, so one that isn't older means
    /// unsaved work. (Not strictly newer: coarse filesystem clocks often
    /// give both files the same timestamp.)
    pub fn has_newer_auto_save(&self) -> bool {
        let Some(a) = self.auto_save_path() else {
            return false;
        };
        let Ok(am) = std::fs::metadata(&a).and_then(|m| m.modified()) else {
            return false;
        };
        match self
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        {
            Some(fm) => am >= fm,
            None => true,
        }
    }

    /// Replace the text with the auto-save file's (undoable; the buffer
    /// stays modified until saved).
    pub fn recover_auto_save(&mut self) -> io::Result<()> {
        let a = self
            .auto_save_path()
            .ok_or_else(|| io::Error::other("buffer is not visiting a file"))?;
        let text = std::fs::read_to_string(a)?;
        let text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        self.break_undo_group();
        let end = self.char_count();
        self.edit_offsets(0, end, &text, GroupKind::Command);
        self.break_undo_group();
        self.set_offset(0);
        // Like recover-file: the auto-save file stays (it's still
        // the only copy on disk until you save), and the buffer counts as
        // auto-saved, so it isn't written again straight away.
        self.auto_saved_version = self.version();
        self.save_length = Some(self.char_count());
        Ok(())
    }

    // ------------------------------------------------ positions & offsets

    /// The text as a rope (cheap to copy: it shares storage).
    pub fn rope(&self) -> Rope {
        self.text.clone()
    }

    pub fn char_count(&self) -> usize {
        self.text.len_chars()
    }

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    /// Length of a line in chars, not counting its newline.
    pub fn line_len(&self, line: usize) -> usize {
        let l = self.text.line(line);
        let n = l.len_chars();
        if n > 0 && l.char(n - 1) == '\n' {
            n - 1
        } else {
            n
        }
    }

    /// The text of one line, without its newline.
    pub fn line(&self, line: usize) -> String {
        let mut s = self.text.line(line).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    fn clamp(&self, p: Pos) -> Pos {
        let line = min(p.line, self.line_count() - 1);
        Pos::new(line, min(p.col, self.line_len(line)))
    }

    pub fn pos_to_offset(&self, p: Pos) -> usize {
        let p = self.clamp(p);
        self.text.line_to_char(p.line) + p.col
    }

    /// A pair of char offsets, ordered and both inside the buffer. Offsets
    /// past the end are pulled back to it, so the pair is never reversed
    /// and a range that lies wholly past the end collapses to the end.
    fn clamp_offsets(&self, a: usize, b: usize) -> (usize, usize) {
        let n = self.char_count();
        (min(a, b).min(n), a.max(b).min(n))
    }

    pub fn offset_to_pos(&self, offset: usize) -> Pos {
        let offset = min(offset, self.char_count());
        let line = self.text.char_to_line(offset);
        Pos::new(line, offset - self.text.line_to_char(line))
    }

    fn ordered(&self, a: Pos, b: Pos) -> (Pos, Pos) {
        let (a, b) = (self.clamp(a), self.clamp(b));
        if a <= b { (a, b) } else { (b, a) }
    }

    pub fn point(&self) -> Pos {
        self.offset_to_pos(self.point)
    }

    /// Byte offset (in the UTF-8 text) of a char offset.
    pub fn char_to_byte(&self, offset: usize) -> usize {
        self.text.char_to_byte(min(offset, self.char_count()))
    }

    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.text.byte_to_char(min(byte, self.text.len_bytes()))
    }

    pub fn point_offset(&self) -> usize {
        self.point
    }

    /// Move point (snapped to the start of a grapheme cluster).
    pub fn set_point(&mut self, p: Pos) {
        let p = self.clamp(p);
        let col = width::snap(&self.line(p.line), p.col);
        self.point = self.text.line_to_char(p.line) + col;
        self.goal_col = None;
    }

    pub fn set_offset(&mut self, offset: usize) {
        let p = self.offset_to_pos(offset);
        self.set_point(p);
    }

    pub fn start(&self) -> Pos {
        Pos::new(0, 0)
    }

    pub fn end(&self) -> Pos {
        let last = self.line_count() - 1;
        Pos::new(last, self.line_len(last))
    }

    /// The position one grapheme cluster after `p` (the next line's start
    /// at end of line).
    pub fn next_pos(&self, p: Pos) -> Option<Pos> {
        let p = self.clamp(p);
        if p.col < self.line_len(p.line) {
            let b = width::boundaries(&self.line(p.line));
            let next = b
                .into_iter()
                .find(|&c| c > p.col)
                .unwrap_or_else(|| self.line_len(p.line));
            Some(Pos::new(p.line, next))
        } else if p.line + 1 < self.line_count() {
            Some(Pos::new(p.line + 1, 0))
        } else {
            None
        }
    }

    /// The position one grapheme cluster before `p`.
    pub fn prev_pos(&self, p: Pos) -> Option<Pos> {
        let p = self.clamp(p);
        if p.col > 0 {
            let b = width::boundaries(&self.line(p.line));
            let prev = b.into_iter().rev().find(|&c| c < p.col).unwrap_or(0);
            Some(Pos::new(p.line, prev))
        } else if p.line > 0 {
            Some(Pos::new(p.line - 1, self.line_len(p.line - 1)))
        } else {
            None
        }
    }

    /// Character at `p`; line ends read as '\n'; `None` at end of buffer.
    pub fn char_at(&self, p: Pos) -> Option<char> {
        let o = self.pos_to_offset(p);
        (o < self.char_count()).then(|| self.text.char(o))
    }

    // ------------------------------------------------------------ journal

    /// Number of edits ever applied to this buffer.
    pub fn version(&self) -> u64 {
        self.journal.version()
    }

    /// Edits made since `version`, oldest first. None if the journal no
    /// longer reaches that far back (or the text was replaced wholesale).
    pub fn edits_since(&self, version: u64) -> Option<&[Edit]> {
        self.journal.since(version)
    }

    /// Carry an offset remembered at `version` across the edits since.
    pub fn transform_offset(&self, offset: usize, version: u64) -> usize {
        match self.edits_since(version) {
            Some(edits) => edits
                .iter()
                .fold(offset, |o, e| e.transform(o))
                .min(self.char_count()),
            None => offset.min(self.char_count()),
        }
    }

    // --------------------------------------------------------------- edits

    /// The one place text changes. Updates the rope, the journal, the mark
    /// and point (point ends up after the inserted text).
    fn apply(&mut self, e: Edit) {
        if let Some(s) = self.syntax.0.as_mut() {
            s.edit(&self.text, &e);
        }
        self.decorations.retain_mut(|d| d.follow(&e));
        self.links.retain_mut(|l| l.follow(&e));
        self.folds.retain_mut(|f| f.follow(&e));
        if let Some(p) = self.protected_from {
            // Typed right at its start, text goes before it.
            self.protected_from = Some(if e.deleted.is_empty() && e.at == p {
                p + e.inserted_len()
            } else {
                e.transform(p)
            });
        }
        if let Some(h) = self.protected_head {
            // Typed right at it, text goes after it (stays editable).
            self.protected_head = Some(if e.deleted.is_empty() && e.at == h {
                h
            } else {
                e.transform(h)
            });
        }
        let del = e.deleted_len();
        self.text.remove(e.at..e.at + del);
        self.text.insert(e.at, &e.inserted);
        self.mark = self.mark.map(|m| e.transform(m));
        for m in &mut self.mark_ring {
            *m = e.transform(*m);
        }
        self.point = e.at + e.inserted_len();
        self.journal.push(e);
        if !self.modified {
            self.modified = true;
            self.lock_on_first_change();
        }
        self.mark_active = false;
        self.goal_col = None;
    }

    /// Replace chars `a..b` with `s` the way Forge's
    /// with-silent-modifications does: not recorded for undo, the
    /// modified flag left as it was and no lock file taken. For text that
    /// is only there for a moment (an input method's translation in
    /// progress), which is gone again before anything else edits.
    pub fn replace_silently(&mut self, a: usize, b: usize, s: &str) {
        let b = b.min(self.char_count());
        let a = a.min(b);
        let e = Edit {
            at: a,
            deleted: self.text.slice(a..b).to_string(),
            inserted: s.to_string(),
        };
        if (e.deleted.is_empty() && e.inserted.is_empty()) || self.refuse_edit_at(a, b - a) {
            return;
        }
        let modified = self.modified;
        // Marked modified for apply's sake, which would otherwise take
        // the file's lock.
        self.modified = true;
        self.apply(e);
        self.modified = modified;
    }

    /// Replace chars `a..b` with `s` and record it for undo.
    fn edit_offsets(&mut self, a: usize, b: usize, s: &str, kind: GroupKind) -> bool {
        let e = Edit {
            at: a,
            deleted: self.text.slice(a..b).to_string(),
            inserted: s.to_string(),
        };
        if e.deleted.is_empty() && e.inserted.is_empty() {
            return true;
        }
        if self.refuse_edit_at(a, b - a) {
            return false;
        }
        let before = self.point;
        self.record_undo(&e, before, kind);
        self.apply(e);
        if let Some(g) = self.undo.last_mut() {
            g.point_after = self.point;
        }
        true
    }

    fn record_undo(&mut self, e: &Edit, point_before: usize, kind: GroupKind) {
        // The saved text was further back than here (in the redo list,
        // which this edit throws away): undo can't get back to it.
        if self.saved_depth.is_some_and(|d| self.undo.len() < d) {
            self.saved_depth = None;
        }
        self.redo.clear();
        let join = match (self.open_group, kind) {
            (Some(GroupKind::Typing(n)), GroupKind::Typing(_)) if n < INSERT_GROUP_LIMIT => {
                self.open_group = Some(GroupKind::Typing(n + 1));
                true
            }
            (Some(GroupKind::Command), GroupKind::Command) => true,
            _ => false,
        };
        if join && self.saved_depth == Some(self.undo.len()) {
            // Growing the step the saved text ends with.
            self.saved_depth = None;
        }
        match self.undo.last_mut() {
            Some(g) if join => {
                // Typing: grow the last insertion instead of adding an entry.
                match g.edits.last_mut() {
                    Some(last)
                        if last.deleted.is_empty()
                            && e.deleted.is_empty()
                            && last.at + last.inserted_len() == e.at =>
                    {
                        last.inserted.push_str(&e.inserted)
                    }
                    _ => g.edits.push(e.clone()),
                }
            }
            _ => {
                self.undo.push(UndoGroup {
                    edits: vec![e.clone()],
                    point_before,
                    point_after: point_before,
                });
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                    self.saved_depth = self.saved_depth.and_then(|d| d.checked_sub(1));
                }
                self.open_group = Some(match kind {
                    GroupKind::Typing(_) => GroupKind::Typing(1),
                    k => k,
                });
            }
        }
    }

    /// Ends the current undo step: the next edit starts a new one. The
    /// editor calls this after every command, so one command is one step.
    pub fn break_undo_group(&mut self) {
        self.open_group = None;
    }

    /// Is there anything to undo? (`buffer-undo-list` has an entry.)
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Is there anything to redo: has the last change been undone?
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        if self.refuse_edit() {
            return false;
        }
        self.break_undo_group();
        let Some(g) = self.undo.pop() else {
            return false;
        };
        for e in g.edits.iter().rev() {
            self.apply(e.inverse());
        }
        self.set_offset(g.point_before);
        self.redo.push(g);
        self.after_undo_step();
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.refuse_edit() {
            return false;
        }
        self.break_undo_group();
        let Some(g) = self.redo.pop() else {
            return false;
        };
        for e in &g.edits {
            self.apply(e.clone());
        }
        self.set_offset(g.point_after);
        self.undo.push(g);
        self.after_undo_step();
        true
    }

    /// Undo every change since the text was saved (one step if the saved
    /// text can't be reached). False if nothing could be undone.
    pub fn undo_to_saved(&mut self) -> bool {
        match self.saved_depth {
            Some(d) if d <= self.undo.len() => {
                while self.undo.len() > d {
                    if !self.undo() {
                        return false;
                    }
                }
                true
            }
            _ => self.undo(),
        }
    }

    /// Undo or redo landed on the saved text: the buffer is unmodified
    /// again (and lets go of its lock).
    fn after_undo_step(&mut self) {
        if self.saved_depth == Some(self.undo.len()) {
            self.set_modified(false);
        }
    }

    // --------------------------------------------------------------- mark

    pub fn mark(&self) -> Option<Pos> {
        self.mark.map(|m| self.offset_to_pos(m))
    }

    pub fn mark_active(&self) -> bool {
        self.mark_active
    }

    /// Set the mark. `activate` highlights the region (transient-mark-mode).
    /// The old mark, if any and different, goes on the mark ring.
    pub fn push_mark(&mut self, p: Pos, activate: bool) {
        let offset = self.pos_to_offset(p);
        if let Some(old) = self.mark
            && old != offset
        {
            self.mark_ring.retain(|m| *m != old);
            self.mark_ring.insert(0, old);
            self.mark_ring.truncate(mark_ring_max());
        }
        self.mark = Some(offset);
        self.mark_active = activate;
    }

    /// Older marks, newest first (not including the current mark), as
    /// (char offset, position).
    pub fn mark_ring(&self) -> Vec<(usize, Pos)> {
        self.mark_ring
            .iter()
            .map(|o| (*o, self.offset_to_pos(*o)))
            .collect()
    }

    /// `pop-mark`: take the newest entry on the ring as the new
    /// mark, and put the mark we had at the end of the ring, so the
    /// positions we walk through are not lost. Point does not move and
    /// the mark is not activated. False if the ring is empty.
    pub fn pop_mark(&mut self) -> bool {
        if self.mark_ring.is_empty() {
            return false;
        }
        if let Some(old) = self.mark {
            self.mark_ring.push(old);
        }
        let next = self.mark_ring.remove(0);
        self.mark = Some(next);
        self.mark_active = false;
        true
    }

    /// `pop-to-mark-command`: jump to the mark, then pop the
    /// next entry off the ring into the mark, so the next `C-u C-SPC`
    /// goes further back. False if no mark is set.
    pub fn pop_to_mark(&mut self) -> bool {
        let Some(m) = self.mark else { return false };
        self.set_offset(m);
        self.deactivate_mark();
        self.pop_mark();
        true
    }

    pub fn deactivate_mark(&mut self) {
        self.mark_active = false;
    }

    /// Region between point and mark (whether highlighted or not), ordered.
    pub fn region(&self) -> Option<(Pos, Pos)> {
        self.mark().map(|m| self.ordered(m, self.point()))
    }

    /// Region only while it's highlighted.
    pub fn active_region(&self) -> Option<(Pos, Pos)> {
        if self.mark_active {
            self.region()
        } else {
            None
        }
    }

    pub fn exchange_point_and_mark(&mut self) -> bool {
        let Some(m) = self.mark else { return false };
        let p = self.point;
        self.set_offset(m);
        self.mark = Some(p);
        self.mark_active = true;
        true
    }

    // --------------------------------------------------------------- text

    pub fn text_range(&self, a: Pos, b: Pos) -> String {
        let (a, b) = self.ordered(a, b);
        self.text
            .slice(self.pos_to_offset(a)..self.pos_to_offset(b))
            .to_string()
    }

    pub fn text_offsets(&self, a: usize, b: usize) -> String {
        let (a, b) = self.clamp_offsets(a, b);
        self.text.slice(a..b).to_string()
    }

    /// Type a character at point. Runs of typed characters undo together.
    pub fn insert_char(&mut self, ch: char) {
        let mut b = [0u8; 4];
        let p = self.point;
        self.edit_offsets(p, p, ch.encode_utf8(&mut b), GroupKind::Typing(1));
    }

    /// Insert text (may contain newlines) at point, leaving point after it.
    pub fn insert_str(&mut self, s: &str) {
        let p = self.point;
        self.edit_offsets(p, p, s, GroupKind::Command);
    }

    pub fn insert_newline(&mut self) {
        self.insert_str("\n");
    }

    /// C-o: insert a newline after point without moving.
    pub fn open_line(&mut self) {
        let p = self.point;
        self.insert_newline();
        self.point = p;
    }

    /// Delete text between two positions and return it. Point goes to the
    /// start. A refused edit deletes nothing and returns an empty string,
    /// so a caller that kills the text does not put text that is still in
    /// the buffer on the kill ring.
    pub fn delete_range(&mut self, a: Pos, b: Pos) -> String {
        let (a, b) = self.ordered(a, b);
        let (a, b) = (self.pos_to_offset(a), self.pos_to_offset(b));
        let text = self.text.slice(a..b).to_string();
        if self.edit_offsets(a, b, "", GroupKind::Command) {
            text
        } else {
            String::new()
        }
    }

    /// Replace text between two positions; point ends after the new text.
    pub fn replace_range(&mut self, a: Pos, b: Pos, s: &str) {
        let (a, b) = self.ordered(a, b);
        let (a, b) = (self.pos_to_offset(a), self.pos_to_offset(b));
        self.edit_offsets(a, b, s, GroupKind::Command);
    }

    /// Replace chars `a..b` (offsets); point ends after the new text.
    pub fn replace_offsets(&mut self, a: usize, b: usize, s: &str) {
        self.try_replace_offsets(a, b, s);
    }

    /// Replace chars `a..b` (offsets); point ends after the new text.
    /// Returns false if the edit was refused and the buffer is unchanged,
    /// so a caller that keeps a copy of the text can keep it in step.
    pub fn try_replace_offsets(&mut self, a: usize, b: usize, s: &str) -> bool {
        let (a, b) = self.clamp_offsets(a, b);
        self.edit_offsets(a, b, s, GroupKind::Command)
    }

    /// Delete the grapheme cluster after point (joins lines at end of line).
    pub fn delete_char(&mut self) -> bool {
        let p = self.point();
        match self.next_pos(p) {
            Some(n) => {
                self.delete_range(p, n);
                true
            }
            None => false,
        }
    }

    /// Delete the grapheme cluster before point.
    pub fn backspace(&mut self) -> bool {
        let p = self.point();
        match self.prev_pos(p) {
            Some(prev) => {
                self.delete_range(prev, p);
                true
            }
            None => false,
        }
    }

    /// C-t: swap the grapheme clusters around point (at end of line, the
    /// last two) and move past them.
    pub fn transpose_chars(&mut self) -> bool {
        let p = self.point();
        let len = self.line_len(p.line);
        if p.col == 0 || len < 2 {
            return false;
        }
        let at = if p.col >= len {
            match self.prev_pos(p) {
                Some(q) => q,
                None => return false,
            }
        } else {
            p
        };
        let (Some(left), Some(right)) = (self.prev_pos(at), self.next_pos(at)) else {
            return false;
        };
        if left.line != at.line || right.line != at.line {
            return false;
        }
        let (a, b) = (self.text_range(left, at), self.text_range(at, right));
        self.replace_range(left, right, &format!("{b}{a}"));
        true
    }

    // ----------------------------------------------------------- movement

    pub fn move_left(&mut self) -> bool {
        match self.prev_pos(self.point()) {
            Some(p) => {
                self.set_point(p);
                true
            }
            None => false,
        }
    }

    pub fn move_right(&mut self) -> bool {
        match self.next_pos(self.point()) {
            Some(p) => {
                self.set_point(p);
                true
            }
            None => false,
        }
    }

    /// Vertical motion keeps the display column (the goal column).
    fn move_vertically(&mut self, down: bool) -> bool {
        let p = self.point();
        if (down && p.line + 1 >= self.line_count()) || (!down && p.line == 0) {
            return false;
        }
        let goal = self
            .goal_col
            .unwrap_or_else(|| width::display_col(&self.line(p.line), p.col));
        // Hidden (folded) lines are stepped over.
        let line = if down {
            self.next_visible_line(p.line)
        } else {
            self.prev_visible_line(p.line)
        };
        let Some(line) = line else { return false };
        let col = width::col_at_display(&self.line(line), goal);
        self.point = self.text.line_to_char(line) + col;
        self.goal_col = Some(goal);
        true
    }

    pub fn move_up(&mut self) -> bool {
        self.move_vertically(false)
    }

    pub fn move_down(&mut self) -> bool {
        self.move_vertically(true)
    }

    /// Move `n` lines down (negative = up). Returns how many lines moved.
    pub fn move_lines(&mut self, n: isize) -> usize {
        let mut moved = 0;
        for _ in 0..n.unsigned_abs() {
            let ok = if n > 0 {
                self.move_down()
            } else {
                self.move_up()
            };
            if !ok {
                break;
            }
            moved += 1;
        }
        moved
    }

    pub fn move_line_start(&mut self) {
        let line = self.point().line;
        self.point = self.text.line_to_char(line);
        self.goal_col = None;
    }

    pub fn move_line_end(&mut self) {
        let line = self.point().line;
        self.point = self.text.line_to_char(line) + self.line_len(line);
        self.goal_col = None;
    }

    pub fn forward_word(&mut self) -> bool {
        let start = self.point;
        let n = self.char_count();
        let mut o = start;
        while o < n && !is_word_char(self.text.char(o)) {
            o += 1;
        }
        while o < n && is_word_char(self.text.char(o)) {
            o += 1;
        }
        self.set_offset(o);
        o != start
    }

    pub fn backward_word(&mut self) -> bool {
        let start = self.point;
        let mut o = start;
        while o > 0 && !is_word_char(self.text.char(o - 1)) {
            o -= 1;
        }
        while o > 0 && is_word_char(self.text.char(o - 1)) {
            o -= 1;
        }
        self.set_offset(o);
        o != start
    }

    pub fn beginning_of_buffer(&mut self) {
        self.set_point(self.start());
    }

    pub fn end_of_buffer(&mut self) {
        self.set_point(self.end());
    }

    /// 1-based, like goto-line.
    pub fn goto_line(&mut self, line: usize) {
        self.set_point(Pos::new(line.saturating_sub(1), 0));
    }

    // ------------------------------------------------------------- search

    /// Find `query` at or after `from`. Returns the match's (start, end).
    /// Case-insensitive unless the query contains an uppercase letter
    ///.
    pub fn search_forward(&self, query: &str, from: Pos) -> Option<(Pos, Pos)> {
        let q: Vec<char> = query.chars().collect();
        if q.is_empty() || q.contains(&'\n') {
            return None;
        }
        let fold = !query.chars().any(char::is_uppercase);
        let from = self.clamp(from);
        for li in from.line..self.line_count() {
            let chars: Vec<char> = self.text.line(li).chars().filter(|c| *c != '\n').collect();
            if chars.len() < q.len() {
                continue;
            }
            let start = if li == from.line { from.col } else { 0 };
            for c in start..=chars.len() - q.len() {
                if chars[c..c + q.len()]
                    .iter()
                    .zip(&q)
                    .all(|(a, b)| chars_eq(*a, *b, fold))
                {
                    return Some((Pos::new(li, c), Pos::new(li, c + q.len())));
                }
            }
        }
        None
    }

    /// Find the last match of `query` that ends at or before `from`.
    pub fn search_backward(&self, query: &str, from: Pos) -> Option<(Pos, Pos)> {
        let q: Vec<char> = query.chars().collect();
        if q.is_empty() || q.contains(&'\n') {
            return None;
        }
        let fold = !query.chars().any(char::is_uppercase);
        let from = self.clamp(from);
        for li in (0..=from.line).rev() {
            let chars: Vec<char> = self.text.line(li).chars().filter(|c| *c != '\n').collect();
            let limit = if li == from.line {
                from.col
            } else {
                chars.len()
            };
            let Some(max_start) = limit.checked_sub(q.len()) else {
                continue;
            };
            for c in (0..=max_start).rev() {
                if chars[c..c + q.len()]
                    .iter()
                    .zip(&q)
                    .all(|(a, b)| chars_eq(*a, *b, fold))
                {
                    return Some((Pos::new(li, c), Pos::new(li, c + q.len())));
                }
            }
        }
        None
    }

    // ---------------------------------------------------------- accessors

    pub fn get_contents(&self) -> String {
        self.text.to_string()
    }

    /// Replace everything. Resets point, mark, undo history and modified
    /// flag; positions remembered elsewhere go back to the start.
    pub fn set_contents(&mut self, text: &str) {
        let text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text.to_string()
        };
        self.text = Rope::from_str(&text);
        self.point = 0;
        self.goal_col = None;
        self.mark = None;
        self.mark_active = false;
        self.undo.clear();
        self.redo.clear();
        self.open_group = None;
        self.journal.reset();
        self.auto_saved_version = self.version();
        self.saved_depth = Some(0);
        self.save_length = Some(self.char_count());
        self.set_modified(false);
        if let Some(s) = self.syntax.0.as_mut() {
            s.reset();
        }
        self.decorations.clear();
        self.links.clear();
        self.folds.clear();
        self.crypt_cache.clear();
        self.protected_from = None;
        self.protected_head = None;
    }

    /// (line, column) — column is in characters.
    pub fn cursor_pos(&self) -> (usize, usize) {
        let p = self.point();
        (p.line, p.col)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Successful saves so far.
    pub fn save_count(&self) -> u64 {
        self.save_count
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Mark the buffer modified or not. Becoming modified locks the
    /// visited file; becoming unmodified unlocks it.
    pub fn set_modified(&mut self, modified: bool) {
        match (self.modified, modified) {
            (false, true) => {
                self.modified = true;
                self.lock_on_first_change();
            }
            (true, false) => {
                self.modified = false;
                self.unlock();
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- modes

    pub fn major_mode(&self) -> &str {
        &self.major_mode
    }

    pub fn set_major_mode(&mut self, mode: impl Into<String>) {
        self.major_mode = mode.into();
    }

    /// Enabled minor modes, highest keymap priority first.
    pub fn minor_modes(&self) -> &[String] {
        &self.minor_modes
    }

    pub fn minor_mode_enabled(&self, mode: &str) -> bool {
        self.minor_modes.iter().any(|m| m == mode)
    }

    /// Enable a minor mode. It goes to the front, so its keymap wins over
    /// minor modes enabled earlier.
    pub fn enable_minor_mode(&mut self, mode: impl Into<String>) {
        let mode = mode.into();
        self.minor_modes.retain(|m| *m != mode);
        self.minor_modes.insert(0, mode);
    }

    /// Returns false if it wasn't enabled.
    pub fn disable_minor_mode(&mut self, mode: &str) -> bool {
        let before = self.minor_modes.len();
        self.minor_modes.retain(|m| m != mode);
        self.minor_modes.len() != before
    }
}

/// Write `bytes` to `path`, creating it with `mode` (on Unix) and giving
/// an existing file that mode too.
fn write_with_mode(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(m);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(m));
    }
    #[cfg(not(unix))]
    let _ = mode;
    f.write_all(bytes)
}

/// `file~`
pub fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push("~");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        let mut b = Buffer::new("t");
        b.set_contents(text);
        b
    }

    fn lines(b: &Buffer) -> Vec<String> {
        (0..b.line_count()).map(|i| b.line(i)).collect()
    }

    #[test]
    fn offsets_past_the_end_are_pulled_back() {
        let mut b = buf("0123456789");
        assert_eq!(b.text_offsets(100, 200), "");
        assert_eq!(b.text_offsets(4, 100), "456789");
        b.replace_offsets(8, 100, ".");
        assert_eq!(b.get_contents(), "01234567.");
        b.replace_offsets(100, 200, "x");
        assert_eq!(
            b.get_contents(),
            "01234567.x",
            "the range collapses to the end"
        );
    }

    #[test]
    fn a_refused_delete_returns_no_text() {
        let mut b = buf("hello world");
        b.set_locked(true);
        assert_eq!(b.delete_range(Pos::new(0, 0), Pos::new(0, 5)), "");
        assert_eq!(b.get_contents(), "hello world", "the text is still there");
        assert!(b.take_refused());
    }

    #[test]
    fn insert_chars() {
        let mut b = Buffer::new("t");
        b.insert_char('a');
        b.insert_char('b');
        assert_eq!(b.get_contents(), "ab");
    }

    #[test]
    fn newline_splits_line() {
        let mut b = Buffer::new("t");
        b.insert_char('a');
        b.insert_newline();
        b.insert_char('b');
        assert_eq!(b.get_contents(), "a\nb");
        assert_eq!(b.cursor_pos(), (1, 1));
        assert_eq!(lines(&b), ["a", "b"]);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut b = buf("ab\ncd");
        b.move_down();
        b.backspace();
        assert_eq!(b.get_contents(), "abcd");
        assert_eq!(b.cursor_pos(), (0, 2));
    }

    #[test]
    fn delete_at_eol_joins_lines() {
        let mut b = buf("ab\ncd");
        b.move_line_end();
        b.delete_char();
        assert_eq!(b.get_contents(), "abcd");
    }

    #[test]
    fn unicode_is_safe() {
        let mut b = Buffer::new("t");
        b.insert_char('é');
        b.insert_char('ü');
        b.move_left();
        b.insert_char('x');
        assert_eq!(b.get_contents(), "éxü");
        b.backspace();
        b.backspace();
        assert_eq!(b.get_contents(), "ü");
    }

    #[test]
    fn point_moves_by_grapheme_cluster() {
        // e + combining acute, a flag (two code points), a family (ZWJ sequence).
        let mut b = buf("e\u{301}🇹🇹👨\u{200d}👩\u{200d}👧!");
        b.move_right();
        assert_eq!(b.point(), Pos::new(0, 2));
        b.move_right();
        assert_eq!(b.point(), Pos::new(0, 4));
        b.move_right();
        assert_eq!(b.point(), Pos::new(0, 9));
        b.backspace();
        assert_eq!(b.get_contents(), "e\u{301}🇹🇹!");
        b.move_line_start();
        b.delete_char();
        assert_eq!(b.get_contents(), "🇹🇹!");
        // set_point never lands inside a cluster.
        b.set_point(Pos::new(0, 1));
        assert_eq!(b.point(), Pos::new(0, 0));
        b.set_point(Pos::new(0, 2));
        b.transpose_chars();
        assert_eq!(b.get_contents(), "!🇹🇹");
    }

    #[test]
    fn arrows_wrap_across_lines() {
        let mut b = buf("ab\ncd");
        b.move_line_end();
        b.move_right();
        assert_eq!(b.cursor_pos(), (1, 0));
        b.move_left();
        assert_eq!(b.cursor_pos(), (0, 2));
    }

    #[test]
    fn goal_column_survives_short_lines() {
        let mut b = buf("hello\nx\nworld");
        b.move_line_end();
        b.move_down();
        assert_eq!(b.point(), Pos::new(1, 1));
        b.move_down();
        assert_eq!(b.point(), Pos::new(2, 5));
    }

    #[test]
    fn goal_column_is_a_display_column() {
        // Moving down from after "漢" (display column 2) lands after "ab".
        let mut b = buf("漢字\nabcd\n\tx");
        b.set_point(Pos::new(0, 1));
        b.move_down();
        assert_eq!(b.point(), Pos::new(1, 2));
        b.move_down();
        assert_eq!(b.point(), Pos::new(2, 0), "inside a tab snaps to its start");
    }

    #[test]
    fn range_across_lines() {
        let mut b = buf("one\ntwo\nthree");
        let (a, z) = (Pos::new(0, 1), Pos::new(2, 2));
        assert_eq!(b.text_range(a, z), "ne\ntwo\nth");
        assert_eq!(b.delete_range(z, a), "ne\ntwo\nth");
        assert_eq!(b.get_contents(), "oree");
        assert_eq!(b.point(), a);
    }

    #[test]
    fn insert_multiline_string() {
        let mut b = buf("ad");
        b.set_point(Pos::new(0, 1));
        b.insert_str("b\nc");
        assert_eq!(b.get_contents(), "ab\ncd");
        assert_eq!(b.point(), Pos::new(1, 1));
    }

    #[test]
    fn word_motion() {
        let mut b = buf("  foo_bar, baz\nqux");
        b.forward_word();
        assert_eq!(b.point(), Pos::new(0, 9));
        b.forward_word();
        assert_eq!(b.point(), Pos::new(0, 14));
        b.forward_word();
        assert_eq!(b.point(), Pos::new(1, 3));
        b.backward_word();
        assert_eq!(b.point(), Pos::new(1, 0));
        b.backward_word();
        assert_eq!(b.point(), Pos::new(0, 11));
    }

    #[test]
    fn a_locked_buffer_refuses_edits_but_its_owner_writes() {
        let mut b = Buffer::new("t");
        b.insert_str("listing\n");
        b.set_locked(true);
        assert!(b.read_only());
        b.insert_str("typed");
        assert_eq!(b.get_contents(), "listing\n");
        assert!(b.take_refused());
        assert!(!b.take_refused(), "the flag clears");
        assert!(!b.undo());
        assert!(b.take_refused());
        b.set_modified(false);
        b.as_owner(|b| b.replace_offsets(0, 7, "fresh"));
        assert_eq!(b.get_contents(), "fresh\n");
        assert!(
            !b.is_modified(),
            "the owner's rewrite isn't an unsaved change"
        );
        assert!(!b.take_refused());
        b.set_locked(false);
        b.set_offset(0);
        b.insert_str("x");
        assert_eq!(b.get_contents(), "xfresh\n");
    }

    #[test]
    fn a_protected_head_refuses_edits_before_it() {
        let mut b = buf("output\n$ ");
        b.set_protected_head(Some(8));
        b.replace_offsets(0, 6, "X");
        assert_eq!(b.get_contents(), "output\n$ ");
        assert!(b.take_refused());
        b.replace_offsets(6, 9, "X");
        assert_eq!(b.get_contents(), "output\n$ ");
        assert!(b.take_refused());
        // At and after it: fine, and the head tracks the text.
        b.replace_offsets(8, 8, "ls");
        assert_eq!(b.get_contents(), "output\n$ls ");
        assert_eq!(b.protected_head(), Some(8));
        assert!(!b.take_refused());
        b.replace_offsets(11, 11, "x");
        assert_eq!(b.get_contents(), "output\n$ls x");
        // The owner still rewrites anywhere (shell output insertion).
        b.as_owner(|b| b.replace_offsets(0, 0, "more\n"));
        assert_eq!(b.get_contents(), "more\noutput\n$ls x");
        assert_eq!(b.protected_head(), Some(13));
    }

    #[test]
    fn typing_undoes_as_a_group() {
        let mut b = Buffer::new("t");
        for c in "hello".chars() {
            b.insert_char(c);
        }
        b.break_undo_group();
        b.insert_char('!');
        assert!(b.undo());
        assert_eq!(b.get_contents(), "hello");
        assert!(b.undo());
        assert_eq!(b.get_contents(), "");
        assert!(!b.undo());
        assert!(b.redo());
        assert_eq!(b.get_contents(), "hello");
    }

    #[test]
    fn long_typing_splits_every_20_chars() {
        let mut b = Buffer::new("t");
        for _ in 0..25 {
            b.insert_char('x');
        }
        b.undo();
        assert_eq!(b.get_contents().len(), 20);
    }

    #[test]
    fn one_command_is_one_undo_step_and_restores_point() {
        let mut b = buf("abc def");
        b.move_line_end();
        b.delete_range(Pos::new(0, 0), Pos::new(0, 4));
        b.set_point(Pos::new(0, 0));
        b.insert_str(">> ");
        b.break_undo_group();
        assert_eq!(b.get_contents(), ">> def");
        b.undo();
        assert_eq!(b.get_contents(), "abc def");
        assert_eq!(b.point(), Pos::new(0, 7), "back where the command started");
        b.redo();
        assert_eq!(b.get_contents(), ">> def");
        assert_eq!(b.point(), Pos::new(0, 3));
    }

    #[test]
    fn undo_stores_diffs_not_snapshots() {
        let mut b = buf(&"x".repeat(100_000));
        for _ in 0..50 {
            b.insert_char('y');
            b.break_undo_group();
        }
        let stored: usize = b
            .undo
            .iter()
            .flat_map(|g| &g.edits)
            .map(|e| e.inserted.len() + e.deleted.len())
            .sum();
        assert_eq!(stored, 50);
    }

    #[test]
    fn journal_moves_remembered_positions() {
        let mut b = buf("hello world");
        let v = b.version();
        let world = 6;
        b.set_point(Pos::new(0, 0));
        b.insert_str(">> ");
        assert_eq!(b.transform_offset(world, v), 9);
        b.delete_range(Pos::new(0, 3), Pos::new(0, 9));
        assert_eq!(b.transform_offset(world, v), 3);
        assert_eq!(b.edits_since(v).unwrap().len(), 2);
        // Undo is recorded as edits too. A position that was inside deleted
        // text stays where the deletion happened.
        b.break_undo_group();
        let v2 = b.version();
        b.undo();
        assert_eq!(b.edits_since(v2).unwrap().len(), 2);
        assert_eq!(b.get_contents(), "hello world");
        // After a wholesale replacement old positions are clamped.
        b.set_contents("hi");
        assert_eq!(b.transform_offset(world, v), 2);
        assert!(b.edits_since(v).is_none());
    }

    #[test]
    fn mark_follows_edits() {
        let mut b = buf("abc");
        b.push_mark(Pos::new(0, 2), false);
        b.set_point(Pos::new(0, 0));
        b.insert_str("xy");
        assert_eq!(b.mark(), Some(Pos::new(0, 4)));
    }

    #[test]
    fn push_mark_keeps_a_ring_and_pop_walks_it() {
        let mut b = buf("abcdef");
        b.set_point(Pos::new(0, 1));
        b.push_mark(Pos::new(0, 1), true);
        b.set_point(Pos::new(0, 3));
        b.push_mark(Pos::new(0, 3), true);
        // The old mark (col 1) is on the ring, newest first.
        assert_eq!(
            b.mark_ring().iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            vec![Pos::new(0, 1)]
        );
        // Popping takes the ring's newest entry as the new mark and
        // leaves the mark we had at the end of the ring, so the
        // positions we walk through are not lost. Point does not move.
        assert!(b.pop_mark());
        assert_eq!(b.mark(), Some(Pos::new(0, 1)));
        assert_eq!(b.point(), Pos::new(0, 3));
        assert!(!b.mark_active());
        assert_eq!(
            b.mark_ring().iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            vec![Pos::new(0, 3)]
        );
        // The ring is not empty: the mark we had is at the end of it,
        // so popping again walks the ring round.
        assert!(b.pop_mark());
        assert_eq!(b.mark(), Some(Pos::new(0, 3)));
        assert_eq!(b.point(), Pos::new(0, 3));
        assert_eq!(
            b.mark_ring().iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            vec![Pos::new(0, 1)]
        );
    }

    #[test]
    fn pop_to_mark_jumps_to_the_mark_and_pops_the_next_one() {
        let mut b = buf("abcdef");
        b.set_point(Pos::new(0, 0));
        b.push_mark(Pos::new(0, 0), true);
        b.set_point(Pos::new(0, 2));
        b.push_mark(Pos::new(0, 2), true);
        b.set_point(Pos::new(0, 5));
        // C-u C-SPC: point goes to the mark, and the next entry on the
        // ring becomes the mark.
        assert!(b.pop_to_mark());
        assert_eq!(b.point(), Pos::new(0, 2));
        assert_eq!(b.mark(), Some(Pos::new(0, 0)));
        assert!(!b.mark_active());
        // No mark at all: nothing to jump to.
        let mut b = buf("abc");
        assert!(!b.pop_to_mark());
    }

    #[test]
    fn the_mark_ring_keeps_as_many_marks_as_mark_ring_max_says() {
        let old = mark_ring_max();
        set_mark_ring_max(3);
        let mut b = buf("abcdefgh");
        for col in 0..6 {
            b.set_point(Pos::new(0, col));
            b.push_mark(Pos::new(0, col), true);
        }
        // The three newest older marks are kept, the earliest are gone.
        assert_eq!(
            b.mark_ring().iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            vec![Pos::new(0, 4), Pos::new(0, 3), Pos::new(0, 2)]
        );
        set_mark_ring_max(old);
    }

    #[test]
    fn the_mark_ring_follows_edits() {
        let mut b = buf("abc");
        b.push_mark(Pos::new(0, 1), true);
        b.set_point(Pos::new(0, 2));
        b.push_mark(Pos::new(0, 2), true);
        // Insert before both marks: they slide right with the text.
        b.set_point(Pos::new(0, 0));
        b.insert_str("xy");
        let (offset, pos) = b.mark_ring()[0];
        assert_eq!(offset, 3);
        assert_eq!(pos, Pos::new(0, 3));
    }

    #[test]
    fn search_smart_case() {
        let b = buf("Foo foo\nFOO");
        assert_eq!(
            b.search_forward("foo", Pos::new(0, 1)),
            Some((Pos::new(0, 4), Pos::new(0, 7)))
        );
        assert_eq!(
            b.search_forward("FOO", Pos::new(0, 0)),
            Some((Pos::new(1, 0), Pos::new(1, 3)))
        );
        assert_eq!(
            b.search_backward("foo", Pos::new(1, 0)),
            Some((Pos::new(0, 4), Pos::new(0, 7)))
        );
        assert_eq!(b.search_backward("foo", Pos::new(0, 2)), None);
    }

    #[test]
    fn transpose() {
        let mut b = buf("abc");
        b.set_point(Pos::new(0, 1));
        b.transpose_chars();
        assert_eq!(b.get_contents(), "bac");
        b.move_line_end();
        b.transpose_chars();
        assert_eq!(b.get_contents(), "bca");
    }

    #[test]
    fn large_buffer_edits_are_cheap() {
        let line = "the quick brown fox jumps over the lazy dog\n";
        let mut b = buf(&line.repeat(200_000));
        let t = std::time::Instant::now();
        b.set_point(Pos::new(100_000, 5));
        for _ in 0..1000 {
            b.insert_char('x');
            b.move_down();
        }
        assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
        assert_eq!(
            b.line(100_000),
            "the qxuick brown fox jumps over the lazy dog"
        );
    }

    /// A base directory for test files that is not under a temporary
    /// directory (files there get no backups). The checkout's own
    /// target/ is used unless the checkout itself sits in one, as in a sandbox
    /// under /tmp; then a directory under $HOME or the current user's cache is.
    fn non_temp_base() -> std::path::PathBuf {
        let temps = crate::backup::default_temporary_dirs();
        let ok = |p: &std::path::Path| {
            let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            !temps.iter().any(|t| p.starts_with(t))
        };
        let mut cands =
            vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-files")];
        if let Some(h) = std::env::var_os("HOME") {
            cands.push(std::path::PathBuf::from(h).join(".cache/forge-test-files"));
        }
        cands.push(std::path::PathBuf::from("/var/tmp-forge-test-files"));
        for c in cands {
            if std::fs::create_dir_all(&c).is_ok() && ok(&c) {
                return c;
            }
        }
        panic!("no directory outside the temporary directories for test files");
    }

    #[test]
    fn file_round_trip_backup_and_auto_save() {
        // Not in the temporary directory: files there get no backups.
        let dir = non_temp_base().join(format!("forge-buf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "line one\r\nline two\r\n").unwrap();
        let mut b = Buffer::from_file(&path).unwrap();
        assert_eq!(lines(&b), ["line one", "line two", ""]);
        assert!(!b.changed_on_disk());
        b.insert_char('>');
        assert!(b.needs_auto_save());
        b.auto_save().unwrap();
        assert!(dir.join("#f.txt#").exists());
        assert!(!b.needs_auto_save());
        b.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            ">line one\r\nline two\r\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt~")).unwrap(),
            "line one\r\nline two\r\n"
        );
        assert!(
            !dir.join("#f.txt#").exists(),
            "saving removes the auto-save file"
        );

        // Someone else changes the file; revert picks it up as one undo step.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "new text, longer than before\n").unwrap();
        assert!(b.changed_on_disk());
        b.revert().unwrap();
        assert_eq!(b.get_contents(), "new text, longer than before\n");
        assert!(!b.is_modified() && !b.changed_on_disk());
        b.undo();
        assert_eq!(b.get_contents(), ">line one\nline two\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_save_as_keeps_visiting_the_old_file() {
        let dir = non_temp_base().join(format!("forge-save-as-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.txt");
        std::fs::write(&old, "hello\n").unwrap();
        let mut b = Buffer::from_file(&old).unwrap();
        b.insert_char('!');
        // A path whose parent directory doesn't exist: the write must fail.
        let bad = dir.join("missing").join("new.txt");
        assert!(b.save_as(&bad).is_err());
        assert_eq!(
            b.path(),
            Some(old.as_path()),
            "a failed save-as still visits the old file"
        );
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "hello\n",
            "the old file is untouched"
        );
        assert!(b.is_modified(), "a failed save leaves the buffer modified");
        assert!(!bad.exists());
        // A later plain save still goes to the old file, not to nowhere:
        // the failure must not have retargeted the buffer.
        b.save().unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), b.get_contents());
        assert!(!b.is_modified());
        assert!(!bad.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_as_to_a_new_path_keeps_crlf() {
        let dir = non_temp_base().join(format!("forge-eol-as-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("win.txt");
        std::fs::write(&src, "a\r\nb\r\n").unwrap();
        let mut b = Buffer::from_file(&src).unwrap();
        let dst = dir.join("copy.txt");
        b.save_as(&dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "a\r\nb\r\n",
            "save-as writes the file's CRLF endings to the new path"
        );
        assert_eq!(b.path(), Some(dst.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mixed_line_endings_never_double_carriage_returns() {
        let dir = non_temp_base().join(format!("forge-eol-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mixed.txt");
        std::fs::write(&path, "a\r\nb\nc\r\n").unwrap();
        let mut b = Buffer::from_file(&path).unwrap();
        assert_eq!(
            b.get_contents(),
            "a\nb\nc\n",
            "CRLF is normalized in the buffer"
        );
        // A pasted CRLF (or any stray CR) must not survive as `\r\r\n`.
        b.insert_str("x\r\ny\r");
        b.save().unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("\r\r"),
            "no doubled carriage returns: {on_disk:?}"
        );
        assert_eq!(on_disk, "x\r\ny\ra\r\nb\r\nc\r\n");
        // Saving the same buffer again doesn't churn the endings.
        b.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn auto_save_files_go_away_when_they_are_no_longer_needed() {
        let dir = std::env::temp_dir().join(format!("forge-as-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("g.txt");
        let auto = dir.join("#g.txt#");
        std::fs::write(&path, "one\n").unwrap();

        // Reverting throws the changes away, so the auto-save file goes too.
        let mut b = Buffer::from_file(&path).unwrap();
        b.insert_char('>');
        b.auto_save().unwrap();
        assert!(auto.exists());
        b.revert().unwrap();
        assert!(!auto.exists(), "revert discards the auto-save file");
        assert!(
            !b.has_newer_auto_save(),
            "a reverted buffer has nothing left to recover"
        );

        // Recovering pulls the text into the buffer; it is not offered again.
        let mut b = Buffer::from_file(&path).unwrap();
        b.insert_char('>');
        b.auto_save().unwrap();
        assert!(b.has_newer_auto_save());
        b.recover_auto_save().unwrap();
        assert_eq!(b.get_contents(), ">one\n");
        assert!(
            auto.exists(),
            "recovering keeps the auto-save file until you save"
        );
        assert!(b.is_modified());
        assert!(
            !b.needs_auto_save(),
            "recovered text is not immediately re-auto-saved"
        );
        b.save().unwrap();
        assert!(!auto.exists(), "saving the recovered text removes it");

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn lock_test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-buflock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lock_exists(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    #[test]
    fn the_lock_comes_with_the_first_change_and_goes_with_the_last() {
        let dir = lock_test_dir("life");
        let path = dir.join("f.txt");
        let lock = dir.join(".#f.txt");
        std::fs::write(&path, "text\n").unwrap();

        let mut b = Buffer::from_file(&path).unwrap();
        assert!(!lock_exists(&lock), "visiting doesn't lock");
        b.insert_char('x');
        assert!(lock_exists(&lock), "the first change locks");
        assert!(b.holds_file_lock());
        b.save().unwrap();
        assert!(!lock_exists(&lock), "saving unlocks");

        b.insert_char('y');
        assert!(lock_exists(&lock));
        b.undo();
        assert!(
            !b.is_modified(),
            "undo back to the saved text is unmodified"
        );
        assert!(!lock_exists(&lock), "...and unlocks");
        b.redo();
        assert!(b.is_modified() && lock_exists(&lock), "redo modifies again");

        b.revert().unwrap();
        assert!(!lock_exists(&lock), "reverting unlocks");
        b.insert_char('z');
        assert!(lock_exists(&lock));
        drop(b);
        assert!(!lock_exists(&lock), "killing the buffer unlocks");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undo_past_the_save_and_diverging_forgets_the_saved_state() {
        let mut b = Buffer::new("t");
        b.insert_str("a");
        b.break_undo_group();
        b.set_modified(false);
        b.mark_saved();
        b.insert_str("b");
        b.break_undo_group();
        b.undo();
        assert!(!b.is_modified());
        b.undo();
        assert!(b.is_modified(), "before the save is a change too");
        b.insert_str("c");
        b.break_undo_group();
        b.undo();
        assert!(b.is_modified(), "the saved text can't be reached any more");
    }

    #[test]
    fn a_foreign_lock_is_asked_about_once_at_the_first_change() {
        let dir = lock_test_dir("foreign");
        let path = dir.join("g.txt");
        let lock = dir.join(".#g.txt");
        std::fs::write(&path, "text\n").unwrap();
        #[cfg(unix)]
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let theirs = format!("other@{}.{}", crate::filelock::local_hostname(), child.id());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&theirs, &lock).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&lock, &theirs).unwrap();

        let mut b = Buffer::from_file(&path).unwrap();
        assert!(
            b.take_lock_question().is_none(),
            "nothing asked on visiting"
        );
        b.insert_char('x');
        let owner = b.take_lock_question().expect("asked at the first change");
        assert_eq!(owner.raw, theirs);
        assert!(b.take_lock_question().is_none(), "asked once");
        b.insert_char('y');
        assert!(b.take_lock_question().is_none());

        // p: proceed without it; the lock is left alone.
        b.ignore_file_lock();
        assert!(!b.holds_file_lock());
        assert_eq!(crate::filelock::read_lock(&lock).unwrap(), theirs);
        // Saved and changed again: asked again.
        b.save().unwrap();
        b.insert_char('z');
        assert!(b.take_lock_question().is_some());
        b.steal_file_lock().unwrap();
        assert!(b.holds_file_lock());
        assert_eq!(
            crate::filelock::read_lock(&lock).unwrap(),
            crate::filelock::our_lock_text()
        );
        drop(b);
        assert!(!lock_exists(&lock));
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_locks_when_create_lockfiles_is_off() {
        let dir = lock_test_dir("off");
        let path = dir.join("h.txt");
        std::fs::write(&path, "text\n").unwrap();
        let mut b = Buffer::from_file(&path).unwrap();
        b.set_lockfiles(Some(false));
        b.insert_char('x');
        assert!(!lock_exists(&dir.join(".#h.txt")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn auto_save_keeps_the_files_permissions_and_stops_after_a_big_deletion() {
        let dir = lock_test_dir("autosave");
        let path = dir.join("big.txt");
        std::fs::write(&path, "x".repeat(10_000)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut b = Buffer::from_file(&path).unwrap();
        b.insert_char('y');
        b.auto_save().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("#big.txt#"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "a private file's auto-save is private");
        }
        assert!(!b.shrunk_a_lot());
        b.replace_offsets(0, 9_000, "");
        assert!(b.shrunk_a_lot(), "lost 90% of 10000 characters");
        b.disable_auto_save();
        assert!(!b.needs_auto_save());
        b.save().unwrap();
        assert!(b.auto_save_enabled(), "a real save turns it back on");
        b.insert_char('z');
        assert!(b.needs_auto_save());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
