// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dired, the directory editor: M-x dired (C-x d), C-x C-j, `forge DIR`,
//! and visiting a directory with C-x C-f.
//!
//! A Dired buffer is plain text in `dired-mode` (derived from special-mode)
//! laid out as a detailed long listing, with a mark column in front:
//!
//! ```text
//!   /home/me/project:
//!   Permissions Size User Date Modified Name
//!   drwxr-xr-x     - me    4 Oct 21:46  crates
//! * .rw-r--r--  1.4k me   29 Sep 10:33  Cargo.toml
//! D .rwxr-xr-x  340  me   29 Sep 10:33  run.sh
//! ```
//!
//! The first line holds the directory, so a buffer needs no state of its
//! own. How the listing is laid out goes there too: `s` cycles `--sort`,
//! `i`/`T` switch `--tree`, and `a`, `G`, `I`, `c`, `r`, `h`, `F`, `/`,
//! `B` (plus `l` for `--level`) each turn their part the other way round,
//! and a listing with all of them at their defaults writes only
//! the colon above. Anything that isn't the default is listed in brackets,
//! so reverting and reopening keep the whole layout.
//!
//! Colors are decorations (`DiredFace`) computed from the text, so a mark
//! edit just redecorates. That is also what lets the gradient work: the
//! size and date columns are read back out of the listing and measured
//! against each other, which is what `--color-scale` does. Names start at the
//! "Name" column and may be preceded by a Nerd Font icon
//! (`FORGE_DIRED_ICONS=0` turns them off) and followed by ` -> target` for
//! symlinks, which has a color of its own. In a tree the names start after
//! `├── ` cells instead, and each one is the path through every
//! directory above it.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use forge_core::decoration::{DecoFace, DecoSpec, DiredFace};
use forge_core::{Buffer, Pos};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const WDIRED_MODE: &str = "wdired-mode";
pub const MODE: &str = "dired-mode";
const OWNER: &str = "dired";

/// The line of the first entry (0: the directory, 1: the column headers).
const FIRST_ENTRY: usize = 2;
/// Where the mark in front of an entry goes.
const MARK_COL: usize = 0;
/// A mode string is ten characters long.
const PERMS_LEN: usize = 10;
/// "Permissions" is wider than the ten characters of a mode string.
const PERMS_W: usize = 11;
/// What "total N" ends with while Dired's details are hidden (`ls` style).
const HIDDEN_DETAILS: &str = "  (details hidden)";
const DATE_W: usize = "Date Modified".len();

/// How deep a tree goes (`--level`); a listing that
/// never came back out would swallow the buffer, so it stops here.
const TREE_MAX_DEPTH: usize = 8;

/// How a Dired buffer is ordered. `s` cycles through them (`--sort`);
/// like the rest of [`View`] the choice lives in the buffer's
/// first line, so reverting and reopening keep it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sort {
    /// Ignoring case and leading dots.
    Name,
    /// Newest first.
    Date,
    /// Largest first (directories count as empty).
    Size,
    /// By extension, then by name.
    Extension,
}

impl Sort {
    fn cycled(self) -> Sort {
        match self {
            Sort::Name => Sort::Date,
            Sort::Date => Sort::Size,
            Sort::Size => Sort::Extension,
            Sort::Extension => Sort::Name,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Date => "date",
            Sort::Size => "size",
            Sort::Extension => "extension",
        }
    }
}

/// How a Dired buffer is laid out. The parts are the layout options and,
/// like the sort order, live in the buffer's first line, so reverting and
/// reopening keep them. The defaults are: dotfiles hidden, sizes
/// shaded by magnitude.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct View {
    sort: Sort,
    /// Show `.`-files (`--all`).
    all: bool,
    /// Directories before everything else (`--group-directories-first`).
    dirs_first: bool,
    /// Recurse into subdirectories and draw them as a tree (`--tree`).
    /// A tree replaces the inline subdirectories `i` inserts.
    tree: bool,
    /// The inode column (`--inode`).
    inode: bool,
    /// Shade sizes and dates by magnitude (`--color-scale`).
    scale: bool,
    /// Reverse the sort order (`--reverse`).
    reverse: bool,
    /// Hide git-ignored files (`--git-ignore`).
    git_ignore: bool,
    /// Only directories (`-D`).
    only_dirs: bool,
    /// Only files (`-f`).
    only_files: bool,
    /// Nerd Font icons before names (`--icons`; `FORGE_DIRED_ICONS=0`
    /// turns them off entirely).
    icons: bool,
    /// How deep a tree goes (`--level`).
    level: usize,
    /// The columns between the type and the name (`dired-hide-details-mode`
    /// turns them off).
    details: bool,
    /// Leave out backup and auto-save files (`dired-omit-mode`).
    omit: bool,
    /// This listing shows no entries (`dired-hide-subdir`).
    hidden: bool,
    /// `ls -l` lines, as Dired shows them (permissions, links,
    /// owner, group, bytes, date, name), instead of detailed columns.
    ls: bool,
}

