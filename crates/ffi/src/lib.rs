// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge for the macOS app: a small Swift API (generated with UniFFI)
//! over the same editor the terminal runs.
//!
//! The editor lives on its own thread, so the app's main thread never
//! waits for a plugin or init.rhai: keys and mouse events are queued in
//! order, and the getters that need the editor's answer ([`Forge::frame`]
//! and friends) are `async`, so the app awaits them off the main thread
//! and [`Forge::frame`] returns the last frame (marked busy) if the
//! editor doesn't answer quickly. [`Forge::interrupt`] is C-g for a
//! busy plugin. Like the terminal, the core describes the screen
//! (windows of styled text, mode lines, the echo area, the cursor) and
//! the app only draws it.

use forge_core::display::{self, Face};
use forge_core::menu;
use forge_core::{FrameId, Rect, TabId};
use forge_editor::app::App;
use forge_editor::watch::Watch;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

uniffi::setup_scaffolding!();

pub mod desktop;
pub mod macos;

pub mod grid;
pub use grid::{Grid, GridDialog, GridPicture, GridRun};

/// How long [`Forge::frame`] waits for the editor before showing the
/// last frame as busy.
const FRAME_WAIT: Duration = Duration::from_millis(100);

/// Wait up to [`FRAME_WAIT`] for the editor thread's answer, without
/// holding the thread that polls the future. The app awaits these calls, but
/// a Swift `async` call can run its Rust poll on the main thread (the
/// caller's actor), and a plain `recv_timeout` there blocked the window
/// until the editor had worked through every key queued ahead of the
/// request. The wait happens on a short-lived thread instead and wakes the
/// future when it ends.
fn answer<T: Send + 'static>(rx: mpsc::Receiver<T>) -> Answer<T> {
    let shared = Arc::new(Mutex::new(AnswerState {
        value: None,
        waker: None,
    }));
    let for_thread = Arc::clone(&shared);
    std::thread::spawn(move || {
        let value = rx.recv_timeout(FRAME_WAIT);
        let mut state = for_thread.lock().expect("answer lock");
        state.value = Some(value);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    });
    Answer { shared }
}

struct AnswerState<T> {
    value: Option<Result<T, mpsc::RecvTimeoutError>>,
    waker: Option<std::task::Waker>,
}

struct Answer<T> {
    shared: Arc<Mutex<AnswerState<T>>>,
}

impl<T> std::future::Future for Answer<T> {
    type Output = Result<T, mpsc::RecvTimeoutError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut state = self.shared.lock().expect("answer lock");
        match state.value.take() {
            Some(value) => std::task::Poll::Ready(value),
            None => {
                state.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

/// The editor thread's stack: deeper than the 2 MiB default, since Org
/// export and the wasm plugin host recurse through nested structures and
/// a stack overflow can't be caught, only avoided.
const EDITOR_STACK: usize = 16 * 1024 * 1024;

#[derive(Debug, uniffi::Error)]
pub enum ForgeError {
    Start { message: String },
}

impl std::error::Error for ForgeError {}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForgeError::Start { message } => write!(f, "{message}"),
        }
    }
}

/// A key, as the app saw it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Key {
    /// A character (the unmodified one when Control or Meta is held).
    Char {
        c: String,
    },
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F {
        n: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyInput {
    pub key: Key,
    pub control: bool,
    /// Meta (Option, if the app uses Option as Meta).
    pub meta: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MouseKind {
    Down,
    Drag,
    Up,
    RightDown,
    ScrollUp,
    ScrollDown,
}

/// A run of text with one face. `cells`: how many character cells wide
/// it is (wide characters take two).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StyledText {
    pub text: String,
    pub cells: u16,
    /// "default", "region", "match", "keyword", "comment", "error", ...
    /// (see `face_name`).
    pub face: String,
    /// For decorations (underlined faces): the syntax face under them.
    pub syntax: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct WindowFrame {
    /// In character cells; the mode line is the last row.
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// The text is drawn on a grid of cells this many percent of the normal
    /// size (the buffer's text scale; 100 is normal). `rows`, `gutter`,
    /// `hl_rows`, the style runs, pictures and the cursor are counted in
    /// those cells from the window's top-left corner; the mode line and the
    /// border are in normal ones.
    pub cell_scale_pct: u16,
    pub rows: Vec<Vec<StyledText>>,
    pub mode_line: Vec<StyledText>,
    pub selected: bool,
    /// Draw a vertical line in the last column of the text rows.
    pub border: bool,
    /// Cells of line numbers at the start of each row.
    pub gutter: u16,
    /// Rows above the text at the top of the window (Org's header line).
    pub header_rows: u16,
    /// Pixels of the first text row clipped off its top (pixel scrolling).
    pub scroll_y_px: u32,
    /// hl-line-mode: rows to give a background.
    pub hl_rows: Vec<u16>,
    /// display-fill-column-indicator-mode: column for a thin line.
    pub fill_column: Option<u16>,
    /// The character to draw it with, when `fill_column` is set.
    pub fill_column_character: Option<String>,
    /// A completion popup to draw over the text (screen cells).
    pub popup: Option<PopupFrame>,
    /// The auxiliary popup beside it (corfu-popupinfo).
    pub popup_info: Option<PopupFrame>,
    /// "box", "bar" or "hbar" (modal editing shows its state with it).
    pub cursor_shape: String,
    /// Pictures from Org image links, to draw over blank text rows.
    pub images: Vec<ImageFrame>,
    /// Stretches of rows drawn at another height or raised (`display`
    /// property `height` and `raise`).
    pub style_runs: Vec<StyleRunFrame>,
    /// This window shows a web page (webkit): the app draws it over the
    /// text rows.
    pub web: Option<WebFrame>,
}

/// A stretch of a text row with its own font height and raise.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StyleRunFrame {
    /// Window row, and the cells it covers from the window's left edge.
    pub row: u16,
    pub col: u16,
    pub cells: u16,
    /// Font height as a percentage of the default.
    pub height_pct: u16,
    /// Raise as a percentage of the text's own height (negative lowers).
    pub raise_pct: i16,
}

/// The page a webkit window shows, and what to do to it next.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WebFrame {
    /// Which page: each has a web view of its own.
    pub page: u32,
    pub url: String,
    /// Counts up with each request in `command`, so it is done once.
    pub seq: u64,
    /// `load:URL`, `back`, `forward`, `reload`, `stop`, `focus`,
    /// `zoom:in|out|reset` or `close`.
    pub command: String,
}

/// A picture to draw over `rows` blank text rows starting at `row`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImageFrame {
    pub row: u16,
    pub rows: u16,
    pub path: String,
    /// The image's display properties (scale, rotation, width, ...).
    pub descriptor: Option<ImageDescriptorFrame>,
}

/// The display properties of an image, as the
/// SwiftUI app needs them to draw the picture.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImageDescriptorFrame {
    pub margin_x: i64,
    pub margin_y: i64,
    /// Ascent as a percentage of the height; `None` centers on the baseline.
    pub ascent: Option<f64>,
    pub relief: i64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub max_width: Option<f64>,
    pub max_height: Option<f64>,
    pub scale: Option<f64>,
    pub rotation: i32,
    pub flip_h: bool,
    /// Draw the picture centred in the window, not at its left edge.
    pub center: bool,
    /// The slice `(x, y, width, height)`, or empty for the whole picture.
    pub slice: Vec<SliceValueFrame>,
}

/// One number of an image slice: pixels, or a fraction of the picture.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SliceValueFrame {
    pub value: f64,
    pub fraction: bool,
}

