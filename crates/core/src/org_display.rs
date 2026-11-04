// SPDX-License-Identifier: MIT OR Apache-2.0
//! How Org text can look different from what it is in the buffer, for
//! lines point isn't on: tables with `<N>` width cookies shrunk to those
//! widths, links to image files shown as a badge with the picture's size,
//! and the header row of a long table kept in a header line.
//!
//! The text of the buffer never changes; the display asks
//! [`Buffer::org_display_line`] for a stand-in text per line.

use crate::Buffer;
use crate::org_table::{Row, is_table_line, parse_row};
use crate::width::line_width;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// Rows an inline picture may take under its link line; 0 (the default)
/// means the frontend can't draw pictures, and only badges show.
static IMAGE_ROWS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A frontend that draws pictures asks for `rows` rows under each image
/// link (0 turns it off).
pub fn set_inline_image_rows(rows: usize) {
    IMAGE_ROWS.store(rows, std::sync::atomic::Ordering::Relaxed);
}

pub fn inline_image_rows() -> usize {
    IMAGE_ROWS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The frontend's text cell in points (width, height ×100 packed), so a
/// picture asked for in pixels can be given the right number of rows.
static CELL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn set_cell_size(width: f64, height: f64) {
    let w = (width.max(0.0) * 100.0) as u64;
    let h = (height.max(0.0) * 100.0) as u64;
    CELL.store((w << 32) | h, std::sync::atomic::Ordering::Relaxed);
}

pub fn cell_size() -> Option<(f64, f64)> {
    let v = CELL.load(std::sync::atomic::Ordering::Relaxed);
    let (w, h) = ((v >> 32) as f64 / 100.0, (v & 0xFFFF_FFFF) as f64 / 100.0);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// A width asked for by `#+ATTR_ORG: :width` (or another backend's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidthSpec {
    Pixels(f64),
    Percent(f64),
    Columns(f64),
}

/// `250`, `250px`, `50%`, `30em`.
pub fn parse_width_spec(v: &str) -> Option<WidthSpec> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse().ok().map(WidthSpec::Percent);
    }
    let digits: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let n: f64 = digits.parse().ok()?;
    match v[digits.len()..].trim().to_ascii_lowercase().as_str() {
        "" | "px" => Some(WidthSpec::Pixels(n)),
        "em" | "ex" | "ch" | "rem" => Some(WidthSpec::Columns(n)),
        _ => None,
    }
}

/// The `:width` in a `#+ATTR_*:` line's text.
fn attr_line_width(rest: &str) -> Option<WidthSpec> {
    let mut it = rest.split_whitespace();
    while let Some(tok) = it.next() {
        if tok.eq_ignore_ascii_case(":width") {
            return it.next().and_then(parse_width_spec);
        }
    }
    None
}

// -------------------------------------------------- image file names

/// What `image-file-name-extensions` defaults to, minus formats
/// its own image library can't draw.
pub const DEFAULT_IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "ico", "jpeg", "jpg", "pbm", "pgm", "png", "pnm", "ppm", "svg", "tif",
    "tiff", "webp", "xbm", "xpm",
];

/// The extensions and regexps that say a file name is an image.
#[derive(Debug)]
struct ImageNames {
    extensions: Vec<String>,
    regexps: Vec<regex::Regex>,
}

impl Default for ImageNames {
    fn default() -> Self {
        Self {
            extensions: DEFAULT_IMAGE_EXTENSIONS
                .iter()
                .map(|e| (*e).to_string())
                .collect(),
            regexps: Vec::new(),
        }
    }
}

static IMAGE_NAMES: LazyLock<RwLock<ImageNames>> =
    LazyLock::new(|| RwLock::new(ImageNames::default()));

fn image_names() -> std::sync::RwLockReadGuard<'static, ImageNames> {
    IMAGE_NAMES.read().unwrap_or_else(|e| e.into_inner())
}

/// Set the extensions that name an image (`forge.opt.image_file_name_extensions`).
/// Each may be written with or without its leading dot.
pub fn set_image_file_extensions(extensions: Vec<String>) {
    let mut names = IMAGE_NAMES.write().unwrap_or_else(|e| e.into_inner());
    names.extensions = extensions
        .iter()
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
}

/// Set the regexps that match image file names
/// (`forge.opt.image_file_name_regexps`). They are matched against the
/// file's name (not its directory).
pub fn set_image_file_regexps(regexps: Vec<String>) -> Result<(), String> {
    let mut compiled = Vec::with_capacity(regexps.len());
    for r in regexps {
        let re =
            regex::Regex::new(&r).map_err(|e| format!("image_file_name_regexps: {r:?}: {e}"))?;
        compiled.push(re);
    }
    let mut names = IMAGE_NAMES.write().unwrap_or_else(|e| e.into_inner());
    names.regexps = compiled;
    Ok(())
}

fn image_name_matches(names: &ImageNames, path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let base = base.split(['?', '#']).next().unwrap_or(base);
    if names.regexps.iter().any(|r| r.is_match(base)) {
        return true;
    }
    let ext = base
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty());
    ext.is_some_and(|e| names.extensions.contains(&e))
}

/// Whether `path` names an image, by extension or by one of the
/// configured regexps. The query and fragment of a URL are ignored.
pub fn is_image_file_name(path: &str) -> bool {
    image_name_matches(&image_names(), path)
}

/// Whether remote image links are fetched and drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum RemoteImages {
    /// Don't preview remote image links.
    Skip = 0,
    /// Use the cached copy, fetching it once if it's missing (the
    /// default).
    #[default]
    Cache = 1,
    /// Fetch the image if there is no copy yet. Like `Cache`, the copy
    /// stays in the cache, so it is not fetched again on every frame.
    Download = 2,
}

impl RemoteImages {
    pub fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "skip" | "nil" | "none" | "off" => Some(Self::Skip),
            "cache" => Some(Self::Cache),
            "download" | "t" | "on" => Some(Self::Download),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Cache => "cache",
            Self::Download => "download",
        }
    }
}

static REMOTE_IMAGES: AtomicU8 = AtomicU8::new(RemoteImages::Cache as u8);

pub fn set_remote_inline_images(mode: RemoteImages) {
    REMOTE_IMAGES.store(mode as u8, Ordering::Relaxed);
}

pub fn remote_inline_images() -> RemoteImages {
    match REMOTE_IMAGES.load(Ordering::Relaxed) {
        0 => RemoteImages::Skip,
        2 => RemoteImages::Download,
        _ => RemoteImages::Cache,
    }
}

/// Remote images this buffer has asked for: what is on its way, and what
/// failed (so an unreachable URL isn't asked for on every frame).
#[derive(Debug, Default)]
struct RemoteState {
    pending: HashSet<String>,
    failed: HashSet<String>,
}

