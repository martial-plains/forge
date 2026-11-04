// SPDX-License-Identifier: MIT OR Apache-2.0
//! Decorations: faces plugins put on buffer text (a misspelled word, a
//! compiler error), and buffer options (line numbers, indentation).
//!
//! Decorations are anchored to the text: they move with edits like point
//! and mark do, and one whose text is deleted disappears. Each belongs
//! to whoever set it (a plugin's name), which replaces or clears only its
//! own.

use crate::buffer::Edit;
use crate::syntax::SyntaxFace;

/// How a decoration looks. Frontends pick the colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoFace {
    Error,
    Warning,
    Info,
    /// A word the spell checker doesn't know.
    SpellError,
    /// Stands out, like a search match.
    Highlight,
    /// The other occurrences of the symbol at point (Eglot's
    /// eglot-highlight-symbol-face): bold.
    SymbolHighlight,
    /// The file name above a file's matches in the *grep* buffer
    ///.
    GrepHeading,
    /// Underlined: an input method's text
    /// still being typed.
    Underline,
    /// No color of its own: the text is left as it is. A decoration that
    /// is only here for its keymap (see [`Decoration::keymap`]) is one of
    /// these, and so is a name a Dired listing has no type for.
    Plain,
    /// Any syntax face (for semantic highlighting from a plugin).
    Syntax(SyntaxFace),
    /// Dired's columns and file types (see [`DiredFace`]).
    Dired(DiredFace),
    /// Brackets coloured by nesting depth (see [`RainbowFace`]).
    Rainbow(RainbowFace),
    /// Version control: diffs, logs, the status listing, conflicts.
    Vc(VcFace),
    /// org-modern's labels and symbols (see [`ModernFace`]).
    Modern(ModernFace),
    /// A terminal cell's own colours and attributes (see [`TermFace`]).
    Term(TermFace),
}

/// What the `term` emulator paints a run of cells with. Colours are
/// resolved to RGB by the emulator (`None` is the theme's default), so a
/// frontend needs no palette of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TermFace {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    /// [`TermFace::BOLD`] and friends, or-ed together.
    pub attrs: u8,
}

impl TermFace {
    pub const BOLD: u8 = 1;
    pub const DIM: u8 = 2;
    pub const ITALIC: u8 = 4;
    pub const UNDERLINE: u8 = 8;
    pub const REVERSE: u8 = 16;
    pub const STRIKE: u8 = 32;

    /// The name frontends receive: `term:<fg>:<bg>:<attrs>`, colours as
    /// six hex digits or `-`, attributes as hex.
    pub fn encode(self) -> String {
        let c = |c: Option<u32>| c.map_or("-".to_string(), |v| format!("{:06x}", v & 0xff_ffff));
        format!("term:{}:{}:{:x}", c(self.fg), c(self.bg), self.attrs)
    }

    pub fn decode(name: &str) -> Option<Self> {
        let mut it = name.strip_prefix("term:")?.split(':');
        let c = |s: &str| {
            if s == "-" {
                Some(None)
            } else {
                u32::from_str_radix(s, 16).ok().map(Some)
            }
        };
        let fg = c(it.next()?)?;
        let bg = c(it.next()?)?;
        let attrs = u8::from_str_radix(it.next()?, 16).ok()?;
        Some(Self { fg, bg, attrs })
    }
}