/// The core image descriptor as the app's frame record.
fn descriptor_frame(d: &forge_core::display::ImageDescriptor) -> ImageDescriptorFrame {
    let slice = d
        .slice
        .map(|s| {
            s.iter()
                .map(|v| SliceValueFrame {
                    value: v.value,
                    fraction: v.fraction,
                })
                .collect()
        })
        .unwrap_or_default();
    ImageDescriptorFrame {
        margin_x: d.margin.0,
        margin_y: d.margin.1,
        ascent: d.ascent,
        relief: d.relief,
        width: d.width,
        height: d.height,
        max_width: d.max_width,
        max_height: d.max_height,
        scale: d.scale,
        rotation: d.rotation,
        flip_h: d.flip_h,
        center: d.center,
        slice,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PopupFrame {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub rows: Vec<Vec<StyledText>>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Hint {
    pub key: String,
    pub command: String,
}

/// Where the selected window is scrolled, for the app's own (native)
/// scroll bars. The app turns the fractions into scroller positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ScrollInfo {
    /// First visible line (0-based).
    pub top: u32,
    /// Lines in the buffer.
    pub total: u32,
    /// Text rows the window shows.
    pub visible: u32,
    /// Leftmost visible display column.
    pub left: u32,
    /// Display width of the line the horizontal bar tracks (point's line).
    pub line_width: u32,
    /// Text columns the window shows.
    pub width: u32,
    /// Pixel-smooth scroll offset of the top row, in pixels.
    pub scroll_y_px: u32,
}

/// Everything to draw, for a screen of `width` x `height` cells.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Frame {
    pub windows: Vec<WindowFrame>,
    /// Every open web page, shown or not: a view for any other is dropped.
    pub web_pages: Vec<u32>,
    /// The bottom line: messages and prompts.
    pub echo: String,
    /// Vertical minibuffer completion: rows drawn between the windows and
    /// the echo line (the windows already leave room for them).
    pub completions: Vec<Vec<StyledText>>,
    pub cursor_x: u16,
    pub cursor_y: u16,
    /// Keys that can follow the prefix being typed (after a pause).
    pub hints: Vec<Hint>,
    /// "C-x-" while hints show.
    pub hint_prefix: String,
    /// A plugin or init.rhai is running long: its name (C-g stops it).
    pub busy: Option<String>,
    /// For the window title: the current buffer, and whether it's modified.
    pub title: String,
    pub modified: bool,
    /// C-x C-c was confirmed: close the app.
    pub quit: bool,
    /// Set when the editor thread has stopped on a panic: the app shows
    /// this message (and the log) instead of a frozen editor.
    pub crashed: Option<String>,
    /// Where the selected window is scrolled, for the app's native scroll
    /// bars (None when neither bar is on).
    pub scroll: Option<ScrollInfo>,
    /// The vertical scroll bar's mode (scroll-bar-mode).
    pub scroll_bar: bool,
    /// The horizontal scroll bar's mode (horizontal-scroll-bar-mode).
    pub horizontal_scroll_bar: bool,
    /// Animate scrolling a line at a time (forge.opt.smooth_scroll).
    pub smooth_scroll: bool,
    /// Scroll in pixels rather than whole lines (forge.opt.pixel_scroll_precision).
    pub pixel_scroll_precision: bool,
    /// Whether this frame's tab bar is showing (tab-bar-mode). The app
    /// draws its own native strip, so no text row is reserved for it.
    pub tab_bar: bool,
    /// Whether the menu bar is showing (menu-bar-mode). The app uses its
    /// own (native) menu bar, so this tells it to show or hide that bar.
    pub menu_bar: bool,
    /// Whether the tool bar is showing (tool-bar-mode) in this GUI. The
    /// app draws it natively, so no text row is reserved for it.
    pub tool_bar: bool,
    /// True when `tool-bar-position` is bottom; left and right fall back
    /// to the top, as the core's layout does.
    pub tool_bar_bottom: bool,
    /// How much of each tool bar item to show (tool-bar-style).
    pub tool_bar_style: String,
    /// The selected buffer's bar: its major mode's, or the default one.
    pub tool_bar_items: Vec<ToolBarButton>,
    /// Whether the modifier bar is showing (modifier-bar-mode).
    pub modifier_bar: bool,
    /// The modifier buttons, and which already wait for the next key.
    pub modifier_buttons: Vec<ModifierButton>,
    /// The frame's tabs, in order, when `tab_bar` is showing; empty
    /// otherwise.
    pub tabs: Vec<EditorTab>,
}

/// One entry in a frame's tab bar (tab-bar-mode). The macOS app draws
/// these as a native strip above the text: each is a named window
/// configuration, and clicking one switches the frame to it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EditorTab {
    /// The tab's id, for selecting or closing it.
    pub id: u32,
    /// What to show on the tab: `tab-rename`'s name, or its buffers'.
    pub name: String,
    /// True for the tab the frame is showing.
    pub selected: bool,
}

/// One button in a frame's native tool bar (tool-bar-mode). The app runs
/// `command` when it is clicked; `separator` draws a divider instead.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ToolBarButton {
    pub label: String,
    /// A short glyph (text, symbol or emoji, per `icon-preference`), used
    /// when the bar asks for images and as the fallback for the two below.
    pub icon: String,
    /// The native icon to draw instead of the glyph, in the icon set of
    /// `system_platform` ("macos": an SF Symbols name). Empty for none.
    pub system_icon: String,
    pub system_platform: String,
    /// A picture file to draw instead, when the user chose one.
    pub icon_file: String,
    pub command: String,
    /// False when the command isn't defined or its condition doesn't
    /// hold, so the app greys the button out.
    pub available: bool,
    pub separator: bool,
}

/// One button in the native modifier bar (modifier-bar-mode).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModifierButton {
    /// The modifier's name: control, shift, meta, alt, super or hyper.
    pub name: String,
    /// The prefix it adds, like "C-".
    pub label: String,
    /// True while it waits for the next key.
    pub pending: bool,
}

/// One editor frame: a frame, which the app draws as a window of
/// its own. `selected` is the one that has the editor's focus.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EditorFrame {
    pub id: u32,
    /// Its name ("F1", "F2", …), for a window's title.
    pub name: String,
    pub selected: bool,
}

/// A face as a name for the app's theme.
pub fn face_name(face: Face) -> (String, Option<String>) {
    let s = |x: &str| x.to_string();
    match face {
        // A computed shade (Dired's size and date gradients) carries its
        // colour in the name: "dired-shade:rrggbb".
        Face::Decoration(
            forge_core::decoration::DecoFace::Dired(forge_core::decoration::DiredFace::Shade(rgb)),
            _,
        ) => (format!("dired-shade:{:06x}", rgb & 0xff_ffff), None),
        Face::Decoration(forge_core::decoration::DecoFace::Term(t), _) => (t.encode(), None),
        Face::Decoration(_, under) => (s(face.name()), under.map(|u| s(u.name()))),
        // The in-editor menu bar (menu-bar-mode) is the terminal's: macOS
        // uses its own menu bar, so these never reach the app.
        Face::MenuBar => (s("popup"), None),
        Face::MenuSelected => (s("popup-selected"), None),
        Face::MenuDisabled => (s("popup-annotation"), None),
        _ => (s(face.name()), None),
    }
}

fn styled(spans: &[display::Span]) -> Vec<StyledText> {
    spans
        .iter()
        .map(|sp| {
            let (face, syntax) = face_name(sp.face);
            StyledText {
                text: sp.text.clone(),
                cells: sp.text.width() as u16,
                face,
                syntax,
            }
        })
        .collect()
}

/// Describe the selected frame (the terminal draws the same thing with
/// ratatui).
pub fn build_frame(app: &mut App, width: u16, height: u16) -> Frame {
    build_frame_in(app, app.editor.windows.selected_frame_id().0, width, height)
}

/// The same for one frame, at the size of the window that shows it. A
/// frame that isn't the selected one still lays out, scrolls and keeps a
/// cursor in its own windows, but its mode line draws inactive; an
/// unknown frame (one deleted while a window was drawing) draws nothing.
pub fn build_frame_in(app: &mut App, frame: u32, width: u16, height: u16) -> Frame {
    let frame = FrameId(frame);
    if app.editor.windows.frame_name(frame).is_none() {
        return empty_frame(width, height);
    }
    forge_editor::webkit::prune(app);
    forge_editor::few::maybe_rewrap(app);
    let height = height.max(2);
    let vertical = forge_editor::minibuffer::vertical_view(app);
    let list_h = vertical
        .as_ref()
        .map_or(0, |v| v.0.len() as u16)
        .min(height.saturating_sub(3));
    // macOS has its own menu bar at the top of the screen, so no row is
    // reserved here: menu-bar-mode's in-editor row is the terminal's.
    let area = Rect {
        x: 0,
        y: 0,
        width: width.max(1),
        height: height - 1 - list_h,
    };
    let mut opts = forge_editor::ui::display_options(app);
    // macOS draws its own (native) scrollers over the text, so the core
    // reserves no column or row for them here.
    opts.scroll_bar = false;
    opts.horizontal_scroll_bar = false;
    // The GUI draws text at any size: `height` widens characters.
    opts.pixel_text = true;
    let active = frame == app.editor.windows.selected_frame_id();
    let views = display::redisplay_frame(&mut app.editor, frame, area, &opts, active);
    let (mut echo, mut echo_cursor) = forge_editor::ui::echo_line(app);
    if !active {
        // The minibuffer belongs to the frame that has focus: a
        // background frame shows the same line, without the caret.
        echo_cursor = None;
    }
    let mut completions = Vec::new();
    if let Some((rows, count)) = &vertical {
        echo = format!("{count} {echo}");
        echo_cursor = echo_cursor.map(|c| c + count.chars().count() + 1);
        completions = forge_editor::ui::vertical_rows(rows, width as usize)
            .iter()
            .map(|r| styled(r))
            .collect();
    }
    let keypad = if active {
        app.keypad_hints()
    } else {
        // The prefix being typed belongs to the frame that has focus.
        None
    };
    let showing_hints =
        active && (keypad.is_some() || (app.show_prefix_hints && !app.current_prefix().is_empty()));
    let (hint_title, hint_pairs) = match keypad {
        Some((name, hints)) => (name, hints),
        None if showing_hints => (
            format!("{}-", app.current_prefix().join(" ")),
            app.prefix_hints(),
        ),
        None => (String::new(), Vec::new()),
    };
    let hints: Vec<Hint> = hint_pairs
        .into_iter()
        .map(|(key, command)| Hint { key, command })
        .collect();
    let (cursor_x, cursor_y) = match echo_cursor {
        Some(x) => ((x as u16).min(width.saturating_sub(1)), height - 1),
        None => views
            .iter()
            .find(|v| v.selected)
            .map_or((0, 0), |v| v.cursor),
    };
    // The frame's own selected window: its buffer names the frame's
    // window, and its scroll position feeds the app's native scrollers.
    let sel_window = app.editor.windows.frame_selected_id(frame);
    let (title, modified) = app
        .editor
        .windows
        .get(sel_window)
        .map(|w| w.buffer)
        .and_then(|id| app.editor.state.get_buffer(id))
        .map_or_else(
            || {
                let b = app.buf_ref();
                (b.name().to_string(), b.is_modified())
            },
            |b| (b.name().to_string(), b.is_modified()),
        );
    let scroll = views.iter().find(|v| v.selected).and_then(|v| {
        let w = app.editor.windows.get(v.id)?;
        let point = app.editor.window_point(w.id);
        let b = app.editor.state.get_buffer(w.buffer)?;
        let line = b.offset_to_pos(point).line;
        Some(ScrollInfo {
            top: w.scroll_top as u32,
            total: b.line_count() as u32,
            visible: w.height as u32,
            left: w.hscroll as u32,
            line_width: forge_core::width::line_width(&b.line(line)) as u32,
            width: w.width as u32,
            scroll_y_px: v.scroll_y_px as u32,
        })
    });
    // The frame's tab bar: the app draws it natively, so only the bar's
    // tabs (and whether to show the strip) cross over, not any row of it.
    let tab_bar = forge_editor::tabs::visible_in(app, frame);
    let tabs = if tab_bar {
        forge_editor::tabs::tabs_in(app, frame)
            .into_iter()
            .map(|(id, name, selected)| EditorTab {
                id: id.0,
                name,
                selected,
            })
            .collect()
    } else {
        Vec::new()
    };
    // The tool bar and modifier bar are native too: the app draws them,
    // so only their buttons cross over, not a row of text.
    let tool_bar = forge_editor::toolbar::visible(app);
    let tool_bar_items = if tool_bar {
        tool_bar_buttons(app, frame)
    } else {
        Vec::new()
    };
    let modifier_bar = forge_editor::toolbar::modifier_visible(app);
    let modifier_buttons = if modifier_bar {
        modifier_buttons(app)
    } else {
        Vec::new()
    };
    Frame {
        web_pages: app.web.pages.iter().map(|p| p.id).collect(),
        windows: views
            .iter()
            .map(|v| WindowFrame {
                x: v.rect.x,
                y: v.rect.y,
                width: v.rect.width,
                height: v.rect.height,
                cell_scale_pct: v.cell_scale_pct,
                rows: v.rows.iter().map(|r| styled(r)).collect(),
                mode_line: styled(&v.mode_line),
                selected: v.selected,
                border: v.border,
                gutter: v.gutter,
                header_rows: v.header_rows,
                scroll_y_px: v.scroll_y_px as u32,
                hl_rows: v.hl_rows.clone(),
                fill_column: v.fill_column,
                fill_column_character: v.fill_column_character.clone(),
                popup: v.popup.as_ref().map(|p| PopupFrame {
                    x: p.x,
                    y: p.y,
                    width: p.width,
                    rows: p.rows.iter().map(|r| styled(r)).collect(),
                }),
                popup_info: v.popup_info.as_ref().map(|p| PopupFrame {
                    x: p.x,
                    y: p.y,
                    width: p.width,
                    rows: p.rows.iter().map(|r| styled(r)).collect(),
                }),
                cursor_shape: v.cursor_shape.name().to_string(),
                web: app
                    .editor
                    .windows
                    .get(v.id)
                    .and_then(|w| app.web.page_for_buffer(w.buffer))
                    .map(|p| WebFrame {
                        page: p.id,
                        url: p.url.clone(),
                        seq: p.seq,
                        command: p.command.clone(),
                    }),
                style_runs: v
                    .style_runs
                    .iter()
                    .map(|r| StyleRunFrame {
                        row: r.row,
                        col: r.col,
                        cells: r.cells,
                        height_pct: r.height_pct,
                        raise_pct: r.raise_pct,
                    })
                    .collect(),
                images: v
                    .images
                    .iter()
                    .map(|i| ImageFrame {
                        row: i.row,
                        rows: i.rows,
                        path: i.path.clone(),
                        descriptor: i.descriptor.as_ref().map(descriptor_frame),
                    })
                    .collect(),
            })
            .collect(),
        echo,
        completions,
        cursor_x,
        cursor_y,
        hint_prefix: hint_title,
        hints,
        busy: None,
        title,
        modified,
        quit: app.quit,
        crashed: None,
        scroll,
        scroll_bar: app.scroll_bar,
        horizontal_scroll_bar: app.horizontal_scroll_bar,
        smooth_scroll: app.smooth_scroll,
        pixel_scroll_precision: app.pixel_scroll_precision,
        tab_bar,
        menu_bar: forge_editor::menu::visible(app),
        tool_bar,
        tool_bar_bottom: forge_editor::toolbar::position(app).is_bottom(),
        tool_bar_style: forge_editor::toolbar::style(app).name().to_string(),
        tool_bar_items,
        modifier_bar,
        modifier_buttons,
        tabs,
    }
}