impl Default for View {
    fn default() -> View {
        View {
            sort: Sort::Name,
            all: false,
            dirs_first: false,
            tree: false,
            inode: false,
            scale: false,
            reverse: false,
            git_ignore: false,
            only_dirs: false,
            only_files: false,
            icons: true,
            level: TREE_MAX_DEPTH,
            details: true,
            omit: false,
            hidden: false,
            ls: false,
        }
    }
}

impl View {
    /// What a new Dired buffer starts as: `ls -al` lines shows
    /// them, unless `forge.opt.dired_listing` is "columns".
    fn initial(app: &App) -> View {
        let columns = crate::org_flow::setting_string(app, "dired_listing")
            .is_some_and(|s| s.eq_ignore_ascii_case("columns"));
        if columns {
            return View::default();
        }
        View {
            ls: true,
            all: true,
            icons: false,
            ..View::default()
        }
    }

    /// One part turned the other way round.
    fn toggle(&self, what: &str) -> View {
        let mut v = *self;
        match what {
            "sort" => v.sort = v.sort.cycled(),
            "all" => v.all = !v.all,
            "dirs first" => v.dirs_first = !v.dirs_first,
            "tree" => v.tree = !v.tree,
            "inode" => v.inode = !v.inode,
            "scale" => v.scale = !v.scale,
            "reverse" => v.reverse = !v.reverse,
            "git ignore" => v.git_ignore = !v.git_ignore,
            "only dirs" => {
                v.only_dirs = !v.only_dirs;
                if v.only_dirs {
                    v.only_files = false;
                }
            }
            "only files" => {
                v.only_files = !v.only_files;
                if v.only_files {
                    v.only_dirs = false;
                }
            }
            "icons" => v.icons = !v.icons,
            "details" => v.details = !v.details,
            "omit" => v.omit = !v.omit,
            "hidden" => v.hidden = !v.hidden,
            "ls" => {
                v.ls = !v.ls;
                if v.ls {
                    // `dired-listing-switches` is "-al": dotfiles shown, no icons.
                    v.all = true;
                    v.icons = false;
                    v.tree = false;
                    v.inode = false;
                }
            }
            _ => {}
        }
        v
    }

    /// What goes after the directory on the first line: just a colon when
    /// everything is at its default, else a bracketed list of
    /// what was changed.
    fn suffix(self) -> String {
        let mut parts = Vec::new();
        if self.ls {
            parts.push("ls".to_string());
        }
        match self.sort {
            Sort::Name => {}
            Sort::Date => parts.push("by date".to_string()),
            Sort::Size => parts.push("by size".to_string()),
            Sort::Extension => parts.push("by extension".to_string()),
        }
        if self.all {
            parts.push("all".to_string());
        }
        if self.dirs_first {
            parts.push("dirs first".to_string());
        }
        if self.tree {
            parts.push("tree".to_string());
        }
        if self.inode {
            parts.push("inode".to_string());
        }
        if self.scale {
            parts.push("scale".to_string());
        }
        if self.reverse {
            parts.push("reverse".to_string());
        }
        if self.git_ignore {
            parts.push("git ignore".to_string());
        }
        if self.only_dirs {
            parts.push("only dirs".to_string());
        }
        if self.only_files {
            parts.push("only files".to_string());
        }
        if !self.icons {
            parts.push("no icons".to_string());
        }
        if !self.details {
            parts.push("no details".to_string());
        }
        if self.omit {
            parts.push("omit".to_string());
        }
        if self.hidden {
            parts.push("hidden".to_string());
        }
        if self.level != TREE_MAX_DEPTH {
            parts.push(format!("level={}", self.level));
        }
        match parts.is_empty() {
            true => ":".to_string(),
            false => format!(":  [{}]", parts.join(", ")),
        }
    }
}

