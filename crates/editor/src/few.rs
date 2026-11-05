// SPDX-License-Identifier: MIT OR Apache-2.0
//! few: Forge's text web browser. M-x few asks for a URL (or search
//! words), fetches the page in the background (reqwest, so it works the
//! same on every platform), renders it as
//! wrapped text in the *few* buffer, and lets you follow links, fill in
//! forms, go back and forward, read just an article, view the source,
//! download files and keep bookmarks. Pages are rendered by
//! forge_core::few; there is no JavaScript, and pictures show inline
//! where the frontend can draw them.

use crate::app::App;
use crate::org_cmds::{Ask, ask, choose};
use forge_core::decoration::{DecoFace, DecoSpec};
use forge_core::few::{self as engine, FieldKind, Page};
use forge_core::syntax::SyntaxFace;
use forge_core::{Buffer, BufferId, Pos};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const BUFFER: &str = "*few*";
pub const MODE: &str = "few-mode";
/// What a click does on a link, button or picture: `<mouse-1>` uses
/// whatever is under point, as RET does. A region map rather than a
/// mode binding, so it applies only on the clickable text itself.
/// Text boxes take no region: a click there just moves point for typing.
pub const CLICK_REGION: &str = "few-clickable";
/// Lines before the page text: title, address, rule.
const HEADER: usize = 3;
/// Pages bigger than this are cut off (with a notice).
const MAX_BYTES: u64 = 10_000_000;
/// History keeps this many pages; more forgets the oldest.
const MAX_PAGES: usize = 100;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; few/1.0; Forge)";

/// One shared client for every fetch: connections are reused instead of
/// re-dialed per page.
static CLIENT: std::sync::LazyLock<reqwest::blocking::Client> = std::sync::LazyLock::new(|| {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
    );
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let ok = matches!(attempt.url().scheme(), "http" | "https");
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if !ok {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .expect("few http client")
});

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "few",
        "Few: browse a URL, a file, or search the web with the words typed.",
    ),
    forge_docs::CommandDoc::new("few-browse-url", "Few: browse a URL (G in the page)."),
    forge_docs::CommandDoc::new("few-search", "Few: search the web."),
    forge_docs::CommandDoc::new(
        "few-follow-link",
        "Few: follow the link at point (or use the field, or open the picture).",
    ),
    forge_docs::CommandDoc::new("few-next-link", "Few: move to the next link or field."),
    forge_docs::CommandDoc::new(
        "few-previous-link",
        "Few: move to the previous link or field.",
    ),
    forge_docs::CommandDoc::new("few-back", "Few: go back in the history."),
    forge_docs::CommandDoc::new("few-forward", "Few: go forward in the history."),
    forge_docs::CommandDoc::new("few-reload", "Few: fetch the page again."),
    forge_docs::CommandDoc::new("few-quit", "Few: leave the browser (the history stays)."),
    forge_docs::CommandDoc::new(
        "few-copy-url",
        "Few: copy the page's address, or the link or picture at point's,",
    )
    .body("to the kill ring."),
    forge_docs::CommandDoc::new("few-view-source", "Few: show the page's source."),
    forge_docs::CommandDoc::new("few-download", "Few: save the page to a file."),
    forge_docs::CommandDoc::new(
        "few-toggle-readable",
        "Few: show only the article or main text of the page, or everything again.",
    ),
    forge_docs::CommandDoc::new(
        "few-open-external",
        "Few: open the page, or the link or picture at point, in the system browser.",
    ),
    forge_docs::CommandDoc::new("few-list-history", "Few: pick a page from the history."),
    forge_docs::CommandDoc::new("few-list-links", "Few: jump to a link on this page."),
    forge_docs::CommandDoc::new("few-toc", "Few: jump to a heading on this page."),
    forge_docs::CommandDoc::new("few-find", "Few: search this page for text."),
    forge_docs::CommandDoc::new("few-find-next", "Few: the next match on this page."),
    forge_docs::CommandDoc::new("few-find-previous", "Few: the previous match on this page."),
    forge_docs::CommandDoc::new("few-add-bookmark", "Few: bookmark this page."),
    forge_docs::CommandDoc::new("few-list-bookmarks", "Few: pick a bookmark."),
    forge_docs::CommandDoc::new("few-remove-bookmark", "Few: remove a bookmark."),
    forge_docs::CommandDoc::new(
        "few-up-url",
        "Few: go up one level in the address (eww-up-url, ^).",
    ),
    forge_docs::CommandDoc::new(
        "few-top-url",
        "Few: go to the top of the site (eww-top-url, t).",
    ),
    forge_docs::CommandDoc::new(
        "few-next-url",
        "Few: follow the page's <link rel=next> (eww-next-url, n).",
    ),
    forge_docs::CommandDoc::new(
        "few-previous-url",
        "Few: follow the page's <link rel=prev> (eww-previous-url, p).",
    ),
    forge_docs::CommandDoc::new(
        "few-copy-alternate-url",
        "Few: copy the page's <link rel=alternate> address, a feed for instance (A).",
    ),
    forge_docs::CommandDoc::new("few-open-file", "Few: show a file (eww-open-file)."),
    forge_docs::CommandDoc::new(
        "few-search-words",
        "Few: search the web for the region, or the words you type (eww-search-words).",
    ),
    forge_docs::CommandDoc::new(
        "few-toggle-images",
        "Few: show pictures, or leave them out (I).",
    )
    .body("Off, no pictures are fetched and the [alt] text shows alone."),
    forge_docs::CommandDoc::new(
        "few-toggle-colors",
        "Few: use faces for fields and buttons, or plain text (C).",
    ),
    forge_docs::CommandDoc::new(
        "few-toggle-fonts",
        "Few: emphasise headings, or draw them like other text (F).",
    ),
    forge_docs::CommandDoc::new(
        "few-set-character-encoding",
        "Few: decode this page as a character set you name, then reload (E).",
    )
    .body("Answer `auto` to go back to the page's own declaration."),
    forge_docs::CommandDoc::new(
        "few-show-alt-text",
        "Few: show the alt text of the picture at point (a).",
    ),
    forge_docs::CommandDoc::new(
        "few-browse-image",
        "Few: open the picture at point in the system viewer (i).",
    ),
    forge_docs::CommandDoc::new(
        "few-switch-to-buffer",
        "Few: switch to the browser buffer (s).",
    ),
    forge_docs::CommandDoc::new("few-list-buffers", "Few: list the browser buffers (S)."),
];

pub const BINDINGS: &[(&str, &str)] = &[
    // eww-mode-map and shr-map, where few has the command.
    ("RET", "few-follow-link"),
    ("TAB", "few-next-link"),
    ("<backtab>", "few-previous-link"),
    ("n", "few-next-url"),
    ("p", "few-previous-url"),
    ("l", "few-back"),
    ("r", "few-forward"),
    ("g", "few-reload"),
    ("q", "few-quit"),
    ("w", "few-copy-url"),
    ("u", "few-copy-url"),
    ("A", "few-copy-alternate-url"),
    ("v", "few-view-source"),
    ("d", "few-download"),
    ("R", "few-toggle-readable"),
    ("C", "few-toggle-colors"),
    ("F", "few-toggle-fonts"),
    ("I", "few-toggle-images"),
    ("E", "few-set-character-encoding"),
    ("&", "few-open-external"),
    ("^", "few-up-url"),
    ("t", "few-top-url"),
    ("H", "few-list-history"),
    ("b", "few-add-bookmark"),
    ("B", "few-list-bookmarks"),
    ("G", "few-browse-url"),
    ("s", "few-switch-to-buffer"),
    ("S", "few-list-buffers"),
    ("a", "few-show-alt-text"),
    ("i", "few-browse-image"),
    // Beyond eww: finding in the page and jumping to links and headings.
    ("L", "few-list-links"),
    ("T", "few-toc"),
    ("/", "few-find"),
    ("N", "few-find-next"),
    ("P", "few-find-previous"),
];

/// What a click does on a link, button or picture: `<mouse-1>` uses
/// whatever is under point, as RET does.
pub const CLICK_BINDINGS: &[(&str, &str)] = &[("<mouse-1>", "few-follow-link")];

#[derive(Clone)]
struct Entry {
    url: String,
    page: Page,
    source: String,
    html: bool,
    readable: bool,
    line: usize,
    /// Window width the page was wrapped at (HTML only); a resize
    /// re-renders it lazily.
    width: usize,
    /// The fetch hit the size cap and the page is cut off.
    truncated: bool,
    /// Edited form values, overlaid on every re-render.
    field_state: engine::FieldState,
    /// Downloaded pictures: page image address to local file.
    images: Vec<(String, std::path::PathBuf)>,
    /// The picture worker finished (even empty-handed).
    images_done: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Push,
    Replace,
}

/// What the worker thread sends back.
struct Fetched {
    url: String,
    ctype: String,
    status: u32,
    body: Result<String, String>,
    /// The response was bigger than the cap and was cut off.
    truncated: bool,
    /// Cookies the response set: (name, value, domain, path).
    set_cookies: Vec<(String, String, String, String)>,
}

/// How the worker got the page.
#[derive(Clone)]
enum FetchMethod {
    Get,
    Post(String),
}

struct Pending {
    rx: Receiver<Fetched>,
    url: String,
    mode: Mode,
    readable: bool,
}

/// One cookie: value sent to the domain and its subdomains under path.
#[derive(Clone)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    path: String,
}

struct ImgPending {
    rx: Receiver<Vec<(String, std::path::PathBuf)>>,
    page_url: String,
    at: usize,
}

struct DlPending {
    rx: Receiver<Result<Vec<u8>, String>>,
    url: String,
    path: std::path::PathBuf,
}

#[derive(Default)]
pub struct FewState {
    pending: Option<Pending>,
    img_pending: Option<ImgPending>,
    dl_pending: Option<DlPending>,
    history: Vec<Entry>,
    at: usize,
    bookmarks: Vec<(String, String)>,
    bookmarks_loaded: bool,
    buffer: Option<BufferId>,
    /// Case folds unless the query has an uppercase letter.
    find: String,
    cookies: Vec<Cookie>,
    /// The field a `FewField` answer edits.
    field_edit: Option<usize>,
    /// eww's toggles: pictures, colours (faces on fields and buttons) and
    /// fonts (heading emphasis).
    images_off: bool,
    colors_off: bool,
    fonts_off: bool,
}

impl FewState {
    pub fn needs_poll(&self) -> bool {
        self.pending.is_some() || self.img_pending.is_some() || self.dl_pending.is_some()
    }
}

// ----------------------------------------------------------- fetching

fn width(app: &App) -> usize {
    let w = app.win().width;
    if w < 30 {
        80
    } else {
        w.saturating_sub(2).min(100)
    }
}

fn load(app: &mut App, url: &str, mode: Mode, readable: bool) {
    start_fetch(app, url, FetchMethod::Get, mode, readable);
}

/// Submit a form by POST.
fn load_post(app: &mut App, url: &str, body: String, mode: Mode) {
    start_fetch(app, url, FetchMethod::Post(body), mode, false);
}