/// The faces of the version-control buffers, named (diff-mode,
/// log-view, vc-dir, log-edit, smerge, vc-annotate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VcFace {
    DiffAdded,
    DiffRemoved,
    DiffContext,
    /// `diff --git`, `index`, and the like.
    DiffHeader,
    /// `--- a/file` and `+++ b/file`.
    DiffFileHeader,
    /// `@@ -1,3 +1,4 @@`.
    DiffHunkHeader,
    /// The characters that changed inside a changed line.
    DiffRefineAdded,
    DiffRefineRemoved,
    /// `\ No newline at end of file`, and other notes.
    DiffNote,
    /// A log entry's first line, and the revision in it.
    LogViewMessage,
    LogViewRevision,
    LogViewAuthor,
    LogViewDate,
    LogViewBody,
    /// Branch and tag names in a log.
    LogViewRef,
    /// The header lines of the status listing, and their values.
    VcDirHeader,
    VcDirHeaderValue,
    VcDirDirectory,
    VcDirFile,
    VcDirMark,
    VcDirEdited,
    VcDirAdded,
    VcDirUpToDate,
    /// Conflicts, missing files.
    VcDirWarning,
    VcDirIgnored,
    VcDirUnregistered,
    /// A commit message's first line, too-long parts of it, and headers.
    LogEditSummary,
    LogEditOverlong,
    LogEditHeader,
    LogEditUnknownHeader,
    /// Read-only text in a commit buffer (the files being committed).
    LogEditFiles,
    /// The three sides of a conflict, its markers, and refined changes.
    SmergeUpper,
    SmergeLower,
    SmergeBase,
    SmergeMarkers,
    SmergeRefinedAdded,
    SmergeRefinedRemoved,
    /// Annotate lines by age: 1 is the newest, ANNOTATE_AGES the oldest.
    AnnotateAge(u8),
}

/// How many age faces annotate uses.
pub const ANNOTATE_AGES: u8 = 10;

impl VcFace {
    pub const ALL: [VcFace; 39] = [
        Self::DiffAdded,
        Self::DiffRemoved,
        Self::DiffContext,
        Self::DiffHeader,
        Self::DiffFileHeader,
        Self::DiffHunkHeader,
        Self::DiffRefineAdded,
        Self::DiffRefineRemoved,
        Self::DiffNote,
        Self::LogViewMessage,
        Self::LogViewRevision,
        Self::LogViewAuthor,
        Self::LogViewDate,
        Self::LogViewBody,
        Self::LogViewRef,
        Self::VcDirHeader,
        Self::VcDirHeaderValue,
        Self::VcDirDirectory,
        Self::VcDirFile,
        Self::VcDirMark,
        Self::VcDirEdited,
        Self::VcDirAdded,
        Self::VcDirUpToDate,
        Self::VcDirWarning,
        Self::VcDirIgnored,
        Self::VcDirUnregistered,
        Self::LogEditSummary,
        Self::LogEditOverlong,
        Self::LogEditHeader,
        Self::LogEditUnknownHeader,
        Self::LogEditFiles,
        Self::SmergeUpper,
        Self::SmergeLower,
        Self::SmergeBase,
        Self::SmergeMarkers,
        Self::SmergeRefinedAdded,
        Self::SmergeRefinedRemoved,
        Self::AnnotateAge(1),
        Self::AnnotateAge(ANNOTATE_AGES),
    ];

    const AGE_NAMES: [&'static str; ANNOTATE_AGES as usize] = [
        "vc-annotate-age-1",
        "vc-annotate-age-2",
        "vc-annotate-age-3",
        "vc-annotate-age-4",
        "vc-annotate-age-5",
        "vc-annotate-age-6",
        "vc-annotate-age-7",
        "vc-annotate-age-8",
        "vc-annotate-age-9",
        "vc-annotate-age-10",
    ];