/// How wide to draw an inline image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActualWidth {
    /// `t` (the default): the file's own pixel width, capped by max-width.
    #[default]
    Actual,
    /// A number: this many pixels (ImageMagick resize).
    Pixels(usize),
    /// A number in a list: the `#+ATTR_*` width if there is one, else this
    /// many pixels.
    PixelsOrActual(usize),
}

/// A cap on the drawn image width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaxWidth {
    /// The default: `fill-column` characters.
    #[default]
    FillColumn,
    /// The current window's width.
    Window,
    /// This many pixels.
    Pixels(usize),
    /// No cap.
    None,
}

/// Where a standalone image sits on its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl ImageAlign {
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "left" => Some(Self::Left),
            "center" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

/// A width an image asks for, from `#+ATTR_* :width`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrWidth {
    /// `N` or `Npx`: N pixels.
    Pixels(usize),
    /// `N%`: a percentage of the available width.
    Percent(usize),
    /// `Nem`/`N` with a text unit: N characters.
    Columns(usize),
}

#[derive(Clone, Debug)]
pub struct OrgDisplay {
    pub table_shrink: bool,
    pub table_header: bool,
    /// Link previews on for the whole buffer. Per-region previews live in
    /// `link_preview_ranges`.
    pub link_preview: bool,
    /// Line ranges (inclusive) whose links are previewed.
    pub link_preview_ranges: Vec<(usize, usize)>,
    /// Preview links that have a description too. Off: only bare links show
    /// previews.
    pub link_preview_described: bool,
    /// Show only a link's description, hiding the `[[target][...]]` around
    /// it. The markup stays
    /// hidden even when point is inside the link.
    pub link_descriptive: bool,
    /// Org indent mode: body lines align under their headline's text,
    /// headlines shift, leading stars hide. Display-only; the text never
    /// changes.
    pub indent: bool,
    /// Org num mode: headlines show their section number after the stars
    /// (`** 1.2 Title`). Display-only; the text never changes.
    pub num: bool,
    /// Show `\alpha`-style entities as their UTF-8 characters.
    /// Display-only; the text never changes.
    pub pretty_entities: bool,
    /// `org-image-actual-width`: how wide an inline image is drawn.
    pub image_actual_width: ActualWidth,
    /// `org-image-max-width`: a cap on the drawn width.
    pub image_max_width: MaxWidth,
    /// `org-image-align`: where a standalone image sits.
    pub image_align: ImageAlign,
    /// Where remote images are cached; None uses
    /// `cache_dir()/org-images`.
    pub remote_cache_dir: Option<PathBuf>,
    /// Remote image fetches in flight, and those that failed.
    remote: Arc<Mutex<RemoteState>>,
    /// Badges already worked out: file -> text.
    cache: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// The plugins that pinned a picture to a line, in pin order, the most
    /// recent last. The buffer's own map is keyed by plugin name, which
    /// has no order of its own.
    plugin_pins: BTreeMap<usize, Vec<String>>,
    /// LaTeX fragments previewed as pictures, by buffer line.
    pub latex_previews: BTreeMap<usize, PathBuf>,
}

impl Default for OrgDisplay {
    fn default() -> Self {
        Self {
            table_shrink: false,
            table_header: false,
            link_preview: false,
            link_preview_ranges: Vec::new(),
            link_preview_described: false,
            // Forge shows descriptive links by default.
            link_descriptive: true,
            indent: false,
            num: false,
            pretty_entities: false,
            image_actual_width: ActualWidth::default(),
            image_max_width: MaxWidth::default(),
            image_align: ImageAlign::default(),
            remote_cache_dir: None,
            remote: Arc::new(Mutex::new(RemoteState::default())),
            cache: Arc::new(Mutex::new(HashMap::new())),
            plugin_pins: BTreeMap::new(),
            latex_previews: BTreeMap::new(),
        }
    }
}

impl OrgDisplay {
    pub fn any(&self) -> bool {
        self.table_shrink
            || self.previews_any()
            || self.link_descriptive
            || self.indent
            || self.num
            || self.pretty_entities
            || !self.latex_previews.is_empty()
    }

    /// Whether any line's links are previewed (the whole buffer or a
    /// region).
    pub fn previews_any(&self) -> bool {
        self.link_preview || !self.link_preview_ranges.is_empty()
    }

    /// Whether links on `line` are previewed.
    pub fn previews_line(&self, line: usize) -> bool {
        self.link_preview
            || self
                .link_preview_ranges
                .iter()
                .any(|&(a, b)| (a..=b).contains(&line))
    }

    pub fn clear_cache(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
        if let Ok(mut r) = self.remote.lock() {
            r.failed.clear();
        }
    }

    /// Where the copy of a remote image lives, whether or not it has been
    /// fetched yet.
    pub fn remote_path(&self, url: &str) -> Option<PathBuf> {
        let dir = self
            .remote_cache_dir
            .clone()
            .or_else(|| crate::config::cache_dir().map(|d| d.join("org-images")))?;
        Some(dir.join(remote_file_name(url)))
    }

    /// The cached file for a remote image URL, fetching it in the
    /// background when it isn't there yet. None means "not ready": draw
    /// the badge this frame and look again on the next one.
    pub fn remote_image(&self, url: &str) -> Option<PathBuf> {
        if remote_inline_images() == RemoteImages::Skip {
            return None;
        }
        let path = self.remote_path(url)?;
        if remote_image_ready(&path) {
            return Some(path);
        }
        let mut st = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        if st.pending.contains(url) || st.failed.contains(url) {
            return None;
        }
        st.pending.insert(url.to_string());
        drop(st);
        spawn_remote_fetch(Arc::clone(&self.remote), url.to_string(), path);
        None
    }
}

/// What a picture named by web address has come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    /// In the cache: draw this file.
    Ready(PathBuf),
    /// On its way; ask again shortly.
    Pending,
    /// Unreachable, not a picture, or fetching pictures is off.
    Missing,
}

static SHARED_REMOTE: LazyLock<Arc<Mutex<RemoteState>>> = LazyLock::new(Default::default);

/// A picture from a web address for anything that draws pictures (a Bases
/// card's cover): the cached file when it has arrived, else it is fetched in
/// the background (the same cache as inline Org images) and this says so.
pub fn remote_picture(url: &str) -> Remote {
    if remote_inline_images() == RemoteImages::Skip {
        return Remote::Missing;
    }
    let Some(dir) = crate::config::cache_dir().map(|d| d.join("org-images")) else { return Remote::Missing };
    let path = dir.join(remote_file_name(url));
    if remote_image_ready(&path) {
        return Remote::Ready(path);
    }
    let mut st = SHARED_REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    if st.failed.contains(url) {
        return Remote::Missing;
    }
    if !st.pending.contains(url) {
        st.pending.insert(url.to_string());
        drop(st);
        spawn_remote_fetch(Arc::clone(&SHARED_REMOTE), url.to_string(), path);
    }
    Remote::Pending
}