fn start_fetch(app: &mut App, url: &str, method: FetchMethod, mode: Mode, readable: bool) {
    app.few.pending = None;
    if !url.contains("://") {
        if !matches!(method, FetchMethod::Get) {
            app.error("few: cannot post to a file".to_string());
            return;
        }
        // A file. A submitted query or fragment is not part of the path.
        let path = crate::vc::expand_home(url.split(['?', '#']).next().unwrap_or(url));
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (text, truncated) = decode_capped(&bytes, "", true);
                let url = match file_url(&path) {
                    Some(url) => url,
                    None => {
                        app.error(format!("few: cannot make a URL for {}", path.display()));
                        return;
                    }
                };
                let html = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm")
                });
                finish(
                    app,
                    &url,
                    mode,
                    readable,
                    text,
                    html,
                    None,
                    truncated,
                );
            }
            Err(e) => app.error(format!("few: cannot read {}: {e}", path.display())),
        }
        return;
    }
    if url.starts_with("file://") {
        if !matches!(method, FetchMethod::Get) {
            app.error("few: cannot post to a file".to_string());
            return;
        }
        let path = match reqwest::Url::parse(url).ok().and_then(|url| url.to_file_path().ok()) {
            Some(path) => path,
            None => {
                app.error(format!("few: invalid file URL: {url}"));
                return;
            }
        };
        return load(app, &path.to_string_lossy(), mode, readable);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        app.error(format!(
            "few: cannot open {url} here; use & to hand it to the system"
        ));
        return;
    }
    // Dropping the old receiver abandons that fetch: its thread finishes
    // on its own and its answer goes nowhere.
    let (tx, rx) = mpsc::channel();
    let target = url.to_string();
    let cookies = cookie_header(&app.few.cookies, url);
    let spawned = std::thread::Builder::new()
        .name("few-fetch".into())
        .spawn(move || {
            let _ = tx.send(fetch(&target, &method, &cookies));
        });
    match spawned {
        Ok(_) => {
            app.few.pending = Some(Pending {
                rx,
                url: url.to_string(),
                mode,
                readable,
            });
            app.msg(format!("few: fetching {url}…"));
        }
        Err(e) => app.error(format!("few: cannot start the fetch: {e}")),
    }
}

/// Fetch `url` (http or https) on the calling thread.
fn fetch(url: &str, method: &FetchMethod, cookies: &str) -> Fetched {
    use std::io::Read;
    let failed = |e: String| Fetched {
        url: url.to_string(),
        ctype: String::new(),
        status: 0,
        body: Err(e),
        truncated: false,
        set_cookies: Vec::new(),
    };
    let mut req = match method {
        FetchMethod::Get => CLIENT.get(url),
        FetchMethod::Post(body) => CLIENT
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body.clone()),
    };
    if !cookies.is_empty() {
        req = req.header(reqwest::header::COOKIE, cookies.to_string());
    }
    let resp = match req.send() {
        Ok(r) => r,
        Err(e) => return failed(describe(&e)),
    };
    let final_url = resp.url().to_string();
    let status = resp.status().as_u16() as u32;
    let headers = resp.headers().clone();
    let ctype = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let set_cookies = parse_set_cookies(&headers, &final_url);
    if !ctype.is_empty() && !is_textual(&ctype) {
        // Binary (images, PDFs, …): don't pay to decode megabytes of it;
        // the poll step shows the stub from the content type.
        return Fetched {
            url: final_url,
            ctype,
            status,
            body: Ok(String::new()),
            truncated: false,
            set_cookies,
        };
    }
    let mut bytes = Vec::new();
    // One byte past the cap tells a truncated page from a whole one.
    if let Err(e) = resp.take(MAX_BYTES + 1).read_to_end(&mut bytes) {
        return Fetched {
            url: final_url,
            ctype,
            status,
            body: Err(format!("reading the page failed: {e}")),
            truncated: false,
            set_cookies,
        };
    }
    let truncated = (bytes.len() as u64) > MAX_BYTES;
    if truncated {
        bytes.truncate(MAX_BYTES as usize);
    }
    let ctype_for_sniff = ctype.clone();
    Fetched {
        url: final_url,
        ctype,
        status,
        body: Ok(decode_bytes(&bytes, &ctype_for_sniff)),
        truncated,
        set_cookies,
    }
}

/// Split `http(s)://host/path` into (host, path-or-/).
fn split_host_path(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    }
}

/// Cookies for `url`: domain match (and subdomains), path prefix.
fn cookie_header(jar: &[Cookie], url: &str) -> String {
    let (host, path) = split_host_path(url);
    let host = host.to_ascii_lowercase();
    jar.iter()
        .filter(|c| {
            let d = c.domain.to_ascii_lowercase();
            (host == d || host.ends_with(&format!(".{d}"))) && path.starts_with(&c.path)
        })
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Remember what the response set. An empty value deletes the cookie.
fn store_cookies(jar: &mut Vec<Cookie>, set: Vec<(String, String, String, String)>, url: &str) {
    let (host, _) = split_host_path(url);
    for (name, value, domain, path) in set {
        let domain = domain.trim_start_matches('.');
        let domain = if domain.is_empty() {
            host.to_string()
        } else {
            domain.to_string()
        };
        let path = if path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        jar.retain(|c| !(c.name == name && c.domain == domain && c.path == path));
        if !value.is_empty() {
            jar.push(Cookie {
                name,
                value,
                domain,
                path,
            });
        }
    }
    const MAX_COOKIES: usize = 500;
    if jar.len() > MAX_COOKIES {
        jar.drain(..jar.len() - MAX_COOKIES);
    }
}

/// `(name, value, domain, path)` from `Set-Cookie` headers.
fn parse_set_cookies(
    headers: &reqwest::header::HeaderMap,
    url: &str,
) -> Vec<(String, String, String, String)> {
    let (host, _) = split_host_path(url);
    let mut out = Vec::new();
    for v in headers.get_all(reqwest::header::SET_COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        let mut parts = s.split(';');
        let Some(pair) = parts.next() else { continue };
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches('"').to_string();
        let mut domain = host.to_string();
        let mut path = "/".to_string();
        for attr in parts {
            let attr = attr.trim();
            if let Some(rest) = attr
                .get(..7)
                .filter(|p| p.eq_ignore_ascii_case("domain="))
                .and_then(|_| attr.get(7..))
            {
                if !rest.trim().is_empty() {
                    domain = rest.trim().to_string();
                }
            } else if let Some(rest) = attr
                .get(..5)
                .filter(|p| p.eq_ignore_ascii_case("path="))
                .and_then(|_| attr.get(5..))
                && !rest.trim().is_empty()
            {
                path = rest.trim().to_string();
            }
        }
        out.push((name, value, domain, path));
    }
    out
}

/// Content types few renders rather than stubs.
fn is_textual(ctype: &str) -> bool {
    ctype.starts_with("text/")
        || ["xml(", "json(", "javascript(", "html("]
            .iter()
            .any(|k| ctype.contains(k))
}

/// Decode `bytes` in the charset the headers claim, else the page's
/// `<meta charset>`, else UTF-8 with replacements.
/// The character set the user chose with `few-set-character-encoding`.
static FORCED_CHARSET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn forced_charset() -> Option<String> {
    FORCED_CHARSET.lock().ok().and_then(|g| g.clone())
}

fn decode_bytes(bytes: &[u8], ctype: &str) -> String {
    let label = forced_charset()
        .or_else(|| charset_from_content_type(ctype).map(str::to_string))
        .or_else(|| charset_from_meta(bytes));
    let enc = label
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    enc.decode(bytes).0.into_owned()
}

/// Decode with a cap, for local files: returns the text and whether it
/// was cut off. `html` allows the meta-tag sniff.
fn decode_capped(bytes: &[u8], ctype: &str, sniff_meta: bool) -> (String, bool) {
    let truncated = (bytes.len() as u64) > MAX_BYTES;
    let bytes = if truncated {
        &bytes[..MAX_BYTES as usize]
    } else {
        bytes
    };
    let label = charset_from_content_type(ctype)
        .map(str::to_string)
        .or_else(|| sniff_meta.then(|| charset_from_meta(bytes)).flatten());
    let enc = label
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    (enc.decode(bytes).0.into_owned(), truncated)
}

/// `charset=` from a `Content-Type` header value.
fn charset_from_content_type(ctype: &str) -> Option<&str> {
    ctype.split(';').skip(1).find_map(|p| {
        let p = p.trim();
        let rest = p.strip_prefix("charset=")?;
        Some(rest.trim().trim_matches('"').trim_matches('\''))
    })
}

/// `<meta charset=…>` (or an http-equiv content with one) in the first
/// 2KB. The tag itself is ASCII, so lossy scanning is safe.
fn charset_from_meta(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    let lower = head.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("<meta") {
        rest = &rest[i + 5..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let Some(cs) = tag.split("charset").nth(1) {
            let cs = cs.trim_start_matches(['=', ' ', '"', '\'']);
            let cs = cs.split([' ', '"', '\'', ';', '>']).next().unwrap_or("");
            if !cs.is_empty() {
                return Some(cs.to_string());
            }
        }
        rest = &rest[end..];
    }
    None
}

fn describe(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg = format!("{msg}: {s}");
        src = s.source();
    }
    msg
}

/// Called when idle: take the fetch's answer if it has come, take the
/// pictures and downloads too.
pub fn poll(app: &mut App) {
    poll_page(app);
    poll_images(app);
    poll_download(app);
}

fn poll_page(app: &mut App) {
    let Some(p) = app.few.pending.as_ref() else {
        return;
    };
    let got = match p.rx.try_recv() {
        Ok(f) => Ok(f),
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(()),
    };
    let Some(p) = app.few.pending.take() else {
        return;
    };
    let f = match got {
        Ok(f) => f,
        Err(()) => return app.error(format!("few: the fetch of {} stopped unexpectedly", p.url)),
    };
    store_cookies(&mut app.few.cookies, f.set_cookies, &f.url);
    let body = match f.body {
        Ok(b) => b,
        Err(e) => return app.error(format!("few: {} failed ({e})", p.url)),
    };
    let textual = f.ctype.is_empty() || is_textual(&f.ctype);
    if !textual {
        let page = Page {
            title: f.url.clone(),
            lines: vec![
                format!("This is {}, which few cannot show.", f.ctype),
                String::new(),
                "Press & to open it with the system, D to download it, or w to copy the address."
                    .to_string(),
            ],
            links: Vec::new(),
            headings: Vec::new(),
            fields: Vec::new(),
            forms: Vec::new(),
            field_spans: Vec::new(),
            images: Vec::new(),
            image_spans: Vec::new(),
        };
        place(
            app,
            Entry {
                url: f.url,
                page,
                source: String::new(),
                html: false,
                readable: false,
                line: 0,
                width: 0,
                truncated: false,
                field_state: engine::FieldState::default(),
                images: Vec::new(),
                images_done: true,
            },
            p.mode,
        );
        return;
    }
    let html =
        f.ctype.contains("html") || (f.ctype.is_empty() && body.trim_start().starts_with('<'));
    finish(
        app,
        &f.url,
        p.mode,
        p.readable,
        body,
        html,
        Some(f.status),
        f.truncated,
    );
    maybe_fetch_images(app);
}

/// Pictures for the shown page arrive in the background, after the text
/// (which must not wait on twenty image downloads).
fn poll_images(app: &mut App) {
    let Some(p) = app.few.img_pending.as_ref() else {
        return;
    };
    let got = match p.rx.try_recv() {
        Ok(g) => Ok(g),
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(()),
    };
    let Some(p) = app.few.img_pending.take() else {
        return;
    };
    let got = match got {
        Ok(g) => g,
        Err(()) => {
            if let Some(e) = app.few.history.get_mut(p.at)
                && e.url == p.page_url
            {
                e.images_done = true;
            }
            return;
        }
    };
    if let Some(e) = app.few.history.get_mut(p.at)
        && e.url == p.page_url
    {
        e.images.extend(got);
        e.images_done = true;
        refresh_images(app);
    }
}

/// A download finished: write it out.
fn poll_download(app: &mut App) {
    let Some(p) = app.few.dl_pending.as_ref() else {
        return;
    };
    let got = match p.rx.try_recv() {
        Ok(g) => Ok(g),
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(()),
    };
    let Some(p) = app.few.dl_pending.take() else {
        return;
    };
    match got {
        Err(()) => app.error(format!(
            "few: the download of {} stopped unexpectedly",
            p.url
        )),
        Ok(Err(e)) => app.error(format!("few: cannot download {} ({e})", p.url)),
        Ok(Ok(bytes)) => match std::fs::write(&p.path, &bytes) {
            Ok(()) => app.msg(format!(
                "Saved {} ({} bytes)",
                p.path.display(),
                bytes.len()
            )),
            Err(e) => app.error(format!("few: cannot write {}: {e}", p.path.display())),
        },
    }
}

/// Start downloading the shown page's pictures, if any are missing.
fn maybe_fetch_images(app: &mut App) {
    if app.few.img_pending.is_some() || app.few.images_off {
        return;
    }
    let at = app.few.at;
    let missing: Vec<String> = match app.few.history.get(at) {
        Some(e) if e.html && !e.images_done => e
            .page
            .images
            .iter()
            .map(|i| i.src.clone())
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .filter(|s| !e.images.iter().any(|(u, _)| u == s))
            .collect(),
        _ => return,
    };
    if missing.is_empty() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let page_url = app.few.history[at].url.clone();
    let spawned = std::thread::Builder::new()
        .name("few-images".into())
        .spawn(move || {
            let _ = tx.send(download_images(&missing));
        });
    if spawned.is_ok() {
        app.few.img_pending = Some(ImgPending { rx, page_url, at });
    }
}

/// Where downloaded pictures live this session.
fn img_cache_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("forge-few-img-{}", std::process::id()))
}

