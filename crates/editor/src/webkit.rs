// SPDX-License-Identifier: MIT OR Apache-2.0
//! webkit: a real web page inside a Forge window, like xwidget-webkit in
//! Forge. The editor keeps the state (address, title, what to do next)
//! in the `*webkit*` buffer; a frontend that can host a web view (the
//! macOS app, with WKWebView) draws the page over that window and does
//! what the commands ask: load, back, forward, reload, zoom, focus. A
//! frontend that can't (the terminal) just shows the buffer's text,
//! which says so; `few` is the terminal's browser.
//!
//! Any number of pages can be open, each in a buffer of its own
//! (`*webkit*`, `*webkit<2>*`, ...). The frontend reports the address,
//! title, hovered link, loading state, errors and find results with the
//! internal commands `webkit--report`, `webkit--hover`, `webkit--loading`,
//! `webkit--error` and `webkit--found`.

use crate::app::App;
use crate::org_cmds::{Ask, ask};
use forge_core::few as engine;
use forge_core::{Buffer, BufferId, Direction};

pub const BUFFER: &str = "*webkit*";
pub const MODE: &str = "webkit-mode";

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "webkit",
        "Webkit: show a web page (or search the web); in a page, go to it there.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-new",
        "Webkit: show a web page in a new page of its own.",
    ),
    forge_docs::CommandDoc::new("webkit-browse-url", "Webkit: show a web page in a window."),
    forge_docs::CommandDoc::new("webkit-back", "Webkit: go back."),
    forge_docs::CommandDoc::new("webkit-forward", "Webkit: go forward."),
    forge_docs::CommandDoc::new("webkit-reload", "Webkit: load the page again."),
    forge_docs::CommandDoc::new("webkit-stop", "Webkit: stop loading."),
    forge_docs::CommandDoc::new(
        "webkit-focus",
        "Webkit: send keys to the page (Escape or a C-/M- key brings them back).",
    ),
    forge_docs::CommandDoc::new("webkit-zoom-in", "Webkit: make the page bigger."),
    forge_docs::CommandDoc::new("webkit-zoom-out", "Webkit: make the page smaller."),
    forge_docs::CommandDoc::new("webkit-zoom-reset", "Webkit: the page at its normal size."),
    forge_docs::CommandDoc::new("webkit-find", "Webkit: search the page for text."),
    forge_docs::CommandDoc::new("webkit-find-next", "Webkit: the next match."),
    forge_docs::CommandDoc::new("webkit-find-previous", "Webkit: the previous match."),
    forge_docs::CommandDoc::new(
        "webkit-insert-string",
        "Webkit: put text into the page's active field.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-browse-history",
        "Webkit: pick a page from this page's back/forward history.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-copy-selection",
        "Webkit: copy the page's selection to the kill ring.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-copy-url",
        "Webkit: copy the page's address to the kill ring.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-copy-link",
        "Webkit: copy the address of the link the pointer is over.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-edit-mode",
        "Webkit: send keys to the page (Escape or a C-/M- key brings them back).",
    ),
    forge_docs::CommandDoc::new("webkit-scroll-up", "Webkit: scroll down a screen."),
    forge_docs::CommandDoc::new("webkit-scroll-down", "Webkit: scroll up a screen."),
    forge_docs::CommandDoc::new("webkit-scroll-up-line", "Webkit: scroll down a line."),
    forge_docs::CommandDoc::new("webkit-scroll-down-line", "Webkit: scroll up a line."),
    forge_docs::CommandDoc::new("webkit-scroll-forward", "Webkit: scroll right."),
    forge_docs::CommandDoc::new("webkit-scroll-backward", "Webkit: scroll left."),
    forge_docs::CommandDoc::new(
        "webkit-scroll-top",
        "Webkit: scroll to the top of the page.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-scroll-bottom",
        "Webkit: scroll to the bottom of the page.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-clone-below",
        "Webkit: show this page again in a new window below.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-clone-right",
        "Webkit: show this page again in a new window to the right.",
    ),
    forge_docs::CommandDoc::new("webkit-show-element", "Webkit: scroll to a CSS selector."),
    forge_docs::CommandDoc::new(
        "webkit-show-id-element",
        "Webkit: scroll to an element by id.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-show-named-element",
        "Webkit: scroll to an element by name.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-show-id-or-named-element",
        "Webkit: scroll to an element by id, or by name.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-fit-width",
        "Webkit: fit the page to the window (Forge pages always do).",
    ),
    forge_docs::CommandDoc::new(
        "webkit-adjust-size-dispatch",
        "Webkit: fit the page to the window (Forge pages always do).",
    ),
    forge_docs::CommandDoc::new(
        "webkit-adjust-size-to-content",
        "Webkit: size the page to its content (Forge pages always fit the window).",
    ),
    forge_docs::CommandDoc::new(
        "webkit-scroll-half-up",
        "Webkit: scroll down half a screen.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-scroll-half-down",
        "Webkit: scroll up half a screen.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-hints",
        "Webkit: label the links, then follow the one whose label you type.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-hints-new-page",
        "Webkit: label the links, then open the one you type in a new page.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-hints-copy",
        "Webkit: label the links, then copy the address of the one you type.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-focus-input",
        "Webkit: put the cursor in the page's first text field.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-next-link",
        "Webkit: follow the page's \"next\" link.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-previous-link",
        "Webkit: follow the page's \"previous\" link.",
    ),
    forge_docs::CommandDoc::new("webkit-up-url", "Webkit: go up one level in the address."),
    forge_docs::CommandDoc::new("webkit-root-url", "Webkit: go to the top of the site."),
    forge_docs::CommandDoc::new(
        "webkit-open-clipboard",
        "Webkit: open the last copied text as an address or search.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-open-clipboard-new",
        "Webkit: open the last copied text in a new page.",
    ),
    forge_docs::CommandDoc::new(
        "webkit-reopen-closed",
        "Webkit: bring back the page closed last.",
    ),
    forge_docs::CommandDoc::new("webkit-next-page", "Webkit: show the next open page."),
    forge_docs::CommandDoc::new(
        "webkit-previous-page",
        "Webkit: show the previous open page.",
    ),
    forge_docs::CommandDoc::new("webkit-list-pages", "Webkit: pick an open page."),
    forge_docs::CommandDoc::new("webkit-quit", "Webkit: close the page."),
];