fn remote_image_ready(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// The cache file name for `url`: a hash (so any URL is a safe name) and
/// the URL's extension when it has one.
fn remote_file_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(url.as_bytes());
    let digest = hash.finalize();
    let mut name = String::with_capacity(32);
    for b in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(name, "{b:02x}");
    }
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    let ext = clean
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.').map(|(_, e)| e))
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(str::to_ascii_lowercase);
    match ext {
        Some(e) => format!("{name}.{e}"),
        None => name,
    }
}

/// Pictures bigger than this are not fetched.
const MAX_REMOTE_IMAGE_BYTES: u64 = 10_000_000;

static IMAGE_CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; Forge)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("org image http client")
});

fn spawn_remote_fetch(state: Arc<Mutex<RemoteState>>, url: String, path: PathBuf) {
    let thread_url = url.clone();
    let thread_state = Arc::clone(&state);
    let spawned = std::thread::Builder::new()
        .name("org-image".into())
        .spawn(move || {
            let ok = fetch_remote_image(&thread_url, &path);
            if let Ok(mut st) = thread_state.lock() {
                st.pending.remove(&thread_url);
                if !ok {
                    st.failed.insert(thread_url);
                }
            }
        });
    if spawned.is_err()
        && let Ok(mut st) = state.lock()
    {
        st.pending.remove(&url);
        st.failed.insert(url);
    }
}

fn fetch_remote_image(url: &str, path: &Path) -> bool {
    use std::io::Read;
    let Ok(response) = IMAGE_CLIENT.get(url).send() else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let ctype = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ctype.is_empty() && !ctype.starts_with("image/") && !ctype.contains("octet-stream") {
        return false;
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_REMOTE_IMAGE_BYTES)
    {
        return false;
    }
    let mut bytes = Vec::new();
    if response
        .take(MAX_REMOTE_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.is_empty()
        || bytes.len() as u64 > MAX_REMOTE_IMAGE_BYTES
    {
        return false;
    }
    let Some(dir) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let tmp = path.with_file_name(format!(
        "{}.part",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("image")
    ));
    if std::fs::write(&tmp, &bytes).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// A headline's own virtual indent: `(level-1) * per_level`, shifted
/// two left for room (never negative).
pub fn headline_indent(level: usize, per_level: usize) -> usize {
    level
        .saturating_sub(1)
        .saturating_mul(per_level)
        .saturating_sub(2)
}

/// Section numbering in Org's `org-num-mode`. `numbering` is the current
/// number, innermost first (Org's order: `1.2.3` is `[3, 2, 1]`), and is
/// advanced for a headline at `level`. Returns the display string
/// (`"1.2.3 "`). Faithful to `org-num--current-numbering` /
/// `org-num-default-format`.
pub fn org_num_step(numbering: &mut Vec<usize>, level: usize) -> String {
    let last = numbering.len();
    if last == 0 {
        numbering.push(1);
        numbering.extend(std::iter::repeat_n(0, level - 1));
    } else if level == last {
        numbering[0] += 1;
    } else if level < last {
        numbering.drain(0..last - level);
        numbering[0] += 1;
    } else {
        let mut next = vec![1];
        next.extend(std::iter::repeat_n(0, level - last - 1));
        next.extend_from_slice(numbering);
        *numbering = next;
    }
    let mut s = String::new();
    for (i, n) in numbering.iter().rev().enumerate() {
        if i > 0 {
            s.push('.');
        }
        s.push_str(&n.to_string());
    }
    s.push(' ');
    s
}

/// The Org num-mode number of the headline at `line` (`"1.2 "`), or
/// `None` if the line isn't a headline. Numbering starts at the top of
/// the buffer, so this scans from line 0.
pub fn org_line_number(buf: &Buffer, line: usize) -> Option<String> {
    let mut numbering = Vec::new();
    for li in 0..=line {
        if let Some(n) = buf.org_level(li) {
            let s = org_num_step(&mut numbering, n);
            if li == line {
                return Some(s);
            }
        }
    }
    None
}

/// Virtual indent columns for `line` in Org indent mode (`per_level`
/// from the global setting): headlines sit at `(level-1) * per_level`
/// shifted two left (never negative), body lines align under their
/// headline's text. Zero without indent mode or outside Org buffers.
pub fn org_indent_width(buf: &Buffer, line: usize, per_level: usize) -> usize {
    if !buf.org_display.indent || buf.major_mode() != crate::org::MODE {
        return 0;
    }
    if let Some(n) = buf.org_level(line) {
        return headline_indent(n, per_level);
    }
    match buf.org_heading_at(line) {
        Some(h) => {
            let hn = buf.org_level(h).unwrap_or(1);
            headline_indent(hn, per_level) + hn + 1
        }
        None => 0,
    }
}

/// `<N>`, `<lN>`, `<rN>`, `<cN>` in a table cell.
fn cookie_width(cell: &str) -> Option<usize> {
    let c = cell.trim();
    let inner = c.strip_prefix('<')?.strip_suffix('>')?;
    let digits = inner.strip_prefix(['l', 'r', 'c']).unwrap_or(inner);
    if digits.is_empty() || !digits.chars().all(|d| d.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n: &usize| *n > 0)
}

fn pad(s: &str, w: usize) -> String {
    let n = line_width(s);
    format!("{s}{}", " ".repeat(w.saturating_sub(n)))
}

fn clip(s: &str, w: usize) -> String {
    if line_width(s) <= w {
        return s.to_string();
    }
    let mut out = crate::width::truncate(s, w.saturating_sub(1));
    out.push('…');
    out
}

// ------------------------------------------------------- image badges

pub(crate) fn image_size(path: &Path) -> Option<(u32, u32)> {
    // The same few pictures are asked about on every frame; a file that
    // changed (size or time) is read again.
    type Key = (PathBuf, u64, Option<std::time::SystemTime>);
    static SIZES: LazyLock<Mutex<HashMap<Key, Option<(u32, u32)>>>> =
        LazyLock::new(Default::default);
    let meta = std::fs::metadata(path).ok();
    let key: Key = (
        path.to_path_buf(),
        meta.as_ref().map_or(0, std::fs::Metadata::len),
        meta.and_then(|m| m.modified().ok()),
    );
    if let Ok(m) = SIZES.lock()
        && let Some(found) = m.get(&key)
    {
        return *found;
    }
    let size = read_image_size(path);
    if let Ok(mut m) = SIZES.lock() {
        if m.len() > 512 {
            m.clear();
        }
        m.insert(key, size);
    }
    size
}

fn read_image_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head).ok()?;
    let b = &head[..n];
    let be32 = |i: usize| {
        b.get(i..i + 4)
            .map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
    };
    let le16 = |i: usize| {
        b.get(i..i + 2)
            .map(|x| u16::from_le_bytes([x[0], x[1]]) as u32)
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"BM") {
        let w = b
            .get(18..22)
            .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))?;
        let h = b
            .get(22..26)
            .map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]).unsigned_abs())?;
        return Some((w, h));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
                let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
                return Some((w, h));
            }
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            i += 2 + len;
        }
        return None;
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        match b.get(12..16)? {
            b"VP8X" => {
                let w = b
                    .get(24..27)
                    .map(|x| 1 + (x[0] as u32 | (x[1] as u32) << 8 | (x[2] as u32) << 16))?;
                let h = b
                    .get(27..30)
                    .map(|x| 1 + (x[0] as u32 | (x[1] as u32) << 8 | (x[2] as u32) << 16))?;
                return Some((w, h));
            }
            b"VP8 " => return Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = b
                    .get(21..25)
                    .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))?;
                return Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1));
            }
            _ => return None,
        }
    }
    None
}