/// One entry in a menu (the same table the terminal's menu-bar-mode draws).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MenuEntry {
    pub label: String,
    /// The command it runs, "" for a separator or a submenu header.
    pub command: String,
    /// The key(s) that run it, in Forge's notation ("C-x C-s"), or "".
    /// Submenu headers show none: they open rather than run.
    pub keys: String,
    pub separator: bool,
    /// False when the command isn't defined (a plugin unloaded) or the
    /// item's condition doesn't hold, so the app can grey it out rather
    /// than offer something that would do nothing. Always true for a
    /// submenu header, which opens rather than runs.
    pub available: bool,
    /// True for an item that turns something on, while it is on, so the
    /// menu shows a tick where the terminal's does.
    pub checked: bool,
    /// The submenu this item opens. One level: submenu items are always
    /// leaves, which is also all UniFFI's records can carry (a recursive
    /// type is rejected).
    pub submenu: Vec<MenuSubEntry>,
}

/// One entry in a submenu: a leaf, like [`MenuEntry`] without children.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MenuSubEntry {
    pub label: String,
    /// The command it runs, "" for a separator.
    pub command: String,
    /// The key(s) that run it, in Forge's notation ("C-x C-s"), or "".
    pub keys: String,
    pub separator: bool,
    /// False when the command isn't defined or the item's condition
    /// doesn't hold, so the app can grey it out.
    pub available: bool,
    /// True while the display option it toggles is on.
    pub checked: bool,
}

/// One menu in the bar.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MenuGroup {
    pub title: String,
    pub items: Vec<MenuEntry>,
}

/// The context menu (right-click, or S-F10) while it is open: where it
/// was opened and the items it offers, with their commands and
/// availability resolved. None from [`Forge::context_menu`] when it is
/// closed (or `context-menu-mode` is off).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContextMenuFrame {
    /// Frame-relative column and row of the click (or point, for S-F10).
    pub column: u16,
    pub row: u16,
    pub items: Vec<MenuEntry>,
}

/// A clickable stretch of text under the pointer, for the app's hover
/// highlight, pointing-hand cursor and tooltip.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HoverInfo {
    /// Frame row, and the columns it covers (end exclusive).
    pub row: u16,
    pub start: u16,
    pub end: u16,
    /// Help text for `tooltip-mode`; empty when it is off.
    pub tooltip: String,
}

/// One restyled face for the macOS app: what `forge.theme.set` said.
/// Colors are `#rrggbb`; a missing field keeps the app's own style.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ThemeEntry {
    pub face: String,
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
}

/// A key for the editor, in the terminal's terms.
pub fn key_event(input: &KeyInput) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    if input.control {
        mods |= KeyModifiers::CONTROL;
    }
    if input.meta {
        mods |= KeyModifiers::ALT;
    }
    // Shift is a modifier of the key here too, so a binding written the
    // same way works in the app and in the terminal ("S-a"). The character
    // is the shifted one either way.
    if input.shift {
        mods |= KeyModifiers::SHIFT;
    }
    let code = match &input.key {
        Key::Char { c } => {
            let mut chars = c.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            // C-x arrives lower-case, like from a terminal.
            KeyCode::Char(if input.control {
                ch.to_ascii_lowercase()
            } else {
                ch
            })
        }
        Key::Enter => KeyCode::Enter,
        Key::Tab if input.shift => KeyCode::BackTab,
        Key::Tab => KeyCode::Tab,
        Key::BackTab => KeyCode::BackTab,
        Key::Backspace => KeyCode::Backspace,
        Key::Delete => KeyCode::Delete,
        Key::Escape => KeyCode::Esc,
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Insert => KeyCode::Insert,
        Key::F { n } => KeyCode::F(*n),
    };
    if input.shift && !matches!(code, KeyCode::Char(_)) {
        mods |= KeyModifiers::SHIFT;
    }
    Some(KeyEvent::new(code, mods))
}

enum Request {
    Key(KeyEvent),
    Text(String),
    Mouse(MouseEvent),
    Open(String),
    Goto(u32, u32),
    Frame(u16, u16, Sender<Frame>),
    /// The screen as styled runs: columns, rows, dark colours.
    Grid(u16, u16, bool, Sender<Grid>),
    /// The file picker's answer (None: cancelled).
    Dialog(u32, Option<String>),
    /// One frame, at the size of the window that shows it.
    FrameAt(u32, u16, u16, Sender<Frame>),
    /// The frames, so the app keeps one window per frame.
    Frames(Sender<Vec<EditorFrame>>),
    /// Make a frame the one the editor has focused.
    SelectFrame(u32),
    /// Switch a frame to one of its tabs (a click on the app's tab strip).
    SelectTab(u32, u32),
    /// Move a frame's tab to this place in its order (a drag in the app's
    /// tab strip).
    MoveTab(u32, u32, u32),
    /// Click the tool bar item at this index in the selected buffer's bar.
    ToolBarItem(u32),
    /// Click the modifier bar button at this index in `Modifier::ALL`.
    ModifierButton(u32),
    /// Make a new tab in a frame (copying its window configuration).
    NewTab(u32),
    /// Close a frame's tab. False when it was the frame's last tab (the
    /// editor refuses to close it).
    CloseTab(u32, u32, Sender<bool>),
    /// The frame's window was closed: delete the frame. False if it was
    /// the last one (the app quits with it anyway).
    CloseFrame(u32, Sender<bool>),
    SaveSession(Sender<()>),
    /// Run a command by name (the macOS menu bar).
    Command(String),
    /// The menu bar's contents, with availability resolved.
    Menus(Sender<Vec<MenuGroup>>),
    /// The open context menu, with availability resolved.
    ContextMenu(Sender<Option<ContextMenuFrame>>),
    Hover(u16, u16, Sender<Option<HoverInfo>>),
    /// Close the context menu (its native popup was dismissed).
    CloseContextMenu,
    /// The theme overrides, for the app's colors.
    Theme(Sender<Vec<ThemeEntry>>),
    /// The kill ring's top, for the system clipboard.
    KillTop(Sender<Option<String>>),
    /// Put a line at the top of the selected window (a scroll bar drag).
    ScrollTop(u32),
    /// Put a line at the top, clipped by this many pixels (a scroll bar
    /// dragged between lines).
    ScrollTopPixels(u32, u32),
    /// Scroll the selected window by this many pixels (the wheel).
    ScrollPixels {
        delta: i32,
        row_height: u32,
    },
    /// Scroll the selected window sideways to a display column.
    ScrollLeft(u32),
}