    /// Every name, for themes to list.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::ALL
            .into_iter()
            .filter(|f| !matches!(f, Self::AnnotateAge(_)))
            .map(Self::name)
            .chain(Self::AGE_NAMES)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::DiffAdded => "diff-added",
            Self::DiffRemoved => "diff-removed",
            Self::DiffContext => "diff-context",
            Self::DiffHeader => "diff-header",
            Self::DiffFileHeader => "diff-file-header",
            Self::DiffHunkHeader => "diff-hunk-header",
            Self::DiffRefineAdded => "diff-refine-added",
            Self::DiffRefineRemoved => "diff-refine-removed",
            Self::DiffNote => "diff-nonexistent",
            Self::LogViewMessage => "log-view-message",
            Self::LogViewRevision => "log-view-revision",
            Self::LogViewAuthor => "log-view-author",
            Self::LogViewDate => "log-view-date",
            Self::LogViewBody => "log-view-commit-body",
            Self::LogViewRef => "log-view-ref",
            Self::VcDirHeader => "vc-dir-header",
            Self::VcDirHeaderValue => "vc-dir-header-value",
            Self::VcDirDirectory => "vc-dir-directory",
            Self::VcDirFile => "vc-dir-file",
            Self::VcDirMark => "vc-dir-mark-indicator",
            Self::VcDirEdited => "vc-dir-status-edited",
            Self::VcDirAdded => "vc-dir-status-added",
            Self::VcDirUpToDate => "vc-dir-status-up-to-date",
            Self::VcDirWarning => "vc-dir-status-warning",
            Self::VcDirIgnored => "vc-dir-status-ignored",
            Self::VcDirUnregistered => "vc-dir-status-unregistered",
            Self::LogEditSummary => "log-edit-summary",
            Self::LogEditOverlong => "log-edit-overlong",
            Self::LogEditHeader => "log-edit-header",
            Self::LogEditUnknownHeader => "log-edit-unknown-header",
            Self::LogEditFiles => "log-edit-files",
            Self::SmergeUpper => "smerge-upper",
            Self::SmergeLower => "smerge-lower",
            Self::SmergeBase => "smerge-base",
            Self::SmergeMarkers => "smerge-markers",
            Self::SmergeRefinedAdded => "smerge-refined-added",
            Self::SmergeRefinedRemoved => "smerge-refined-removed",
            Self::AnnotateAge(n) => Self::AGE_NAMES[usize::from(n.clamp(1, ANNOTATE_AGES)) - 1],
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(i) = Self::AGE_NAMES.iter().position(|n| *n == name) {
            return Some(Self::AnnotateAge(i as u8 + 1));
        }
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// The faces org-modern draws labels and symbols with: a keyword, tag or
/// date in a box of its own, a progress bar, a bullet. Named after
/// org-modern's, so a theme can restyle them (`org-modern-todo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModernFace {
    Label,
    Todo,
    Done,
    Priority,
    Tag,
    DateActive,
    TimeActive,
    DateInactive,
    TimeInactive,
    ProgressComplete,
    ProgressIncomplete,
    BlockName,
    /// Stars, bullets and checkboxes: the text's own color, as symbols.
    Symbol,
    HorizontalRule,
    Target,
}

impl ModernFace {
    pub const ALL: [ModernFace; 15] = [
        Self::Label,
        Self::Todo,
        Self::Done,
        Self::Priority,
        Self::Tag,
        Self::DateActive,
        Self::TimeActive,
        Self::DateInactive,
        Self::TimeInactive,
        Self::ProgressComplete,
        Self::ProgressIncomplete,
        Self::BlockName,
        Self::Symbol,
        Self::HorizontalRule,
        Self::Target,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Label => "org-modern-label",
            Self::Todo => "org-modern-todo",
            Self::Done => "org-modern-done",
            Self::Priority => "org-modern-priority",
            Self::Tag => "org-modern-tag",
            Self::DateActive => "org-modern-date-active",
            Self::TimeActive => "org-modern-time-active",
            Self::DateInactive => "org-modern-date-inactive",
            Self::TimeInactive => "org-modern-time-inactive",
            Self::ProgressComplete => "org-modern-progress-complete",
            Self::ProgressIncomplete => "org-modern-progress-incomplete",
            Self::BlockName => "org-modern-block-name",
            Self::Symbol => "org-modern-symbol",
            Self::HorizontalRule => "org-modern-horizontal-rule",
            Self::Target => "org-modern-target",
        }
    }

    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::ALL.into_iter().map(Self::name)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// What a display decoration shows in place of its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display {
    /// Nothing: the text is hidden (org-modern's `invisible`).
    Hide,
    /// This text instead (org-modern's `display` property).
    Text(String),
}