fn human(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{} KB", bytes >> 10)
    } else {
        format!("{bytes} B")
    }
}

fn badge(od: &OrgDisplay, path: &Path, shown: &str) -> String {
    let cached = od.cache.lock().ok().and_then(|c| c.get(path).cloned());
    let info = cached.unwrap_or_else(|| {
        let info = match std::fs::metadata(path) {
            Ok(m) => {
                let dims = image_size(path)
                    .map(|(w, h)| format!("{w}×{h}, "))
                    .unwrap_or_default();
                format!("{dims}{}", human(m.len()))
            }
            Err(_) => "missing".to_string(),
        };
        if let Ok(mut c) = od.cache.lock() {
            c.insert(path.to_path_buf(), info.clone());
        }
        info
    });
    format!("🖼 {shown} ({info})")
}

/// The picture a display property inserts, as an image descriptor.
fn picture_from(props: &[crate::buffer::DisplaySpec]) -> Option<crate::display::ImageDescriptor> {
    use crate::buffer::DisplayValue;
    let spec = props.iter().find(|s| s.name == "image")?;
    let path = match spec.arg("file") {
        Some(DisplayValue::Str(p)) => PathBuf::from(p),
        _ => return None,
    };
    let data = match spec.arg("data") {
        Some(DisplayValue::Str(d)) => Some(d.as_bytes().to_vec()),
        _ => None,
    };
    let num = |key: &str| -> Option<f64> {
        match spec.arg(key) {
            Some(DisplayValue::Int(i)) => Some(*i as f64),
            Some(DisplayValue::Real(r)) => Some(*r),
            _ => None,
        }
    };
    let ascent = match spec.arg("ascent") {
        Some(DisplayValue::Symbol(s)) if s == "center" => None,
        Some(DisplayValue::Int(i)) => Some(*i as f64),
        Some(DisplayValue::Real(r)) => Some(*r),
        _ => Some(50.0),
    };
    let relief = match spec.arg("relief") {
        Some(DisplayValue::Int(i)) => *i,
        Some(DisplayValue::Real(r)) => *r as i64,
        _ => 0,
    };
    let margin = match spec.arg("margin") {
        Some(DisplayValue::Int(x)) => (*x, *x),
        Some(DisplayValue::Real(x)) => (*x as i64, *x as i64),
        _ => (0, 0),
    };
    let slice = props.iter().find(|s| s.name == "slice").and_then(|s| {
        use crate::display::SliceValue;
        let vals: Vec<SliceValue> = s
            .args
            .iter()
            .filter_map(|v| match v {
                DisplayValue::Int(i) if *i >= 0 => Some(SliceValue::px(*i as u32)),
                DisplayValue::Real(r) if (0.0..=1.0).contains(r) => Some(SliceValue {
                    value: *r,
                    fraction: true,
                }),
                DisplayValue::Real(r) if *r > 1.0 => Some(SliceValue::px(*r as u32)),
                _ => None,
            })
            .collect();
        (vals.len() >= 4).then(|| [vals[0], vals[1], vals[2], vals[3]])
    });
    Some(crate::display::ImageDescriptor {
        path,
        data,
        margin,
        ascent,
        relief,
        width: num("width"),
        height: num("height"),
        max_width: num("max-width"),
        max_height: num("max-height"),
        scale: num("scale"),
        rotation: num("rotation").unwrap_or(0.0) as i32,
        flip_h: matches!(spec.arg("flip"), Some(DisplayValue::Bool(true))),
        center: false,
        slice,
    })
}

impl Buffer {
    /// The text to show for `line` instead of its own, if any. Nothing
    /// for a line the display shouldn't change (the caller keeps point's
    /// line real).
    pub fn org_display_line(&self, line: usize) -> Option<String> {
        let od = &self.org_display;
        let text = self.line(line);
        if od.table_shrink
            && is_table_line(&text)
            && let Some(t) = self.shrunk_table_line(line)
            && t != text
        {
            return Some(t);
        }
        if od.previews_line(line) && text.contains("[[") {
            let t = self.previewed_links(&text);
            if t != text {
                return Some(t);
            }
        }
        if od.pretty_entities
            && self.major_mode() == crate::org::MODE
            && text.contains('\\')
            && !crate::org_blocks::in_literal_block(
                &(0..self.line_count())
                    .map(|i| self.line(i))
                    .collect::<Vec<_>>(),
                line,
            )
        {
            let t = crate::export::entities::pretty(&text);
            if t != text {
                return Some(t);
            }
        }
        None
    }