/// The `  [by date, all, tree]` a non-default [`View`] writes.
const OPTIONS: &str = ":  [";

/// What a Dired prompt is asking for.
pub enum DiredAction {
    /// M-x dired: a directory to show.
    Open,
    /// C-x 5 d: a directory to show in another frame.
    OpenOtherFrame,
    /// C-x t d: a directory to show in another tab.
    OpenOtherTab,
    Copy(Vec<PathBuf>),
    Rename(Vec<PathBuf>),
    CreateDirectory(PathBuf),
    /// Tree depth for `--level` (1..=8).
    TreeLevel,
    /// A prompt of the commands in `extra`.
    Extra(Extra),
}

// ---------------------------------------------------- the system around

struct Env {
    /// Seconds east of UTC (from `date +%z`; UTC if that fails).
    tz: i64,
    #[cfg(unix)]
    users: HashMap<u32, String>,
    #[cfg(unix)]
    groups: HashMap<u32, String>,
}

/// The month names used in dates.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What `git status` says about the files under a repository, so each
/// listed file can get the two-letter Git column: staged, then unstaged.
struct GitInfo {
    root: PathBuf,
    /// Path from the root (and every directory above it) to its letters.
    status: HashMap<String, (char, char)>,
    /// Ignored files and directories, from the root.
    ignored: HashSet<String>,
}

impl GitInfo {
    /// The two letters for a file: staged, then unstaged ('-' for none).
    fn letters(&self, path: &Path) -> String {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return "--".into();
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        // Anything inside an ignored directory is ignored too.
        let mut prefix = String::new();
        for part in rel.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if self.ignored.contains(&prefix) {
                return "-I".into();
            }
        }
        let (staged, unstaged) = self.status.get(rel.as_str()).copied().unwrap_or(('-', '-'));
        format!("{staged}{unstaged}")
    }
}

// ------------------------------------------------------------ rendering

/// One entry's cells. The colours are not here: they are worked out from
/// this text later, so that redecorating after a mark doesn't mean reading
/// the directory again.
struct Row {
    /// Hard links, group, exact bytes, 1K blocks and the date as `ls -l`
    /// writes it, for the `ls` style.
    links: String,
    group: String,
    raw: u64,
    blocks: u64,
    ls_date: String,
    /// Seconds since the epoch, for sorting by date.
    mtime: i64,
    /// Bytes on disk, for sorting by size (directories count as 0).
    len: u64,
    /// The inode number, when the column is on.
    inode: String,
    perms: String,
    size: String,
    user: String,
    date: String,
    /// Two git letters, or empty outside a repository.
    git: String,
    name: String,
}

/// One cell of a tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Branch {
    /// Rightmost, but the directory goes on.
    Edge,
    /// Not rightmost, and the directory hasn't finished.
    Line,
    /// Rightmost, and the last of the directory.
    Corner,
    /// Not rightmost, and the directory has finished.
    Blank,
}

impl Branch {
    #[rustfmt::skip]
    fn art(self) -> &'static str {
        match self {
            Self::Edge => "├── ",
            Self::Line => "│   ",
            Self::Corner => "└── ",
            Self::Blank => "    ",
        }
    }

    /// Read one cell back off the page, or None where the names start.
    fn parse(cell: &str) -> Option<Branch> {
        Some(match cell {
            "├── " => Self::Edge,
            "│   " => Self::Line,
            "└── " => Self::Corner,
            "    " => Self::Blank,
            _ => return None,
        })
    }
}

/// A **tree trunk** remembers, for each level of the tree, whether that
/// directory goes on: a directory that does is drawn under a line, and one
/// that is finished under a blank. That is the only thing an entry's own
/// cells depend on besides whether it is the last of its directory.
#[derive(Default)]
struct Trunk {
    continues: Vec<bool>,
}

impl Trunk {
    /// The cells in front of the entry `depth` levels down.
    fn row(&self, depth: usize, last: bool) -> Vec<Branch> {
        let mut cells: Vec<Branch> = self
            .continues
            .iter()
            .take(depth)
            .map(|&goes_on| if goes_on { Branch::Line } else { Branch::Blank })
            .collect();
        cells.push(if last { Branch::Corner } else { Branch::Edge });
        cells
    }