/// Commands that open or list pages work anywhere; the rest act on the
/// page in the selected buffer, so command completion only offers them
/// there. They can still be run by name elsewhere, as any command can.
/// `None` leaves a command to the caller's usual rules.
pub fn command_visible(app: &App, name: &str) -> Option<bool> {
    if !name.starts_with("webkit") || name == "webkit-set-search-engine" {
        return None;
    }
    Some(match name {
        "webkit" | "webkit-new" | "webkit-browse-url" | "webkit-list-pages" => true,
        _ => app.buf_ref().major_mode() == MODE,
    })
}

/// The keys of `xwidget-webkit-mode`, with Forge's own extras
/// (`N`, `W`, `s`, `M-n`, `M-p`, `i`, `G`, `0`) alongside them.
pub const BINDINGS: &[(&str, &str)] = &[
    // The map.
    ("g", "webkit-browse-url"),
    ("a", "webkit-adjust-size-dispatch"),
    ("b", "webkit-back"),
    ("f", "webkit-forward"),
    ("r", "webkit-reload"),
    ("RET", "webkit-insert-string"),
    ("w", "webkit-copy-url"),
    ("+", "webkit-zoom-in"),
    ("-", "webkit-zoom-out"),
    ("e", "webkit-edit-mode"),
    ("C-r", "webkit-find"),
    ("C-s", "webkit-find"),
    ("H", "webkit-browse-history"),
    ("SPC", "webkit-scroll-up"),
    ("S-SPC", "webkit-scroll-down"),
    ("DEL", "webkit-scroll-down"),
    ("C-v", "webkit-scroll-up"),
    ("<next>", "webkit-scroll-up"),
    ("M-v", "webkit-scroll-down"),
    ("<prior>", "webkit-scroll-down"),
    ("n", "webkit-scroll-up-line"),
    ("<down>", "webkit-scroll-up-line"),
    ("C-n", "webkit-scroll-up-line"),
    ("p", "webkit-scroll-down-line"),
    ("<up>", "webkit-scroll-down-line"),
    ("C-p", "webkit-scroll-down-line"),
    ("C-f", "webkit-scroll-forward"),
    ("<right>", "webkit-scroll-forward"),
    ("C-b", "webkit-scroll-backward"),
    ("<left>", "webkit-scroll-backward"),
    ("M-<", "webkit-scroll-top"),
    ("C-<home>", "webkit-scroll-top"),
    ("<", "webkit-scroll-top"),
    ("M->", "webkit-scroll-bottom"),
    ("C-<end>", "webkit-scroll-bottom"),
    (">", "webkit-scroll-bottom"),
    ("?", "describe-mode"),
    ("h", "describe-mode"),
    ("q", "webkit-quit"),
    // Forge's extras.
    ("i", "webkit-focus"),
    ("G", "webkit-browse-url"),
    ("N", "webkit-new"),
    ("W", "webkit-copy-link"),
    ("s", "webkit-find"),
    ("M-n", "webkit-find-next"),
    ("M-p", "webkit-find-previous"),
    ("0", "webkit-zoom-reset"),
];

#[derive(Clone, Default)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
}

pub struct Page {
    pub id: u32,
    pub buffer: BufferId,
    pub url: String,
    pub title: String,
    /// Counts up with every request, so a frontend does each once.
    pub seq: u64,
    /// What the frontend should do: `load:URL`, `back`, `forward`,
    /// `reload`, `stop`, `focus`, `zoom:in|out|reset`, `find:TEXT`,
    /// `find-next`, `find-previous`, `insert:JSON`, `selection`,
    /// `history`, `history:INDEX`, `scroll:...` or `show-element:...`.
    pub command: String,
    pub find: String,
    pub hover: String,
    pub history: Vec<HistoryEntry>,
    pub history_current: usize,
    /// A history prompt is waiting for the frontend to report the list.
    pub pending_history: bool,
}

impl Page {
    fn send(&mut self, command: impl Into<String>) {
        self.seq += 1;
        self.command = command.into();
    }
}

#[derive(Default)]
pub struct WebState {
    pub pages: Vec<Page>,
    next_id: u32,
    /// Addresses of pages closed with `webkit-quit`, newest last.
    closed: Vec<String>,
}

impl WebState {
    pub fn page_for_buffer(&self, id: BufferId) -> Option<&Page> {
        self.pages.iter().find(|p| p.buffer == id)
    }

    fn by_id(&mut self, id: u32) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }
}