    /// `line` with descriptive links shown: each `[[target][desc]]` becomes
    /// just `desc` (`[[target]]` becomes `target`) with
    /// `org-link-descriptive`. A link containing `avoid` (point's column,
    /// for the line point is on) is left whole so it can be edited.
    ///
    /// Returns the shown text and, for each shown char, the column it comes
    /// from (to look its face up), or None when nothing is hidden.
    pub fn org_descriptive_line(
        &self,
        line: usize,
        avoid: Option<usize>,
    ) -> Option<(String, Vec<usize>)> {
        if !self.org_display.link_descriptive || self.major_mode() != crate::org::MODE {
            return None;
        }
        let text = self.line(line);
        let links = crate::org_ext::links_in(&text);
        let hides = |l: &crate::org_ext::Link| {
            // Markup (the brackets and the target) is hidden only when
            // there is something other than the visible text to hide.
            (l.start < l.text_start || l.text_end < l.end)
                && !avoid.is_some_and(|c| c >= l.start && c < l.end)
        };
        if !links.iter().any(hides) {
            return None;
        }
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut out = String::new();
        let mut src = Vec::new();
        let mut i = 0;
        let mut emit = |range: std::ops::Range<usize>| {
            for (c, ch) in chars.iter().enumerate().take(range.end).skip(range.start) {
                out.push(*ch);
                src.push(c);
            }
        };
        for l in &links {
            if !hides(l) {
                continue;
            }
            // Text before the link, then only its visible span.
            emit(i..l.start.min(n));
            emit(l.text_start.min(n)..l.text_end.min(n));
            i = l.end;
        }
        emit(i..n);
        Some((out, src))
    }