/// The editor, running on its own thread.
#[derive(uniffi::Object)]
pub struct Forge {
    tx: Mutex<Sender<Request>>,
    last: Mutex<Option<Frame>>,
    last_grid: Mutex<Option<Grid>>,
    /// Each frame's last picture, so a frame the editor didn't answer for
    /// keeps drawing while a plugin runs long.
    last_at: Mutex<HashMap<u32, Frame>>,
    /// The last frame list the editor answered with, so a busy editor
    /// keeps the app's windows rather than looking like it has no frames.
    last_frames: Mutex<Option<Vec<EditorFrame>>>,
    watch: Watch,
    quit: Arc<AtomicBool>,
    /// The panic that stopped the editor thread, if any: the app shows it
    /// instead of a frozen frame.
    crashed: Arc<Mutex<Option<String>>>,
}

/// Where the app's log goes: `~/Library/Logs/Forge/forge.log`, created on
/// first use. `None` when there's no home directory or it can't be made.
fn log_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join("Library/Logs/Forge");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("forge.log"))
}

/// Append one line to the log, best effort: logging must never be the
/// reason the editor goes down, so every failure here is ignored.
fn log_line(message: &str) {
    let Some(path) = log_file() else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(file, "[{secs}] {message}");
}

/// A panic payload as text: the message a `panic!` carried.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Print a full backtrace on any panic from any thread, and keep a copy in
/// the log file. The app is often launched without `RUST_BACKTRACE` set, so
/// the default hook prints only the message; this forces a backtrace (to
/// stderr, visible in the Xcode console) and appends it to the log so a
/// panic the app survives can still be read after the window is gone.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let name = std::thread::current()
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| "<unnamed>".into());
            let backtrace = std::backtrace::Backtrace::force_capture();
            log_line(&format!("panic on thread '{name}': {info}\n{backtrace}"));
            eprintln!("[forge-panic] thread '{name}' panicked: {info}");
            eprintln!("[forge-panic] backtrace:\n{backtrace}");
            previous(info);
        }));
    });
}

/// Start the editor thread at User-initiated QoS. A thread spawned from
/// the app starts at Default, and the app waits on it (at launch, and for
/// every frame), which macOS flags as a priority inversion (Thread
/// Performance Checker). The work is user-triggered, so User-initiated is
/// the honest classification and keeps the wait short.
fn raise_editor_qos() {
    #[cfg(target_os = "macos")]
    unsafe {
        // 0 is the default relative priority within the class.
        let _ = libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
    }
}

impl Forge {
    fn launch(
        files: Vec<String>,
        no_config: bool,
        graphical: bool,
    ) -> Result<Arc<Self>, ForgeError> {
        install_panic_hook();
        // A window that draws pictures: plugins ask (display-graphic-p).
        forge_core::env::set_display_graphic(true);
        // Started from the Dock the directory is `/`; Forge starts in ~.
        forge_editor::start_in_home_from_gui();
        // This frontend draws pictures: Org image links get rows for them.
        forge_core::org_display::set_inline_image_rows(8);
        let opts = forge_editor::StartOptions {
            no_config,
            files,
            plugin_files: Vec::new(),
            // The macOS app draws the tool bar, unlike the terminal.
            graphical,
        };
        let (tx, rx) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Watch, String>>();
        let quit = Arc::new(AtomicBool::new(false));
        let quit2 = quit.clone();
        let crashed = Arc::new(Mutex::new(None::<String>));
        let crashed2 = crashed.clone();
        std::thread::Builder::new()
            .name("forge-editor".into())
            .stack_size(EDITOR_STACK)
            .spawn(move || {
                raise_editor_qos();
                let mut app = match forge_editor::start(&opts) {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("{e:#}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(app.watch.clone()));
                // A panic must not take the whole app down with it: the
                // thread stops, but the app keeps running and shows the
                // crash (and the log), and auto-save keeps the buffers.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine(&mut app, &rx, &quit2);
                }));
                match outcome {
                    Ok(()) => {
                        app.save_clipboard_on_exit();
                        app.editor.save_session()
                    }
                    Err(payload) => {
                        app.auto_save_all();
                        let message = panic_message(payload.as_ref());
                        log_line(&format!("editor thread crashed: {message}"));
                        *crashed2.lock().expect("crash lock") = Some(message);
                    }
                }
            })
            .map_err(|e| ForgeError::Start {
                message: e.to_string(),
            })?;
        let watch = match ready_rx.recv() {
            Ok(Ok(w)) => w,
            Ok(Err(message)) => return Err(ForgeError::Start { message }),
            Err(_) => {
                return Err(ForgeError::Start {
                    message: "the editor thread stopped".into(),
                });
            }
        };
        Ok(Arc::new(Forge {
            tx: Mutex::new(tx),
            last: Mutex::new(None),
            last_grid: Mutex::new(None),
            last_at: Mutex::new(HashMap::new()),
            last_frames: Mutex::new(None),
            watch,
            quit,
            crashed,
        }))
    }
}

#[uniffi::export]
impl Forge {
    /// Start the editor: `files` to open; `no_config` like `forge -q`.
    #[uniffi::constructor]
    pub fn new(files: Vec<String>, no_config: bool) -> Result<Arc<Self>, ForgeError> {
        Self::launch(files, no_config, true)
    }

    /// Start the editor for a frontend that draws [`Forge::grid`]: the
    /// tool bar, menu bar and tabs are drawn by Forge itself, as in the
    /// terminal.
    #[uniffi::constructor]
    pub fn new_grid(files: Vec<String>, no_config: bool) -> Result<Arc<Self>, ForgeError> {
        // These frontends have a native file picker (see `Grid::dialog`).
        forge_editor::dialog::set_capable(true);
        Self::launch(files, no_config, false)
    }

    /// A key press (queued; keys are handled in order).
    pub fn key(&self, input: KeyInput) {
        if let Some(k) = key_event(&input) {
            self.send(Request::Key(k));
        }
    }

    /// Text typed without modifiers (several characters from an input
    /// method, or a paste): inserted as if typed.
    pub fn text(&self, text: String) {
        self.send(Request::Text(text));
    }