/// Forget pages whose buffer was killed: the frontend drops their views.
pub fn prune(app: &mut App) {
    let live: Vec<BufferId> = app.editor.state.buffers.keys().copied().collect();
    app.web.pages.retain(|p| live.contains(&p.buffer));
}

fn describe(app: &mut App, idx: usize) {
    let p = &app.web.pages[idx];
    let text = format!(
        "{}\n{}\n\nThis buffer holds a web page, which the Forge app draws here.\nThis frontend can't; use M-x few for a text browser.\n",
        if p.title.is_empty() {
            "(loading)"
        } else {
            &p.title
        },
        p.url
    );
    let id = p.buffer;
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_read_only(false);
        b.set_contents(&text);
        b.set_read_only(true);
    }
}

/// Index of the page in the selected buffer.
fn current(app: &App) -> Option<usize> {
    let id = app.active_id();
    app.web.pages.iter().position(|p| p.buffer == id)
}

fn current_or_error(app: &mut App) -> Option<usize> {
    let c = current(app);
    if c.is_none() {
        app.error("Not in a web page; start with M-x webkit");
    }
    c
}

/// A new page in a buffer of its own, shown in the selected window.
fn new_page(app: &mut App) -> usize {
    prune(app);
    let mut name = BUFFER.to_string();
    let mut k = 1;
    while app.editor.find_buffer(&name).is_some() {
        k += 1;
        name = format!("*webkit<{k}>*");
    }
    let mut b = Buffer::new(&name);
    b.set_major_mode(MODE);
    b.set_read_only(true);
    let buffer = app.editor.add_buffer(b);
    app.web.next_id += 1;
    let id = app.web.next_id;
    app.web.pages.push(Page {
        id,
        buffer,
        url: String::new(),
        title: String::new(),
        seq: 0,
        command: String::new(),
        find: String::new(),
        hover: String::new(),
        history: Vec::new(),
        history_current: 0,
        pending_history: false,
    });
    app.switch_to(buffer);
    app.web.pages.len() - 1
}

/// Open the selected page's address again in a new window.
fn clone_in_split(app: &mut App, dir: Direction) {
    let Some(i) = current_or_error(app) else {
        return;
    };
    let url = app.web.pages[i].url.clone();
    let window = match app.editor.split_window(dir) {
        Ok(window) => window,
        Err(e) => return app.error(e),
    };
    app.select_window(window);
    let i = new_page(app);
    if url.is_empty() {
        return app.msg("New web page");
    }
    let p = &mut app.web.pages[i];
    p.url = url.clone();
    p.send(format!("load:{url}"));
    describe(app, i);
    app.msg(format!("Loading {url}"));
}