/// Pictures bigger than this don't load inline (open them with &).
const MAX_IMAGE_BYTES: u64 = 3_000_000;

fn download_images(srcs: &[String]) -> Vec<(String, std::path::PathBuf)> {
    use std::io::Read;
    let dir = img_cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut out = Vec::new();
    for src in srcs {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        let name = format!("{:x}", h.finish());
        // Extension from the address, else the content type.
        let ext = src
            .rsplit(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let mut path = dir.join(format!("{name}{ext}"));
        if path.is_file() {
            out.push((src.clone(), path));
            continue;
        }
        let data = match CLIENT
            .get(src)
            .timeout(std::time::Duration::from_secs(15))
            .send()
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        let ctype = data
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if !ctype.is_empty() && !ctype.starts_with("image/") && !ctype.contains("octet-stream") {
            continue;
        }
        if ext.is_empty() {
            let guess = if ctype.contains("png") {
                ".png"
            } else if ctype.contains("jpeg") || ctype.contains("jpg") {
                ".jpg"
            } else if ctype.contains("gif") {
                ".gif"
            } else if ctype.contains("webp") {
                ".webp"
            } else if ctype.contains("svg") {
                ".svg"
            } else {
                ".img"
            };
            path.set_extension(&guess[1..]);
        }
        if path.is_file() {
            out.push((src.clone(), path));
            continue;
        }
        let mut bytes = Vec::new();
        if data
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        bytes.truncate(MAX_IMAGE_BYTES as usize);
        if std::fs::write(&path, &bytes).is_ok() {
            out.push((src.clone(), path));
        }
    }
    out
}

fn file_url(path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path)
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|dir| dir.join(path)))?;
    reqwest::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

/// Pictures that need no downloading: `file://` addresses that exist.
fn local_images(page: &Page, _url: &str) -> Vec<(String, std::path::PathBuf)> {
    page.images
        .iter()
        .filter_map(|img| local_path(&img.src).map(|p| (img.src.clone(), p)))
        .collect()
}

/// A `file://` address as a path, when the file is there (decoding any
/// %XX escapes first).
fn local_path(src: &str) -> Option<std::path::PathBuf> {
    let raw = src.strip_prefix("file://")?;
    let decoded = percent_decode_path(raw);
    let p = std::path::PathBuf::from(&decoded);
    p.is_file().then_some(p)
}

fn percent_decode_path(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as char);
            i += 3;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Buffer lines (with the header) holding downloaded pictures.
fn image_map(e: &Entry) -> BTreeMap<usize, std::path::PathBuf> {
    let mut map = BTreeMap::new();
    for (i, img) in e.page.images.iter().enumerate() {
        let path = e
            .images
            .iter()
            .find(|(u, _)| *u == img.src)
            .map(|(_, p)| p.clone())
            .or_else(|| local_path(&img.src));
        let Some(path) = path else { continue };
        if let Some(s) = e.page.image_spans.iter().find(|s| s.image == i) {
            map.entry(s.line + HEADER).or_insert(path);
        }
    }
    map
}

/// Pin the current entry's pictures to the buffer, when it's shown.
fn refresh_images(app: &mut App) {
    let at = app.few.at;
    let Some(e) = app.few.history.get(at).cloned() else {
        return;
    };
    let Some(id) = app.few.buffer else { return };
    if app.active_id() != id {
        return;
    }
    let map = if app.few.images_off {
        BTreeMap::new()
    } else {
        image_map(&e)
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_inline_images(map);
    }
}

fn finish(
    app: &mut App,
    url: &str,
    mode: Mode,
    readable: bool,
    body: String,
    html: bool,
    status: Option<u32>,
    truncated: bool,
) {
    let w = width(app);
    let page = if html {
        engine::render_html(&body, url, w, readable)
    } else {
        engine::render_text(&body)
    };
    let field_state = engine::FieldState::capture(&page);
    // Local pictures resolve without downloading.
    let images = if html {
        local_images(&page, url)
    } else {
        Vec::new()
    };
    place(
        app,
        Entry {
            url: url.to_string(),
            page,
            source: body,
            html,
            readable,
            line: 0,
            width: if html { w } else { 0 },
            truncated,
            field_state,
            images,
            images_done: false,
        },
        mode,
    );
    if let Some(s) = status.filter(|s| *s >= 400) {
        app.error(format!("few: the server answered {s}"));
    }
}

fn place(app: &mut App, entry: Entry, mode: Mode) {
    let st = &mut app.few;
    match mode {
        Mode::Push => {
            st.history
                .truncate(if st.history.is_empty() { 0 } else { st.at + 1 });
            st.history.push(entry);
            // Long sessions forget the oldest pages.
            if st.history.len() > MAX_PAGES {
                st.history.drain(..st.history.len() - MAX_PAGES);
            }
            st.at = st.history.len() - 1;
        }
        Mode::Replace => {
            if st.history.is_empty() {
                st.history.push(entry);
                st.at = 0;
            } else {
                let at = st.at;
                let keep = st.history[at].line;
                st.history[at] = Entry {
                    line: keep,
                    ..entry
                };
            }
        }
    }
    show(app);
}

fn remember_point(app: &mut App) {
    if app.buf_ref().major_mode() == MODE {
        let line = app.buf_ref().point().line;
        let at = app.few.at;
        if let Some(h) = app.few.history.get_mut(at) {
            h.line = line;
        }
    }
}

/// Put the current history entry in the buffer.
fn show(app: &mut App) {
    show_impl(app, true);
}

/// Same, without the echo-area announcement: for re-wraps, where the
/// message would flicker every step of a resize drag.
fn show_quiet(app: &mut App) {
    show_impl(app, false);
}

/// Re-wrap the shown page after a resize. Both frontends call this every
/// frame; it returns cheaply when the width didn't change.
pub fn maybe_rewrap(app: &mut App) {
    if app.few.history.is_empty() {
        return;
    }
    let Some(id) = app.few.buffer else { return };
    if app.active_id() != id || app.buf_ref().major_mode() != MODE {
        return;
    }
    let w = width(app);
    let stale = app
        .few
        .history
        .get(app.few.at)
        .is_some_and(|e| e.html && e.width != w);
    if !stale {
        return;
    }
    remember_point(app);
    show_impl(app, false);
}

fn show_impl(app: &mut App, announce: bool) {
    // A resize since the render re-wraps the page first, so back and
    // forward never show a stale width either. Edited form values
    // survive the re-render.
    let w = width(app);
    {
        let at = app.few.at;
        if let Some(e) = app.few.history.get_mut(at)
            && e.html
            && e.width != w
        {
            let st = e.field_state.clone();
            e.page = engine::render_html_with(&e.source, &e.url, w, e.readable, &st);
            e.width = w;
        }
    }
    let Some(e) = app.few.history.get(app.few.at).cloned() else {
        return;
    };
    let mut lines = vec![
        if e.page.title.is_empty() {
            e.url.clone()
        } else {
            e.page.title.clone()
        },
        e.url.clone(),
        "─".repeat(width(app).min(72)),
    ];
    lines.extend(e.page.lines.iter().cloned());
    // Character offsets of each buffer line, for decorations.
    let mut starts: Vec<usize> = Vec::with_capacity(lines.len());
    let mut off = 0usize;
    for l in &lines {
        starts.push(off);
        off += l.chars().count() + 1;
    }
    let id = match app
        .few
        .buffer
        .filter(|id| app.editor.state.buffers.contains_key(id))
        .or_else(|| app.editor.find_buffer(BUFFER))
    {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(BUFFER)),
    };
    app.few.buffer = Some(id);
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_read_only(false);
        b.set_contents(&lines.join("\n"));
        b.set_major_mode(MODE);
        b.set_read_only(true);
        // Faces: links underlined (their address on the echo line),
        // headings in the keyword face, editable fields standing out.
        // Links, buttons and pictures also take the click region, so
        // <mouse-1> uses them like RET does; text boxes take none, so a
        // click there just moves point for typing.
        let at = |line: usize, col: usize| starts.get(line).map(|s| s + col).unwrap_or(off);
        let mut specs: Vec<DecoSpec> = Vec::new();
        for l in &e.page.links {
            let (bl, s, en) = (
                l.line + HEADER,
                at(l.line + HEADER, l.start),
                at(l.line + HEADER, l.end),
            );
            if s < en && bl < starts.len() {
                specs.push(
                    DecoSpec::new(s, en, DecoFace::Underline)
                        .message(l.href.clone())
                        .keymap(CLICK_REGION),
                );
            }
        }
        for h in e.page.headings.iter().filter(|_| !app.few.fonts_off) {
            let bl = h.line + HEADER;
            if bl < lines.len() && !lines[bl].is_empty() {
                let s = starts[bl];
                specs.push(DecoSpec::new(
                    s,
                    s + lines[bl].chars().count(),
                    DecoFace::Syntax(SyntaxFace::Keyword),
                ));
            }
        }
        for s in &e.page.field_spans {
            let bl = s.line + HEADER;
            if bl >= starts.len() {
                continue;
            }
            let (st, en) = (at(bl, s.start), at(bl, s.end));
            if st >= en {
                continue;
            }
            let Some(f) = e.page.fields.get(s.field) else {
                continue;
            };
            let face = match f.kind {
                FieldKind::Submit | FieldKind::Button => DecoFace::Info,
                _ => DecoFace::Highlight,
            };
            let face = if app.few.colors_off {
                DecoFace::Underline
            } else {
                face
            };
            let spec = DecoSpec::new(st, en, face);
            // Buttons and toggles follow clicks; text boxes don't (a
            // click there moves point for typing instead).
            match f.kind {
                FieldKind::Text | FieldKind::Password | FieldKind::TextArea | FieldKind::Hidden => {
                    specs.push(spec)
                }
                _ => specs.push(spec.keymap(CLICK_REGION)),
            }
        }
        for s in &e.page.image_spans {
            let bl = s.line + HEADER;
            if bl >= starts.len() {
                continue;
            }
            let (st, en) = (at(bl, s.start), at(bl, s.end));
            if st >= en {
                continue;
            }
            let Some(img) = e.page.images.get(s.image) else {
                continue;
            };
            specs.push(
                DecoSpec::new(st, en, DecoFace::Underline)
                    .message(img.src.clone())
                    .keymap(CLICK_REGION),
            );
        }
        b.set_decorations("few", specs);
        b.set_inline_images(if app.few.images_off {
            BTreeMap::new()
        } else {
            image_map(&e)
        });
    }
    if app.active_id() != id {
        app.switch_to(id);
    }
    let last = app.buf_ref().line_count().saturating_sub(1);
    let (line, col) = if e.line == 0 {
        first_link(&e).unwrap_or((HEADER, 0))
    } else {
        (e.line, 0)
    };
    app.buf().set_point(Pos::new(line.min(last), col));
    app.win_mut().scroll_top = 0;
    app.win_mut().scroll_y_px = 0;
    if announce {
        let n = e.page.links.len();
        let mut m = format!("{} — {n} link{}", e.url, if n == 1 { "" } else { "s" });
        if e.truncated {
            m.push_str(&format!(" (truncated at {} MB)", MAX_BYTES / 1_000_000));
        }
        app.msg(m);
    }
}