/// The faces of rainbow-delimiters: one per nesting level, and two for
/// closing brackets that don't close anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RainbowFace {
    /// Nesting depth 1 to [`RAINBOW_DEPTHS`]; 1 is the outermost.
    Depth(u8),
    /// A closing bracket with nothing to close.
    Unmatched,
    /// A closing bracket of the wrong kind, like the `]` in `(]`.
    Mismatched,
}

/// How many depth faces there are.
pub const RAINBOW_DEPTHS: u8 = 9;

impl RainbowFace {
    const DEPTH_NAMES: [&'static str; RAINBOW_DEPTHS as usize] = [
        "rainbow-depth-1",
        "rainbow-depth-2",
        "rainbow-depth-3",
        "rainbow-depth-4",
        "rainbow-depth-5",
        "rainbow-depth-6",
        "rainbow-depth-7",
        "rainbow-depth-8",
        "rainbow-depth-9",
    ];

    /// Every face, for themes to list.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::DEPTH_NAMES
            .into_iter()
            .chain(["rainbow-unmatched", "rainbow-mismatched"])
    }

    pub fn name(self) -> &'static str {
        match self {
            // Out of range (not made by from_name) clamps to the ends.
            Self::Depth(n) => Self::DEPTH_NAMES[usize::from(n.clamp(1, RAINBOW_DEPTHS)) - 1],
            Self::Unmatched => "rainbow-unmatched",
            Self::Mismatched => "rainbow-mismatched",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "rainbow-unmatched" => Some(Self::Unmatched),
            "rainbow-mismatched" => Some(Self::Mismatched),
            _ => Self::DEPTH_NAMES
                .iter()
                .position(|n| *n == name)
                .map(|i| Self::Depth(i as u8 + 1)),
        }
    }
}

/// The faces of a Dired buffer, which look like eza's long listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiredFace {
    /// The directory on the first line.
    Header,
    /// "Permissions Size User Date Modified Name".
    ColumnHeader,
    Directory,
    Executable,
    Symlink,
    /// A file marked with `m`.
    Marked,
    /// A file flagged for deletion with `d`.
    Flagged,
    PermRead,
    PermWrite,
    PermExec,
    PermDir,
    PermLink,
    /// Dashes, dots and empty sizes.
    Dim,
    /// Bytes and kilobytes.
    Size,
    SizeMega,
    SizeGiga,
    SizeHuge,
    User,
    /// The group (the `ls` style).
    Group,
    Date,
    Inode,
    /// The `-> target` after a symlink's name.
    SymlinkTarget,
    /// The `├── ` prefixes of a tree listing.
    TreePrefix,
    /// What a file is by name and extension, like eza's file types.
    Image,
    Video,
    Music,
    Lossless,
    Crypto,
    Document,
    Compressed,
    Temp,
    Compiled,
    Build,
    Source,
    /// Git status letters: new, modified, deleted, renamed, type change,
    /// conflicted and ignored.
    GitNew,
    GitModified,
    GitDeleted,
    GitRenamed,
    GitTypeChange,
    GitConflicted,
    GitIgnored,
    /// A shade computed from the listing's own range of sizes or dates,
    /// packed as 0xRRGGBB. The gradient is eza's `--color-scale`, so this
    /// is the only face whose colour isn't a fixed name.
    Shade(u32),
}

