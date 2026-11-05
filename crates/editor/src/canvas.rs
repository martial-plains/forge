// SPDX-License-Identifier: MIT OR Apache-2.0
//! Canvases in the editor: drawing a plugin's scene, turning mouse events
//! into scene events, and the commands that move around in one.
//!
//! The model lives in `forge_core::canvas` (the scenes, views and events)
//! and `forge-canvas` (shapes, hit testing, the renderer). This module is
//! the glue that needs the screen: it draws each changed canvas to a PNG
//! the frontend places over the window (the way Image mode does), and maps
//! a mouse cell to the point of the picture under it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use forge_canvas::{Op, Scene, Stroke, Transform};
use forge_core::BufferId;
use forge_core::WindowId;
use forge_core::canvas::Pointer;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::App;
use crate::org_cmds::Ask;

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::line(
        "canvas-demo",
        "Draw a sample canvas: shapes, text, arrows, a gradient.",
    ),
    forge_docs::CommandDoc::line("canvas-export", "Save this canvas as a PNG or SVG file."),
    forge_docs::CommandDoc::line(
        "canvas-list",
        "List the canvases that exist and who made them.",
    ),
    forge_docs::CommandDoc::line("canvas-pan-down", "Move the view of this canvas down."),
    forge_docs::CommandDoc::line("canvas-pan-left", "Move the view of this canvas left."),
    forge_docs::CommandDoc::line("canvas-pan-right", "Move the view of this canvas right."),
    forge_docs::CommandDoc::line("canvas-pan-up", "Move the view of this canvas up."),
    forge_docs::CommandDoc::line("canvas-refresh", "Draw this canvas again."),
    forge_docs::CommandDoc::line(
        "canvas-renderer",
        "Say what draws canvases: the GPU (wgpu) or the CPU.",
    ),
    forge_docs::CommandDoc::line(
        "canvas-reset-view",
        "Show the whole canvas again, unzoomed.",
    ),
    forge_docs::CommandDoc::line("canvas-zoom-in", "Zoom this canvas in."),
    forge_docs::CommandDoc::line("canvas-zoom-out", "Zoom this canvas out."),
];

/// canvas-mode's keys.
pub const CANVAS_MODE_BINDINGS: &[(&str, &str)] = &[
    ("+", "canvas-zoom-in"),
    ("=", "canvas-zoom-in"),
    ("-", "canvas-zoom-out"),
    ("0", "canvas-reset-view"),
    ("<left>", "canvas-pan-left"),
    ("<right>", "canvas-pan-right"),
    ("<up>", "canvas-pan-up"),
    ("<down>", "canvas-pan-down"),
    ("g", "canvas-refresh"),
    ("w", "canvas-export"),
    ("q", "quit-window"),
];

/// Width of a terminal cell over its height; a frontend that knows the
/// real shape can say so through `canvas_pointer`.
const CELL_ASPECT: f32 = 0.5;
/// Most pixels along a side when a canvas is drawn for the screen.
const MAX_SIDE: f32 = 2560.0;
/// Two presses this close together (in time, on one canvas) are a double.
const DOUBLE: Duration = Duration::from_millis(400);

fn temp_path(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let safe: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(16)
        .collect();
    let ext = if forge_core::canvas::native_scenes() {
        forge_core::canvas::SCENE_SUFFIX
    } else {
        ".png"
    };
    std::env::temp_dir().join(format!(
        "forge-canvas-{}-{safe}-{n}{ext}",
        std::process::id()
    ))
}

fn is_temp(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("forge-canvas-"))
}

/// The shape (width over height) of the window showing a canvas's buffer,
/// as the picture will be laid over it.
fn pane_aspect(app: &App, buffer: BufferId) -> Option<f32> {
    let frame = app.editor.windows.frame();
    let cell = forge_core::org_display::cell_size().map_or(CELL_ASPECT, |(w, h)| (w / h) as f32);
    app.editor
        .windows
        .layout(frame)
        .into_iter()
        .find_map(|(id, r)| {
            let w = app.editor.windows.get(id)?;
            (w.buffer == buffer && r.width > 0 && w.height > 0)
                .then(|| f32::from(r.width) * cell / w.height as f32)
        })
}

/// Give each canvas the shape of its window, so the picture fills it and
/// the board isn't cut off at the picture's edge.
fn fit_frames(app: &mut App) {
    let shapes: Vec<(String, f32)> = app
        .editor
        .canvases
        .iter()
        .filter_map(|(n, c)| pane_aspect(app, c.buffer).map(|a| (n.clone(), a)))
        .collect();
    for (name, aspect) in shapes {
        if let Some(c) = app.editor.canvases.get_mut(&name) {
            c.set_frame_aspect(aspect);
        }
    }
}