fn first_link(e: &Entry) -> Option<(usize, usize)> {
    e.page.links.first().map(|l| (l.line + HEADER, l.start))
}

// ------------------------------------------------------------ commands

fn current(app: &App) -> Option<&Entry> {
    app.few.history.get(app.few.at)
}

fn in_few(app: &mut App) -> bool {
    let ok = app.buf_ref().major_mode() == MODE && !app.few.history.is_empty();
    if !ok {
        app.error("Not in a few page; start with M-x few");
    }
    ok
}

/// The link under point, if any.
fn link_at(app: &App) -> Option<engine::Link> {
    let e = current(app)?;
    let p = app.buf_ref().point();
    let line = p.line.checked_sub(HEADER)?;
    e.page
        .links
        .iter()
        .find(|l| l.line == line && p.col >= l.start && p.col < l.end)
        .cloned()
}

/// The form field under point, if any.
fn field_at(app: &App) -> Option<usize> {
    let e = current(app)?;
    let p = app.buf_ref().point();
    let line = p.line.checked_sub(HEADER)?;
    e.page
        .field_spans
        .iter()
        .find(|s| s.line == line && p.col >= s.start && p.col < s.end)
        .map(|s| s.field)
}

/// The picture under point, if any.
fn image_at(app: &App) -> Option<engine::PageImage> {
    let e = current(app)?;
    let p = app.buf_ref().point();
    let line = p.line.checked_sub(HEADER)?;
    e.page
        .image_spans
        .iter()
        .find(|s| s.line == line && p.col >= s.start && p.col < s.end)
        .and_then(|s| e.page.images.get(s.image).cloned())
}

/// Point is on an editable text box (text, password, textarea), in the
/// browser's own buffer: the field and the value index under point.
/// Plain typing there edits the box (below) instead of running
/// few-mode's single-letter commands.
fn text_field_at(app: &App) -> Option<(usize, usize)> {
    if app.buf_ref().major_mode() != MODE || app.few.history.is_empty() {
        return None;
    }
    let e = current(app)?;
    let p = app.buf_ref().point();
    let line = p.line.checked_sub(HEADER)?;
    let s = e
        .page
        .field_spans
        .iter()
        .find(|s| s.line == line && p.col >= s.start && p.col < s.end)?;
    let f = e.page.fields.get(s.field)?;
    if !matches!(
        f.kind,
        FieldKind::Text | FieldKind::Password | FieldKind::TextArea
    ) {
        return None;
    }
    Some((s.field, value_index(e, s.field, line, p.col)))
}

/// Which value character point's column means: the label's brackets
/// live on the first and last visual lines of a wrapped box.
fn value_index(e: &Entry, field: usize, line: usize, col: usize) -> usize {
    let mut spans: Vec<_> = e
        .page
        .field_spans
        .iter()
        .filter(|s| s.field == field)
        .collect();
    spans.sort_by_key(|s| (s.line, s.start));
    let mut consumed = 0usize;
    for (i, s) in spans.iter().enumerate() {
        let len = s.end.saturating_sub(s.start);
        let lead = if i == 0 { 1 } else { 0 };
        let trail = if i + 1 == spans.len() { 1 } else { 0 };
        let vlen = len.saturating_sub(lead + trail);
        if s.line == line && col >= s.start && col < s.end {
            let into = col.saturating_sub(s.start + lead);
            return consumed + into.min(vlen);
        }
        consumed += vlen;
    }
    consumed
}

/// Put point on a field's value character: the reverse of `value_index`.
fn place_on_value(app: &mut App, field: usize, idx: usize) {
    let at = app.few.at;
    let Some(e) = app.few.history.get(at) else {
        return;
    };
    let mut spans: Vec<_> = e
        .page
        .field_spans
        .iter()
        .filter(|s| s.field == field)
        .collect();
    spans.sort_by_key(|s| (s.line, s.start));
    let mut remaining = idx;
    let mut target = spans.last().map(|s| (s.line, s.end.saturating_sub(1)));
    for (i, s) in spans.iter().enumerate() {
        let len = s.end.saturating_sub(s.start);
        let lead = if i == 0 { 1 } else { 0 };
        let trail = if i + 1 == spans.len() { 1 } else { 0 };
        let vlen = len.saturating_sub(lead + trail);
        if remaining <= vlen {
            target = Some((
                s.line,
                (s.start + lead + remaining).min(s.end.saturating_sub(trail).max(s.start)),
            ));
            break;
        }
        remaining -= vlen;
    }
    if let Some((line, col)) = target {
        let last = app.buf_ref().line_count().saturating_sub(1);
        app.buf()
            .set_point(Pos::new((line + HEADER).min(last), col));
    }
}

/// Plain typing (and Backspace/Delete) on a text box edits it inline,
/// like eww: the keys never reach few-mode's commands. True when the
/// key was consumed.
pub fn handle_key(app: &mut App, key: &KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    match key.code {
        KeyCode::Char(c) if !c.is_control() => insert_char(app, c),
        KeyCode::Backspace | KeyCode::Delete => delete_char(app, key.code == KeyCode::Delete),
        _ => false,
    }
}

/// Type into the text box under point.
fn insert_char(app: &mut App, c: char) -> bool {
    let Some((field, idx)) = text_field_at(app) else {
        return false;
    };
    let at = app.few.at;
    if let Some(e) = app.few.history.get_mut(at)
        && let Some(f) = e.page.fields.get_mut(field)
    {
        let mut v: Vec<char> = f.value.chars().collect();
        let idx = idx.min(v.len());
        v.insert(idx, c);
        f.value = v.into_iter().collect();
        e.field_state = engine::FieldState::capture(&e.page);
        redisplay_entry(app);
        place_on_value(app, field, idx + 1);
    }
    true
}

/// Backspace (or Delete) in the text box under point.
fn delete_char(app: &mut App, forward: bool) -> bool {
    let Some((field, idx)) = text_field_at(app) else {
        return false;
    };
    let at = app.few.at;
    let mut new_idx = idx;
    if let Some(e) = app.few.history.get_mut(at)
        && let Some(f) = e.page.fields.get_mut(field)
    {
        let mut v: Vec<char> = f.value.chars().collect();
        if forward {
            if idx < v.len() {
                v.remove(idx);
            }
        } else if idx > 0 {
            v.remove(idx - 1);
            new_idx = idx - 1;
        }
        f.value = v.into_iter().collect();
        e.field_state = engine::FieldState::capture(&e.page);
        redisplay_entry(app);
        place_on_value(app, field, new_idx);
    }
    true
}

/// TAB stops: links and the first line of every shown field, in order.
fn tab_targets(e: &Entry) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = e.page.links.iter().map(|l| (l.line, l.start)).collect();
    for (i, _) in e.page.fields.iter().enumerate() {
        if let Some(s) = e
            .page
            .field_spans
            .iter()
            .filter(|s| s.field == i)
            .min_by_key(|s| (s.line, s.start))
        {
            v.push((s.line, s.start));
        }
    }
    v.sort();
    v.dedup();
    v
}

fn goto_link(app: &mut App, forward: bool) {
    let Some(e) = current(app) else { return };
    let stops = tab_targets(e);
    if stops.is_empty() {
        app.msg("No links or fields on this page".to_string());
        return;
    }
    let p = app.buf_ref().point();
    let here = (p.line.saturating_sub(HEADER), p.col);
    let target = if forward {
        stops
            .iter()
            .find(|t| p.line >= HEADER && **t > here)
            .or(stops.first())
    } else {
        stops
            .iter()
            .rev()
            .find(|t| p.line >= HEADER && **t < here)
            .or(stops.last())
    };
    match target {
        Some((line, col)) => app.buf().set_point(Pos::new(*line + HEADER, *col)),
        None => app.msg("No links or fields on this page".to_string()),
    }
}

fn history_labels(app: &App) -> Vec<String> {
    app.few
        .history
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "{}{} — {}",
                if i == app.few.at { "> " } else { "  " },
                if e.page.title.is_empty() {
                    "(untitled)"
                } else {
                    &e.page.title
                },
                e.url
            )
        })
        .collect()
}