impl DiredFace {
    /// Every face with a fixed name. [`Shade`](Self::Shade) is left out: it
    /// is a computed colour rather than one a theme can name.
    pub const ALL: [DiredFace; 41] = [
        Self::Header,
        Self::ColumnHeader,
        Self::Directory,
        Self::Executable,
        Self::Symlink,
        Self::Marked,
        Self::Flagged,
        Self::PermRead,
        Self::PermWrite,
        Self::PermExec,
        Self::PermDir,
        Self::PermLink,
        Self::Dim,
        Self::Size,
        Self::SizeMega,
        Self::SizeGiga,
        Self::SizeHuge,
        Self::User,
        Self::Group,
        Self::Date,
        Self::Inode,
        Self::SymlinkTarget,
        Self::TreePrefix,
        Self::Image,
        Self::Video,
        Self::Music,
        Self::Lossless,
        Self::Crypto,
        Self::Document,
        Self::Compressed,
        Self::Temp,
        Self::Compiled,
        Self::Build,
        Self::Source,
        Self::GitNew,
        Self::GitModified,
        Self::GitDeleted,
        Self::GitRenamed,
        Self::GitTypeChange,
        Self::GitConflicted,
        Self::GitIgnored,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Header => "dired-header",
            Self::ColumnHeader => "dired-column-header",
            Self::Directory => "dired-directory",
            Self::Executable => "dired-executable",
            Self::Symlink => "dired-symlink",
            Self::Marked => "dired-marked",
            Self::Flagged => "dired-flagged",
            Self::PermRead => "dired-perm-read",
            Self::PermWrite => "dired-perm-write",
            Self::PermExec => "dired-perm-exec",
            Self::PermDir => "dired-perm-dir",
            Self::PermLink => "dired-perm-link",
            Self::Dim => "dired-dim",
            Self::Size => "dired-size",
            Self::SizeMega => "dired-size-mega",
            Self::SizeGiga => "dired-size-giga",
            Self::SizeHuge => "dired-size-huge",
            Self::User => "dired-user",
            Self::Group => "dired-group",
            Self::Date => "dired-date",
            Self::Inode => "dired-inode",
            Self::SymlinkTarget => "dired-symlink-target",
            Self::TreePrefix => "dired-tree-prefix",
            Self::Image => "dired-image",
            Self::Video => "dired-video",
            Self::Music => "dired-music",
            Self::Lossless => "dired-lossless",
            Self::Crypto => "dired-crypto",
            Self::Document => "dired-document",
            Self::Compressed => "dired-compressed",
            Self::Temp => "dired-temp",
            Self::Compiled => "dired-compiled",
            Self::Build => "dired-build",
            Self::Source => "dired-source",
            Self::GitNew => "dired-git-new",
            Self::GitModified => "dired-git-modified",
            Self::GitDeleted => "dired-git-deleted",
            Self::GitRenamed => "dired-git-renamed",
            Self::GitTypeChange => "dired-git-typechange",
            Self::GitConflicted => "dired-git-conflicted",
            Self::GitIgnored => "dired-git-ignored",
            Self::Shade(_) => "dired-shade",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }
}

impl DecoFace {
    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::SpellError => "spell-error",
            Self::Highlight => "highlight",
            Self::SymbolHighlight => "symbol-highlight",
            Self::GrepHeading => "grep-heading",
            Self::Underline => "underline",
            Self::Plain => "plain",
            Self::Syntax(f) => f.name(),
            Self::Dired(f) => f.name(),
            Self::Rainbow(f) => f.name(),
            Self::Vc(f) => f.name(),
            Self::Modern(f) => f.name(),
            Self::Term(_) => "term",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, String> {
        Ok(match name {
            "error" => Self::Error,
            "warning" => Self::Warning,
            "info" => Self::Info,
            "spell-error" => Self::SpellError,
            "highlight" => Self::Highlight,
            "symbol-highlight" => Self::SymbolHighlight,
            "grep-heading" => Self::GrepHeading,
            "underline" => Self::Underline,
            "plain" => Self::Plain,
            _ => match SyntaxFace::from_name(name)
                .map(Self::Syntax)
                .or_else(|| DiredFace::from_name(name).map(Self::Dired))
                .or_else(|| RainbowFace::from_name(name).map(Self::Rainbow))
                .or_else(|| VcFace::from_name(name).map(Self::Vc))
                .or_else(|| ModernFace::from_name(name).map(Self::Modern))
            {
                Some(f) => f,
                None => {
                    let syntax: Vec<&str> = SyntaxFace::ALL.iter().map(|f| f.name()).collect();
                    return Err(format!(
                        "unknown face {name:?}: use error, warning, info, spell-error, highlight, plain, rainbow-depth-1 to {}, rainbow-unmatched, rainbow-mismatched or {}",
                        RAINBOW_DEPTHS,
                        syntax.join(", ")
                    ));
                }
            },
        })
    }
}