    /// A mouse event at a character cell.
    pub fn mouse(&self, kind: MouseKind, column: u16, row: u16) {
        let kind = match kind {
            MouseKind::Down => MouseEventKind::Down(MouseButton::Left),
            MouseKind::Drag => MouseEventKind::Drag(MouseButton::Left),
            MouseKind::Up => MouseEventKind::Up(MouseButton::Left),
            MouseKind::RightDown => MouseEventKind::Down(MouseButton::Right),
            MouseKind::ScrollUp => MouseEventKind::ScrollUp,
            MouseKind::ScrollDown => MouseEventKind::ScrollDown,
        };
        self.send(Request::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    /// Open a file (File > Open, a file dropped on the window).
    pub fn open_file(&self, path: String) {
        self.send(Request::Open(path));
    }

    /// Open a file from a `forge://open?file=...&line=...` URL.
    /// Returns false for a URL Forge does not recognize.
    pub fn open_url(&self, url: String) -> bool {
        let Some(request) = desktop::parse_open_url(&url) else {
            return false;
        };
        self.open_file(request.file);
        if let Some(line) = request.line {
            self.goto_position(line, 0);
        }
        true
    }

    /// The text formats the native macOS pasteboard helper supports.
    pub fn macos_pasteboard_types(&self) -> Vec<String> {
        macos::TEXT_PASTEBOARD_TYPES
            .iter()
            .map(|type_id| (*type_id).to_string())
            .collect()
    }

    /// Move to a zero-based line and column.
    pub fn goto_position(&self, line: u32, column: u32) {
        self.send(Request::Goto(line, column));
    }

    /// Say whether the system is in dark mode, so canvases, Bases and graphs
    /// are drawn in matching colours (a frontend that asks for a `grid`
    /// does this already).
    pub fn set_dark_mode(&self, dark: bool) {
        forge_core::canvas::set_dark_mode(Some(dark));
    }

    /// Say that this frontend draws canvases (Bases cards and boards, the
    /// graph, drawings) itself from the scene: they arrive as image paths
    /// ending in `.scene.json`, a JSON scene with its colours resolved
    /// (see docs/native-drawing.md), instead of as PNGs. Call it before the
    /// first frame.
    pub fn set_native_scenes(&self, on: bool) {
        forge_core::canvas::set_native_scenes(on);
    }

    /// Say whether mouse events are really a finger (default: a mouse). On a
    /// finger, a drag that starts on a Kanban card scrolls the page unless
    /// the card was held first; with a mouse the card is picked up at once.
    pub fn set_touch_input(&self, touch: bool) {
        forge_editor::canvas::set_touch(touch);
    }

    /// The size of one text cell in points. Org pictures asked for in
    /// pixels (`#+attr_org: :width 250`) are given rows to match.
    pub fn set_cell_size(&self, width: f64, height: f64) {
        forge_core::org_display::set_cell_size(width, height);
    }

    /// Put a line at the top of the selected window (the vertical scroller).
    pub fn scroll_to_line(&self, line: u32) {
        self.send(Request::ScrollTop(line));
    }

    /// Put a line at the top, clipped by `pixels` (the vertical scroller,
    /// dragged between two lines).
    pub fn scroll_to_line_pixels(&self, line: u32, pixels: u32) {
        self.send(Request::ScrollTopPixels(line, pixels));
    }

    /// Scroll the selected window by `delta` pixels (the wheel in
    /// pixel-scroll-precision-mode). Positive scrolls toward the end;
    /// `row_height` is the frontend's text row height in pixels.
    pub fn scroll_pixels(&self, delta: i32, row_height: u32) {
        self.send(Request::ScrollPixels { delta, row_height });
    }

    /// Scroll the selected window sideways (the horizontal scroller).
    pub fn scroll_to_column(&self, column: u32) {
        self.send(Request::ScrollLeft(column));
    }

    /// What to draw now. If the editor is busy (a plugin running long),
    /// the last frame, with `busy` set. Async so the app awaits it off
    /// the main thread: the wait is the editor thread's answer, up to
    /// [`FRAME_WAIT`].
    pub async fn frame(&self, width: u16, height: u16) -> Frame {
        if let Some(message) = self.crashed.lock().expect("crash lock").clone() {
            let mut f = self
                .last
                .lock()
                .expect("frame lock")
                .clone()
                .unwrap_or_else(|| empty_frame(width, height));
            f.busy = None;
            f.crashed = Some(message);
            return f;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(Request::Frame(width, height, reply_tx));
        match answer(reply_rx).await {
            Ok(f) => {
                *self.last.lock().expect("frame lock") = Some(f.clone());
                f
            }
            Err(_) => {
                let mut f = self
                    .last
                    .lock()
                    .expect("frame lock")
                    .clone()
                    .unwrap_or_else(|| empty_frame(width, height));
                f.busy = Some(
                    self.watch
                        .busy()
                        .map_or_else(|| "Forge".to_string(), |(name, _)| name),
                );
                f.quit = self.quit.load(Ordering::SeqCst);
                f
            }
        }
    }

    /// The answer to `Grid::dialog`: the path the person picked, or None if
    /// they cancelled.
    pub fn dialog_result(&self, id: u32, path: Option<String>) {
        self.send(Request::Dialog(id, path));
    }

    /// The screen as styled runs, `cols` by `rows` cells, in a dark or light
    /// palette (see [`Grid`]). Like [`Forge::frame`], a busy editor shows
    /// the last picture.
    pub async fn grid(&self, cols: u16, rows: u16, dark: bool) -> Grid {
        let last = || self.last_grid.lock().expect("grid lock").clone();
        if let Some(message) = self.crashed.lock().expect("crash lock").clone() {
            let mut g = last().unwrap_or_else(|| empty_grid(cols, rows, dark));
            g.crashed = Some(message);
            return g;
        }
        let (tx, rx) = mpsc::channel();
        self.send(Request::Grid(cols, rows, dark, tx));
        match answer(rx).await {
            Ok(mut g) => {
                g.quit = self.quit.load(Ordering::SeqCst);
                *self.last_grid.lock().expect("grid lock") = Some(g.clone());
                g
            }
            Err(_) => {
                let mut g = last().unwrap_or_else(|| empty_grid(cols, rows, dark));
                g.busy = Some(
                    self.watch
                        .busy()
                        .map_or_else(|| "Forge".to_string(), |(name, _)| name),
                );
                g.quit = self.quit.load(Ordering::SeqCst);
                g
            }
        }
    }

    /// Every frame the editor has, in order, so the app can keep one
    /// window per frame and know which one the editor has focused.
    pub async fn frames(&self) -> Vec<EditorFrame> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::Frames(tx));
        match answer(rx).await {
            Ok(all) => {
                *self.last_frames.lock().expect("frames lock") = Some(all.clone());
                all
            }
            // The editor is busy (a plugin, a long command): keep the last
            // frame list it gave, never an empty one. An empty list reads
            // as "no frames", and the app closes every window and quits.
            Err(_) => self
                .last_frames
                .lock()
                .expect("frames lock")
                .clone()
                .unwrap_or_default(),
        }
    }

    /// One frame's picture, at the size of the window that shows it. A
    /// frame the editor didn't answer for (busy) keeps its last picture,
    /// marked busy, as [`Forge::frame`] does.
    pub async fn frame_at(&self, id: u32, width: u16, height: u16) -> Frame {
        if let Some(message) = self.crashed.lock().expect("crash lock").clone() {
            let mut f = self
                .last_at
                .lock()
                .expect("frame lock")
                .get(&id)
                .cloned()
                .unwrap_or_else(|| empty_frame(width, height));
            f.busy = None;
            f.crashed = Some(message);
            return f;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(Request::FrameAt(id, width, height, reply_tx));
        match answer(reply_rx).await {
            Ok(f) => {
                self.last_at
                    .lock()
                    .expect("frame lock")
                    .insert(id, f.clone());
                f
            }
            Err(_) => {
                let mut f = self
                    .last_at
                    .lock()
                    .expect("frame lock")
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| empty_frame(width, height));
                f.busy = Some(
                    self.watch
                        .busy()
                        .map_or_else(|| "Forge".to_string(), |(name, _)| name),
                );
                f.quit = self.quit.load(Ordering::SeqCst);
                f
            }
        }
    }

    /// Make `id` the frame the editor has focused: its window became key
    /// in the app, so the editor's own selection follows it.
    pub fn select_frame(&self, id: u32) {
        self.send(Request::SelectFrame(id));
    }

    /// Switch the frame to one of its tabs (a click on the app's native
    /// tab strip). The frame is selected first, the way a click on its
    /// window does, so the editor's focus follows the tab that changed.
    pub fn select_tab(&self, frame: u32, tab: u32) {
        self.send(Request::SelectTab(frame, tab));
    }

    /// Move one of the frame's tabs to `to` in the tab order (dragging it
    /// in the native tab strip). The selected tab stays selected.
    pub fn move_tab(&self, frame: u32, tab: u32, to: u32) {
        self.send(Request::MoveTab(frame, tab, to));
    }

    /// Click one of the frame's native tool bar buttons. The index is its
    /// place in `Frame.tool_bar_items`; separators and unavailable items
    /// do nothing.
    pub fn tool_bar_item(&self, index: u32) {
        self.send(Request::ToolBarItem(index));
    }

    /// Click one of the native modifier bar's buttons, adding it to (or
    /// taking it from) the modifiers waiting for the next key.
    pub fn modifier_button(&self, index: u32) {
        self.send(Request::ModifierButton(index));
    }

    /// Make a new tab in the frame, a copy of its current window
    /// configuration, and switch to it.
    pub fn new_tab(&self, frame: u32) {
        self.send(Request::NewTab(frame));
    }

    /// Close one of the frame's tabs. False when it was the last one,
    /// which the editor refuses to close.
    pub async fn close_tab(&self, frame: u32, tab: u32) -> bool {
        let (tx, rx) = mpsc::channel();
        self.send(Request::CloseTab(frame, tab, tx));
        answer(rx).await.unwrap_or(false)
    }

    /// Delete a frame because its window was closed. False when it was
    /// the last frame, which the app quits with anyway.
    pub async fn close_frame(&self, id: u32) -> bool {
        let (tx, rx) = mpsc::channel();
        self.send(Request::CloseFrame(id, tx));
        answer(rx).await.unwrap_or(false)
    }

    /// C-g for a plugin or init.rhai that's running long.
    pub fn interrupt(&self) {
        self.watch.interrupt();
    }

    /// Remember places, recent files and history now (the app is quitting).
    pub fn save_session(&self) {
        let (tx, rx) = mpsc::channel();
        self.send(Request::SaveSession(tx));
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }

    /// The panic that stopped the editor thread, if any, so the app can
    /// show it. The process stays up so the message (and the log) can be
    /// read and reported.
    pub fn crash(&self) -> Option<String> {
        self.crashed.lock().expect("crash lock").clone()
    }

    /// Where the log lives (`~/Library/Logs/Forge/forge.log`), for the app
    /// to point the user at. Empty if it couldn't be created.
    pub fn log_path(&self) -> String {
        log_file().map_or_else(String::new, |p| p.display().to_string())
    }

    /// The menu bar's contents, in order. The same table the terminal's
    /// menu-bar-mode draws, so both frontends offer the same commands.
    /// `available` is false for a command that isn't defined. Empty when
    /// the editor didn't answer; frontends keep the bar they have.
    pub async fn menu_bar(&self) -> Vec<MenuGroup> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::Menus(tx));
        answer(rx).await.unwrap_or_default()
    }

    /// The context menu while it is open (right-click, or S-F10), with
    /// each item's command and availability resolved. None when it is
    /// closed, so the app can pop up its native menu.
    pub async fn context_menu(&self) -> Option<ContextMenuFrame> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::ContextMenu(tx));
        answer(rx).await.ok().flatten()
    }

    /// What a click would do at a screen cell: the stretch of text, and its
    /// tooltip. None when nothing clickable is there.
    pub async fn hover(&self, column: u16, row: u16) -> Option<HoverInfo> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::Hover(column, row, tx));
        answer(rx).await.ok().flatten()
    }

    /// Close the context menu, as dismissing its native popup does.
    pub fn close_context_menu(&self) {
        self.send(Request::CloseContextMenu);
    }

    /// The theme overrides, so the app paints text the way the terminal does.
    pub async fn theme(&self) -> Vec<ThemeEntry> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::Theme(tx));
        answer(rx).await.unwrap_or_default()
    }

    /// Run a command by name, as the menu bar does.
    pub fn run_command(&self, name: &str) {
        self.send(Request::Command(name.to_string()));
    }

    /// The kill ring's top, so copies (M-w and friends) reach the system
    /// clipboard. None when nothing was ever killed.
    pub async fn kill_top(&self) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        self.send(Request::KillTop(tx));
        answer(rx).await.ok().flatten()
    }

    /// The menus, built the way the app's menu bar wants them: the labels
    /// to show, the command each runs, and the key that runs it, so a
    /// plugin can build its own menu items with a shortcut.
    pub fn menu_entry(&self, label: &str, command: &str) -> MenuEntry {
        let _ = (label, command);
        MenuEntry {
            label: label.to_string(),
            command: command.to_string(),
            keys: String::new(),
            separator: command.is_empty(),
            available: false,
            checked: false,
            submenu: Vec::new(),
        }
    }
}