    /// Go down a level, into a directory that does or doesn't go on.
    fn descend(&mut self, goes_on: bool) {
        self.continues.push(goes_on);
    }

    /// Come back up a level.
    fn ascend(&mut self) {
        self.continues.pop();
    }
}
// ------------------------------------------------------------- the layout

/// Where a listing's columns start, found from its column header line.
/// The inode and git columns come and go and a tree pushes the names
/// along, so nothing here can be a constant.
#[derive(Clone, Copy, Debug)]
struct Layout {
    inode: Option<usize>,
    perms: usize,
    /// `ls` style: hard links and group.
    links: Option<usize>,
    group: Option<usize>,
    size: Option<usize>,
    user: Option<usize>,
    date: Option<usize>,
    git: Option<usize>,
    name: usize,
}

impl Layout {
    fn of(head: &str) -> Option<Layout> {
        let name = head.find("Name")?;
        Some(Layout {
            inode: head.find("Inode"),
            links: None,
            group: None,
            perms: head.find("Permissions").or_else(|| head.find("Mode"))?,
            size: head.find("Size"),
            user: head.find("User"),
            date: head.find("Date Modified"),
            git: head.find("Git"),
            name,
        })
    }

    /// The columns of an `ls -l` line (after its mark): permissions, links,
    /// owner, group, size, a date of three words, then the name. Read from
    /// where the words start, since every line of a listing is padded alike.
    fn of_ls(line: &str) -> Option<Layout> {
        let chars: Vec<char> = line.chars().collect();
        let mut starts = Vec::new();
        let mut i = 2;
        while i < chars.len() && starts.len() < 9 {
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            starts.push(i);
            // The name is the rest of the line; stop before reading it.
            if starts.len() == 9 {
                break;
            }
            while i < chars.len() && chars[i] != ' ' {
                i += 1;
            }
        }
        let [perms, links, user, group, size, date, _, _, name] = starts[..] else {
            return None;
        };
        Some(Layout {
            inode: None,
            perms,
            links: Some(links),
            group: Some(group),
            size: Some(size),
            user: Some(user),
            date: Some(date),
            git: None,
            name,
        })
    }

    /// Every column that is there, left to right.
    fn starts(self) -> Vec<usize> {
        let mut v: Vec<usize> = [
            self.inode,
            Some(self.perms),
            self.links,
            self.size,
            self.user,
            self.group,
            self.date,
            self.git,
            Some(self.name),
        ]
        .into_iter()
        .flatten()
        .collect();
        v.sort_unstable();
        v
    }

    /// Where the column at `start` stops: at the next one along.
    fn end_of(self, start: usize, starts: &[usize]) -> usize {
        starts
            .iter()
            .copied()
            .find(|&s| s > start)
            .unwrap_or(self.name)
    }
}

/// One line of a listing: the row, and the tree cells its name sits behind.
struct Line {
    row: Row,
    /// The file's own name, for marks and for climbing back to its path.
    plain: String,
    /// The full path, which in a tree is under the directories above it.
    path: PathBuf,
    branches: Vec<Branch>,
}

/// How wide each column has to be for every row of a listing to fit.
struct Widths {
    inode: usize,
    size: usize,
    user: usize,
}

impl Widths {
    fn of(lines: &[Line]) -> Widths {
        let widest = |f: fn(&Row) -> &str, floor: usize| {
            lines
                .iter()
                .map(|l| f(&l.row).chars().count())
                .max()
                .unwrap_or(0)
                .max(floor)
        };
        Widths {
            inode: widest(|r| &r.inode, "Inode".len()),
            size: widest(|r| &r.size, "Size".len()),
            user: widest(|r| &r.user, "User".len()),
        }
    }
}

/// What a listing needs to know about itself, so that walking down a tree
/// doesn't mean passing all of it down at every level.
struct Listing<'a> {
    icons: bool,
    now: SystemTime,
    git: Option<&'a GitInfo>,
    view: View,
}