/// One decoration to set: where it is, how it looks, and what it says.
///
/// The four in a row (start, end, face, message) are the usual case, so
/// they convert from a tuple; a decoration that carries a keymap is
/// written out or built with [`DecoSpec::keymap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoSpec {
    /// Char offsets, `start < end`.
    pub start: usize,
    pub end: usize,
    pub face: DecoFace,
    /// Shown in the echo area when point is on it.
    pub message: Option<String>,
    /// The region map this text brings with it: its keys are searched
    /// while point is inside the decoration (see
    /// [`Scope::Region`](crate::keymap::Scope::Region)).
    pub keymap: Option<String>,
    /// Shown in place of the text, on every line but point's.
    pub display: Option<Display>,
}

impl DecoSpec {
    pub fn new(start: usize, end: usize, face: DecoFace) -> Self {
        Self {
            start,
            end,
            face,
            message: None,
            keymap: None,
            display: None,
        }
    }

    /// Show `display` instead of the text (the face colors what is shown).
    pub fn display(mut self, display: Display) -> Self {
        self.display = Some(display);
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Like [`DecoSpec::message`], but for a message that may be absent.
    pub fn message_opt(mut self, message: Option<String>) -> Self {
        self.message = message;
        self
    }

    /// Like [`DecoSpec::keymap`], but for a keymap that may be absent.
    pub fn keymap_opt(mut self, keymap: Option<String>) -> Self {
        self.keymap = keymap;
        self
    }

    /// A decoration with no color of its own, here for its keymap.
    pub fn region(start: usize, end: usize, keymap: impl Into<String>) -> Self {
        Self::new(start, end, DecoFace::Plain).keymap(keymap)
    }

    pub fn keymap(mut self, keymap: impl Into<String>) -> Self {
        self.keymap = Some(keymap.into());
        self
    }
}

impl From<(usize, usize, DecoFace, Option<String>)> for DecoSpec {
    fn from((start, end, face, message): (usize, usize, DecoFace, Option<String>)) -> Self {
        Self {
            start,
            end,
            face,
            message,
            keymap: None,
            display: None,
        }
    }
}

/// A decoration: a face on a stretch of text, and what it brings with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    /// Who set it (a plugin's name, "init.rhai").
    pub owner: String,
    /// Char offsets, `start < end`.
    pub start: usize,
    pub end: usize,
    pub face: DecoFace,
    /// Shown in the echo area when point is on it.
    pub message: Option<String>,
    /// The region map that applies while point is on this text, so a
    /// stretch of buffer can have keys of its own (a Dired entry, an org
    /// link). See [`Scope::Region`](crate::keymap::Scope::Region).
    pub keymap: Option<String>,
    /// Shown in place of the text (see [`Display`]).
    pub display: Option<Display>,
}

impl Decoration {
    /// Follow an edit. False when the decorated text is gone.
    pub(crate) fn follow(&mut self, e: &Edit) -> bool {
        let deleted = e.deleted.chars().count();
        let inserted = e.inserted.chars().count();
        // Text inserted right at the start goes before it, not into it.
        self.start = if deleted == 0 && self.start == e.at {
            self.start + inserted
        } else {
            e.transform(self.start)
        };
        self.end = e.transform(self.end);
        self.start < self.end
    }

    /// Is `offset` on this decoration?
    pub fn covers(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// How line numbers are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineNumbers {
    #[default]
    Off,
    Absolute,
    /// The current line's number, and distances for the others (handy
    /// with prefix arguments: `C-u 7 C-n`).
    Relative,
}

impl LineNumbers {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Absolute => "absolute",
            Self::Relative => "relative",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s {
            "off" | "false" => Ok(Self::Off),
            "absolute" | "on" | "true" => Ok(Self::Absolute),
            "relative" => Ok(Self::Relative),
            _ => Err(format!(
                "line-numbers is off, absolute or relative, not {s:?}"
            )),
        }
    }
}