    /// The first local picture linked on `line`, when previews are on and
    /// the file exists: what a frontend that draws pictures shows under
    /// the line.
    pub fn org_line_image(&self, line: usize) -> Option<PathBuf> {
        if !self.org_display.previews_line(line) {
            return None;
        }
        let text = self.line(line);
        if !text.contains("[[") {
            return None;
        }
        static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\[\[([^\]]+)\](?:\[[^\]]*\])?\]").expect("valid regex")
        });
        let base = self.path().and_then(|p| p.parent().map(Path::to_path_buf));
        for c in LINK.captures_iter(&text) {
            if !self.org_display.link_preview_described
                && c.get(2)
                    .map(|d| d.as_str())
                    .filter(|d| !d.is_empty())
                    .is_some()
            {
                continue;
            }
            let target = c[1].trim();
            let remote = target.starts_with("http://") || target.starts_with("https://");
            let file = target.strip_prefix("file:").unwrap_or(target);
            let file = file.split("::").next().unwrap_or(file);
            if !is_image_file_name(file) {
                continue;
            }
            if remote {
                if let Some(p) = self.org_display.remote_image(file) {
                    return Some(p);
                }
                continue;
            }
            if !(target.starts_with("file:")
                || file.starts_with('/')
                || file.starts_with("./")
                || file.starts_with("../")
                || file.starts_with("~/"))
            {
                continue;
            }
            let mut p = PathBuf::from(if let Some(rest) = file.strip_prefix("~/") {
                format!("{}/{rest}", std::env::var("HOME").unwrap_or_default())
            } else {
                file.to_string()
            });
            if p.is_relative() {
                p = base.as_ref()?.join(p);
            }
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// A picture for `line` from any source: Org link previews first,
    /// then pictures a plugin pinned there (`images.set`), then pictures
    /// another mode pinned there (the text browser pins its image cache).
    /// What a frontend that draws pictures shows under the line.
    /// The width `#+ATTR_ORG: :width 250` (or `#+ATTR_HTML`, `#+ATTR_LATEX`)
    /// above `line` asks for. ATTR_ORG wins.
    pub fn image_attr_width(&self, line: usize) -> Option<WidthSpec> {
        let mut other = None;
        let mut li = line;
        while li > 0 {
            li -= 1;
            let text = self.line(li);
            let t = text.trim();
            let Some(rest) = t.strip_prefix("#+") else {
                break;
            };
            let Some((key, value)) = rest.split_once(':') else {
                break;
            };
            let key = key.trim().to_ascii_uppercase();
            if key == "ATTR_ORG" {
                if let Some(w) = attr_line_width(value) {
                    return Some(w);
                }
            } else if key.starts_with("ATTR_") && other.is_none() {
                other = attr_line_width(value);
            }
        }
        other
    }

    /// How many rows to give the picture under an Org image link on `line`,
    /// and the width to draw it at, in points. Without a known cell size or
    /// picture size, `default_rows` and the attribute's width in pixels.
    pub fn inline_image_plan(
        &self,
        line: usize,
        path: &Path,
        avail_cols: usize,
        max_rows: usize,
        default_rows: usize,
    ) -> (usize, Option<f64>) {
        if self.org_line_image(line).is_none() {
            return (default_rows, None);
        }
        let attr = self.image_attr_width(line);
        let px_only = match attr {
            Some(WidthSpec::Pixels(p)) => Some(p),
            _ => None,
        };
        let (Some((cw, ch)), Some((nw, nh))) = (cell_size(), image_size(path)) else {
            return (default_rows, px_only);
        };
        if nw == 0 || nh == 0 {
            return (default_rows, px_only);
        }
        let avail = avail_cols as f64 * cw;
        let natural = nw as f64;
        let wanted = match attr {
            Some(WidthSpec::Pixels(p)) => p,
            Some(WidthSpec::Percent(p)) => avail * p / 100.0,
            Some(WidthSpec::Columns(c)) => c * cw,
            None => match self.org_display.image_max_width {
                MaxWidth::FillColumn => natural.min(self.fill_column as f64 * cw),
                MaxWidth::Window => natural.min(avail),
                MaxWidth::Pixels(n) => natural.min(n as f64),
                MaxWidth::None => natural,
            },
        };
        let width = wanted.min(avail).max(1.0);
        let height = nh as f64 * width / nw as f64;
        let rows = ((height / ch).ceil() as usize).clamp(1, max_rows.max(1));
        (rows, Some(width))
    }

    pub fn inline_image(&self, line: usize) -> Option<PathBuf> {
        if let Some(p) = self.org_display.latex_previews.get(&line) {
            return Some(p.clone());
        }
        if let Some(p) = self.org_line_image(line) {
            return Some(p);
        }
        if let Some(p) = self.plugin_image(line) {
            return Some(p);
        }
        if let Some(p) = self.display_image(line) {
            return Some(p);
        }
        self.inline_images.get(&line).cloned()
    }

    /// A picture from the `display` text property: an `image` specification
    /// whose range covers `line`, with a `:file` argument.
    pub fn display_image(&self, line: usize) -> Option<PathBuf> {
        self.display_image_descriptor(line).map(|d| d.path)
    }

    /// The `slice` specification a `display` property puts on `line`'s
    /// picture, as `(x, y, w, h)`.
    pub fn display_slice(&self, line: usize) -> Option<[crate::display::SliceValue; 4]> {
        self.display_image_descriptor(line).and_then(|d| d.slice)
    }

    /// The image descriptor a `display` property inserts at `line`.
    pub fn display_image_descriptor(&self, line: usize) -> Option<crate::display::ImageDescriptor> {
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let end = start + self.line_len(line);
        // An overlay wins over the buffer's text property.
        for o in &self.overlay {
            if o.end <= start || o.start >= end {
                continue;
            }
            if let Some(found) = picture_from(&o.display.0) {
                return Some(found);
            }
        }
        for r in &self.display {
            if r.end <= start || r.start >= end {
                continue;
            }
            if let Some(found) = picture_from(&r.props) {
                return Some(found);
            }
        }
        None
    }

    /// The picture the last plugin to pin `line` set, if any. Plugins pin
    /// a line in load order, so the most recent one wins when they differ.
    pub fn plugin_image(&self, line: usize) -> Option<PathBuf> {
        let owners = self.plugin_images.get(&line)?;
        // The last plugin to pin the line wins. Pictures set straight in
        // the buffer's map have no pin order: fall back to any of them.
        let last = self
            .org_display
            .plugin_pins
            .get(&line)
            .and_then(|order| order.iter().rev().find(|o| owners.contains_key(*o)));
        match last {
            Some(owner) => owners.get(owner).cloned(),
            None => owners.values().next_back().cloned(),
        }
    }

    /// Pin `owner`'s pictures to buffer lines, replacing the ones it set
    /// before (other plugins' pixels stay). Paths are used as they are;
    /// an empty list clears this owner's pictures.
    pub fn set_plugin_images(&mut self, owner: &str, images: Vec<(usize, PathBuf)>) {
        for owners in self.plugin_images.values_mut() {
            owners.remove(owner);
        }
        self.plugin_images.retain(|_, owners| !owners.is_empty());
        for owners in self.plugin_centered.values_mut() {
            owners.remove(owner);
        }
        self.plugin_centered.retain(|_, o| !o.is_empty());
        // The old pins are gone, so this plugin no longer pinned those
        // lines last.
        for order in self.org_display.plugin_pins.values_mut() {
            order.retain(|o| o != owner);
        }
        self.org_display.plugin_pins.retain(|_, o| !o.is_empty());
        for (line, path) in images {
            self.plugin_images
                .entry(line)
                .or_default()
                .insert(owner.to_string(), path);
            let order = self.org_display.plugin_pins.entry(line).or_default();
            order.retain(|o| o != owner);
            order.push(owner.to_string());
        }
    }

    /// [`Buffer::set_plugin_images`], with the pictures centred in the window.
    pub fn set_plugin_images_centered(&mut self, owner: &str, images: Vec<(usize, PathBuf)>) {
        let lines: Vec<usize> = images.iter().map(|(l, _)| *l).collect();
        self.set_plugin_images(owner, images);
        for l in lines {
            self.plugin_centered.entry(l).or_default().insert(owner.to_string());
        }
    }

    /// Whether the picture a plugin pinned to `line` is centred.
    pub fn plugin_image_centered(&self, line: usize) -> bool {
        self.plugin_image(line).is_some() && self.plugin_centered.get(&line).is_some_and(|o| !o.is_empty())
    }

    /// Remove every picture `owner` pinned (the plugin is unloading).
    pub fn clear_plugin_images(&mut self, owner: &str) {
        for owners in self.plugin_centered.values_mut() {
            owners.remove(owner);
        }
        self.plugin_centered.retain(|_, o| !o.is_empty());
        for owners in self.plugin_images.values_mut() {
            owners.remove(owner);
        }
        self.plugin_images.retain(|_, owners| !owners.is_empty());
        for order in self.org_display.plugin_pins.values_mut() {
            order.retain(|o| o != owner);
        }
        self.org_display.plugin_pins.retain(|_, o| !o.is_empty());
    }

    /// Pin pictures to buffer lines (the text browser's image cache).
    /// Replaces any pins; pass an empty map to clear.
    pub fn set_inline_images(&mut self, map: BTreeMap<usize, PathBuf>) {
        self.inline_images = map;
    }

    fn previewed_links(&self, text: &str) -> String {
        static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\[\[([^\]]+)\](?:\[([^\]]*)\])?\]").expect("valid regex")
        });
        let base = self.path().and_then(|p| p.parent().map(Path::to_path_buf));
        LINK.replace_all(text, |c: &regex::Captures| {
            let target = c[1].trim();
            let file = target.strip_prefix("file:").unwrap_or(target);
            let file = file.split("::").next().unwrap_or(file);
            let remote = file.starts_with("http://") || file.starts_with("https://");
            if !is_image_file_name(file)
                || !(target.starts_with("file:")
                    || file.starts_with('/')
                    || file.starts_with("./")
                    || file.starts_with("../")
                    || file.starts_with("~/")
                    || remote)
            {
                return c[0].to_string();
            }
            let desc = c.get(2).map(|d| d.as_str()).filter(|d| !d.is_empty());
            // By default only bare links get previews; a described link is
            // left alone unless org-link-preview was called with a numeric
            // prefix argument.
            if desc.is_some() && !self.org_display.link_preview_described {
                return c[0].to_string();
            }
            let name = Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file);
            let shown = desc.unwrap_or(name);
            if remote {
                if remote_inline_images() == RemoteImages::Skip {
                    return c[0].to_string();
                }
                return match self.org_display.remote_image(file) {
                    Some(p) => badge(&self.org_display, &p, shown),
                    None => format!("🖼 {shown} (remote)"),
                };
            }
            let mut p = PathBuf::from(file.replacen(
                '~',
                &std::env::var("HOME").unwrap_or_default(),
                if file.starts_with("~/") { 1 } else { 0 },
            ));
            if p.is_relative() {
                match &base {
                    Some(b) => p = b.join(p),
                    None => return c[0].to_string(),
                }
            }
            badge(&self.org_display, &p, shown)
        })
        .into_owned()
    }

    /// The table line at `line` drawn with `<N>` columns cut to N.
    fn shrunk_table_line(&self, line: usize) -> Option<String> {
        let (a, b) = self.org_table_bounds(line)?;
        let rows: Vec<Row> = (a..=b).map(|l| parse_row(&self.line(l))).collect();
        let mut widths: Vec<Option<usize>> = Vec::new();
        for r in &rows {
            if let Row::Data(cells) = r
                && cells.iter().any(|c| cookie_width(c).is_some())
            {
                widths = cells.iter().map(|c| cookie_width(c)).collect();
                break;
            }
        }
        if widths.iter().all(Option::is_none) {
            return None;
        }
        let ncols = rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        let cell = |c: usize, text: &str| -> String {
            match widths.get(c).copied().flatten() {
                Some(w) => clip(text.trim(), w),
                None => text.trim().to_string(),
            }
        };
        let mut col_w = vec![0usize; ncols];
        for r in &rows {
            if let Row::Data(cells) = r {
                for (c, t) in cells.iter().enumerate() {
                    col_w[c] = col_w[c].max(line_width(&cell(c, t)));
                }
            }
        }
        let indent: String = self
            .line(a)
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        Some(match &rows[line - a] {
            Row::Rule => {
                let parts: Vec<String> = col_w.iter().map(|w| "-".repeat(w + 2)).collect();
                format!("{indent}|{}|", parts.join("+"))
            }
            Row::Data(cells) => {
                let parts: Vec<String> = (0..ncols)
                    .map(|c| pad(&cell(c, cells.get(c).map_or("", String::as_str)), col_w[c]))
                    .collect();
                format!("{indent}| {} |", parts.join(" | "))
            }
        })
    }

    /// For point in a table whose header (the rows above its first rule)
    /// is above `top`: that header row's line and text.
    pub fn org_table_header_line(&self, point_line: usize, top: usize) -> Option<(usize, String)> {
        if !self.org_display.table_header {
            return None;
        }
        let (a, b) = self.org_table_bounds(point_line)?;
        let rule = (a..=b).find(|&l| matches!(parse_row(&self.line(l)), Row::Rule))?;
        if rule == a || point_line <= rule {
            return None;
        }
        let header = rule - 1;
        if header >= top {
            return None;
        }
        let text = if self.org_display.table_shrink {
            self.shrunk_table_line(header)
                .unwrap_or_else(|| self.line(header))
        } else {
            self.line(header)
        };
        Some((header, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(t: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(t);
        b
    }

    fn indented(t: &str) -> Buffer {
        let mut b = buf(t);
        b.set_major_mode(crate::org::MODE);
        b.org_display.indent = true;
        b
    }

    #[test]
    fn indent_widths_align_body_under_headline_text() {
        // per_level 2: headlines shift two left, body aligns under text.
        let b = indented("* A\nbody\n** B\ndeep\n*** C\nx\npreamble-free\n");
        assert_eq!(org_indent_width(&b, 0, 2), 0, "L1 headline");
        assert_eq!(org_indent_width(&b, 1, 2), 2, "body under `* `");
        assert_eq!(org_indent_width(&b, 2, 2), 0, "L2 headline");
        assert_eq!(org_indent_width(&b, 3, 2), 3, "body under `** `");
        assert_eq!(org_indent_width(&b, 4, 2), 2, "L3 headline");
        assert_eq!(org_indent_width(&b, 5, 2), 6, "body under `*** `");
        // A wider per-level spreads deeper headlines.
        assert_eq!(org_indent_width(&b, 4, 4), 6);
        assert_eq!(org_indent_width(&b, 5, 4), 10);
        let mut off = buf("* A\nbody\n");
        off.set_major_mode(crate::org::MODE);
        assert_eq!(org_indent_width(&off, 1, 2), 0);
        let mut plain = buf("* A\nbody\n");
        plain.org_display.indent = true;
        assert_eq!(org_indent_width(&plain, 1, 2), 0);
    }

    fn org(t: &str) -> Buffer {
        let mut b = buf(t);
        b.set_major_mode(crate::org::MODE);
        b
    }

    #[test]
    fn pretty_entities_replace_only_outside_literal_blocks() {
        let mut b = org("\\alpha_1 and x^2\n#+BEGIN_EXAMPLE\n\\alpha\n#+END_EXAMPLE\n");
        b.org_display.pretty_entities = true;
        assert_eq!(b.org_display_line(0).unwrap(), "α_1 and x^2");
        assert_eq!(b.org_display_line(2), None);
    }

    #[test]
    fn latex_preview_is_an_inline_image_for_its_line() {
        let mut b = org("$x$\n");
        b.org_display
            .latex_previews
            .insert(0, PathBuf::from("/tmp/fragment.png"));
        assert_eq!(b.inline_image(0), Some(PathBuf::from("/tmp/fragment.png")));
        assert_eq!(b.inline_image(1), None);
    }

    #[test]
    fn descriptive_links_show_only_the_description() {
        let b = org("see [[https://x.org][the site]] and [[https://y.org]] here\n");
        let (t, src) = b.org_descriptive_line(0, None).unwrap();
        assert_eq!(t, "see the site and https://y.org here");
        // Each shown char maps back to its own source column, so the face
        // lookup paints exactly the description and the target.
        let desc: Vec<usize> = (21..29).collect();
        assert_eq!(&src[4..12], &desc);
        // Point inside the first link reveals it whole; the second still
        // collapses.
        let (t2, _) = b.org_descriptive_line(0, Some(10)).unwrap();
        assert_eq!(t2, "see [[https://x.org][the site]] and https://y.org here");
        // Point outside every link collapses all of them.
        let (t3, _) = b.org_descriptive_line(0, Some(0)).unwrap();
        assert_eq!(t3, "see the site and https://y.org here");
        let plain = org("just text\n");
        assert_eq!(plain.org_descriptive_line(0, None), None);
    }

    #[test]
    fn descriptive_links_off_or_outside_org() {
        let mut b = org("[[https://x.org][x]]\n");
        b.org_display.link_descriptive = false;
        assert_eq!(b.org_descriptive_line(0, None), None);
        let outside = buf("[[https://x.org][x]]\n");
        assert_eq!(outside.org_descriptive_line(0, None), None);
    }

    #[test]
    fn shrinks_columns_with_cookies() {
        let mut b = buf("| <5> | |\n|-----+-|\n| abcdefghij | x |\n| ab | y |\n");
        assert_eq!(b.org_display_line(2), None, "off by default");
        b.org_display.table_shrink = true;
        let t = b.org_display_line(2).unwrap();
        assert!(t.contains("abcd…"), "{t}");
        assert!(!t.contains("abcdefghij"));
        // Short cells are untouched but the columns still line up.
        let short = b.org_display_line(3).unwrap_or_else(|| b.line(3));
        assert_eq!(line_width(&short), line_width(&t));
        assert_eq!(
            b.org_display_line(0).map(|s| line_width(&s)),
            Some(line_width(&t))
        );
    }

    #[test]
    fn tables_without_cookies_are_left_alone() {
        let mut b = buf("| a | b |\n| c | d |\n");
        b.org_display.table_shrink = true;
        assert_eq!(b.org_display_line(0), None);
    }

    #[test]
    fn image_links_become_badges() {
        let dir = std::env::temp_dir().join(format!("forge-orgdisp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 3x2 PNG header is enough for the size.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&2u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        std::fs::write(dir.join("p.png"), &png).unwrap();
        let mut b = buf(
            "see [[file:p.png]] and [[file:p.png][the pic]] and [[file:notes.org]] and [[https://x.org][site]]\n",
        );
        b.set_visited_file(dir.join("t.org"));
        b.org_display.link_preview = true;
        // By default only bare links preview; the described one is left as-is.
        let t = b.org_display_line(0).unwrap();
        assert!(t.contains("🖼 p.png (3×2, "), "{t}");
        assert!(t.contains("[[file:p.png][the pic]]"), "{t}");
        assert!(
            t.contains("[[file:notes.org]]") && t.contains("[[https://x.org][site]]"),
            "{t}"
        );
        // With described previews on, the described image gets a badge too.
        b.org_display.link_preview_described = true;
        let t = b.org_display_line(0).unwrap();
        assert!(t.contains("🖼 the pic (3×2"), "{t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_webp_is_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("forge-webp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.webp");
        // A RIFF/WEBP header with a chunk name and nothing after it.
        std::fs::write(&p, b"RIFF\0\0\0\0WEBPVP8X").unwrap();
        assert_eq!(image_size(&p), None);
        std::fs::write(&p, b"RIFF\0\0\0\0WEBPVP8L").unwrap();
        assert_eq!(image_size(&p), None);
        // A whole VP8X chunk gives the size.
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.extend_from_slice(&[0; 8]);
        webp.extend_from_slice(&2u32.to_le_bytes()[..3]);
        webp.extend_from_slice(&1u32.to_le_bytes()[..3]);
        std::fs::write(&p, &webp).unwrap();
        assert_eq!(image_size(&p), Some((3, 2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_pictures_use_the_last_pin() {
        let mut b = buf("one\ntwo\n");
        b.set_plugin_images("a", vec![(0, PathBuf::from("/a.png"))]);
        b.set_plugin_images("b", vec![(0, PathBuf::from("/b.png"))]);
        // "b" pinned last, even though "a" sorts last.
        assert_eq!(b.plugin_image(0), Some(PathBuf::from("/b.png")));
        // When "b" pins elsewhere, "a" is the one left on the line.
        b.set_plugin_images("b", vec![(1, PathBuf::from("/b2.png"))]);
        assert_eq!(b.plugin_image(0), Some(PathBuf::from("/a.png")));
        assert_eq!(b.plugin_image(1), Some(PathBuf::from("/b2.png")));
        b.clear_plugin_images("a");
        assert_eq!(b.plugin_image(0), None);
    }

    #[test]
    fn image_names_use_extensions_and_regexps() {
        let names = ImageNames::default();
        assert!(image_name_matches(&names, "cat.png"));
        assert!(image_name_matches(&names, "/tmp/cat.JPEG"));
        assert!(!image_name_matches(&names, "cat.txt"));
        assert!(!image_name_matches(&names, "cat"));
        assert!(is_image_file_name("https://example.com/cat.png?raw=1"));

        // Extensions and regexps can be replaced, each matched against
        // the file name only.
        let names = ImageNames {
            extensions: vec!["pic".into()],
            regexps: vec![regex::Regex::new(r"(?i)\.img\d+$").unwrap()],
        };
        assert!(image_name_matches(&names, "a.PIC"));
        assert!(image_name_matches(&names, "/tmp/a.img12"));
        assert!(!image_name_matches(&names, "a.png"));
        assert!(!image_name_matches(&names, "dir.png/a.img1x"));
    }

    #[test]
    fn remote_image_uses_the_cache_file() {
        let dir = std::env::temp_dir().join(format!("forge-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let od = OrgDisplay {
            remote_cache_dir: Some(dir.clone()),
            ..Default::default()
        };
        let url = "https://example.com/img/cat.png?x=1";
        let path = od.remote_path(url).unwrap();
        assert!(path.starts_with(&dir));
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        // Once the download finished, the next look finds it.
        std::fs::write(&path, b"not really a png").unwrap();
        assert_eq!(od.remote_image(url), Some(path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_image_is_fetched_and_cached() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = [0u8; 1024];
            let _ = s.read(&mut head);
            let body = b"\x89PNG\r\n\x1a\n";
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(body).unwrap();
        });

        let dir = std::env::temp_dir().join(format!("forge-remote-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let od = OrgDisplay {
            remote_cache_dir: Some(dir.clone()),
            ..Default::default()
        };
        let url = format!("http://{addr}/cat.png");
        assert_eq!(
            od.remote_image(&url),
            None,
            "the fetch starts in the background"
        );
        let mut fetched = None;
        for _ in 0..300 {
            if let Some(p) = od.remote_image(&url) {
                fetched = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        server.join().unwrap();
        let path = fetched.expect("image was fetched");
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG\r\n\x1a\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_image_links_preview_from_the_cache() {
        let dir = std::env::temp_dir().join(format!("forge-remote-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let url = "https://example.com/cat.png";
        let mut b = org("[[https://example.com/cat.png]]\n");
        b.org_display.link_preview = true;
        b.org_display.remote_cache_dir = Some(dir.clone());
        let path = b.org_display.remote_path(url).unwrap();
        std::fs::write(&path, b"png").unwrap();
        assert_eq!(b.org_line_image(0), Some(path));
        // The badge names it and shows its size, like a local image.
        let t = b.org_display_line(0).unwrap();
        assert!(t.contains("🖼 cat.png"), "{t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn header_line_appears_when_the_header_scrolls_off() {
        let mut b = buf("| h1 | h2 |\n|----+----|\n| a | b |\n| c | d |\n| e | f |\n");
        assert_eq!(b.org_table_header_line(4, 3), None, "off by default");
        b.org_display.table_header = true;
        assert_eq!(b.org_table_header_line(4, 3).map(|h| h.0), Some(0));
        assert_eq!(b.org_table_header_line(4, 0), None, "header is on screen");
        assert_eq!(b.org_table_header_line(1, 3), None, "point on the rule");
    }

    #[test]
    fn num_step_follows_org_num() {
        let mut n = Vec::new();
        assert_eq!(org_num_step(&mut n, 1), "1 ");
        assert_eq!(org_num_step(&mut n, 2), "1.1 ");
        assert_eq!(org_num_step(&mut n, 2), "1.2 ");
        assert_eq!(org_num_step(&mut n, 3), "1.2.1 ");
        assert_eq!(org_num_step(&mut n, 1), "2 ");
        assert_eq!(org_num_step(&mut n, 3), "2.0.1 ");
        // A document that starts deeper numbers the outer levels 0.
        let mut d = Vec::new();
        assert_eq!(org_num_step(&mut d, 3), "0.0.1 ");
        assert_eq!(org_num_step(&mut d, 3), "0.0.2 ");
        assert_eq!(org_num_step(&mut d, 2), "0.1 ");
    }

    #[test]
    fn size_readers() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2 KB");
        assert_eq!(cookie_width("<10>"), Some(10));
        assert_eq!(cookie_width(" <r8> "), Some(8));
        assert_eq!(cookie_width("<>"), None);
        assert_eq!(cookie_width("x"), None);
    }
}