/// Pictures replaced but not yet deleted, oldest first.
static RETIRED: Mutex<std::collections::VecDeque<PathBuf>> =
    Mutex::new(std::collections::VecDeque::new());
/// How many replaced pictures stay on disk for a frame still showing one.
const KEEP_RETIRED: usize = 12;

fn retire(path: PathBuf) {
    let mut q = RETIRED.lock().unwrap_or_else(|e| e.into_inner());
    q.push_back(path);
    while q.len() > KEEP_RETIRED {
        if let Some(old) = q.pop_front() {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// Canvases waiting on a picture from the web, and when they were drawn.
static WAITING: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// How long to wait before looking again for a picture on its way.
const RETRY: Duration = Duration::from_millis(700);

/// One canvas to draw, handed to the drawing thread.
struct Job {
    token: u64,
    name: String,
    buffer: BufferId,
    frame: Scene,
    scale: f32,
    palette: forge_canvas::Palette,
    path: PathBuf,
}

/// A canvas drawn.
struct Done {
    token: u64,
    name: String,
    buffer: BufferId,
    path: PathBuf,
    size: (u32, u32),
    drawn: bool,
    waiting: bool,
}

/// Drawing happens on its own thread, so scrolling, typing and the screen
/// never wait for it: a canvas asked for again while it is queued is
/// replaced by the newer ask (a fast scroll draws the last view, not every
/// one on the way), and the last picture stays up until the next is ready.
struct Worker {
    queue: Mutex<Vec<Job>>,
    wake: std::sync::Condvar,
    done: Mutex<Vec<Done>>,
    /// The token of the set of canvases being drawn now (0: none).
    busy: AtomicU64,
}

static WORKER: std::sync::OnceLock<std::sync::Arc<Worker>> = std::sync::OnceLock::new();

fn worker() -> &'static std::sync::Arc<Worker> {
    WORKER.get_or_init(|| {
        let w = std::sync::Arc::new(Worker {
            queue: Mutex::new(Vec::new()),
            wake: std::sync::Condvar::new(),
            done: Mutex::new(Vec::new()),
            busy: AtomicU64::new(0),
        });
        let theirs = std::sync::Arc::clone(&w);
        let spawned = std::thread::Builder::new()
            .name("forge-canvas".into())
            .spawn(move || {
                loop {
                    let job = {
                        let mut q = theirs.queue.lock().unwrap_or_else(|e| e.into_inner());
                        while q.is_empty() {
                            q = theirs.wake.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                        let job = q.remove(0);
                        theirs.busy.store(job.token, Ordering::SeqCst);
                        job
                    };
                    let size = (job.frame.width as u32, job.frame.height as u32);
                    let native = job
                        .path
                        .to_str()
                        .is_some_and(|p| p.ends_with(forge_core::canvas::SCENE_SUFFIX));
                    let (png, waiting) = if native {
                        forge_core::canvas::scene_json_waiting(&job.frame, &job.palette)
                    } else {
                        forge_core::canvas::render_png_waiting(&job.frame, job.scale, &job.palette)
                    };
                    let drawn = png.is_some_and(|png| std::fs::write(&job.path, png).is_ok());
                    theirs
                        .done
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(Done {
                            token: job.token,
                            name: job.name,
                            buffer: job.buffer,
                            path: job.path,
                            size,
                            drawn,
                            waiting,
                        });
                    theirs.busy.store(0, Ordering::SeqCst);
                }
            });
        // Without the thread nothing is drawn; there is nowhere better to say so.
        debug_assert!(
            spawned.is_ok(),
            "canvas: could not start the drawing thread"
        );
        w
    })
}

/// A canvas is being drawn or its picture is waiting to be picked up: the
/// event loop should look again soon.
pub fn rendering(app: &App) -> bool {
    let token = app.editor.canvases.token;
    let Some(w) = WORKER.get().filter(|_| token != 0) else {
        return false;
    };
    w.busy.load(Ordering::SeqCst) == token
        || w.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|j| j.token == token)
        || w.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|d| d.token == token)
}

/// Hand finished pictures to their buffers.
fn collect(app: &mut App) {
    let Some(w) = WORKER.get() else { return };
    let token = app.editor.canvases.token;
    let finished: Vec<Done> = {
        let mut all = w.done.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest): (Vec<Done>, Vec<Done>) = std::mem::take(&mut *all)
            .into_iter()
            .partition(|d| d.token == token);
        *all = rest;
        mine
    };
    for d in finished {
        if d.waiting
            && let Ok(mut wt) = WAITING.lock()
        {
            wt.retain(|(n, _)| *n != d.name);
            wt.push((d.name.clone(), Instant::now()));
        }
        let Some(b) = app.editor.state.buffers.get_mut(&d.buffer) else {
            let _ = std::fs::remove_file(&d.path);
            continue;
        };
        let st = b.image.get_or_insert_with(Default::default);
        let old = st.rendered.take();
        if d.drawn {
            st.rendered = Some(d.path);
            st.size = d.size;
        }
        // The frontend may still be drawing a frame that names the last
        // picture (or the one before); deleting it at once makes the canvas
        // blink blank. Old pictures go only when several newer ones exist.
        if let Some(o) = old.filter(|o| is_temp(o)) {
            retire(o);
        }
    }
}