/// What happens to lines wider than the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wrap {
    /// Cut off (`$`), and the window scrolls sideways.
    #[default]
    Off,
    /// Continued on the next screen row at any character.
    Char,
    /// Continued at word boundaries (visual-line-mode).
    Word,
}

impl Wrap {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Char => "char",
            Self::Word => "word",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s {
            "off" | "false" => Ok(Self::Off),
            "char" => Ok(Self::Char),
            "word" | "true" | "on" => Ok(Self::Word),
            _ => Err(format!("wrap is off, char or word, not {s:?}")),
        }
    }
}

/// How the cursor looks (modal editing shows the state with it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Box,
    /// A thin vertical bar (insert state).
    Bar,
    /// An underline.
    Underline,
}

impl CursorShape {
    pub fn name(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Bar => "bar",
            Self::Underline => "hbar",
        }
    }

    pub fn from_name(s: &str) -> Result<Self, String> {
        match s {
            "box" => Ok(Self::Box),
            "bar" => Ok(Self::Bar),
            "hbar" | "underline" => Ok(Self::Underline),
            _ => Err(format!("cursor is box, bar or hbar, not {s:?}")),
        }
    }
}

/// Where display-fill-column-indicator draws its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillColumnColumn {
    /// `t`: at fill-column.
    FillColumn,
    /// A column of its own.
    Column(usize),
    /// Any other value: no line.
    Off,
}

impl FillColumnColumn {
    pub fn from_value(v: &str) -> Result<Self, String> {
        match v {
            "t" | "true" | "on" => Ok(Self::FillColumn),
            "nil" | "off" | "false" => Ok(Self::Off),
            _ => parse_number("display-fill-column-indicator-column", v, 1..=10_000)
                .map(Self::Column),
        }
    }

    pub fn value(self) -> String {
        match self {
            Self::FillColumn => "t".to_string(),
            Self::Column(n) => n.to_string(),
            Self::Off => "nil".to_string(),
        }
    }
}

/// The character display-fill-column-indicator draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillColumnChar {
    /// A single character.
    Char(char),
    /// `nil`: draw nothing.
    Off,
}

impl FillColumnChar {
    pub fn from_value(v: &str) -> Result<Self, String> {
        match v {
            "nil" | "off" => Ok(Self::Off),
            _ => {
                let mut chars = v.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(Self::Char(c)),
                    _ => Err(format!(
                        "display-fill-column-indicator-character is one character, not {v:?}"
                    )),
                }
            }
        }
    }

    pub fn value(self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::Off => "nil".to_string(),
        }
    }
}

/// Per-buffer settings. Unset ones use
/// the default (the frontend's, or the major mode's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferOptions {
    pub line_numbers: Option<LineNumbers>,
    /// Columns per indentation level.
    pub indent_width: Option<usize>,
    /// Indent with tabs.
    pub indent_tabs: Option<bool>,
    pub wrap: Option<Wrap>,
    /// Highlight the line point is on (hl-line-mode).
    pub hl_line: Option<bool>,
    /// A line at fill-column (display-fill-column-indicator-mode).
    pub fill_column_indicator: Option<bool>,
    /// Where that line goes (display-fill-column-indicator-column).
    pub fill_column_indicator_column: Option<FillColumnColumn>,
    /// The character it is drawn with
    /// (display-fill-column-indicator-character).
    pub fill_column_indicator_character: Option<FillColumnChar>,
    /// Blanks at the ends of lines stand out.
    pub show_trailing_whitespace: Option<bool>,
    /// Spaces and tabs are drawn visibly (whitespace-mode).
    pub whitespace: Option<bool>,
    /// On save: remove trailing blanks (from .editorconfig).
    pub trim_trailing_whitespace: Option<bool>,
    /// On save: make sure the file ends with a newline.
    pub insert_final_newline: Option<bool>,
    /// How the cursor looks in this buffer.
    pub cursor: Option<CursorShape>,
    /// Electric Quote mode in this buffer (electric-quote-local-mode).
    pub electric_quote: Option<bool>,
}