/// Bookmarks live in the session (surviving restarts); load them once.
fn ensure_bookmarks(app: &mut App) {
    if !app.few.bookmarks_loaded {
        app.few.bookmarks = app.editor.session.few_bookmarks();
        app.few.bookmarks_loaded = true;
    }
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("few") {
        return false;
    }
    ensure_bookmarks(app);
    match cmd {
        "few" | "few-browse-url" => {
            ask(app, "few URL or search: ", Ask::FewGo);
        }
        "few-open-file" => ask_prefilled(
            app,
            "Open file: ",
            Ask::FewFile,
            &format!("{}/", crate::compile::default_directory(app).display()),
        ),
        "few-search-words" => {
            let region = app.buf_ref().active_region().map(|(a, z)| {
                let b = app.buf_ref();
                let (lo, hi) = if b.pos_to_offset(a) <= b.pos_to_offset(z) {
                    (a, z)
                } else {
                    (z, a)
                };
                b.text_range(lo, hi)
            });
            match region.filter(|t| !t.trim().is_empty()) {
                Some(words) => {
                    answer(app, &Ask::FewSearch, &words);
                }
                None => ask(app, "Search for words: ", Ask::FewSearch),
            }
        }
        "few-switch-to-buffer" | "few-list-buffers" if app.few.buffer.is_some() => {
            let id = app.few.buffer.unwrap();
            app.switch_to(id);
        }
        "few-search" => ask(app, "few search: ", Ask::FewSearch),
        "few-list-bookmarks" => {
            let b: Vec<String> = app
                .few
                .bookmarks
                .iter()
                .map(|(t, u)| format!("{t} — {u}"))
                .collect();
            if b.is_empty() {
                app.error("No bookmarks yet; press b on a page");
            } else {
                choose(app, "Bookmark: ", Ask::FewBookmark, b);
            }
        }
        "few-remove-bookmark" => {
            let b: Vec<String> = app
                .few
                .bookmarks
                .iter()
                .map(|(t, u)| format!("{t} — {u}"))
                .collect();
            if b.is_empty() {
                app.error("No bookmarks");
            } else {
                choose(app, "Remove bookmark: ", Ask::FewUnbookmark, b);
            }
        }
        _ => {
            if !in_few(app) {
                return true;
            }
            match cmd {
                "few-follow-link" => {
                    if let Some(f) = field_at(app) {
                        activate_field(app, f);
                    } else if let Some(l) = link_at(app) {
                        follow(app, &l.href);
                    } else if let Some(img) = image_at(app) {
                        external(app, &img.src);
                    } else {
                        app.error("Nothing to follow at point");
                    }
                }
                "few-next-link" => goto_link(app, true),
                "few-previous-link" => goto_link(app, false),
                "few-back" | "few-forward" => {
                    remember_point(app);
                    let at = app.few.at;
                    let to = if cmd == "few-back" {
                        at.checked_sub(1)
                    } else {
                        Some(at + 1).filter(|i| *i < app.few.history.len())
                    };
                    match to {
                        Some(i) => {
                            app.few.at = i;
                            show(app);
                        }
                        None => app.msg(
                            if cmd == "few-back" {
                                "No earlier page"
                            } else {
                                "No later page"
                            }
                            .to_string(),
                        ),
                    }
                }
                "few-reload" => {
                    remember_point(app);
                    if let Some(e) = current(app).cloned() {
                        load(app, &e.url, Mode::Replace, e.readable);
                    }
                }
                "few-quit" => {
                    remember_point(app);
                    let id = app.active_id();
                    app.kill_buffer(id);
                    app.few.buffer = None;
                }
                "few-copy-url" => {
                    let url = link_at(app)
                        .map(|l| l.href)
                        .or_else(|| image_at(app).map(|i| i.src))
                        .or_else(|| current(app).map(|e| e.url.clone()));
                    if let Some(u) = url {
                        app.kill(u.clone(), false);
                        app.msg(format!("Copied {u}"));
                    }
                }
                "few-view-source" => {
                    if let Some(e) = current(app).cloned() {
                        if e.source.is_empty() {
                            app.error("No source for this page");
                        } else {
                            app.show_special_buffer("*few source*", "special-mode", &e.source);
                        }
                    }
                }
                "few-toggle-readable" => {
                    remember_point(app);
                    let Some(mut e) = current(app).cloned() else {
                        return true;
                    };
                    if !e.html {
                        app.msg("Only HTML pages have a readable view".to_string());
                        return true;
                    }
                    e.readable = !e.readable;
                    let w = width(app);
                    let st = e.field_state.clone();
                    e.page = engine::render_html_with(&e.source, &e.url, w, e.readable, &st);
                    e.width = w;
                    e.line = 0;
                    let at = app.few.at;
                    let on = e.readable;
                    app.few.history[at] = e;
                    show(app);
                    app.msg(format!("Readable view {}", if on { "on" } else { "off" }));
                }
                "few-open-external" => {
                    let url = link_at(app)
                        .map(|l| l.href)
                        .or_else(|| image_at(app).map(|i| i.src))
                        .or_else(|| current(app).map(|e| e.url.clone()));
                    if let Some(u) = url {
                        external(app, &u);
                    }
                }
                "few-download" => {
                    let Some(e) = current(app).cloned() else {
                        return true;
                    };
                    if e.url.starts_with("file://") {
                        app.msg(format!(
                            "Already a file: {}",
                            e.url.trim_start_matches("file://")
                        ));
                        return true;
                    }
                    if app.few.dl_pending.is_some() {
                        app.error("A download is already running");
                        return true;
                    }
                    let name = e
                        .url
                        .rsplit(['?', '#'])
                        .next()
                        .unwrap_or("")
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("download")
                        .to_string();
                    let initial = format!(
                        "{}/Downloads/{name}",
                        std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
                    );
                    crate::org_cmds::ask_with(app, "Download to: ", Ask::FewDownload, &initial);
                }
                "few-list-history" => {
                    let labels = history_labels(app);
                    choose(app, "History: ", Ask::FewHistory, labels);
                }
                "few-list-links" => {
                    let Some(labels) = link_labels(app) else {
                        app.msg("No links on this page".to_string());
                        return true;
                    };
                    choose(app, "Link: ", Ask::FewLink, labels);
                }
                "few-toc" => {
                    let Some(labels) = toc_labels(app) else {
                        app.msg("No headings on this page".to_string());
                        return true;
                    };
                    choose(app, "Heading: ", Ask::FewToc, labels);
                }
                "few-find" => {
                    let initial = app.few.find.clone();
                    if initial.is_empty() {
                        ask(app, "Find in page: ", Ask::FewFind);
                    } else {
                        ask_prefilled(app, "Find in page: ", Ask::FewFind, &initial);
                    }
                }
                "few-find-next" => find_in_page(app, true),
                "few-find-previous" => find_in_page(app, false),
                "few-up-url" | "few-top-url" => {
                    let Some(e) = current(app).cloned() else {
                        return true;
                    };
                    match up_or_top(&e.url, cmd == "few-top-url") {
                        Some(u) => {
                            remember_point(app);
                            load(app, &u, Mode::Push, false);
                        }
                        None => app.msg("Already at the top".to_string()),
                    }
                }
                "few-next-url" | "few-previous-url" => {
                    let rel = if cmd == "few-next-url" {
                        ["next"]
                    } else {
                        ["prev"]
                    };
                    let Some(e) = current(app).cloned() else {
                        return true;
                    };
                    let found = rel_url(&e.source, &e.url, rel[0]).or_else(|| {
                        if rel[0] == "prev" {
                            rel_url(&e.source, &e.url, "previous")
                        } else {
                            None
                        }
                    });
                    match found {
                        Some(u) => {
                            remember_point(app);
                            load(app, &u, Mode::Push, false);
                        }
                        None => app.msg(format!(
                            "No {} page here",
                            if rel[0] == "next" { "next" } else { "previous" }
                        )),
                    }
                }
                "few-copy-alternate-url" => {
                    let Some(e) = current(app).cloned() else {
                        return true;
                    };
                    match rel_url(&e.source, &e.url, "alternate") {
                        Some(u) => {
                            app.kill(u.clone(), false);
                            app.msg(format!("Copied {u}"));
                        }
                        None => app.msg("No alternate URL on this page".to_string()),
                    }
                }
                "few-toggle-images" | "few-toggle-colors" | "few-toggle-fonts" => {
                    let (flag, name) = match cmd {
                        "few-toggle-images" => (&mut app.few.images_off, "Images"),
                        "few-toggle-colors" => (&mut app.few.colors_off, "Colors"),
                        _ => (&mut app.few.fonts_off, "Fonts"),
                    };
                    *flag = !*flag;
                    let on = !*flag;
                    remember_point(app);
                    let line = app.buf_ref().point().line;
                    show(app);
                    let last = app.buf_ref().line_count().saturating_sub(1);
                    app.buf().set_point(Pos::new(line.min(last), 0));
                    if cmd == "few-toggle-images" && on {
                        maybe_fetch_images(app);
                    }
                    app.msg(format!("{name} {}", if on { "on" } else { "off" }));
                }
                "few-set-character-encoding" => {
                    let current = forced_charset().unwrap_or_else(|| "auto".into());
                    ask_prefilled(
                        app,
                        "Character set (auto for the page's own): ",
                        Ask::FewCharset,
                        &current,
                    );
                }
                "few-show-alt-text" => match image_at(app) {
                    Some(img) if !img.alt.is_empty() => app.msg(img.alt),
                    Some(_) => app.msg("This picture has no alt text".to_string()),
                    None => app.error("No picture at point"),
                },
                "few-browse-image" => match image_at(app) {
                    Some(img) => external(app, &img.src),
                    None => app.error("No picture at point"),
                },
                "few-switch-to-buffer" | "few-list-buffers" => {
                    // One browser buffer, as with eww's default.
                    if let Some(id) = app.few.buffer {
                        app.switch_to(id);
                    }
                }
                "few-add-bookmark" => {
                    if let Some(e) = current(app).cloned() {
                        let title = if e.page.title.is_empty() {
                            e.url.clone()
                        } else {
                            e.page.title.clone()
                        };
                        if !app.few.bookmarks.iter().any(|(_, u)| *u == e.url) {
                            app.few.bookmarks.push((title.clone(), e.url.clone()));
                        }
                        app.editor.session.add_few_bookmark(&title, &e.url);
                        let _ = app.editor.session.save();
                        app.msg(format!("Bookmarked {}", e.url));
                    }
                }
                _ => return false,
            }
        }
    }
    true
}

/// Ask with an initial value (used to repeat the last find).
fn ask_prefilled(app: &mut App, prompt: &str, kind: Ask, initial: &str) {
    crate::org_cmds::ask_with(app, prompt, kind, initial);
}

fn follow(app: &mut App, href: &str) {
    if href.starts_with("mailto:") || href.starts_with("tel:") {
        external(app, href);
    } else if href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("file://")
    {
        remember_point(app);
        load(app, href, Mode::Push, false);
    } else {
        app.error(format!("few: cannot follow {href}"));
    }
}

/// Use the form control at `field`: edit text, flip a checkbox or radio,
/// step through a select, or submit its form.
fn activate_field(app: &mut App, field: usize) {
    let at = app.few.at;
    let Some(f) = app
        .few
        .history
        .get(at)
        .and_then(|e| e.page.fields.get(field).cloned())
    else {
        return;
    };
    match f.kind {
        FieldKind::Text | FieldKind::Password | FieldKind::TextArea => {
            app.few.field_edit = Some(field);
            crate::org_cmds::ask_with(
                app,
                &format!("{}: ", field_prompt(&f)),
                Ask::FewField,
                &f.value,
            );
        }
        FieldKind::Checkbox => {
            if let Some(e) = app.few.history.get_mut(at) {
                if let Some(f) = e.page.fields.get_mut(field) {
                    f.checked = !f.checked;
                }
                e.field_state = engine::FieldState::capture(&e.page);
            }
            redisplay_entry(app);
            app.msg(
                if app.few.history[at].page.fields[field].checked {
                    "Checked"
                } else {
                    "Unchecked"
                }
                .to_string(),
            );
        }
        FieldKind::Radio => {
            if let Some(e) = app.few.history.get_mut(at) {
                let (form, name) = (e.page.fields[field].form, e.page.fields[field].name.clone());
                for (i, other) in e.page.fields.iter_mut().enumerate() {
                    if other.kind == FieldKind::Radio && other.form == form && other.name == name {
                        other.checked = i == field;
                    }
                }
                e.field_state = engine::FieldState::capture(&e.page);
            }
            redisplay_entry(app);
            app.msg("Selected".to_string());
        }
        FieldKind::Select => {
            if let Some(e) = app.few.history.get_mut(at) {
                let (next, label) = {
                    let Some(f) = e.page.fields.get_mut(field) else {
                        return;
                    };
                    if f.options.is_empty() {
                        return app.msg("No options".to_string());
                    }
                    f.selected = (f.selected + 1) % f.options.len();
                    let (label, value) = f.options[f.selected].clone();
                    f.value = value;
                    (f.selected, label)
                };
                e.field_state = engine::FieldState::capture(&e.page);
                let _ = next;
                redisplay_entry(app);
                app.msg(format!("Chose {label}"));
            }
        }
        FieldKind::Submit => {
            if let Some(form) = f.form {
                submit_form(app, form, Some(field));
            } else {
                app.msg("This button belongs to no form".to_string());
            }
        }
        FieldKind::Button => app.msg("This button does nothing here".to_string()),
        FieldKind::Hidden => {}
    }
}

fn field_prompt(f: &engine::Field) -> &str {
    match f.kind {
        FieldKind::Password => "Password",
        FieldKind::TextArea => "Text",
        _ => "Edit",
    }
}

/// Re-render the current entry from its source (a field changed) and
/// show it quietly, keeping point where it is.
fn redisplay_entry(app: &mut App) {
    remember_point(app);
    let w = width(app);
    let at = app.few.at;
    if let Some(e) = app.few.history.get_mut(at)
        && e.html
    {
        let st = e.field_state.clone();
        e.page = engine::render_html_with(&e.source, &e.url, w, e.readable, &st);
        e.width = w;
    }
    show_quiet(app);
}