/// Draw every canvas whose scene or view changed (on the drawing thread),
/// and pick up the pictures that are ready.
pub fn sync(app: &mut App) {
    fit_frames(app);
    // A canvas that was drawn without a picture still on its way is drawn
    // again after a moment, and so on until it has arrived.
    if let Ok(mut waiting) = WAITING.lock() {
        waiting.retain(|(name, at)| {
            if at.elapsed() < RETRY {
                return true;
            }
            if let Some(c) = app.editor.canvases.get_mut(name) {
                c.dirty = true;
            }
            false
        });
    }
    // The theme, or the system's light or dark mode, changed: every canvas is
    // drawn again in the new colours (scenes name colour roles, so nothing
    // has to ask the plugins that made them).
    let palette = app.editor.canvas_palette();
    if app.editor.canvases.drawn_in.as_ref() != Some(&palette) {
        if app.editor.canvases.drawn_in.is_some() {
            for (_, c) in app.editor.canvases.iter_mut() {
                c.dirty = true;
            }
        }
        app.editor.canvases.drawn_in = Some(palette.clone());
    }
    collect(app);
    if !app.editor.canvases.any_dirty() {
        return;
    }
    let names: Vec<String> = app
        .editor
        .canvases
        .iter()
        .filter(|(_, c)| c.dirty)
        .map(|(n, _)| n.clone())
        .collect();
    let w = worker();
    if app.editor.canvases.token == 0 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        app.editor.canvases.token = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    let token = app.editor.canvases.token;
    for name in names {
        let Some(c) = app.editor.canvases.get_mut(&name) else {
            continue;
        };
        c.dirty = false;
        c.generation += 1;
        let (buffer, frame) = (c.buffer, c.framed());
        let scale = (MAX_SIDE / frame.width.max(frame.height).max(1.0)).clamp(0.25, 2.0);
        let job = Job {
            token,
            name: name.clone(),
            buffer,
            frame,
            scale,
            palette: palette.clone(),
            path: temp_path(&name),
        };
        let mut q = w.queue.lock().unwrap_or_else(|e| e.into_inner());
        // A newer ask for the same canvas replaces one still waiting.
        q.retain(|j| {
            let stale = j.token == token && j.name == name;
            if stale {
                let _ = std::fs::remove_file(&j.path);
            }
            !stale
        });
        q.push(job);
        w.wake.notify_one();
    }
}