/// The options [`Buffer::set_option`](crate::Buffer::set_option) knows.
pub const OPTIONS: &[&str] = &[
    "cursor",
    "display-fill-column-indicator",
    "display-fill-column-indicator-character",
    "display-fill-column-indicator-column",
    "electric-quote",
    "fill-column",
    "hl-line",
    "indent-tabs",
    "indent-width",
    "insert-final-newline",
    "line-numbers",
    "read-only",
    "show-trailing-whitespace",
    "trim-trailing-whitespace",
    "whitespace",
    "wrap",
];

pub(crate) fn parse_bool(name: &str, v: &str) -> Result<bool, String> {
    match v {
        "true" | "on" | "yes" => Ok(true),
        "false" | "off" | "no" => Ok(false),
        _ => Err(format!("{name} is true or false, not {v:?}")),
    }
}

pub(crate) fn parse_number(
    name: &str,
    v: &str,
    range: std::ops::RangeInclusive<usize>,
) -> Result<usize, String> {
    v.parse::<usize>()
        .ok()
        .filter(|n| range.contains(n))
        .ok_or_else(|| {
            format!(
                "{name} is a number from {} to {}, not {v:?}",
                range.start(),
                range.end()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deco(start: usize, end: usize) -> Decoration {
        Decoration {
            owner: "p".into(),
            start,
            end,
            face: DecoFace::Error,
            message: None,
            keymap: None,
            display: None,
        }
    }

    fn edit(at: usize, deleted: &str, inserted: &str) -> Edit {
        Edit {
            at,
            deleted: deleted.into(),
            inserted: inserted.into(),
        }
    }

    #[test]
    fn decorations_follow_edits() {
        let mut d = deco(5, 10);
        assert!(d.follow(&edit(0, "", "ab")));
        assert_eq!((d.start, d.end), (7, 12));
        assert!(
            d.follow(&edit(7, "", "x")),
            "insert at the start: before it"
        );
        assert_eq!((d.start, d.end), (8, 13));
        assert!(d.follow(&edit(13, "", "x")), "insert at the end: after it");
        assert_eq!((d.start, d.end), (8, 13));
        assert!(d.follow(&edit(10, "", "yy")), "insert inside: grows");
        assert_eq!((d.start, d.end), (8, 15));
        assert!(
            !d.follow(&edit(6, "0123456789ab", "")),
            "deleted with its text"
        );
    }

    #[test]
    fn names() {
        assert_eq!(DecoFace::from_name("spell-error"), Ok(DecoFace::SpellError));
        assert_eq!(
            DecoFace::from_name("rainbow-depth-3"),
            Ok(DecoFace::Rainbow(RainbowFace::Depth(3)))
        );
        assert_eq!(
            DecoFace::from_name("rainbow-mismatched"),
            Ok(DecoFace::Rainbow(RainbowFace::Mismatched))
        );
        assert!(DecoFace::from_name("rainbow-depth-0").is_err());
        assert!(DecoFace::from_name("rainbow-depth-10").is_err());
        for n in RainbowFace::names() {
            assert_eq!(DecoFace::from_name(n).map(DecoFace::name), Ok(n));
        }
        assert_eq!(
            DecoFace::from_name("keyword"),
            Ok(DecoFace::Syntax(SyntaxFace::Keyword))
        );
        assert!(
            DecoFace::from_name("red")
                .unwrap_err()
                .contains("spell-error")
        );
        assert_eq!(
            LineNumbers::from_name("relative"),
            Ok(LineNumbers::Relative)
        );
        assert!(LineNumbers::from_name("x").is_err());
    }
}