/// Submit a form: its named controls as `name=value` pairs, GET in the
/// address or POST in the body.
fn submit_form(app: &mut App, form_idx: usize, submit_idx: Option<usize>) {
    let at = app.few.at;
    let Some(e) = app.few.history.get(at).cloned() else {
        return;
    };
    let Some(form) = e.page.forms.get(form_idx).cloned() else {
        return;
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for fi in &form.fields {
        let Some(f) = e.page.fields.get(*fi) else {
            continue;
        };
        match f.kind {
            FieldKind::Text | FieldKind::Password | FieldKind::Hidden | FieldKind::TextArea => {
                if !f.name.is_empty() {
                    pairs.push((f.name.clone(), f.value.clone()));
                }
            }
            FieldKind::Checkbox | FieldKind::Radio => {
                if f.checked && !f.name.is_empty() {
                    pairs.push((
                        f.name.clone(),
                        if f.value.is_empty() {
                            "on".to_string()
                        } else {
                            f.value.clone()
                        },
                    ));
                }
            }
            FieldKind::Select => {
                if !f.name.is_empty()
                    && let Some((_, v)) = f.options.get(f.selected)
                {
                    pairs.push((f.name.clone(), v.clone()));
                }
            }
            FieldKind::Submit => {
                if Some(*fi) == submit_idx && !f.name.is_empty() {
                    pairs.push((f.name.clone(), f.value.clone()));
                }
            }
            FieldKind::Button => {}
        }
    }
    let query = pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                engine::percent_encode(k),
                engine::percent_encode(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let mut action = form.action.clone();
    if let Some(i) = action.find('#') {
        action.truncate(i);
    }
    if form.method == "post" {
        if !(action.starts_with("http://") || action.starts_with("https://")) {
            app.error("few: cannot post here; use & to hand it to the system".to_string());
            return;
        }
        remember_point(app);
        load_post(app, &action, query, Mode::Push);
    } else {
        let url = if query.is_empty() {
            action
        } else if action.contains('?') {
            format!("{action}&{query}")
        } else {
            format!("{action}?{query}")
        };
        remember_point(app);
        load(app, &url, Mode::Push, false);
    }
}

/// The web search address, from the search engine in force for few.
fn search_url_for(app: &App, q: &str) -> String {
    crate::search::url_for(app, crate::search::Target::Few, q)
}

fn external(app: &mut App, url: &str) {
    if !(url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("mailto:")
        || url.starts_with("tel:")
        || url.starts_with("file://"))
    {
        app.error(
            "few: only web, file, mail and telephone addresses are handed to the system"
                .to_string(),
        );
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => app.msg(format!("Opening {url}")),
        Err(e) => app.error(format!("Cannot open {url}: {e}")),
    }
}

/// Search the shown page for the last find query. Uppercase in the
/// query makes it case-sensitive, like the web browser's find.
fn find_in_page(app: &mut App, forward: bool) {
    let q = app.few.find.clone();
    if q.is_empty() {
        app.error("Nothing to find yet; press /");
        return;
    }
    let case = q.chars().any(|c| c.is_uppercase());
    let matches = find_matches(app, &q, case);
    if matches.is_empty() {
        app.error("Not found");
        return;
    }
    let p = app.buf_ref().point();
    let total = matches.len();
    let (idx, (line, col)) = if forward {
        matches
            .iter()
            .enumerate()
            .find(|(_, (l, c))| (*l, *c) > (p.line, p.col))
            .map(|(i, m)| (i, *m))
            .unwrap_or((0, matches[0]))
    } else {
        matches
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (l, c))| (*l, *c) < (p.line, p.col))
            .map(|(i, m)| (i, *m))
            .unwrap_or((total - 1, matches[total - 1]))
    };
    app.buf().set_point(Pos::new(line, col));
    app.msg(format!("match {} of {total}", idx + 1));
}