impl Listing<'_> {
    /// Every line of a listing, one directory at a time: this one, then — in
    /// tree view — the ones below each of its directories, in place.
    ///
    /// `trunk` says what the levels above this one look like, so their cells
    /// can be put in front of this directory's entries.
    fn read(&self, dir: &Path, trunk: &mut Trunk, depth: usize, out: &mut Vec<Line>) {
        let Listing {
            icons,
            now,
            git,
            view,
        } = self;
        // A directory that can't be read lists as nothing rather than as an
        // error: it is already on the page, and a stray permission bit
        // shouldn't wipe the whole buffer.
        let Ok(names) = names_of(dir, *view) else {
            return;
        };
        let mut lines: Vec<Line> = names
            .iter()
            .map(|n| Line {
                row: make_row(&dir.join(n), n, *icons, *now, *git),
                plain: n.clone(),
                path: dir.join(n),
                branches: Vec::new(),
            })
            .filter(|l| {
                if view.git_ignore && l.row.git == "-I" {
                    return false;
                }
                if view.only_dirs && !l.row.perms.starts_with('d') {
                    return false;
                }
                if view.only_files && l.row.perms.starts_with('d') {
                    return false;
                }
                true
            })
            .collect();
        sort_lines(&mut lines, *view);
        // Whether each entry is the last of its directory is what its own
        // cell and its children's depend on, so it is settled before any is
        // printed.
        let count = lines.len();
        for (i, line) in lines.iter_mut().enumerate() {
            line.branches = match view.tree {
                true => trunk.row(depth, i + 1 == count),
                false => Vec::new(),
            };
        }
        for (i, line) in lines.into_iter().enumerate() {
            let last = i + 1 == count;
            // Only real directories are walked into, so a symlink to one
            // can't make this go round for ever.
            let below = view.tree
                && depth + 1 < view.level
                && line.row.perms.starts_with('d')
                && !line.path.is_symlink();
            let path = line.path.clone();
            out.push(line);
            if below {
                trunk.descend(!last);
                self.read(&path, trunk, depth + 1, out);
                trunk.ascend();
            }
        }
    }
}

/// The smallest and largest of a column over one listing: what
/// `--color-scale` measures a value against.
#[derive(Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
}

impl Range {
    fn of(
        values: impl Iterator<Item = String>,
        read: impl Fn(&str) -> Option<f64>,
    ) -> Option<Range> {
        let mut it = values.filter_map(|v| read(&v));
        let first = it.next()?;
        let (mut min, mut max) = (first, first);
        for v in it {
            min = min.min(v);
            max = max.max(v);
        }
        Some(Range { min, max })
    }

    /// Where `v` sits in the range: 0 for the smallest, 1 for the largest.
    /// A range of one value has no scale to it, so everything is the top.
    fn ratio(&self, v: f64) -> f64 {
        let r = (v - self.min) / (self.max - self.min);
        match r.is_nan() {
            true => 1.0,
            false => r.clamp(0.0, 1.0),
        }
    }

    /// `base` lightened or deepened by how far along `v` is.
    fn shade(&self, v: f64, base: u32) -> u32 {
        shade(base, self.ratio(v))
    }
}

/// The minimum luminance: no shade is darker than this.
const MIN_LUMINANCE: f64 = 0.5;

/// The keymap a listing entry's name brings with it, so clicking one runs
/// whatever is bound there. A user can rebind `<mouse-1>` in it:
/// `forge.keymap.set("<mouse-1>", "...", { region = "dired-entry" })`.
pub const ENTRY_REGION: &str = "dired-entry";
/// The header line of a listing ("  /home/me/src:").
pub const SECTION_REGION: &str = "dired-section";
/// The column header line of a listing.
pub const COLUMNS_REGION: &str = "dired-columns";

/// Everything colouring a listing needs that isn't on the page: where the
/// columns are, what each file is called (a file next to the source it was
/// built from is compiled), and — when the view asks for it — the range
/// each gradient column spans.
struct Context {
    layout: Layout,
    /// Every column that is there, left to right: each one ends where the
    /// next begins.
    starts: Vec<usize>,
    sizes: Option<Range>,
    dates: Option<Range>,
    now: i64,
}