/// One menu's items as the app sees them, with each item's command and
/// availability resolved.
fn menu_entries(
    items: &[menu::Item],
    ctx: &menu::Context,
    keys: &dyn Fn(&str) -> Vec<String>,
) -> Vec<MenuEntry> {
    items
        .iter()
        .map(|i| MenuEntry {
            label: i.label.clone(),
            command: i.command.clone(),
            // What the menu shows: one binding when the item names one
            // (the terminal's menu shows the same), else every binding,
            // like the terminal's does. Empty for one with no binding;
            // headers show none.
            keys: if i.has_submenu() {
                String::new()
            } else {
                i.shown_keys
                    .clone()
                    .unwrap_or_else(|| keys(&i.command).join(", "))
            },
            separator: i.is_separator(),
            // A header opens rather than runs, so it is always there to
            // open.
            available: i.has_submenu() || ctx.enabled(i),
            checked: ctx.checked(i),
            submenu: i
                .submenu
                .iter()
                .map(|c| MenuSubEntry {
                    label: c.label.clone(),
                    command: c.command.clone(),
                    keys: c
                        .shown_keys
                        .clone()
                        .unwrap_or_else(|| keys(&c.command).join(", ")),
                    separator: c.is_separator(),
                    available: ctx.enabled(c),
                    checked: ctx.checked(c),
                })
                .collect(),
        })
        .collect()
}

/// The editor's menus as the app's own menu bar, with each item's
/// command looked up. The same registry the terminal's menu-bar-mode
/// draws, so a plugin that adds a menu gets it in both.
fn menu_groups(app: &App, keys: &dyn Fn(&str) -> Vec<String>) -> Vec<MenuGroup> {
    // The editor state the items' conditions are answered from: taken
    // once per menu request, so every item in the menu bar is asking about
    // the same editor, the way the terminal takes it when it draws.
    let ctx = menu::Context {
        exists: Box::new(|c: &str| {
            app.editor.commands.contains(c) || c.starts_with(menu::SELECT_BUFFER)
        }),
        keys: Box::new(|c: &str| keys(c)),
        state: menu_state(app),
    };
    app.editor
        .menus
        .menus()
        .iter()
        .filter(|m| m.visible(&ctx.state))
        .map(|m| MenuGroup {
            title: m.title.clone(),
            items: menu_entries(&m.items, &ctx, keys),
        })
        .collect()
}

/// The selected buffer's tool bar, resolved for this frame: every item,
/// including separators, with availability worked out the way the
/// terminal's row does.
fn tool_bar_buttons(app: &App, frame: FrameId) -> Vec<ToolBarButton> {
    let ctx = forge_editor::menu::context(app);
    forge_editor::toolbar::bar_in(app, frame)
        .items
        .iter()
        .map(|item| {
            let system = forge_editor::icon::toolbar_system(app, &item.command);
            let glyph = forge_editor::icon::toolbar_glyph(app, &item.command)
                .unwrap_or_else(|| forge_editor::icon::toolbar_fallback(app, &item.command));
            ToolBarButton {
                label: item.label.clone(),
                icon: if glyph.is_empty() {
                    item.icon.clone()
                } else {
                    glyph
                },
                system_icon: system.as_ref().map(|s| s.1.clone()).unwrap_or_default(),
                system_platform: system.map(|s| s.0.name().to_string()).unwrap_or_default(),
                icon_file: forge_editor::icon::toolbar_file(app, &item.command).unwrap_or_default(),
                command: item.command.clone(),
                available: (ctx.exists)(&item.command) && item.enabled.holds(&ctx.state),
                separator: item.is_separator(),
            }
        })
        .collect()
}

/// The modifier bar's buttons, marked with the modifiers already waiting.
fn modifier_buttons(app: &App) -> Vec<ModifierButton> {
    forge_core::toolbar::Modifier::ALL
        .iter()
        .map(|m| ModifierButton {
            name: m.name().to_string(),
            label: m.label().to_string(),
            pending: app.pending_modifiers.contains(m),
        })
        .collect()
}

/// The open context menu as the app's native menu, with the commands its
/// items run. None when it is closed.
fn context_menu_frame(app: &App, keys: &dyn Fn(&str) -> Vec<String>) -> Option<ContextMenuFrame> {
    let cm = &app.editor.context_menu;
    if !cm.is_open() {
        return None;
    }
    let ctx = menu::Context {
        // The context menu's items include the kill-ring paste commands
        // and the buffer list, which aren't in the editor's command table.
        exists: Box::new(|c: &str| {
            app.editor.commands.contains(c)
                || c.starts_with(forge_core::context_menu::YANK_PREFIX)
                || c.starts_with(menu::SELECT_BUFFER)
        }),
        keys: Box::new(|c: &str| keys(c)),
        state: menu_state(app),
    };
    let menu = cm.registry.menu(0)?;
    Some(ContextMenuFrame {
        column: cm.column,
        row: cm.row,
        items: menu_entries(&menu.items, &ctx, keys),
    })
}

/// The theme overrides as the app paints them: one entry per restyled
/// face, colors as `#rrggbb`.
fn theme_entries(app: &App) -> Vec<ThemeEntry> {
    app.editor
        .theme
        .entries()
        .into_iter()
        .map(|(face, s)| ThemeEntry {
            face,
            fg: s.fg.map(|c| c.hex()),
            bg: s.bg.map(|c| c.hex()),
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
        })
        .collect()
}

/// The editor state a menu item's condition is answered from.
fn menu_state(app: &App) -> menu::MenuState<'_> {
    // The same answers the terminal's menus use, so a tick means the same
    // thing in both (tool bar, tab bar, line-number-mode, ...).
    forge_editor::menu::state_for_mode_line(app)
}

impl Forge {
    fn send(&self, r: Request) {
        let _ = self.tx.lock().expect("sender lock").send(r);
    }
}

fn empty_frame(width: u16, height: u16) -> Frame {
    let _ = (width, height);
    Frame {
        web_pages: Vec::new(),
        windows: Vec::new(),
        echo: String::new(),
        completions: Vec::new(),
        cursor_x: 0,
        cursor_y: 0,
        hints: Vec::new(),
        hint_prefix: String::new(),
        busy: None,
        title: String::new(),
        modified: false,
        quit: false,
        crashed: None,
        scroll: None,
        scroll_bar: false,
        horizontal_scroll_bar: false,
        smooth_scroll: true,
        pixel_scroll_precision: false,
        tab_bar: false,
        menu_bar: false,
        tool_bar: false,
        tool_bar_bottom: false,
        tool_bar_style: forge_core::toolbar::Style::default().name().to_string(),
        tool_bar_items: Vec::new(),
        modifier_bar: false,
        modifier_buttons: Vec::new(),
        tabs: Vec::new(),
    }
}

/// The editor thread: handle requests in order; idle work when nothing
/// comes (auto-save, prefix hints, buffer-changed events, plugin reloads).
fn empty_grid(cols: u16, rows: u16, dark: bool) -> Grid {
    let (fg, bg) = if dark {
        (0xcdd6f4, 0x1e1e2e)
    } else {
        (0x4c4f69, 0xeff1f5)
    };
    Grid {
        cols,
        rows,
        runs: Vec::new(),
        pictures: Vec::new(),
        cursor_col: 0,
        cursor_row: 0,
        fg,
        bg,
        title: String::new(),
        modified: false,
        busy: None,
        quit: false,
        crashed: None,
        dialog: None,
    }
}