/// Wait for every canvas to be drawn and picked up (tests, and anything
/// that must see the picture now).
pub fn finish(app: &mut App) {
    let start = Instant::now();
    loop {
        sync(app);
        if !rendering(app) && !app.editor.canvases.any_dirty() {
            return;
        }
        if start.elapsed() > Duration::from_secs(30) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The canvas a window shows, and where a screen cell lies on its picture
/// (0..1 across and down), if it lies on it.
fn locate(app: &App, x: u16, y: u16) -> Option<(WindowId, String, f32, f32)> {
    let frame = app.editor.windows.frame();
    for (id, r) in app.editor.windows.layout(frame) {
        if x < r.x || x >= r.x + r.width || y < r.y || y >= r.y + r.height {
            continue;
        }
        let w = app.editor.windows.get(id)?;
        let name = app.editor.canvases.name_of(w.buffer)?.to_string();
        let c = app.editor.canvases.get(&name)?;
        let (cols, rows) = (f32::from(r.width), w.height as f32);
        let (cx, cy) = (
            f32::from(x - r.x) + 0.5,
            f32::from(y - r.y) - w.header_rows as f32 + 0.5,
        );
        if rows < 1.0 || cy < 0.0 || cy > rows {
            return None;
        }
        // The picture is fitted to the body and centred: find its box.
        let (bw, bh) = (cols * CELL_ASPECT, rows);
        let (fw, fh) = c.size();
        let aspect = (fw / fh.max(1.0)).max(1e-3);
        let (dw, dh) = if bw / bh > aspect {
            (bh * aspect, bh)
        } else {
            (bw, bw / aspect)
        };
        let (ox, oy) = ((bw - dw) / 2.0, (bh - dh) / 2.0);
        let (px, py) = (cx * CELL_ASPECT - ox, cy - oy);
        if px < 0.0 || py < 0.0 || px > dw || py > dh {
            return None;
        }
        return Some((id, name, px / dw, py / dh));
    }
    None
}

struct Drag {
    name: String,
    button: u8,
    fx: f32,
    fy: f32,
    /// What this drag does on a scrolling page: undecided, moving the page
    /// (a swipe), or carrying the thing it started on (a held press).
    mode: DragMode,
    /// When the press landed, and whether it landed on a note's card.
    since: Instant,
    on_card: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Undecided,
    Pan,
    Carry,
}

/// A press this long on a card, before moving, picks the card up; a quicker
/// move is a swipe that scrolls the page (what a finger means). Only for
/// touch: a mouse picks a card up the moment it moves.
const HOLD: Duration = Duration::from_millis(350);

/// Whether pointer events come from a finger. A frontend that delivers touch
/// as mouse events says so (`set_touch`); the default is a mouse.
static TOUCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_touch(on: bool) {
    TOUCH.store(on, std::sync::atomic::Ordering::Relaxed);
}

static DRAG: Mutex<Option<Drag>> = Mutex::new(None);
static LAST_PRESS: Mutex<Option<(String, Instant, Option<String>)>> = Mutex::new(None);

fn button_number(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    }
}

/// A mouse event over a canvas. True when it was one (and was handled).
pub fn mouse(app: &mut App, ev: &MouseEvent) -> bool {
    let drag = DRAG.lock().ok().and_then(|mut d| d.take());
    let located = locate(app, ev.column, ev.row);
    let (kind, button, wheel) = match ev.kind {
        MouseEventKind::Down(b) => ("press", button_number(b), 0.0),
        MouseEventKind::Up(b) => ("release", button_number(b), 0.0),
        MouseEventKind::Drag(b) => ("drag", button_number(b), 0.0),
        MouseEventKind::ScrollUp => ("scroll", 0, 1.0),
        MouseEventKind::ScrollDown => ("scroll", 0, -1.0),
        MouseEventKind::ScrollLeft => ("hscroll", 0, 1.0),
        MouseEventKind::ScrollRight => ("hscroll", 0, -1.0),
        _ => {
            restore(drag);
            return located.is_some();
        }
    };
    // A drag continues on its own canvas even when the pointer leaves the
    // picture (the position is then clamped to its edge).
    let (name, fx, fy) = match (&located, &drag) {
        (Some((_, n, fx, fy)), _) => (n.clone(), *fx, *fy),
        (None, Some(d)) if matches!(kind, "drag" | "release") => (d.name.clone(), d.fx, d.fy),
        _ => {
            restore(drag);
            return false;
        }
    };
    if let Some((id, ..)) = &located
        && kind == "press"
    {
        app.select_window(*id);
    }
    let mods = ev.modifiers;
    let mut kind = kind;
    if kind == "press" {
        let hit = app.editor.canvases.get(&name).and_then(|c| {
            let (x, y) = c.to_scene(fx, fy);
            c.scene.hit(x, y)
        });
        let mut last = LAST_PRESS.lock().unwrap_or_else(|e| e.into_inner());
        if last
            .as_ref()
            .is_some_and(|(n, t, h)| *n == name && *h == hit && t.elapsed() < DOUBLE)
        {
            kind = "double";
            *last = None;
        } else {
            *last = Some((name.clone(), Instant::now(), hit));
        }
    }
    let handles_view = app
        .editor
        .canvases
        .get(&name)
        .is_some_and(|c| c.handles_view);
    let scrolls = app.editor.canvases.get(&name).is_some_and(|c| c.scroll);
    let on_card = kind == "press"
        && app.editor.canvases.get(&name).is_some_and(|c| {
            let (x, y) = c.to_scene(fx, fy);
            c.scene.hit(x, y).is_some_and(|id| id.starts_with("note:"))
        });
    // What the drag in progress is (carried from the press).
    let mut mode = drag
        .as_ref()
        .filter(|d| d.name == name)
        .map_or(DragMode::Undecided, |d| d.mode);
    let since = drag
        .as_ref()
        .filter(|d| d.name == name && kind != "press")
        .map_or_else(Instant::now, |d| d.since);
    let on_card = if kind == "press" {
        on_card
    } else {
        drag.as_ref().is_some_and(|d| d.on_card)
    };
    if let Some(c) = app.editor.canvases.get_mut(&name) {
        if kind == "hscroll" {
            // A trackpad's or tilt wheel's sideways travel.
            c.pan(-wheel * 0.08, 0.0);
            sync(app);
            return true;
        }
        if scrolls {
            // A page: the wheel scrolls it (ctrl zooms, shift goes sideways).
            match kind {
                "scroll" if mods.contains(KeyModifiers::CONTROL) => {
                    c.zoom_about(fx, fy, if wheel > 0.0 { 1.1 } else { 1.0 / 1.1 })
                }
                "scroll" if mods.contains(KeyModifiers::SHIFT) => c.pan(-wheel * 0.08, 0.0),
                "scroll" => c.pan(0.0, -wheel * 0.12),
                "press" if button == 1 => mode = DragMode::Undecided,
                "drag" if button == 1 => {
                    if let Some(d) = drag.as_ref().filter(|d| d.name == name && d.button == 1) {
                        let (dx, dy) = (d.fx - fx, d.fy - fy);
                        if mode == DragMode::Undecided && dx.abs() + dy.abs() > 0.01 {
                            // On a card: a mouse carries it at once, a finger
                            // when it has been held. Anywhere else, the page moves.
                            let touch = TOUCH.load(std::sync::atomic::Ordering::Relaxed);
                            mode = if on_card && (!touch || since.elapsed() >= HOLD) {
                                DragMode::Carry
                            } else {
                                DragMode::Pan
                            };
                        }
                        if mode == DragMode::Pan {
                            c.pan(dx, dy);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // A swipe moves the page and nothing else: the plugin hears neither the
    // drag nor the release, so no click or drop follows.
    if scrolls && matches!(kind, "drag" | "release") && button == 1 && mode == DragMode::Pan {
        if kind == "drag" {
            restore(Some(Drag {
                name: name.clone(),
                button,
                fx,
                fy,
                mode,
                since,
                on_card,
            }));
        }
        sync(app);
        return true;
    }
    // A press the finger hasn't decided about yet is held back too, so a
    // swipe that starts on a card doesn't also press it.
    let _ = handles_view;
    if !scrolls
        && !handles_view
        && let Some(c) = app.editor.canvases.get_mut(&name)
    {
        match kind {
            "scroll" if mods.contains(KeyModifiers::SHIFT) || mods.contains(KeyModifiers::ALT) => {
                c.pan(-wheel * 0.08, 0.0)
            }
            "scroll" => c.zoom_about(fx, fy, if wheel > 0.0 { 1.1 } else { 1.0 / 1.1 }),
            "drag" => {
                if let Some(d) = drag.as_ref().filter(|d| d.button == 2 && d.name == name) {
                    c.pan(d.fx - fx, d.fy - fy);
                }
            }
            _ => {}
        }
    }
    let pointer = Pointer {
        kind: match kind {
            "press" => "press",
            "double" => "double",
            "release" => "release",
            "drag" => "drag",
            _ => "scroll",
        },
        button,
        fx,
        fy,
        dy: wheel,
        shift: mods.contains(KeyModifiers::SHIFT),
        ctrl: mods.contains(KeyModifiers::CONTROL),
        alt: mods.contains(KeyModifiers::ALT),
    };
    if matches!(kind, "press" | "double" | "drag") && button != 0 {
        restore(Some(Drag {
            name: name.clone(),
            button,
            fx,
            fy,
            mode,
            since,
            on_card,
        }));
    }
    if let Some(cmd) = app.editor.canvas_event(&name, &pointer) {
        app.run_command(&cmd);
    }
    sync(app);
    true
}

fn restore(d: Option<Drag>) {
    if let (Some(d), Ok(mut slot)) = (d, DRAG.lock()) {
        *slot = Some(d);
    }
}

fn current(app: &App) -> Option<String> {
    let id = app.editor.state.active_buffer?;
    app.editor.canvases.name_of(id).map(str::to_string)
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "canvas-demo" => {
            demo(app);
            true
        }
        "canvas-renderer" => {
            app.msg(format!(
                "Canvases are drawn by {}",
                forge_core::canvas::renderer_name()
            ));
            true
        }
        "canvas-list" => {
            list(app);
            true
        }
        "canvas-export" | "canvas-zoom-in" | "canvas-zoom-out" | "canvas-reset-view"
        | "canvas-pan-left" | "canvas-pan-right" | "canvas-pan-up" | "canvas-pan-down"
        | "canvas-refresh" => {
            let Some(name) = current(app) else {
                app.error("Not in a canvas");
                return true;
            };
            if cmd == "canvas-export" {
                app.prompt(
                    "Save canvas to (.png or .svg): ",
                    crate::minibuffer::Purpose::OrgAsk(Ask::CanvasExport(name)),
                );
                return true;
            }
            if let Some(c) = app.editor.canvases.get_mut(&name) {
                match cmd {
                    "canvas-zoom-in" => c.zoom_about(0.5, 0.5, 1.25),
                    "canvas-zoom-out" => c.zoom_about(0.5, 0.5, 0.8),
                    "canvas-reset-view" => {
                        c.view = Default::default();
                        c.dirty = true;
                    }
                    "canvas-pan-left" => c.pan(-0.1, 0.0),
                    "canvas-pan-right" => c.pan(0.1, 0.0),
                    "canvas-pan-up" => c.pan(0.0, -0.1),
                    "canvas-pan-down" => c.pan(0.0, 0.1),
                    _ => c.dirty = true,
                }
            }
            sync(app);
            true
        }
        // The demo's own listener: say what was clicked.
        "canvas-demo-event" => {
            if let Some(json) = app
                .editor
                .canvases
                .get("demo")
                .and_then(|c| c.last_event.clone())
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&json)
                && matches!(v["kind"].as_str(), Some("press" | "double"))
            {
                let id = v["id"].as_str().unwrap_or("nothing");
                app.msg(format!(
                    "{} on {id} at ({:.0}, {:.0})",
                    v["kind"].as_str().unwrap_or("?"),
                    v["x"].as_f64().unwrap_or(0.0),
                    v["y"].as_f64().unwrap_or(0.0)
                ));
            }
            true
        }
        _ => false,
    }
}

pub fn answer(app: &mut App, ask: &Ask, input: &str) -> bool {
    let Ask::CanvasExport(name) = ask else {
        return false;
    };
    let path = input.trim();
    if path.is_empty() {
        return true;
    }
    let Some(scene) = app.editor.canvases.get(name).map(|c| c.scene.clone()) else {
        app.error(format!("No canvas named {name}"));
        return true;
    };
    let path = forge_core::permissions::expand_home(path);
    let result = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        std::fs::write(&path, scene.resolved(&app.editor.canvas_palette()).to_svg())
            .map_err(|e| e.to_string())
    } else {
        forge_core::canvas::render_png_waiting(&scene, 2.0, &app.editor.canvas_palette())
            .0
            .ok_or_else(|| "nothing to draw".to_string())
            .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()))
    };
    match result {
        Ok(()) => app.msg(format!("Wrote {}", path.display())),
        Err(e) => app.error(format!("{}: {e}", path.display())),
    }
    true
}

fn list(app: &mut App) {
    let rows: Vec<String> = app
        .editor
        .canvases
        .iter()
        .map(|(n, c)| format!("{n}  ({}, {} shapes)", c.owner, c.scene.ops.len()))
        .collect();
    if rows.is_empty() {
        app.msg("No canvases (try M-x canvas-demo)");
    } else {
        app.msg(rows.join("; "));
    }
}

/// A scene that shows what a canvas can do.
pub fn demo_scene() -> Scene {
    let mut s = Scene::new(960.0, 600.0).background("#1e2030");
    s.push(Op::text(40.0, 28.0, "Forge canvas", 34.0, "#ffffff"));
    s.push(Op::text(
        42.0,
        74.0,
        "Shapes keep their ids: click one, or the empty background.",
        16.0,
        "#a6adc8",
    ));
    let card = |x: f32, y: f32, id: &str, title: &str, body: &str, color: &str| {
        Op::group(vec![
            Op::rect(0.0, 0.0, 240.0, 150.0)
                .fill_linear(0.0, 0.0, 0.0, 150.0, &[(0.0, color), (1.0, "#313244")])
                .stroked(Stroke::new("#585b70", 1.5))
                .rounded(14.0),
            Op::text(16.0, 14.0, title, 20.0, "#ffffff"),
            Op::text(16.0, 48.0, body, 14.0, "#cdd6f4").wrapped(208.0),
        ])
        .transformed(Transform::translate(x, y))
        .with_id(id)
    };
    s.push(card(
        40.0,
        130.0,
        "notes",
        "Notes",
        "Cards are groups: one id, any shapes, moved by a transform.",
        "#45475a",
    ));
    s.push(card(
        360.0,
        130.0,
        "tasks",
        "Tasks",
        "Text wraps to the box and is cut with an ellipsis when it must be.",
        "#3b5a6b",
    ));
    s.push(card(
        680.0,
        130.0,
        "ideas",
        "Ideas",
        "Wheel zooms, middle-drag pans, and a plugin sees every event.",
        "#5a4570",
    ));
    s.push(
        Op::line(
            280.0,
            205.0,
            360.0,
            205.0,
            Stroke::new("#f9e2af", 3.0).arrow(),
        )
        .with_id("a"),
    );
    s.push(
        Op::path("M600 205 C 640 205, 640 205, 680 205")
            .stroked(Stroke::new("#a6e3a1", 3.0).dashed(8.0, 6.0).arrow())
            .with_id("b"),
    );
    s.push(
        Op::ellipse(160.0, 440.0, 90.0, 60.0)
            .fill("#89b4fa")
            .stroked(Stroke::new("#ffffff", 2.0))
            .with_id("ellipse"),
    );
    s.push(
        Op::path("M360 500 L 460 380 L 560 500 Z")
            .fill("#f38ba8")
            .with_id("triangle"),
    );
    s.push(Op::text(700.0, 400.0, "Export with w: PNG or SVG.", 18.0, "#f9e2af").with_id("hint"));
    s
}

fn demo(app: &mut App) {
    let id: BufferId = app.editor.canvas_show("forge", "demo", demo_scene());
    if let Some(c) = app.editor.canvases.get_mut("demo") {
        c.listener = Some("canvas-demo-event".into());
    }
    app.editor.show_buffer(id);
    sync(app);
    app.msg("Canvas demo: click shapes, wheel to zoom, w to export");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(forge_core::Editor::new())
    }

    #[test]
    fn the_demo_draws_and_clicks_report_shapes() {
        let mut a = app();
        demo(&mut a);
        finish(&mut a);
        let c = a.editor.canvases.get("demo").unwrap();
        assert_eq!(a.buf_ref().major_mode(), forge_core::canvas::MODE);
        let png = a
            .buf_ref()
            .image
            .as_ref()
            .and_then(|s| s.rendered.clone())
            .expect("drawn");
        assert!(png.exists());
        let bytes = std::fs::read(&png).unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(c.scene.hit(160.0, 440.0).as_deref(), Some("ellipse"));
        let cmd = a
            .editor
            .canvas_event(
                "demo",
                &Pointer {
                    kind: "press",
                    button: 1,
                    fx: 160.0 / 960.0,
                    fy: 440.0 / 600.0,
                    dy: 0.0,
                    shift: false,
                    ctrl: false,
                    alt: false,
                },
            )
            .unwrap();
        assert_eq!(cmd, "canvas-demo-event");
        assert!(
            a.editor
                .canvases
                .get("demo")
                .unwrap()
                .last_event
                .as_deref()
                .unwrap()
                .contains("\"id\":\"ellipse\"")
        );
        a.run_command(&cmd);
        assert!(
            a.message
                .as_deref()
                .unwrap_or("")
                .contains("press on ellipse"),
            "{:?}",
            a.message
        );
    }

    fn mouse_at(a: &mut App, kind: MouseEventKind) -> bool {
        mouse(
            a,
            &MouseEvent {
                kind,
                column: 10,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
        )
    }

    #[test]
    fn a_swipe_scrolls_a_page_and_a_held_card_is_carried() {
        let mut a = app();
        let mut scene = Scene::new(2000.0, 6000.0);
        scene.ops.push(
            Op::rect(0.0, 0.0, 2000.0, 6000.0)
                .with_id("card")
                .with_id("note:0"),
        );
        let id = a.editor.canvas_show("me", "page", scene);
        {
            let c = a.editor.canvases.get_mut("page").unwrap();
            c.scroll = true;
            c.scroll_w = 1000.0;
            c.listener = Some("page-event".into());
        }
        a.editor.show_buffer(id);
        sync(&mut a);
        let before = a.editor.canvases.get("page").unwrap().view;
        // With a mouse, a drag that starts on a card carries it at once: the
        // page stays and the plugin hears the drag.
        set_touch(false);
        assert!(mouse_at(&mut a, MouseEventKind::Down(MouseButton::Left)));
        a.editor.canvases.get_mut("page").unwrap().last_event = None;
        mouse(
            &mut a,
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(
            a.editor.canvases.get("page").unwrap().last_event.is_some(),
            "the plugin heard the drag"
        );
        assert_eq!(
            a.editor.canvases.get("page").unwrap().view.y,
            before.y,
            "the page didn't move"
        );
        mouse(
            &mut a,
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        // A quick press and drag with a finger is a swipe: the page moves, nothing is sent.
        set_touch(true);
        assert!(mouse_at(&mut a, MouseEventKind::Down(MouseButton::Left)));
        a.editor.canvases.get_mut("page").unwrap().last_event = None;
        mouse(
            &mut a,
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 10,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        mouse(
            &mut a,
            &MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 10,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        let after = a.editor.canvases.get("page").unwrap();
        assert!(
            after.view.y > before.y,
            "the page scrolled: {:?} -> {:?}",
            before,
            after.view
        );
        assert!(
            after.last_event.is_none(),
            "the plugin heard nothing of the swipe"
        );
        set_touch(false);
        // Sideways wheel travel moves it across.
        let x0 = a.editor.canvases.get("page").unwrap().view.x;
        mouse_at(&mut a, MouseEventKind::ScrollRight);
        assert!(a.editor.canvases.get("page").unwrap().view.x > x0);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer_still() {
        let mut a = app();
        demo(&mut a);
        let c = a.editor.canvases.get_mut("demo").unwrap();
        let before = c.to_scene(0.25, 0.75);
        c.zoom_about(0.25, 0.75, 2.0);
        let after = c.to_scene(0.25, 0.75);
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
        assert!((c.view.zoom - 2.0).abs() < 1e-6);
        c.zoom_about(0.5, 0.5, 1000.0);
        assert_eq!(c.view.zoom, forge_core::canvas::MAX_ZOOM);
    }

    #[test]
    fn view_commands_move_the_view_and_redraw() {
        let mut a = app();
        demo(&mut a);
        finish(&mut a);
        let first = a.buf_ref().image.as_ref().unwrap().rendered.clone();
        assert!(first.is_some());
        assert!(execute(&mut a, "canvas-zoom-in"));
        finish(&mut a);
        let c = a.editor.canvases.get("demo").unwrap();
        assert!(c.view.zoom > 1.0 && !c.dirty);
        assert_ne!(
            a.buf_ref().image.as_ref().unwrap().rendered,
            first,
            "drawn again"
        );
        assert!(execute(&mut a, "canvas-reset-view"));
        assert_eq!(a.editor.canvases.get("demo").unwrap().view.zoom, 1.0);
    }

    #[test]
    fn export_writes_png_and_svg() {
        let mut a = app();
        demo(&mut a);
        let dir = std::env::temp_dir().join(format!("forge-canvas-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["out.png", "out.svg"] {
            let p = dir.join(f);
            assert!(answer(
                &mut a,
                &Ask::CanvasExport("demo".into()),
                &p.display().to_string()
            ));
            let bytes = std::fs::read(&p).unwrap();
            assert!(if f.ends_with("png") {
                bytes.starts_with(&[0x89])
            } else {
                bytes.starts_with(b"<svg")
            });
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_and_keys_are_consistent() {
        assert!(COMMANDS.windows(2).all(|w| w[0].name() < w[1].name()));
        for (_, cmd) in CANVAS_MODE_BINDINGS {
            assert!(
                *cmd == "quit-window" || COMMANDS.iter().any(|c| c.name() == *cmd),
                "{cmd} is documented"
            );
        }
    }
}

#[cfg(test)]
mod background_tests {
    use super::*;

    /// Many asks while one draw is going come out as the newest ask, and a
    /// theme change draws the canvas again in the new colours, with no
    /// plugin involved.
    #[test]
    fn scrolling_asks_are_coalesced_and_a_theme_change_redraws() {
        let mut a = App::new(forge_core::Editor::new());
        // A scene that names colour roles, as the Bases and graph plugins' do.
        let pal = forge_canvas::Palette::current();
        let mut scene = Scene::new(400.0, 300.0).background(&pal.bg);
        scene.ops.push(
            Op::rect(20.0, 20.0, 200.0, 120.0)
                .fill(&pal.card)
                .stroked(Stroke::new(&pal.line, 2.0))
                .rounded(12.0),
        );
        scene
            .ops
            .push(Op::text(30.0, 30.0, "A card", 20.0, &pal.fg));
        let id = a.editor.canvas_show("test", "demo", scene);
        a.editor.show_buffer(id);
        finish(&mut a);
        let first = a.buf_ref().image.as_ref().unwrap().rendered.clone();
        for _ in 0..20 {
            assert!(execute(&mut a, "canvas-zoom-in"));
        }
        // The editor did not wait: at most a draw or two is behind it.
        finish(&mut a);
        assert!(a.editor.canvases.get("demo").is_some_and(|c| !c.dirty));
        let zoomed = a.buf_ref().image.as_ref().unwrap().rendered.clone();
        assert_ne!(zoomed, first);
        // Light mode: the same scene is drawn again, in other colours.
        forge_core::canvas::set_dark_mode(Some(false));
        sync(&mut a);
        assert!(a.editor.canvases.get("demo").is_some_and(|c| c.dirty) || rendering(&a));
        finish(&mut a);
        let light = a.buf_ref().image.as_ref().unwrap().rendered.clone();
        assert_ne!(light, zoomed, "a theme change draws the canvas again");
        let dark_bytes = std::fs::read(zoomed.unwrap()).ok();
        let light_bytes = std::fs::read(light.unwrap()).ok();
        forge_core::canvas::set_dark_mode(None);
        assert_ne!(dark_bytes, light_bytes);
    }
}