/// Every match of `q` in the page text (below the header), as buffer
/// positions, in order.
fn find_matches(app: &App, q: &str, case: bool) -> Vec<(usize, usize)> {
    let fold = |s: &str| {
        if case {
            s.to_string()
        } else {
            s.to_lowercase()
        }
    };
    let needle = fold(q);
    let mut out = Vec::new();
    let n = app.buf_ref().line_count();
    for line in HEADER..n {
        let text = app.buf_ref().line(line);
        let hay = fold(&text);
        let hchars: Vec<char> = hay.chars().collect();
        let nchars: Vec<char> = needle.chars().collect();
        if nchars.is_empty() || hchars.len() < nchars.len() {
            continue;
        }
        // Character columns, matching buffer positions.
        let mut i = 0;
        while i + nchars.len() <= hchars.len() {
            if hchars[i..i + nchars.len()] == nchars[..] {
                out.push((line, i));
                i += nchars.len().max(1);
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Completable labels for the page's links: their text, then address.
fn link_labels(app: &App) -> Option<Vec<String>> {
    let e = current(app)?;
    if e.page.links.is_empty() {
        return None;
    }
    Some(
        e.page
            .links
            .iter()
            .map(|l| {
                let text = e
                    .page
                    .lines
                    .get(l.line)
                    .map(|line| {
                        let chars: Vec<char> = line.chars().collect();
                        chars
                            .get(l.start..l.end.min(chars.len()))
                            .unwrap_or(&[])
                            .iter()
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let mut text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if text.chars().count() > 48 {
                    text = text.chars().take(47).collect::<String>() + "…";
                }
                if text.is_empty() {
                    text = "(link)".to_string();
                }
                format!("{text} — {}", l.href)
            })
            .collect(),
    )
}

/// Completable labels for the page's headings, indented by level.
fn toc_labels(app: &App) -> Option<Vec<String>> {
    let e = current(app)?;
    if e.page.headings.is_empty() {
        return None;
    }
    Some(
        e.page
            .headings
            .iter()
            .map(|h| format!("{}{}", "  ".repeat(h.level.saturating_sub(1)), h.text))
            .collect(),
    )
}

/// Answers to few's questions. True if `a` was one of them.
pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::FewGo | Ask::FewSearch => {
            if t.is_empty() {
                return true;
            }
            let url = if matches!(a, Ask::FewSearch) {
                search_url_for(app, t)
            } else {
                engine::normalize_input_with(t, &|q| search_url_for(app, q))
            };
            remember_point(app);
            load(app, &url, Mode::Push, false);
        }
        Ask::FewFile => {
            if !t.is_empty() {
                remember_point(app);
                load(app, t, Mode::Push, false);
            }
        }
        Ask::FewCharset => {
            if let Ok(mut g) = FORCED_CHARSET.lock() {
                *g = (!t.is_empty() && !t.eq_ignore_ascii_case("auto")).then(|| t.to_string());
            }
            if t.is_empty()
                || t.eq_ignore_ascii_case("auto")
                || encoding_rs::Encoding::for_label(t.as_bytes()).is_some()
            {
                remember_point(app);
                if let Some(e) = current(app).cloned() {
                    load(app, &e.url, Mode::Replace, e.readable);
                }
            } else {
                if let Ok(mut g) = FORCED_CHARSET.lock() {
                    *g = None;
                }
                app.error(format!("Unknown character set {t}"));
            }
        }
        Ask::FewHistory => {
            let labels = history_labels(app);
            if let Some(i) = labels.iter().position(|l| {
                l == t || l.trim_start_matches(['>', ' ']) == t.trim_start_matches(['>', ' '])
            }) {
                remember_point(app);
                app.few.at = i;
                show(app);
            }
        }
        Ask::FewBookmark => {
            let url = t.rsplit_once(" — ").map_or(t, |(_, u)| u).to_string();
            remember_point(app);
            load(app, &url, Mode::Push, false);
        }
        Ask::FewUnbookmark => {
            let url = t.rsplit_once(" — ").map_or(t, |(_, u)| u).to_string();
            app.few.bookmarks.retain(|(_, u)| *u != url);
            app.editor.session.remove_few_bookmark(&url);
            let _ = app.editor.session.save();
            app.msg("Bookmark removed".to_string());
        }
        Ask::FewFind => {
            if t.is_empty() {
                return true;
            }
            app.few.find = t.to_string();
            find_in_page(app, true);
        }
        Ask::FewLink => {
            let url = t.rsplit_once(" — ").map_or(t, |(_, u)| u);
            let at = app.few.at;
            let target = app.few.history.get(at).and_then(|e| {
                e.page
                    .links
                    .iter()
                    .find(|l| l.href == url)
                    .map(|l| (l.line + HEADER, l.start))
            });
            match target {
                Some((line, col)) => {
                    let last = app.buf_ref().line_count().saturating_sub(1);
                    app.buf().set_point(Pos::new(line.min(last), col));
                }
                None => app.error("No such link anymore; the page changed"),
            }
        }
        Ask::FewToc => {
            // `t` is trimmed, so match the bare heading text (labels are
            // indented by level).
            let at = app.few.at;
            let target = app.few.history.get(at).and_then(|e| {
                e.page
                    .headings
                    .iter()
                    .find(|h| h.text == t)
                    .map(|h| h.line + HEADER)
            });
            match target {
                Some(line) => {
                    let last = app.buf_ref().line_count().saturating_sub(1);
                    app.buf().set_point(Pos::new(line.min(last), 0));
                }
                None => app.error("No such heading anymore; the page changed"),
            }
        }
        Ask::FewField => {
            let Some(field) = app.few.field_edit.take() else {
                return true;
            };
            let at = app.few.at;
            if let Some(e) = app.few.history.get_mut(at) {
                if let Some(f) = e.page.fields.get_mut(field) {
                    f.value = input.to_string();
                }
                e.field_state = engine::FieldState::capture(&e.page);
            }
            redisplay_entry(app);
        }
        Ask::FewDownload => {
            if t.is_empty() {
                return true;
            }
            const MAX_DOWNLOAD: u64 = 100_000_000;
            let Some(e) = current(app).cloned() else {
                return true;
            };
            let url = e.url.clone();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                app.error("few: only web pages download".to_string());
                return true;
            }
            let path = crate::vc::expand_home(t);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && std::fs::create_dir_all(parent).is_err()
            {
                app.error(format!("few: cannot make {}", parent.display()));
                return true;
            }
            let cookies = cookie_header(&app.few.cookies, &url);
            let (tx, rx) = mpsc::channel();
            let url2 = url.clone();
            let spawned = std::thread::Builder::new()
                .name("few-download".into())
                .spawn(move || {
                    use std::io::Read;
                    let done: Result<Vec<u8>, String> = (|| {
                        let mut req = CLIENT.get(&url2);
                        if !cookies.is_empty() {
                            req = req.header(reqwest::header::COOKIE, cookies);
                        }
                        let resp = req.send().map_err(|e| describe(&e))?;
                        if resp.status().as_u16() >= 400 {
                            return Err(format!("the server answered {}", resp.status()));
                        }
                        let mut bytes = Vec::new();
                        resp.take(MAX_DOWNLOAD + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|e| format!("reading failed: {e}"))?;
                        if (bytes.len() as u64) > MAX_DOWNLOAD {
                            return Err("the file is bigger than 100 MB".to_string());
                        }
                        Ok(bytes)
                    })();
                    let _ = tx.send(done);
                });
            match spawned {
                Ok(_) => {
                    app.few.dl_pending = Some(DlPending {
                        rx,
                        url: url.clone(),
                        path,
                    });
                    app.msg("few: downloading…".to_string());
                }
                Err(e) => app.error(format!("few: cannot start the download: {e}")),
            }
        }
        _ => return false,
    }
    true
}

/// The address one level up (`https://x/a/b/` to `https://x/a/`), or the
/// site's top. None when already there.
fn up_or_top(url: &str, top: bool) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    let top_url = format!("{scheme}://{host}/");
    if top {
        return (url != top_url).then_some(top_url);
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let parent = match trimmed.rsplit_once('/') {
        Some((p, _)) => format!("{scheme}://{host}/{p}/"),
        None => top_url,
    };
    (parent != url).then_some(parent)
}

/// The `href` of the first `<link>` or `<a>` whose `rel` names `rel`,
/// resolved against `base`.
fn rel_url(source: &str, base: &str, rel: &str) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find('<') {
        let start = from + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &source[start..end];
        let low = &lower[start..end];
        if (low.starts_with("<link") || low.starts_with("<a "))
            && attr(low, tag, "rel")
                .is_some_and(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case(rel)))
            && let Some(href) = attr(low, tag, "href")
        {
            return Some(engine::resolve(base, &href));
        }
        from = end.max(start + 1);
    }
    None
}

/// An attribute's value in a tag (`low` is the lowercased tag, for finding).
fn attr(low: &str, tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let mut at = 0;
    while let Some(i) = low[at..].find(&key) {
        let pos = at + i;
        let boundary = pos == 0
            || !low.as_bytes()[pos - 1].is_ascii_alphanumeric() && low.as_bytes()[pos - 1] != b'-';
        if boundary {
            let v = &tag[pos + key.len()..];
            let (quote, v) = match v.chars().next()? {
                q @ ('"' | '\'') => (Some(q), &v[1..]),
                _ => (None, v),
            };
            let end = match quote {
                Some(q) => v.find(q)?,
                None => v
                    .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .unwrap_or(v.len()),
            };
            return Some(v[..end].to_string());
        }
        at = pos + key.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    fn write_page(name: &str, html: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("forge-few-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, html).unwrap();
        p
    }

    #[test]
    fn opens_a_local_page_and_navigates() {
        let p = write_page(
            "a.html",
            "<title>Page A</title><h1>Hi</h1><p>see <a href=\"https://example.com/b\">the link</a> and <a href=\"https://example.com/c\">another</a></p>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        assert_eq!(app.buf_ref().major_mode(), MODE);
        let text = app.buf_ref().get_contents();
        assert!(text.starts_with("Page A\n"), "{text}");
        assert!(text.contains("see the link and another"), "{text}");
        // Point starts on the first link; TAB goes to the next, S-TAB back.
        assert!(execute(&mut app, "few-next-link"));
        assert!(link_at(&app).unwrap().href.ends_with("/c"));
        assert!(execute(&mut app, "few-previous-link"));
        assert!(link_at(&app).unwrap().href.ends_with("/b"));
        // The link at point can be copied.
        assert!(execute(&mut app, "few-copy-url"));
        assert_eq!(
            app.editor.kill_ring.current(),
            Some("https://example.com/b")
        );
        // Bookmarks and history.
        assert!(execute(&mut app, "few-add-bookmark"));
        assert_eq!(app.few.bookmarks.len(), 1);
        assert_eq!(history_labels(&app).len(), 1);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn back_and_forward_walk_the_history() {
        let a = write_page("one.html", "<title>One</title><p>first</p>");
        let b = write_page("two.html", "<title>Two</title><p>second</p>");
        let mut app = app();
        answer(&mut app, &Ask::FewGo, a.to_str().unwrap());
        answer(&mut app, &Ask::FewGo, b.to_str().unwrap());
        assert!(app.buf_ref().get_contents().contains("second"));
        execute(&mut app, "few-back");
        assert!(app.buf_ref().get_contents().contains("first"));
        execute(&mut app, "few-forward");
        assert!(app.buf_ref().get_contents().contains("second"));
        // Going somewhere new from the middle drops the later pages.
        execute(&mut app, "few-back");
        answer(&mut app, &Ask::FewGo, b.to_str().unwrap());
        assert_eq!(app.few.history.len(), 2);
        let _ = std::fs::remove_dir_all(a.parent().unwrap());
    }

    #[test]
    fn readable_view_and_source() {
        let p = write_page(
            "r.html",
            "<nav>menu</nav><article><p>story text</p></article>",
        );
        let mut app = app();
        answer(&mut app, &Ask::FewGo, p.to_str().unwrap());
        assert!(app.buf_ref().get_contents().contains("menu"));
        execute(&mut app, "few-toggle-readable");
        let t = app.buf_ref().get_contents();
        assert!(t.contains("story text") && !t.contains("menu"), "{t}");
        execute(&mut app, "few-view-source");
        assert!(app.buf_ref().get_contents().contains("<article>"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    /// Poll until every background job (page, pictures, download) is done.
    fn settle(app: &mut App) {
        for _ in 0..400 {
            poll(app);
            if !app.few.needs_poll() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!app.few.needs_poll(), "a background job never finished");
    }

    /// A local HTTP server routing (method, path) to (status, type, body,
    /// extra headers). Returns the base URL.
    fn serve(
        routes: Vec<(
            String,
            String,
            String,
            String,
            String,
            Vec<(String, String)>,
        )>,
    ) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let mut c = match conn {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let head = req.lines().next().unwrap_or("").to_string();
                let mut hit = None;
                for (m, path, status, ctype, body, headers) in &routes {
                    if head.starts_with(&format!("{m} {path}")) {
                        hit = Some((status, ctype, body, headers));
                        break;
                    }
                }
                // Record what came in, for cookie assertions.
                let log = std::env::temp_dir().join(format!("forge-few-srv-{port}.log"));
                let _ = std::fs::write(&log, &req);
                match hit {
                    Some((status, ctype, body, headers)) => {
                        let mut extra = String::new();
                        for (k, v) in headers {
                            extra.push_str(&format!("{k}: {v}\r\n"));
                        }
                        let _ = write!(
                            c,
                            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
                            body.len()
                        );
                    }
                    None => {
                        let _ = write!(
                            c,
                            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                    }
                }
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn forms_fill_toggle_and_submit() {
        let base = serve(vec![
            ("GET".into(), "/form ".into(), "200 OK".into(), "text/html".into(),
                "<title>F</title><form action=\"/search\" method=\"get\"><input name=q><input type=checkbox name=c><input type=submit value=Go></form>".into(), vec![]),
            ("GET".into(), "/search".into(), "200 OK".into(), "text/html".into(), "<title>R</title><p>results here</p>".into(), vec![]),
        ]);
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/form")));
        settle(&mut app);
        // Point starts on the first stop; round the horn to land on it.
        assert!(execute(&mut app, "few-previous-link"));
        assert!(execute(&mut app, "few-next-link"));
        assert_eq!(field_at(&app), Some(0));
        assert!(execute(&mut app, "few-follow-link"));
        assert!(answer(&mut app, &Ask::FewField, "hello"));
        assert!(app.buf_ref().get_contents().contains("[hello]"));
        // The checkbox flips.
        assert!(execute(&mut app, "few-next-link"));
        assert_eq!(field_at(&app), Some(1));
        assert!(execute(&mut app, "few-follow-link"));
        assert!(app.buf_ref().get_contents().contains("[x]"));
        // Submitting GETs the form with both values (point reset to the
        // line start by the re-display, so two stops forward).
        assert!(execute(&mut app, "few-next-link"));
        assert!(execute(&mut app, "few-next-link"));
        assert!(execute(&mut app, "few-follow-link"));
        settle(&mut app);
        let text = app.buf_ref().get_contents();
        assert!(text.contains("results here"), "{text}");
        assert!(
            app.few.history[app.few.at].url.contains("q=hello"),
            "{}",
            app.few.history[app.few.at].url
        );
        assert!(
            app.few.history[app.few.at].url.contains("c=on"),
            "{}",
            app.few.history[app.few.at].url
        );
    }

    #[test]
    fn radios_group_and_values_survive_rewrap() {
        let p = write_page(
            "radio.html",
            "<form><input type=radio name=r value=1 checked><input type=radio name=r value=2></form>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        assert!(execute(&mut app, "few-next-link"));
        assert_eq!(field_at(&app), Some(1));
        assert!(execute(&mut app, "few-follow-link"));
        let e = &app.few.history[app.few.at];
        assert!(!e.page.fields[0].checked && e.page.fields[1].checked);
        // A resize re-renders from source but keeps the choice.
        app.win_mut().width = 40;
        maybe_rewrap(&mut app);
        let e = &app.few.history[app.few.at];
        assert!(!e.page.fields[0].checked && e.page.fields[1].checked);
        assert!(app.buf_ref().get_contents().contains("(*)"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn links_headings_and_fields_have_faces() {
        let p = write_page(
            "faces.html",
            "<title>T</title><h1>Hi</h1><p>see <a href=\"https://example.com\">there</a></p><form><input name=q></form>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        let decos = app.buf_ref().decorations();
        assert!(decos.iter().any(|d| matches!(
            d.face,
            forge_core::decoration::DecoFace::Underline
        ) && d.message.as_deref() == Some("https://example.com")));
        assert!(
            decos
                .iter()
                .any(|d| matches!(d.face, forge_core::decoration::DecoFace::Syntax(_)))
        );
        assert!(
            decos
                .iter()
                .any(|d| matches!(d.face, forge_core::decoration::DecoFace::Highlight))
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn cookies_round_trip() {
        let base = serve(vec![
            (
                "GET".into(),
                "/login ".into(),
                "200 OK".into(),
                "text/html".into(),
                "<title>L</title><p>in</p>".into(),
                vec![("Set-Cookie".into(), "session=abc; Path=/".into())],
            ),
            (
                "GET".into(),
                "/echo ".into(),
                "200 OK".into(),
                "text/html".into(),
                "<title>E</title><p>out</p>".into(),
                vec![],
            ),
        ]);
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/login")));
        settle(&mut app);
        assert_eq!(app.few.cookies.len(), 1);
        assert_eq!(
            (
                app.few.cookies[0].name.as_str(),
                app.few.cookies[0].value.as_str()
            ),
            ("session", "abc")
        );
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/echo")));
        settle(&mut app);
        let log = std::fs::read_to_string(std::env::temp_dir().join(format!(
            "forge-few-srv-{}.log",
            base.rsplit(':').next().unwrap()
        )))
        .unwrap_or_default();
        assert!(log.to_lowercase().contains("cookie: session=abc"), "{log}");
    }

    #[test]
    fn downloads_save_the_body() {
        let base = serve(vec![
            (
                "GET".into(),
                "/page ".into(),
                "200 OK".into(),
                "text/html".into(),
                "<title>P</title><p>hi</p>".into(),
                vec![],
            ),
            (
                "GET".into(),
                "/file ".into(),
                "200 OK".into(),
                "application/octet-stream".into(),
                "BYTES42".into(),
                vec![],
            ),
        ]);
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/page")));
        settle(&mut app);
        let dir =
            std::env::temp_dir().join(format!("forge-few-dl-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        // Download the binary URL by visiting it first (stub page), then saving.
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/file")));
        settle(&mut app);
        let dest = dir.join("out.bin");
        assert!(answer(&mut app, &Ask::FewDownload, dest.to_str().unwrap()));
        settle(&mut app);
        assert_eq!(std::fs::read(&dest).unwrap(), b"BYTES42");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inline_pictures_download_and_pin() {
        // A 1x1 PNG.
        let png: Vec<u8> = vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 8, 215, 99, 248, 207,
            192, 0, 0, 3, 1, 1, 0, 24, 221, 141, 189, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        let png_str = String::from_utf8_lossy(&png).into_owned();
        let base = serve(vec![
            (
                "GET".into(),
                "/pic ".into(),
                "200 OK".into(),
                "text/html".into(),
                "<title>P</title><p>look <img src=\"/i.png\" alt=\"dot\"> here</p>".into(),
                vec![],
            ),
            (
                "GET".into(),
                "/i.png ".into(),
                "200 OK".into(),
                "image/png".into(),
                png_str,
                vec![],
            ),
        ]);
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, &format!("{base}/pic")));
        settle(&mut app);
        let e = &app.few.history[app.few.at];
        assert_eq!(e.images.len(), 1);
        assert!(e.images[0].1.is_file());
        assert!(!app.buf_ref().inline_images.is_empty());
    }

    #[test]
    fn search_uses_the_default_engine() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewSearch, "hello world"));
        let url = app
            .few
            .pending
            .as_ref()
            .map(|p| p.url.clone())
            .unwrap_or_default();
        assert!(url.contains("q=hello+world"), "{url}");
    }

    fn type_key(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn typing_in_a_text_box_edits_it() {
        let p = write_page(
            "type.html",
            "<form><input name=q value=ab><input type=submit value=Go></form>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        // Point on the value's second letter (columns are traditional:
        // point on a letter means before it).
        app.buf().set_point(Pos::new(HEADER, 2));
        // Plain letters insert instead of running few-mode's commands:
        // 'n' would be next-link anywhere else.
        type_key(&mut app, 'X');
        assert!(
            app.buf_ref().get_contents().contains("[aXb]"),
            "{}",
            app.buf_ref().get_contents()
        );
        type_key(&mut app, 'n');
        assert!(
            app.buf_ref().get_contents().contains("[aXnb]"),
            "{}",
            app.buf_ref().get_contents()
        );
        assert_eq!(app.few.history.len(), 1, "no navigation happened");
        // Backspace deletes back, staying in the box.
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            app.buf_ref().get_contents().contains("[ab]"),
            "{}",
            app.buf_ref().get_contents()
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn clicking_a_link_follows_it() {
        let dir = std::env::temp_dir().join(format!(
            "forge-few-click-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.html"),
            "<title>A</title><p>to <a href=\"b.html\">bee</a></p>",
        )
        .unwrap();
        std::fs::write(dir.join("b.html"), "<title>B</title><p>here</p>").unwrap();
        let mut app = app();
        assert!(answer(
            &mut app,
            &Ask::FewGo,
            dir.join("a.html").to_str().unwrap()
        ));
        app.buf().set_point(Pos::new(HEADER, 4));
        assert!(link_at(&app).is_some());
        app.dispatch_mouse("<mouse-1>");
        assert!(
            app.buf_ref().get_contents().contains("here"),
            "{}",
            app.buf_ref().get_contents()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_a_button_submits_and_text_boxes_take_no_click() {
        let dir = std::env::temp_dir().join(format!(
            "forge-few-clickb-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.html"), "<title>A</title><form action=\"b.html\"><input name=q value=hi><input type=submit value=Go></form>").unwrap();
        std::fs::write(dir.join("b.html"), "<title>B</title><p>done</p>").unwrap();
        let mut app = app();
        assert!(answer(
            &mut app,
            &Ask::FewGo,
            dir.join("a.html").to_str().unwrap()
        ));
        // The submit button's label is "[Go]" after "[hi] ": point on it.
        let text = app.buf_ref().get_contents();
        let col = text.lines().nth(HEADER).unwrap().find("[Go]").unwrap();
        app.buf().set_point(Pos::new(HEADER, col + 1));
        app.dispatch_mouse("<mouse-1>");
        assert!(
            app.buf_ref().get_contents().contains("done"),
            "{}",
            app.buf_ref().get_contents()
        );
        // Back on a text box, a click moves point but opens no prompt.
        assert!(answer(
            &mut app,
            &Ask::FewGo,
            dir.join("a.html").to_str().unwrap()
        ));
        app.buf().set_point(Pos::new(HEADER, 1));
        app.dispatch_mouse("<mouse-1>");
        assert!(app.minibuffer.is_none());
        assert_eq!(app.buf_ref().point(), Pos::new(HEADER, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetches_over_http_in_the_background() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming().take(2) {
                let mut c = conn.unwrap();
                let mut buf = [0u8; 2048];
                let n = c.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (status, ctype, body) = if req.starts_with("GET /missing") {
                    ("404 Not Found", "text/html", "<p>nothing here</p>")
                } else {
                    (
                        "200 OK",
                        "text/html; charset=utf-8",
                        "<title>Served</title><p>hello <a href=\"/next\">next</a></p>",
                    )
                };
                let _ = write!(
                    c,
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let mut app = app();
        let url = format!("http://127.0.0.1:{port}/");
        answer(&mut app, &Ask::FewGo, &url);
        assert!(app.few.needs_poll());
        for _ in 0..200 {
            poll(&mut app);
            if !app.few.needs_poll() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!app.few.needs_poll(), "the fetch never finished");
        let text = app.buf_ref().get_contents();
        assert!(text.starts_with("Served\n"), "{text}");
        assert!(text.contains("hello next"), "{text}");
        assert_eq!(
            link_at(&app).unwrap().href,
            format!("http://127.0.0.1:{port}/next")
        );
        // An error status still shows the page and says so.
        answer(&mut app, &Ask::FewGo, &format!("{url}missing"));
        for _ in 0..200 {
            poll(&mut app);
            if !app.few.needs_poll() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(app.buf_ref().get_contents().contains("nothing here"));
    }

    #[test]
    fn a_refused_connection_is_reported_not_fatal() {
        let port = {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        };
        let mut app = app();
        answer(&mut app, &Ask::FewGo, &format!("http://127.0.0.1:{port}/"));
        for _ in 0..200 {
            poll(&mut app);
            if !app.few.needs_poll() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(app.few.history.is_empty());
    }

    #[test]
    fn commands_outside_a_page_explain_themselves() {
        let mut app = app();
        assert!(execute(&mut app, "few-back"));
        assert!(!execute(&mut app, "org-todo"));
    }

    #[test]
    fn relative_links_on_local_files_load() {
        let dir =
            std::env::temp_dir().join(format!("forge-few-rel-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.html"),
            "<title>A</title><p>to <a href=\"b.html\">bee</a></p>",
        )
        .unwrap();
        std::fs::write(dir.join("b.html"), "<title>B</title><p>here</p>").unwrap();
        let mut app = app();
        assert!(answer(
            &mut app,
            &Ask::FewGo,
            dir.join("a.html").to_str().unwrap()
        ));
        assert!(app.buf_ref().get_contents().contains('A'));
        assert!(execute(&mut app, "few-follow-link"));
        assert!(
            app.buf_ref().get_contents().contains("here"),
            "{}",
            app.buf_ref().get_contents()
        );
        assert_eq!(app.few.history.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_walks_matches_and_wraps() {
        let p = write_page(
            "find.html",
            "<title>F</title><p>apple banana apple</p><p>cherry apple</p>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        assert!(answer(&mut app, &Ask::FewFind, "apple"));
        // Point starts at the first match's line; the search moves past it.
        assert_eq!(app.buf_ref().point(), Pos::new(HEADER, 13));
        assert!(execute(&mut app, "few-find-next"));
        assert_eq!(app.buf_ref().point(), Pos::new(HEADER + 2, 7));
        // Wraps to the first match.
        assert!(execute(&mut app, "few-find-next"));
        assert_eq!(app.buf_ref().point(), Pos::new(HEADER, 0));
        assert!(execute(&mut app, "few-find-previous"));
        assert_eq!(app.buf_ref().point(), Pos::new(HEADER + 2, 7));
        // Uppercase folds off: no match for the capital query.
        assert!(answer(&mut app, &Ask::FewFind, "APPLE"));
        assert!(app.message.as_deref() == Some("Not found"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn link_list_and_toc_jump() {
        let p = write_page(
            "nav.html",
            "<title>N</title><h1>Top</h1><p>see <a href=\"https://example.com/b\">the link</a></p><h2>Deep</h2><p>tail</p>",
        );
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        let links = link_labels(&app).expect("links");
        assert_eq!(links.len(), 1);
        assert!(links[0].contains("https://example.com/b"), "{}", links[0]);
        assert!(answer(&mut app, &Ask::FewLink, &links[0]));
        assert_eq!(link_at(&app).unwrap().href, "https://example.com/b");
        let toc = toc_labels(&app).expect("headings");
        assert_eq!(toc, vec!["Top".to_string(), "  Deep".to_string()]);
        assert!(answer(&mut app, &Ask::FewToc, "  Deep"));
        let at = app.buf_ref().point().line;
        assert!(app.buf_ref().line(at).contains("Deep"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn history_forgets_the_oldest_pages() {
        let dir =
            std::env::temp_dir().join(format!("forge-few-cap-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = app();
        for i in 0..MAX_PAGES + 5 {
            let f = dir.join(format!("p{i}.html"));
            std::fs::write(&f, format!("<title>P{i}</title><p>page {i}</p>")).unwrap();
            assert!(answer(&mut app, &Ask::FewGo, f.to_str().unwrap()));
        }
        assert_eq!(app.few.history.len(), MAX_PAGES);
        assert!(app.buf_ref().get_contents().contains("page 104"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_pages_are_cut_with_a_notice() {
        let dir =
            std::env::temp_dir().join(format!("forge-few-big-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.txt");
        let mut big = String::new();
        for i in 0..300_000 {
            big.push_str(&format!("line {i} with some words to fill space\n"));
        }
        assert!(big.len() > MAX_BYTES as usize);
        std::fs::write(&f, &big).unwrap();
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, f.to_str().unwrap()));
        assert!(app.few.history[app.few.at].truncated);
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("truncated")),
            "{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn latin1_pages_decode_by_their_meta_charset() {
        let dir =
            std::env::temp_dir().join(format!("forge-few-enc-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("latin.html");
        let mut bytes = b"<html><head><meta charset=\"windows-1252\"></head><body><p>caf\xe9 \x93quote\x94</p></body></html>".to_vec();
        std::fs::write(&f, &bytes).unwrap();
        bytes.clear();
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, f.to_str().unwrap()));
        let text = app.buf_ref().get_contents();
        assert!(text.contains("café"), "{text}");
        assert!(text.contains("“quote”"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bookmarks_survive_in_the_session() {
        let p = write_page("bm.html", "<title>BM</title><p>saved</p>");
        let mut app = app();
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        assert!(execute(&mut app, "few-add-bookmark"));
        assert_eq!(
            app.editor.session.few_bookmarks(),
            vec![("BM".to_string(), app.few.history[0].url.clone())]
        );
        let url = app.few.history[0].url.clone();
        assert!(answer(
            &mut app,
            &Ask::FewUnbookmark,
            &format!("BM — {url}")
        ));
        assert!(app.editor.session.few_bookmarks().is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn resizing_rewraps_the_shown_page() {
        let p = write_page(
            "wide.html",
            &format!("<title>W</title><p>{}</p>", "word ".repeat(40)),
        );
        let mut app = app();
        app.win_mut().width = 100;
        assert!(answer(&mut app, &Ask::FewGo, p.to_str().unwrap()));
        let wide = app.buf_ref().get_contents();
        app.win_mut().width = 40;
        maybe_rewrap(&mut app);
        let narrow = app.buf_ref().get_contents();
        assert_ne!(wide, narrow);
        assert!(narrow.lines().count() > wide.lines().count());
        // Already at this width: a no-op that stays quiet.
        app.message = None;
        maybe_rewrap(&mut app);
        assert!(app.message.is_none());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}

#[cfg(test)]
mod eww_tests {
    use super::*;

    #[test]
    fn up_and_top_walk_the_address() {
        assert_eq!(
            up_or_top("https://a.org/x/y/z.html", false).as_deref(),
            Some("https://a.org/x/y/")
        );
        assert_eq!(
            up_or_top("https://a.org/x/y/", false).as_deref(),
            Some("https://a.org/x/")
        );
        assert_eq!(
            up_or_top("https://a.org/x/", false).as_deref(),
            Some("https://a.org/")
        );
        assert_eq!(up_or_top("https://a.org/", false), None);
        assert_eq!(
            up_or_top("https://a.org/x/y?q=1#f", true).as_deref(),
            Some("https://a.org/")
        );
        assert_eq!(up_or_top("https://a.org/", true), None);
    }

    #[test]
    fn rel_links_are_found_and_resolved() {
        let html = r#"<head><LINK rel="alternate" type="application/rss+xml" href="/feed.xml">
            <link rel='next' href=page2.html><a class="x" rel="prev nofollow" href="../p0">older</a></head>"#;
        assert_eq!(
            rel_url(html, "https://a.org/blog/p1", "alternate").as_deref(),
            Some("https://a.org/feed.xml")
        );
        assert_eq!(
            rel_url(html, "https://a.org/blog/p1", "next").as_deref(),
            Some("https://a.org/blog/page2.html")
        );
        assert_eq!(
            rel_url(html, "https://a.org/blog/p1", "prev").as_deref(),
            Some("https://a.org/p0")
        );
        assert_eq!(rel_url(html, "https://a.org/", "author"), None);
    }

    #[test]
    fn every_binding_names_a_few_command() {
        for (_, cmd) in BINDINGS {
            assert!(COMMANDS.iter().any(|c| c.name() == *cmd), "{cmd}");
        }
        let keys: Vec<_> = BINDINGS.iter().map(|(k, _)| *k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "a key is bound twice");
    }
}