impl Context {
    fn of(b: &Buffer, sec: &Section, end: usize, layout: Layout, now: i64) -> Context {
        let lines = sec.header + 2..end;
        let starts = layout.starts();
        let cell = |li: usize, from: usize| -> Option<String> {
            let chars: Vec<char> = b.line(li).chars().collect();
            let to = layout.end_of(from, &starts).min(chars.len());
            let seg = chars.get(from..to)?;
            let lead = seg.iter().take_while(|c| **c == ' ').count();
            let used = seg.iter().rposition(|c| *c != ' ')? + 1;
            (lead < used).then(|| seg[lead..used].iter().collect())
        };
        // A gradient has to see the whole column before it can colour any of
        // it, and there is only one when the view asked for it.
        let (sizes, dates) = match (layout.size, current_view(b).scale) {
            (Some(size), true) => (
                Range::of(lines.clone().filter_map(|li| cell(li, size)), |c| {
                    bytes_in(c).map(|n| n as f64)
                }),
                Range::of(lines.filter_map(|li| cell(li, layout.date?)), |c| {
                    seconds_in(c, now, env().tz).map(|t| t as f64)
                }),
            ),
            _ => (None, None),
        };
        Context {
            layout,
            starts,
            sizes,
            dates,
            now,
        }
    }

    /// The face for `bytes`, in its own magnitude's colour, or shaded by
    /// where it sits in the listing when the gradient is on.
    fn size_face(&self, bytes: u64) -> DiredFace {
        let base = size_face(bytes);
        match self.sizes {
            Some(range) => DiredFace::Shade(range.shade(bytes as f64, base_rgb(base))),
            None => base,
        }
    }

    /// The same for a date cell, read back out of the text.
    fn date_face(&self, cell: &str) -> DiredFace {
        match (self.dates, seconds_in(cell, self.now, env().tz)) {
            (Some(range), Some(secs)) => {
                DiredFace::Shade(range.shade(secs as f64, base_rgb(DiredFace::Date)))
            }
            _ => DiredFace::Date,
        }
    }
}

/// One directory listed in the buffer.
struct Section {
    /// The line of "  /dir:"; the column headers are on the next line and
    /// the entries follow.
    header: usize,
    dir: PathBuf,
}

/// The column the names start at in the first listing.
/// The column a listing's file names start at, or None when the line
/// isn't a listing entry. Public so the terminal's tests can check it.
#[doc(hidden)]
pub fn name_col(b: &Buffer) -> Option<usize> {
    if b.line_count() < 2 {
        return None;
    }
    layout_for(b, 0).map(|l| l.name)
}

/// A file listed in the buffer.
#[derive(Clone)]
struct Entry {
    /// Its full path, which in a tree runs through every directory above it.
    path: PathBuf,
    /// The last component.
    name: String,
    /// The first character of the mode string: 'd', 'l', '.', ...
    kind: char,
}
/// Show `dir` in a Dired buffer (reusing the one that already shows it).
pub fn open(app: &mut App, dir: &Path) {
    if let Some(name) = dir.to_str()
        && crate::tether::visit(app, name, false, false, false)
    {
        return;
    }
    let dir = match fs::canonicalize(dir) {
        Ok(d) => d,
        Err(e) => return app.error(format!("Cannot open {}: {e}", dir.display())),
    };
    open_resolved(app, dir);
}

/// Show a remote directory (a Tether name already resolved) in Dired.
pub fn open_remote(app: &mut App, dir: &crate::tether::Remote) {
    let mut name = dir.display();
    while name.len() > 1 && name.ends_with('/') && dir.path != "/" {
        name.pop();
    }
    open_resolved(app, PathBuf::from(name));
}

fn open_resolved(app: &mut App, dir: PathBuf) {
    let existing = app.editor.buffer_ids().into_iter().find(|id| {
        app.editor.state.get_buffer(*id).is_some_and(|b| {
            b.major_mode() == MODE && header_dir(b).as_deref() == Some(dir.as_path())
        })
    });
    let old = existing.and_then(|id| app.editor.state.get_buffer(id));
    let view = old.map_or_else(|| View::initial(app), current_view);
    // Inline subdirectories survive a second visit, unless the listing is a
    // tree, which is all of them already.
    let mut dirs = vec![dir.clone()];
    let mut trees = HashSet::new();
    if !view.tree {
        dirs.extend(
            old.map(sections)
                .unwrap_or_default()
                .into_iter()
                .skip(1)
                .map(|s| s.dir),
        );
        if let Some(old) = old {
            trees.extend(tree_dirs_of(old));
        }
    }
    let hidden: HashSet<PathBuf> = old
        .map(hidden_dirs_of)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let text = match render_all_with(&dirs, &HashMap::new(), view, &trees, &hidden) {
        Ok(t) => t,
        Err(e) => return app.error(format!("Cannot read {}: {e}", dir.display())),
    };
    let name = match old {
        Some(b) => b.name().to_string(),
        None => {
            let base = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string());
            app.editor.unique_name(&base)
        }
    };
    app.show_special_buffer(&name, MODE, &text);
    let b = app.buf();
    // A dired buffer is a listing, not text to edit, which is also what the
    // `read-only` keymap condition answers.
    b.set_read_only(true);
    decorate(b);
    let (line, col) = (
        FIRST_ENTRY.min(last_entry_line(b)),
        name_col(b).unwrap_or(0),
    );
    b.set_point(Pos::new(line, col));
    app.msg("Dired: ? for a key summary, C-h m for every key");
}