fn engine(app: &mut App, rx: &mpsc::Receiver<Request>, quit: &AtomicBool) {
    loop {
        let req = match app.idle_timeout() {
            Some(t) => match rx.recv_timeout(t) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => {
                    app.on_idle();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            },
        };
        match req {
            Request::Key(k) => app.handle_key(k),
            // A paste (an input method, the macOS app): one character is
            // typed like a key, several are inserted as is.
            Request::Text(t) => app.paste_text(&t),
            Request::Mouse(m) => forge_editor::mouse::handle(app, m),
            Request::Open(path) => {
                // A directory opens in Dired, like `forge DIR` does.
                if std::fs::metadata(&path)
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
                {
                    forge_editor::dired::open(app, std::path::Path::new(&path));
                } else {
                    match app.editor.open_file(&path) {
                        Ok((id, _)) => app.switch_to(id),
                        Err(e) => app.error(format!("Cannot open {path}: {e}")),
                    }
                }
            }
            Request::Goto(line, column) => app
                .buf()
                .set_point(forge_core::Pos::new(line as usize, column as usize)),
            Request::ScrollTop(line) => app.scroll_to_line(line as usize),
            Request::ScrollTopPixels(line, pixels) => {
                app.scroll_to_line_pixels(line as usize, pixels as usize);
            }
            Request::ScrollPixels { delta, row_height } => app.scroll_pixels(delta, row_height),
            Request::ScrollLeft(column) => app.scroll_to_column(column as usize),
            Request::Frame(w, h, reply) => {
                let _ = reply.send(build_frame(app, w, h));
            }
            Request::Dialog(id, path) => forge_editor::dialog::finish(app, id, path),
            Request::Grid(cols, rows, dark, reply) => {
                forge_core::canvas::set_dark_mode(Some(dark));
                let _ = reply.send(grid::build(app, cols, rows, dark));
            }
            Request::FrameAt(id, w, h, reply) => {
                let _ = reply.send(build_frame_in(app, id, w, h));
            }
            Request::Frames(reply) => {
                let selected = app.editor.windows.selected_frame_id();
                let frames = app
                    .editor
                    .windows
                    .frame_ids()
                    .into_iter()
                    .map(|id| EditorFrame {
                        id: id.0,
                        name: app.editor.windows.frame_name(id).unwrap_or("").to_string(),
                        selected: id == selected,
                    })
                    .collect();
                let _ = reply.send(frames);
            }
            Request::SelectFrame(id) => app.editor.select_frame(FrameId(id)),
            Request::ToolBarItem(index) => {
                forge_editor::toolbar::run_item(app, index as usize);
            }
            Request::ModifierButton(index) => {
                if let Some(m) = forge_core::toolbar::Modifier::ALL.get(index as usize) {
                    forge_editor::toolbar::toggle_modifier(app, *m);
                }
            }
            Request::SelectTab(frame, tab) => {
                app.editor.select_frame(FrameId(frame));
                app.editor.select_tab(TabId(tab));
            }
            Request::MoveTab(frame, tab, to) => {
                app.editor
                    .move_tab_to(FrameId(frame), TabId(tab), to as usize);
            }
            Request::NewTab(frame) => {
                app.editor.select_frame(FrameId(frame));
                app.editor.make_tab();
            }
            Request::CloseTab(frame, tab, reply) => {
                app.editor.select_frame(FrameId(frame));
                let done = match app.editor.delete_tab(TabId(tab)) {
                    Ok(()) => true,
                    Err(message) => {
                        app.error(message);
                        false
                    }
                };
                let _ = reply.send(done);
            }
            Request::CloseFrame(id, reply) => {
                let id = FrameId(id);
                let done = match app.editor.delete_frame(id) {
                    Ok(()) => true,
                    Err(message) => {
                        app.error(message);
                        false
                    }
                };
                let _ = reply.send(done);
            }
            Request::SaveSession(reply) => {
                app.editor.save_session();
                let _ = reply.send(());
            }
            Request::Command(name) => {
                forge_editor::commands::execute(app, &name);
            }
            Request::Menus(reply) => {
                app.editor.refresh_menus();
                let maps = app.editor.active_keymaps_now();
                let keys = move |c: &str| maps.where_is(c);
                let _ = reply.send(menu_groups(app, &keys));
            }
            Request::ContextMenu(reply) => {
                let maps = app.editor.active_keymaps_now();
                let keys = move |c: &str| maps.where_is(c);
                let _ = reply.send(context_menu_frame(app, &keys));
            }
            Request::Hover(column, row, reply) => {
                let _ = reply.send(
                    forge_editor::hover::at(app, column, row).map(|h| HoverInfo {
                        row: h.row,
                        start: h.start,
                        end: h.end,
                        tooltip: h.tooltip,
                    }),
                );
            }
            Request::CloseContextMenu => forge_editor::menu::close(app),
            Request::Theme(reply) => {
                let _ = reply.send(theme_entries(app));
            }
            Request::KillTop(reply) => {
                let top = if app.select_enable_clipboard {
                    app.editor
                        .kill_ring
                        .current()
                        .map(|s| s.replace('\0', "\\0"))
                } else {
                    None
                };
                let _ = reply.send(top);
            }
        }
        if app.quit {
            quit.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the app is shown when the editor thread panics: the panic's
    /// own message, for a `&str` and for a formatted `String` alike.
    #[test]
    fn a_panic_payload_reads_as_its_message() {
        let literal = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        let formatted = std::panic::catch_unwind(|| panic!("boom {}", 42)).unwrap_err();
        assert_eq!(panic_message(literal.as_ref()), "plain");
        assert_eq!(panic_message(formatted.as_ref()), "boom 42");
    }

    /// Option+Shift on a symbol (M-%, M-<) must name the same key the
    /// terminal's does, or the bindings would miss in the app.
    #[test]
    fn option_shift_symbols_name_the_terminal_key() {
        for (c, want) in [("%", "M-%"), ("<", "M-<"), (">", "M->")] {
            let ev = key_event(&KeyInput {
                key: Key::Char { c: c.to_string() },
                control: false,
                meta: true,
                shift: true,
            })
            .unwrap();
            assert_eq!(
                forge_editor::keymap::describe_key(&ev, false).as_deref(),
                Some(want)
            );
        }
    }

    fn text_of(row: &[StyledText]) -> String {
        row.iter().map(|s| s.text.as_str()).collect()
    }

    fn key(c: char, control: bool) -> KeyInput {
        KeyInput {
            key: Key::Char { c: c.to_string() },
            control,
            meta: false,
            shift: false,
        }
    }

    /// Drive an async getter to its value. The getters wait for the
    /// editor thread inside their first poll, so this never parks.
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};
        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    #[test]
    fn the_app_menu_bar_reads_the_editors_registry() {
        let mut app =
            forge_editor::start(&forge_editor::StartOptions::default()).expect("a fresh editor");
        let keys = |_: &str| Vec::new();
        // What the terminal's menu-bar-mode draws, and what the native
        // menu bar shows, are the same list.
        let groups = menu_groups(&app, &keys);
        let (visible, built_in) = {
            let state = menu_state(&app);
            let visible = app.editor.menus.visible_indices(&state);
            let titles: Vec<String> = visible
                .iter()
                .map(|&i| app.editor.menus.menus()[i].title.clone())
                .collect();
            (visible, titles)
        };
        assert_eq!(
            groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            built_in
        );
        for (g, &menu) in groups.iter().zip(&visible) {
            let m = &app.editor.menus.menus()[menu];
            assert_eq!(g.items.len(), m.items.len());
            for (e, i) in g.items.iter().zip(&m.items) {
                assert_eq!(e.label, i.label);
                assert_eq!(e.command, i.command);
                assert_eq!(e.separator, i.is_separator());
                assert_eq!(e.checked, i.checked.holds(&menu_state(&app)));
                // An item is never available without its command, but a
                // defined command can still be greyed: `Save` in File is,
                // on a buffer with nothing in it. A submenu header opens
                // rather than runs, so it is always available.
                assert!(
                    !e.available
                        || app.editor.commands.contains(&i.command)
                        || !e.submenu.is_empty()
                );
            }
        }
        // A builtin command is available; a plugin's is not until it loads.
        assert!(groups[0].items.iter().any(|i| i.available));
        // Nothing is available in a buffer with no changes yet: that's the
        // `:enable` the editor's own menus carry, and it's what greys Save
        // and Revert out.
        assert!(
            groups[0]
                .items
                .iter()
                .filter(|i| i.command == "save-buffer")
                .all(|i| !i.available)
        );
        assert!(
            groups
                .iter()
                .flat_map(|g| &g.items)
                .all(|i| !i.available || !i.separator),
            "a separator is never available"
        );

        // Submenus cross the boundary too: Edit's Search opens the two
        // searches, with their shortcuts.
        let edit = groups
            .iter()
            .find(|g| g.title == "Edit")
            .expect("Edit is in the bar");
        let search = edit
            .items
            .iter()
            .find(|i| i.label == "Incremental Search")
            .expect("Incremental Search is in Edit");
        assert_eq!(
            search
                .submenu
                .iter()
                .map(|s| (s.label.as_str(), s.command.as_str(), s.keys.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Forward String...", "isearch-forward", "C-s"),
                ("Backward String...", "isearch-backward", "C-r"),
            ]
        );

        // A plugin's menu reaches the native bar too.
        app.editor
            .menus
            .define(
                "Meow",
                vec![forge_core::menu::Item::new("Motion", "meow-insert")],
                "meow",
            )
            .unwrap();
        let groups = menu_groups(&app, &keys);
        assert_eq!(groups.last().unwrap().title, "Meow");
        assert_eq!(groups.last().unwrap().items[0].command, "meow-insert");
        // Unloading it takes the menu away again.
        app.editor.menus.remove_plugin("meow");
        assert!(menu_groups(&app, &keys).iter().all(|g| g.title != "Meow"));

        // A menu tied to a major mode shows only while that mode is on.
        app.editor
            .menus
            .define_when(
                "Markdown",
                vec![forge_core::menu::Item::new("Preview", "markdown-preview")],
                "markdown-mode",
                forge_core::menu::When::MajorMode("markdown-mode".into()),
            )
            .unwrap();
        assert!(
            menu_groups(&app, &keys)
                .iter()
                .all(|g| g.title != "Markdown"),
            "fundamental-mode has no Markdown menu"
        );
        app.buf().set_major_mode("markdown-mode");
        assert!(
            menu_groups(&app, &keys)
                .iter()
                .any(|g| g.title == "Markdown"),
            "the mode's menu is there while it is on"
        );
        app.editor
            .modes
            .define_major(
                "gfm-mode",
                Some("markdown-mode"),
                &[],
                Some("markdown-mode"),
            )
            .unwrap();
        app.buf().set_major_mode("gfm-mode");
        assert!(
            menu_groups(&app, &keys)
                .iter()
                .any(|g| g.title == "Markdown"),
            "and while a mode derived from it is on"
        );
        app.editor.menus.remove_plugin("markdown-mode");
    }

    #[test]
    fn the_app_context_menu_getter_follows_the_editor() {
        let dir = std::env::temp_dir().join(format!("forge-ffi-menu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let forge = Forge::new(vec![file.display().to_string()], true).unwrap();

        assert!(block_on(forge.context_menu()).is_none(), "closed");
        forge.run_command("context-menu-mode");
        forge.text("x".into());
        forge.mouse(MouseKind::RightDown, 1, 1);
        let menu = block_on(forge.context_menu()).expect("right-click opens it");
        assert_eq!((menu.column, menu.row), (1, 1));
        assert!(
            menu.items
                .iter()
                .any(|i| i.command == "undo" && i.available),
            "the edit can be undone"
        );
        forge.close_context_menu();
        assert!(block_on(forge.context_menu()).is_none(), "closed again");
    }

    #[test]
    fn the_app_context_menu_reads_the_open_menu() {
        let mut app =
            forge_editor::start(&forge_editor::StartOptions::default()).expect("a fresh editor");
        let keys = |_: &str| Vec::new();
        assert!(context_menu_frame(&app, &keys).is_none(), "closed");

        let input = forge_core::context_menu::Input {
            region: true,
            kill_ring: vec!["newest".into(), "older".into()],
            ..Default::default()
        };
        app.editor
            .context_menu
            .open(&["region".to_string()], &input, 3, 4, Some(0));
        let frame = context_menu_frame(&app, &keys).expect("open");
        assert_eq!((frame.column, frame.row), (3, 4));
        assert!(
            frame
                .items
                .iter()
                .any(|i| i.command == "kill-region" && i.available)
        );
        assert!(
            frame
                .items
                .iter()
                .any(|i| i.command == "kill-ring-save" && i.available)
        );
        // The kill-ring commands aren't in the editor's command table, but
        // they are available because the context menu's registry knows them.
        let paste = frame
            .items
            .iter()
            .find(|i| i.label == "Paste from Kill Menu")
            .expect("the kill menu");
        assert_eq!(paste.submenu.len(), 2);
        assert_eq!(paste.submenu[0].command, "context-menu-yank-1");
        assert!(paste.submenu.iter().all(|s| s.available));

        forge_editor::menu::close(&mut app);
        assert!(context_menu_frame(&app, &keys).is_none(), "closed again");
    }

    #[test]
    fn a_gui_frame_carries_the_native_tool_bar() {
        let mut app = forge_editor::start(&forge_editor::StartOptions {
            no_config: true,
            graphical: true,
            ..Default::default()
        })
        .expect("a fresh editor");
        let frame = build_frame(&mut app, 80, 24);
        assert!(frame.tool_bar);
        assert!(!frame.tool_bar_bottom);
        assert!(
            frame
                .tool_bar_items
                .iter()
                .any(|i| i.command == "save-buffer" && !i.separator),
            "the default bar crosses over"
        );
        assert!(!frame.modifier_bar);

        forge_editor::toolbar::set_modifier_mode(&mut app, true);
        let frame = build_frame(&mut app, 80, 24);
        assert!(frame.modifier_bar);
        assert_eq!(frame.modifier_buttons.len(), 6);
        assert!(frame.modifier_buttons.iter().all(|b| !b.pending));
    }

    #[test]
    fn waiting_for_the_editor_does_not_hold_the_polling_thread() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let (tx, rx) = mpsc::channel::<u32>();
        let mut fut = std::pin::pin!(answer(rx));
        // The first poll returns at once even though nothing was sent: it
        // used to sit in `recv_timeout` for the full wait.
        let started = std::time::Instant::now();
        let first = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert!(matches!(first, Poll::Pending));
        assert!(started.elapsed() < Duration::from_millis(20));
        tx.send(7).unwrap();
        assert_eq!(block_on(fut).ok(), Some(7));
    }

    #[test]
    fn an_unanswered_wait_times_out() {
        let (_tx, rx) = mpsc::channel::<u32>();
        assert!(block_on(answer(rx)).is_err());
    }

    #[test]
    fn a_terminal_frame_has_no_native_tool_bar() {
        let mut app = forge_editor::start(&forge_editor::StartOptions {
            no_config: true,
            graphical: false,
            ..Default::default()
        })
        .expect("a fresh editor");
        forge_editor::toolbar::set_mode(&mut app, true);
        let frame = build_frame(&mut app, 80, 24);
        assert!(!frame.tool_bar);
        assert!(frame.tool_bar_items.is_empty());
    }

    #[test]
    fn the_app_theme_reads_the_editors_table() {
        let mut app =
            forge_editor::start(&forge_editor::StartOptions::default()).expect("a fresh editor");
        assert!(theme_entries(&app).is_empty());
        app.editor
            .theme
            .set(
                "comment",
                forge_core::theme::FaceStyle {
                    fg: Some(forge_core::theme::Rgb(1, 2, 3)),
                    italic: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let entries = theme_entries(&app);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].face, "comment");
        assert_eq!(entries[0].fg.as_deref(), Some("#010203"));
        assert_eq!(entries[0].italic, Some(true));
        assert_eq!(entries[0].bg, None);
    }

    /// The always-visible titles of the macOS menu bar. The mode menus
    /// (Dired, Org, Webkit, ...) come and go with the buffer's major mode,
    /// and the app rebuilds its bar when the set of titles changes.
    #[test]
    fn the_macos_menu_bar_names_every_menu() {
        let built_in = forge_core::menu::Registry::builtin();
        let titles: Vec<&str> = built_in
            .menus()
            .iter()
            .filter(|m| m.when == forge_core::menu::When::Always)
            .map(|m| m.title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["File", "Edit", "Options", "Buffers", "Tools", "Help"],
            "frontends/apple/Forge/ForgeApp.swift names these"
        );
        assert_eq!(
            built_in.menus().last().map(|m| m.title.as_str()),
            Some("Help")
        );
    }

    #[test]
    fn keys_frames_and_quitting() {
        let dir = std::env::temp_dir().join(format!("forge-ffi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let forge = Forge::new(vec![file.display().to_string()], true).unwrap();

        let f = block_on(forge.frame(40, 10));
        assert_eq!(f.windows.len(), 1);
        assert_eq!(f.title, "main.rs");
        let first = &f.windows[0].rows[0];
        assert_eq!(text_of(first), "fn main() {}");
        assert_eq!(first[0].face, "keyword", "highlighted");
        assert_eq!(f.windows[0].rows.len(), 8, "the mode line is separate");

        // Typing goes through the same editor as the terminal.
        forge.key(key('e', true));
        for c in " // hi".chars() {
            forge.text(c.to_string());
        }
        let f = block_on(forge.frame(40, 10));
        assert_eq!(text_of(&f.windows[0].rows[0]), "fn main() {} // hi");
        assert!(f.modified);
        assert_eq!((f.cursor_x, f.cursor_y), (18, 0));

        // A prompt puts the cursor in the echo area.
        forge.key(KeyInput {
            key: Key::Char { c: "x".into() },
            control: false,
            meta: true,
            shift: false,
        });
        let f = block_on(forge.frame(40, 10));
        assert_eq!(f.echo, "M-x ");
        assert_eq!((f.cursor_x, f.cursor_y), (4, 9));
        forge.key(key('g', true));

        // A paste goes in as it is.
        forge.text("\nlet x = 1;\n".into());
        let f = block_on(forge.frame(40, 10));
        assert_eq!(text_of(&f.windows[0].rows[1]), "let x = 1;");

        // The kill ring starts empty; killing a line exposes it for the
        // system clipboard.
        assert_eq!(block_on(forge.kill_top()), None);
        forge.key(KeyInput {
            key: Key::Up,
            control: false,
            meta: false,
            shift: false,
        });
        forge.key(key('a', true));
        forge.key(key('k', true));
        assert_eq!(block_on(forge.kill_top()).as_deref(), Some("let x = 1;"));

        // C-x C-c with unsaved changes asks; y quits.
        forge.key(key('x', true));
        forge.key(key('c', true));
        forge.key(key('y', false));
        assert!(block_on(forge.frame(40, 10)).quit);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn key_conversion() {
        let k = key_event(&KeyInput {
            key: Key::Char { c: "X".into() },
            control: true,
            meta: false,
            shift: true,
        })
        .unwrap();
        // Control+Shift+X: lower-cased, with Shift kept, so it describes
        // as C-S-x rather than C-x.
        assert_eq!(
            k,
            KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
        );
        let k = key_event(&KeyInput {
            key: Key::Char { c: "A".into() },
            control: false,
            meta: false,
            shift: true,
        })
        .unwrap();
        assert_eq!(
            k,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            "and Shift+a describes as S-a, like the terminal's"
        );
        let k = key_event(&KeyInput {
            key: Key::Tab,
            control: false,
            meta: false,
            shift: true,
        })
        .unwrap();
        assert_eq!(k.code, KeyCode::BackTab);
        assert!(
            key_event(&KeyInput {
                key: Key::Char { c: "ab".into() },
                control: false,
                meta: false,
                shift: false
            })
            .is_none()
        );
    }

    #[test]
    fn external_file_open_and_goto_position() {
        let dir = std::env::temp_dir().join(format!("forge-ffi-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "first line\nsecond line\n").unwrap();

        let forge = Forge::new(vec![], true).unwrap();
        forge.open_file(file.display().to_string());
        forge.goto_position(1, 3);
        let frame = block_on(forge.frame(40, 10));
        assert_eq!(frame.title, "note.txt");
        assert_eq!((frame.cursor_x, frame.cursor_y), (3, 1));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_grid_shows_the_terminals_screen_with_resolved_colours() {
        let dir = std::env::temp_dir().join(format!("forge-ffi-grid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "first line\nsecond line\n").unwrap();

        let forge = Forge::new_grid(vec![], true).unwrap();
        forge.open_file(file.display().to_string());
        let g = block_on(forge.grid(60, 12, true));
        assert_eq!((g.cols, g.rows), (60, 12));
        assert_eq!(g.title, "note.txt");
        let text: String = g
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("|");
        assert!(text.contains("first line"), "{text:?}");
        assert!(
            g.runs.iter().any(|r| r.row == 0 && r.text.contains("File")),
            "the menu bar is drawn by Forge"
        );
        assert!(
            g.runs
                .iter()
                .any(|r| r.row == 10 && r.text.contains("note.txt")),
            "the mode line"
        );
        // The mode line is reversed: its background is the text colour.
        let mode = g.runs.iter().find(|r| r.row == 10).unwrap();
        assert_ne!(mode.bg, g.bg);
        assert_eq!(g.cursor_row, 1, "below the menu bar");
        // Light colours differ.
        let light = block_on(forge.grid(60, 12, false));
        assert_ne!(light.bg, g.bg);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