fn one_line(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn history_label(i: usize, current: usize, h: &HistoryEntry) -> String {
    let title = if h.title.is_empty() { &h.url } else { &h.title };
    format!(
        "{i}. {} — {}{}",
        one_line(title),
        one_line(&h.url),
        if i == current { " (current)" } else { "" }
    )
}

fn history_labels(p: &Page) -> Vec<String> {
    p.history
        .iter()
        .enumerate()
        .map(|(i, h)| history_label(i, p.history_current, h))
        .collect()
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("webkit") {
        return false;
    }
    match cmd {
        "webkit" | "webkit-browse-url" => ask(app, "Web URL or search: ", Ask::WebGo),
        "webkit-new" => ask(app, "Web URL or search (new page): ", Ask::WebNew),
        "webkit-back"
        | "webkit-forward"
        | "webkit-reload"
        | "webkit-stop"
        | "webkit-focus"
        | "webkit-find-next"
        | "webkit-find-previous" => {
            if let Some(i) = current_or_error(app) {
                let word = cmd.trim_start_matches("webkit-");
                if word.starts_with("find") && app.web.pages[i].find.is_empty() {
                    app.error("Nothing to find yet; press C-s");
                } else {
                    app.web.pages[i].send(word);
                }
            }
        }
        "webkit-edit-mode" => {
            if let Some(i) = current_or_error(app) {
                app.web.pages[i].send("focus");
            }
        }
        "webkit-zoom-in" | "webkit-zoom-out" | "webkit-zoom-reset" => {
            if let Some(i) = current_or_error(app) {
                app.web.pages[i].send(format!("zoom:{}", cmd.trim_start_matches("webkit-zoom-")));
            }
        }
        "webkit-find" => {
            if current_or_error(app).is_some() {
                ask(app, "Find in page: ", Ask::WebFind);
            }
        }
        "webkit-insert-string" => {
            if current_or_error(app).is_some() {
                ask(app, "Text: ", Ask::WebInsert);
            }
        }
        "webkit-browse-history" => {
            if let Some(i) = current_or_error(app) {
                app.web.pages[i].pending_history = true;
                app.web.pages[i].send("history");
            }
        }
        "webkit-copy-selection" => {
            if let Some(i) = current_or_error(app) {
                app.web.pages[i].send("selection");
            }
        }
        "webkit-scroll-up"
        | "webkit-scroll-down"
        | "webkit-scroll-up-line"
        | "webkit-scroll-down-line"
        | "webkit-scroll-forward"
        | "webkit-scroll-backward"
        | "webkit-scroll-top"
        | "webkit-scroll-bottom" => {
            if let Some(i) = current_or_error(app) {
                let word = cmd.trim_start_matches("webkit-");
                let command = match word {
                    "scroll-up" | "scroll-down" if app.has_arg() => {
                        format!("{word}-px:{}", app.count())
                    }
                    "scroll-up-line" | "scroll-down-line" | "scroll-forward"
                    | "scroll-backward" => format!("{word}:{}", app.count()),
                    _ => word.to_string(),
                };
                app.web.pages[i].send(command);
            }
        }
        "webkit-scroll-half-up" | "webkit-scroll-half-down" | "webkit-focus-input" => {
            if let Some(i) = current_or_error(app) {
                let word = cmd.trim_start_matches("webkit-");
                app.web.pages[i].send(word);
            }
        }
        "webkit-hints" | "webkit-hints-new-page" | "webkit-hints-copy" => {
            if let Some(i) = current_or_error(app) {
                let mode = match cmd {
                    "webkit-hints-new-page" => "new",
                    "webkit-hints-copy" => "copy",
                    _ => "same",
                };
                app.web.pages[i].send(format!("hints:{mode}"));
                app.msg("Type a label (Escape cancels)");
            }
        }
        "webkit-next-link" | "webkit-previous-link" => {
            if let Some(i) = current_or_error(app) {
                let which = if cmd == "webkit-next-link" {
                    "next"
                } else {
                    "prev"
                };
                app.web.pages[i].send(format!("link:{which}"));
            }
        }
        "webkit-up-url" | "webkit-root-url" => {
            if let Some(i) = current_or_error(app) {
                let url = app.web.pages[i].url.clone();
                match parent_url(&url, cmd == "webkit-root-url") {
                    Some(up) => open_url(app, &up, false),
                    None => app.error("Already at the top"),
                }
            }
        }
        "webkit-open-clipboard" | "webkit-open-clipboard-new" => {
            let text = app
                .editor
                .kill_ring
                .current()
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if text.is_empty() {
                app.error("Nothing has been copied");
            } else {
                open_url(app, &text, cmd.ends_with("-new") || current(app).is_none());
            }
        }
        "webkit-reopen-closed" => match app.web.closed.pop() {
            Some(url) => open_url(app, &url, true),
            None => app.error("No closed page to bring back"),
        },
        "webkit-next-page" | "webkit-previous-page" => {
            prune(app);
            let n = app.web.pages.len();
            if n == 0 {
                app.error("No web pages");
            } else if let Some(i) = current(app) {
                if n == 1 {
                    app.msg("Only one web page");
                } else {
                    let to = if cmd == "webkit-next-page" {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    };
                    let b = app.web.pages[to].buffer;
                    app.switch_to(b);
                }
            } else {
                let b = app.web.pages[0].buffer;
                app.switch_to(b);
            }
        }
        "webkit-clone-below" | "webkit-clone-right" => {
            let dir = if cmd.ends_with("right") {
                Direction::Columns
            } else {
                Direction::Rows
            };
            clone_in_split(app, dir);
        }
        "webkit-show-element"
        | "webkit-show-id-element"
        | "webkit-show-named-element"
        | "webkit-show-id-or-named-element" => {
            if current_or_error(app).is_some() {
                let kind = match cmd {
                    "webkit-show-id-element" => "id",
                    "webkit-show-named-element" => "name",
                    "webkit-show-id-or-named-element" => "id-or-name",
                    _ => "css",
                };
                let prompt = match kind {
                    "css" => "Element selector: ",
                    "id" => "Element id: ",
                    "name" => "Element name: ",
                    _ => "Name or element id: ",
                };
                ask(app, prompt, Ask::WebShow(kind.to_string()));
            }
        }
        "webkit-fit-width" | "webkit-adjust-size-dispatch" | "webkit-adjust-size-to-content" => {
            if current_or_error(app).is_some() {
                app.msg("Web pages always fit their window");
            }
        }
        "webkit-copy-url" | "webkit-copy-link" => {
            if let Some(i) = current_or_error(app) {
                let p = &app.web.pages[i];
                let u = if cmd.ends_with("link") {
                    p.hover.clone()
                } else {
                    p.url.clone()
                };
                if u.is_empty() {
                    app.error(if cmd.ends_with("link") {
                        "No link under the pointer"
                    } else {
                        "No web page"
                    });
                } else {
                    app.kill(u.clone(), false);
                    app.msg(format!("Copied {u}"));
                }
            }
        }
        "webkit-list-pages" => {
            prune(app);
            if app.web.pages.is_empty() {
                app.error("No web pages");
            } else {
                let labels: Vec<String> = app.web.pages.iter().map(label).collect();
                crate::org_cmds::choose(app, "Page: ", Ask::WebPage, labels);
            }
        }
        "webkit-quit" => {
            if let Some(i) = current_or_error(app) {
                let id = app.web.pages[i].buffer;
                let url = app.web.pages[i].url.clone();
                if !url.is_empty() {
                    app.web.closed.push(url);
                    let extra = app.web.closed.len().saturating_sub(20);
                    app.web.closed.drain(..extra);
                }
                app.kill_buffer(id);
                prune(app);
            }
        }
        _ => {
            // The frontend telling the editor what the page is doing.
            let parts =
                |rest: &str| -> Vec<String> { rest.split('\t').map(str::to_string).collect() };
            if let Some(rest) = cmd.strip_prefix("webkit--report ") {
                let f = parts(rest);
                if let (Some(id), Some(url)) =
                    (f.first().and_then(|s| s.parse::<u32>().ok()), f.get(1))
                    && !url.is_empty()
                {
                    if let Some(p) = app.web.by_id(id) {
                        p.url = url.clone();
                        p.title = f.get(2).cloned().unwrap_or_default();
                    }
                    if let Some(i) = app.web.pages.iter().position(|p| p.id == id) {
                        describe(app, i);
                    }
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--loading ") {
                let f = parts(rest);
                if let (Some(id), Some(url)) =
                    (f.first().and_then(|s| s.parse::<u32>().ok()), f.get(1))
                    && !url.is_empty()
                {
                    if let Some(p) = app.web.by_id(id) {
                        p.url = url.clone();
                    }
                    if let Some(i) = app.web.pages.iter().position(|p| p.id == id) {
                        describe(app, i);
                    }
                    app.msg(format!("Loading {url}"));
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--error ") {
                let f = parts(rest);
                if let Some(id) = f.first().and_then(|s| s.parse::<u32>().ok()) {
                    let message = f.get(2).cloned().unwrap_or_default();
                    if message.is_empty() {
                        return true;
                    }
                    // A failed load keeps the page (back still works) but
                    // says what happened, in the echo area and the buffer.
                    if let Some(p) = app.web.by_id(id)
                        && p.title.is_empty()
                    {
                        p.title = message.clone();
                    }
                    if let Some(i) = app.web.pages.iter().position(|p| p.id == id) {
                        describe(app, i);
                    }
                    app.error(message);
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--hover ") {
                let f = parts(rest);
                if let Some(id) = f.first().and_then(|s| s.parse::<u32>().ok()) {
                    // Silently drop hovers for closed pages; only the
                    // selected page echoes the link.
                    if app.web.pages.iter().all(|p| p.id != id) {
                        return true;
                    }
                    let link = f.get(1).cloned().unwrap_or_default();
                    let selected = app
                        .web
                        .page_for_buffer(app.active_id())
                        .is_some_and(|p| p.id == id);
                    if let Some(p) = app.web.by_id(id) {
                        p.hover = link.clone();
                    }
                    if selected && !link.is_empty() {
                        app.msg(link);
                    }
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--found ") {
                let f = parts(rest);
                match f.get(1).map(String::as_str) {
                    Some("0") => app.error("Not found"),
                    Some("1") => {}
                    _ => {}
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--selection ") {
                let f = parts(rest);
                if let Some(id) = f.first().and_then(|s| s.parse::<u32>().ok())
                    && app.web.pages.iter().any(|p| p.id == id)
                {
                    let text = f
                        .get(1)
                        .and_then(|json| serde_json::from_str::<String>(json).ok())
                        .unwrap_or_default();
                    if text.is_empty() {
                        app.error("No selection");
                    } else {
                        app.kill(text, false);
                        app.msg("Copied selection");
                    }
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--history ") {
                let f = parts(rest);
                let Some(id) = f.first().and_then(|s| s.parse::<u32>().ok()) else {
                    return true;
                };
                let Some(report) = f
                    .get(1)
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                else {
                    return true;
                };
                let history: Vec<HistoryEntry> = report
                    .get("items")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| HistoryEntry {
                                title: item
                                    .get("title")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                url: item
                                    .get("url")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let current = report
                    .get("current")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default() as usize;
                let active = app.active_id();
                let mut labels = None;
                if let Some(p) = app.web.by_id(id) {
                    p.history = history;
                    p.history_current = current.min(p.history.len().saturating_sub(1));
                    if p.pending_history && p.buffer == active {
                        p.pending_history = false;
                        labels = Some(history_labels(p));
                    }
                }
                if let Some(labels) = labels {
                    if labels.is_empty() {
                        app.error("No history for this page");
                    } else {
                        crate::org_cmds::choose(app, "History: ", Ask::WebHistory, labels);
                    }
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--inserted ") {
                let f = parts(rest);
                if f.get(1).map(String::as_str) != Some("1") {
                    app.error("The page has no active field");
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--linked ") {
                let f = parts(rest);
                if f.get(1).map(String::as_str) != Some("1") {
                    app.error("No such link on this page");
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--hinted ") {
                // The page finished a hint prompt: `kind` is what to do
                // with the address (a plain follow was already clicked).
                let f = parts(rest);
                let Some(id) = f.first().and_then(|s| s.parse::<u32>().ok()) else {
                    return true;
                };
                if app.web.pages.iter().all(|p| p.id != id) {
                    return true;
                }
                let url = f
                    .get(2)
                    .and_then(|json| serde_json::from_str::<String>(json).ok())
                    .unwrap_or_default();
                match f.get(1).map(String::as_str) {
                    Some("new") if !url.is_empty() => open_url(app, &url, true),
                    Some("copy") if !url.is_empty() => {
                        app.kill(url.clone(), false);
                        app.msg(format!("Copied {url}"));
                    }
                    Some("none") => app.error("No link there"),
                    Some("cancel") => app.msg("Quit"),
                    _ => {}
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--inputted ") {
                let f = parts(rest);
                if f.get(1).map(String::as_str) != Some("1") {
                    app.error("The page has no text field");
                }
                return true;
            }
            if let Some(rest) = cmd.strip_prefix("webkit--shown ") {
                let f = parts(rest);
                if f.get(1).map(String::as_str) != Some("1") {
                    app.error("Element not found");
                }
                return true;
            }
            return false;
        }
    }
    true
}

/// Open a web address the way `browse_url_browser` says: `webkit` (the
/// embedded browser), `few`, `external` (the system's), or `auto` (the
/// default: webkit in a graphical frontend, few in a terminal).
pub fn browse_url(app: &mut App, url: &str) {
    let choice = app
        .rhai
        .as_ref()
        .and_then(|l| {
            l.settings()
                .strings
                .get("browse_url_browser")
                .and_then(|v| v.first().cloned())
        })
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let web = url.starts_with("http://") || url.starts_with("https://");
    let how = match choice.as_str() {
        "webkit" | "few" | "external" => choice.as_str(),
        _ if app.graphical => "webkit",
        _ => "few",
    };
    match how {
        "webkit" if web => open_url(app, url, false),
        "few" if web => {
            crate::few::answer(app, &Ask::FewGo, url);
        }
        _ => crate::org_cmds::open_external(app, url),
    }
}

/// Show `input` (an address, or words to search for) in the selected page,
/// or in a new page of its own.
fn open_url(app: &mut App, input: &str, new: bool) {
    let url = engine::normalize_input_with(input.trim(), &|q| {
        crate::search::url_for(app, crate::search::Target::Webkit, q)
    });
    let i = match (new, current(app)) {
        (false, Some(i)) => i,
        _ => new_page(app),
    };
    let p = &mut app.web.pages[i];
    p.url = url.clone();
    p.title.clear();
    p.send(format!("load:{url}"));
    describe(app, i);
    app.msg(format!("Loading {url}"));
}

/// The address one level up (`root`: the top of the site). None when
/// already there, or when this isn't a web address.
pub(crate) fn parent_url(url: &str, root: bool) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let top = format!("{scheme}://{host}/");
    if root {
        return (format!("{scheme}://{host}{path}") != top || url.contains(['?', '#']))
            .then_some(top);
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) if i > 0 => Some(format!("{scheme}://{host}{}/", &trimmed[..i])),
        Some(_) if !trimmed.is_empty() => Some(top),
        _ if url.contains(['?', '#']) => Some(top),
        _ => None,
    }
}

fn label(p: &Page) -> String {
    let t = if p.title.is_empty() { &p.url } else { &p.title };
    format!("{t} — {}", p.url)
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::WebGo | Ask::WebNew => {
            if t.is_empty() {
                return true;
            }
            open_url(app, t, !matches!(a, Ask::WebGo));
        }
        Ask::WebFind => {
            if let Some(i) = current(app)
                && !t.is_empty()
            {
                app.web.pages[i].find = t.to_string();
                app.web.pages[i].send(format!("find:{t}"));
            }
        }
        Ask::WebInsert => {
            if let Some(i) = current(app)
                && !input.is_empty()
            {
                let text = serde_json::to_string(input).unwrap_or_default();
                app.web.pages[i].send(format!("insert:{text}"));
            }
        }
        Ask::WebHistory => {
            if let Some(i) = current(app) {
                let labels = history_labels(&app.web.pages[i]);
                if let Some(n) = labels.iter().position(|label| label == t) {
                    app.web.pages[i].send(format!("history:{n}"));
                }
            }
        }
        Ask::WebPage => {
            if let Some(p) = app.web.pages.iter().find(|p| label(p) == t) {
                let b = p.buffer;
                app.switch_to(b);
            }
        }
        Ask::WebShow(kind) => {
            if let Some(i) = current(app)
                && !t.is_empty()
            {
                app.web.pages[i].send(format!("show-element:{kind}:{t}"));
            }
        }
        _ => return false,
    }
    true
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

    #[test]
    fn webkit_keys_are_the_defaults() {
        let expected = [
            ("g", "webkit-browse-url"),
            ("a", "webkit-adjust-size-dispatch"),
            ("b", "webkit-back"),
            ("f", "webkit-forward"),
            ("r", "webkit-reload"),
            ("RET", "webkit-insert-string"),
            ("w", "webkit-copy-url"),
            ("+", "webkit-zoom-in"),
            ("-", "webkit-zoom-out"),
            ("e", "webkit-edit-mode"),
            ("C-r", "webkit-find"),
            ("C-s", "webkit-find"),
            ("H", "webkit-browse-history"),
            ("SPC", "webkit-scroll-up"),
            ("S-SPC", "webkit-scroll-down"),
            ("DEL", "webkit-scroll-down"),
            ("C-n", "webkit-scroll-up-line"),
            ("C-p", "webkit-scroll-down-line"),
            ("C-f", "webkit-scroll-forward"),
            ("C-b", "webkit-scroll-backward"),
            ("M-<", "webkit-scroll-top"),
            ("M->", "webkit-scroll-bottom"),
            ("<", "webkit-scroll-top"),
            (">", "webkit-scroll-bottom"),
            ("?", "describe-mode"),
            ("h", "describe-mode"),
            ("q", "webkit-quit"),
        ];
        for (key, cmd) in expected {
            assert_eq!(
                BINDINGS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c),
                Some(cmd),
                "{key} should run {cmd}"
            );
        }
    }

    #[test]
    fn scrolling_inserting_showing_and_copying() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::WebGo, "example.com"));
        let id = app.web.pages[0].id;

        assert!(execute(&mut app, "webkit-scroll-up"));
        assert_eq!(app.web.pages[0].command, "scroll-up");
        assert!(execute(&mut app, "webkit-scroll-up-line"));
        assert_eq!(app.web.pages[0].command, "scroll-up-line:1");
        assert!(execute(&mut app, "webkit-scroll-top"));
        assert_eq!(app.web.pages[0].command, "scroll-top");

        assert!(execute(&mut app, "webkit-insert-string"));
        assert!(answer(&mut app, &Ask::WebInsert, "hello\nworld"));
        assert_eq!(app.web.pages[0].command, r#"insert:"hello\nworld""#);

        assert!(execute(&mut app, "webkit-show-id-element"));
        assert!(answer(&mut app, &Ask::WebShow("id".into()), "main"));
        assert_eq!(app.web.pages[0].command, "show-element:id:main");

        assert!(execute(&mut app, "webkit-copy-selection"));
        assert_eq!(app.web.pages[0].command, "selection");
        let selected = serde_json::to_string("a\tb\n").unwrap();
        assert!(execute(
            &mut app,
            &format!("webkit--selection {id}\t{selected}")
        ));
        assert_eq!(app.editor.kill_ring.current(), Some("a\tb\n"));
    }

    #[test]
    fn history_opens_and_jumps_to_an_entry() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::WebGo, "one.example"));
        let id = app.web.pages[0].id;
        assert!(execute(&mut app, "webkit-browse-history"));
        assert_eq!(app.web.pages[0].command, "history");
        assert!(app.web.pages[0].pending_history);

        let report = serde_json::json!({
            "current": 1,
            "items": [
                {"title": "One", "url": "https://one.example/"},
                {"title": "Two", "url": "https://two.example/"},
                {"title": "Three", "url": "https://three.example/"},
            ],
        });
        assert!(execute(
            &mut app,
            &format!("webkit--history {id}\t{report}")
        ));
        assert_eq!(app.web.pages[0].history.len(), 3);
        assert_eq!(app.web.pages[0].history_current, 1);
        let label = history_label(2, 1, &app.web.pages[0].history[2]);
        assert!(answer(&mut app, &Ask::WebHistory, &label));
        assert_eq!(app.web.pages[0].command, "history:2");
    }

    #[test]
    fn cloning_splits_the_window_and_loads_the_same_address() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::WebGo, "example.com"));
        let url = app.web.pages[0].url.clone();
        assert!(execute(&mut app, "webkit-clone-below"));
        assert_eq!(app.editor.windows.count(), 2);
        assert_eq!(app.web.pages.len(), 2);
        assert_eq!(app.buf_ref().name(), "*webkit<2>*");
        assert_eq!(app.web.pages[1].url, url);
        assert_eq!(app.web.pages[1].command, format!("load:{url}"));
    }

    #[test]
    fn every_command_is_registered() {
        let app = App::new(forge_core::Editor::default());
        for cd in COMMANDS {
            assert!(
                app.editor.commands.contains(cd.name()),
                "{} is not registered, so M-x cannot run it",
                cd.name()
            );
        }
    }

    #[test]
    fn page_commands_are_offered_only_in_a_page() {
        let mut app = app();
        assert_eq!(command_visible(&app, "webkit"), Some(true));
        assert_eq!(command_visible(&app, "webkit-new"), Some(true));
        assert_eq!(command_visible(&app, "webkit-browse-url"), Some(true));
        assert_eq!(command_visible(&app, "webkit-list-pages"), Some(true));
        assert_eq!(command_visible(&app, "save-buffer"), None);
        assert_eq!(command_visible(&app, "webkit-back"), Some(false));
        assert_eq!(command_visible(&app, "webkit-quit"), Some(false));

        assert!(answer(&mut app, &Ask::WebGo, "example.com"));
        assert_eq!(command_visible(&app, "webkit-back"), Some(true));
        assert_eq!(command_visible(&app, "webkit-quit"), Some(true));
    }

    #[test]
    fn browsing_shows_the_buffer_and_asks_the_frontend_to_load() {
        let mut app = app();
        assert!(execute(&mut app, "webkit"));
        assert!(answer(&mut app, &Ask::WebGo, "example.com"));
        assert_eq!(app.buf_ref().name(), BUFFER);
        let p = &app.web.pages[0];
        assert!(p.command.starts_with("load:") && p.command.contains("example.com"));
        let (id, seq) = (p.id, p.seq);
        assert!(execute(&mut app, "webkit-back"));
        assert_eq!(
            (app.web.pages[0].command.as_str(), app.web.pages[0].seq),
            ("back", seq + 1)
        );
        assert!(execute(
            &mut app,
            &format!("webkit--report {id}\thttps://example.com/\tExample")
        ));
        assert_eq!(app.web.pages[0].title, "Example");
        assert!(app.buf_ref().get_contents().contains("Example"));
        assert!(execute(&mut app, "webkit-copy-url"));
        assert_eq!(app.editor.kill_ring.current(), Some("https://example.com/"));
        assert!(execute(&mut app, "webkit-quit"));
        assert!(app.web.pages.is_empty());
        assert_ne!(app.buf_ref().name(), BUFFER);
    }

    #[test]
    fn several_pages_find_and_hover() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::WebGo, "a.example"));
        assert!(
            answer(&mut app, &Ask::WebGo, "b.example"),
            "in a page, go there"
        );
        assert_eq!(app.web.pages.len(), 1);
        assert!(answer(&mut app, &Ask::WebNew, "c.example"));
        assert_eq!(app.web.pages.len(), 2);
        assert_eq!(app.buf_ref().name(), "*webkit<2>*");
        let (first, second) = (app.web.pages[0].id, app.web.pages[1].id);
        assert_ne!(first, second);
        assert!(execute(&mut app, "webkit-find-next"));
        assert!(
            app.web.pages[1].command.starts_with("load:"),
            "nothing to find yet"
        );
        assert!(answer(&mut app, &Ask::WebFind, "needle"));
        assert_eq!(app.web.pages[1].command, "find:needle");
        assert!(execute(&mut app, "webkit-find-next"));
        assert_eq!(app.web.pages[1].command, "find-next");
        // Hover on the selected page shows the link and copies it.
        assert!(execute(
            &mut app,
            &format!("webkit--hover {second}\thttps://x.example/l")
        ));
        assert!(execute(&mut app, "webkit-copy-link"));
        assert_eq!(app.editor.kill_ring.current(), Some("https://x.example/l"));
        let b = app.web.pages[0].buffer;
        app.kill_buffer(b);
        prune(&mut app);
        assert_eq!(app.web.pages.len(), 1);
        assert_eq!(app.web.pages[0].id, second);
    }

    #[test]
    fn loading_error_found_and_stale_hover() {
        let mut app = app();
        assert!(answer(&mut app, &Ask::WebGo, "example.com"));
        let id = app.web.pages[0].id;
        // In-page navigation reports loading without a new request.
        assert!(execute(
            &mut app,
            &format!("webkit--loading {id}\thttps://example.com/next")
        ));
        assert_eq!(app.web.pages[0].url, "https://example.com/next");
        // A hit reports back without an error; a miss does.
        assert!(answer(&mut app, &Ask::WebFind, "needle"));
        assert!(execute(&mut app, &format!("webkit--found {id}\t1")));
        assert!(execute(&mut app, &format!("webkit--found {id}\t0")));
        // A failed load keeps the page and reports the error.
        assert!(execute(
            &mut app,
            &format!("webkit--error {id}\thttps://bad.example/\tboom")
        ));
        assert_eq!(app.web.pages[0].url, "https://example.com/next");
        // Empty reports and hovers for closed pages are dropped quietly.
        assert!(execute(&mut app, &format!("webkit--report {id}\t\tNoUrl")));
        assert_eq!(app.web.pages[0].url, "https://example.com/next");
        assert!(execute(&mut app, "webkit--hover 9999\thttps://x.example/"));
    }

    #[test]
    fn up_and_root_urls() {
        let up = |u, r| parent_url(u, r);
        assert_eq!(
            up("https://a.com/b/c/d.html?x=1#y", false).unwrap(),
            "https://a.com/b/c/"
        );
        assert_eq!(up("https://a.com/b/c/", false).unwrap(), "https://a.com/b/");
        assert_eq!(up("https://a.com/b", false).unwrap(), "https://a.com/");
        assert_eq!(up("https://a.com/", false), None);
        assert_eq!(up("https://a.com", false), None);
        assert_eq!(up("https://a.com/?q=1", false).unwrap(), "https://a.com/");
        assert_eq!(up("https://a.com/b/c?q", true).unwrap(), "https://a.com/");
        assert_eq!(up("https://a.com/", true), None);
        assert_eq!(up("about:blank", false), None);
    }

    #[test]
    fn vim_style_commands_drive_the_page() {
        let mut a = app();
        execute(&mut a, "webkit-new");
        a.minibuffer = None;
        open_url(&mut a, "https://example.com/a/b", true);
        let sent = |a: &App| a.web.pages[a.web.pages.len() - 1].command.clone();
        for (cmd, want) in [
            ("webkit-hints", "hints:same"),
            ("webkit-hints-new-page", "hints:new"),
            ("webkit-hints-copy", "hints:copy"),
            ("webkit-scroll-half-up", "scroll-half-up"),
            ("webkit-scroll-half-down", "scroll-half-down"),
            ("webkit-focus-input", "focus-input"),
            ("webkit-next-link", "link:next"),
            ("webkit-previous-link", "link:prev"),
        ] {
            assert!(execute(&mut a, cmd));
            assert_eq!(sent(&a), want, "{cmd}");
        }
        execute(&mut a, "webkit-up-url");
        assert_eq!(sent(&a), "load:https://example.com/a/");
        execute(&mut a, "webkit-root-url");
        assert_eq!(sent(&a), "load:https://example.com/");

        // Paging between pages, closing and bringing one back.
        open_url(&mut a, "https://second.example/", true);
        assert_eq!(a.web.pages.len(), 2);
        execute(&mut a, "webkit-next-page");
        assert_eq!(a.web.page_for_buffer(a.active_id()).unwrap().id, 1);
        execute(&mut a, "webkit-previous-page");
        assert_eq!(a.web.page_for_buffer(a.active_id()).unwrap().id, 2);
        execute(&mut a, "webkit-quit");
        assert_eq!(a.web.pages.len(), 1);
        execute(&mut a, "webkit-reopen-closed");
        assert_eq!(a.web.pages.len(), 2);
        assert_eq!(sent(&a), "load:https://second.example/");

        // The page reporting a hint prompt's result.
        execute(&mut a, "webkit--hinted 3\tcopy\t\"https://x.test/\"");
        assert_eq!(a.editor.kill_ring.current(), Some("https://x.test/"));
        execute(&mut a, "webkit--hinted 3\tnew\t\"https://y.test/\"");
        assert_eq!(a.web.pages.len(), 3);

        // The clipboard opens as an address.
        a.kill("rust lang".to_string(), false);
        execute(&mut a, "webkit-open-clipboard-new");
        assert_eq!(a.web.pages.len(), 4);
        assert!(sent(&a).starts_with("load:"));
    }
}