pub(crate) fn start_dir(app: &App) -> PathBuf {
    point_dir(app)
        .or_else(|| {
            app.buf_ref()
                .path()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .filter(|p| !p.as_os_str().is_empty())
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Mark (`mark` is `*` or a space) the entry for `path` in the Dired
/// buffer showing its directory, switching to that buffer. False when no
/// such listing is open.
pub fn mark_file(app: &mut App, path: &Path, mark: char) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name() else {
        return false;
    };
    let id = app.editor.buffer_ids().into_iter().find(|id| {
        app.editor.state.get_buffer(*id).is_some_and(|b| {
            b.major_mode() == MODE && listing::header_dir(b).as_deref() == Some(dir)
        })
    });
    let Some(id) = id else {
        return false;
    };
    let line = {
        let b = app.editor.state.get_buffer(id).unwrap();
        (0..b.line_count()).find(|&l| {
            listing::entry_on_line(b, l).is_some_and(|e| e.path.file_name() == Some(name))
        })
    };
    let Some(line) = line else {
        return false;
    };
    app.switch_to(id);
    command::set_mark(app, line, mark);
    true
}

// ------------------------------------------------------------ commands

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "dired" => {
            let dir = start_dir(app);
            app.prompt("Dired (directory): ", Purpose::Dired(DiredAction::Open));
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        "dired-jump" => {
            let file = app
                .buf_ref()
                .path()
                .and_then(|p| p.file_name())
                .map(|n| n.to_owned());
            let dir = start_dir(app);
            open(app, &dir);
            if let Some(f) = file {
                let dir = fs::canonicalize(&dir).unwrap_or(dir);
                goto_path(app, &dir.join(f));
            }
        }
        "wdired-change-to-wdired-mode" => {
            if app.buf_ref().major_mode() != MODE {
                app.error("Not in a Dired buffer");
            } else {
                extra::wdired_start(app);
            }
        }
        "image-dired-display-thumbs" => extra::image_dired(app),
        "image-dired-thumbnail-open" => extra::image_dired_open(app),
        "wdired-finish-edit" | "wdired-abort-changes" => return wdired_command(app, cmd),
        _ if !cmd.starts_with("dired-") => return false,
        _ => {
            if app.buf_ref().major_mode() != MODE {
                app.error("Not in a Dired buffer");
                return true;
            }
            dired_command(app, cmd)
        }
    }
    true
}

/// The marked files, or the one on point's line (for version control's
/// commands run from a listing, as vc-dired-deduce-fileset).
pub fn selected_paths(app: &App) -> Option<Vec<PathBuf>> {
    let line = app.buf_ref().point().line;
    let here = entry_on_line(app.buf_ref(), line);
    targets(app, here)
}

/// The directory a listing shows.
pub fn directory(b: &Buffer) -> Option<PathBuf> {
    header_dir(b)
}

/// After the user said yes. Directories are only removed when empty.
pub fn delete_confirmed(app: &mut App, paths: Vec<PathBuf>) {
    let (mut done, mut failed) = (0, Vec::new());
    for p in &paths {
        let result = rfs::remove(p, rfs::is_dir(p));
        match result {
            Ok(()) => done += 1,
            Err(e) => failed.push(format!(
                "{}: {e}",
                p.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }
    if app.buf_ref().major_mode() == MODE {
        revert(app, true);
    }
    match failed.first() {
        None => app.msg(format!("Deleted {done} file(s)")),
        Some(first) => app.error(format!("Deleted {done}, {} failed ({first})", failed.len())),
    }
}

// --------------------------------------------------- copy, rename, mkdir

/// The answer to a Dired prompt.
pub fn answered(app: &mut App, action: DiredAction, input: &str) {
    if let DiredAction::Extra(x) = action {
        return extra::answered(app, x, input);
    }
    if let DiredAction::TreeLevel = action {
        match input.trim().parse::<usize>() {
            Ok(n) if (1..=TREE_MAX_DEPTH).contains(&n) => {
                if app.buf_ref().major_mode() != MODE {
                    return app.error("Not in a Dired buffer");
                }
                let mut view = current_view(app.buf_ref());
                view.level = n;
                // A level only means something in a tree, so asking for one
                // turns the tree on, like --tree --level.
                view.tree = true;
                let dir = match current_dir(app) {
                    Some(d) => d,
                    None => return,
                };
                let marks = marks_of(app.buf_ref());
                let point = app.buf_ref().point();
                match render_all(std::slice::from_ref(&dir), &marks, view, &HashSet::new()) {
                    Ok(text) => {
                        let b = app.buf();
                        b.set_contents(&text);
                        decorate(b);
                        let col = name_col(b).unwrap_or(0);
                        b.set_point(Pos::new(point.line.min(last_entry_line(b)), col));
                        app.msg(format!("Tree level {n}"));
                    }
                    Err(e) => app.error(format!("Cannot read {}: {e}", dir.display())),
                }
            }
            _ => app.error("Please enter a number 1-8"),
        }
        return;
    }
    if input.is_empty() {
        return;
    }
    let typed = crate::minibuffer::expand_path(input);
    let base = start_dir(app);
    let target = if typed.is_absolute() {
        typed
    } else {
        base.join(typed)
    };
    match action {
        DiredAction::Open if rfs::is_remote(&target) => open(app, &target),
        DiredAction::Open if target.is_dir() => open(app, &target),
        DiredAction::Open if target.exists() => crate::minibuffer::visit_file(app, &target, false),
        DiredAction::Open => app.error(format!("No such directory: {}", target.display())),
        DiredAction::OpenOtherFrame if target.is_dir() => {
            app.editor.make_frame();
            open(app, &target)
        }
        DiredAction::OpenOtherFrame if target.exists() => {
            app.editor.make_frame();
            crate::minibuffer::visit_file(app, &target, false)
        }
        DiredAction::OpenOtherFrame => {
            app.error(format!("No such directory: {}", target.display()))
        }
        DiredAction::CreateDirectory(_) => match rfs::mkdir(&target) {
            Ok(()) => {
                revert(app, true);
                app.msg(format!("Created {}", target.display()));
            }
            Err(e) => app.error(format!("Cannot create {}: {e}", target.display())),
        },
        DiredAction::Copy(sources) => transfer(app, sources, &target, true),
        DiredAction::Rename(sources) => transfer(app, sources, &target, false),
        DiredAction::TreeLevel | DiredAction::Extra(_) => {}
        DiredAction::OpenOtherTab if target.is_dir() => {
            crate::tabs::new_tab_public(app);
            open(app, &target)
        }
        DiredAction::OpenOtherTab if target.exists() => {
            crate::tabs::new_tab_public(app);
            crate::minibuffer::visit_file(app, &target, false)
        }
        DiredAction::OpenOtherTab => app.error(format!("No such directory: {}", target.display())),
    }
}

mod color;
mod command;
mod env;
mod extra;
mod format;
mod git;
mod listing;
mod render;
mod rfs;

use color::*;
use command::*;
use env::*;
pub use extra::{Extra, THUMBS_MODE, wdired_command};
use format::*;
use listing::*;
use render::*;

#[cfg(test)]
mod tests;

/// Read a Dired buffer's directory again (auto-revert). The buffer must be
/// the selected one.
pub(crate) fn refresh(app: &mut App) {
    command::revert(app, true);
}

/// The directory a Dired buffer lists and when it last changed, so
/// auto-revert can tell when to read it again. Remote ones are left alone.
pub(crate) fn watch_stamp(b: &Buffer) -> Option<(PathBuf, SystemTime)> {
    if b.major_mode() != MODE {
        return None;
    }
    let dir = header_dir(b)?;
    if rfs::is_remote(&dir) {
        return None;
    }
    let at = fs::metadata(&dir).and_then(|m| m.modified()).ok()?;
    Some((dir, at))
}
