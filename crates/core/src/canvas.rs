// SPDX-License-Identifier: MIT OR Apache-2.0
//! Canvases: scenes plugins draw and the editor shows.
//!
//! A canvas is a named [`Scene`] shown in a `canvas-mode` buffer. The scene
//! stays data: Forge keeps it, draws it (the editor crate renders it to a
//! picture the frontend places over the window, the way Image mode does),
//! and answers pointer events with what is under the pointer. A plugin only
//! has to say what the picture is and what to do when it is used.
//!
//! Each canvas also has a *view*: a pan and zoom applied to the scene when
//! it is drawn and undone when a pointer position is turned back into scene
//! coordinates, so an infinite whiteboard moves without the plugin sending
//! its scene again.

use std::collections::BTreeMap;

use forge_canvas::{Op, Scene, Transform};

use crate::BufferId;
use crate::app::Editor;
use crate::buffer::ImageState;

pub const MODE: &str = "canvas-mode";

/// The prefix of a canvas buffer's name: `*Canvas: <name>*`.
pub fn buffer_name(name: &str) -> String {
    format!("*Canvas: {name}*")
}

/// What part of the scene the frame shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The scene point at the frame's top-left corner.
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 20.0;

#[derive(Debug, Clone)]
pub struct Canvas {
    pub scene: Scene,
    pub view: View,
    pub buffer: BufferId,
    /// Which plugin made it, and the command it wants pointer events to run.
    pub owner: String,
    pub listener: Option<String>,
    /// The last pointer event, as JSON (see [`Editor::canvas_event`]).
    pub last_event: Option<String>,
    /// The plugin handles the wheel and middle-drag itself.
    pub handles_view: bool,
    /// A page that scrolls (Bases cards): the frame is as wide as the scene
    /// and as tall as the window needs, the view starts at the top, and
    /// panning stays on the page.
    pub scroll: bool,
    /// A scrolling page's visible width in scene units (a board wider than
    /// the window scrolls sideways); 0: the scene's own width.
    pub scroll_w: f32,
    /// The picture needs drawing again.
    pub dirty: bool,
    /// The frame's size when the window that shows the canvas is not the
    /// scene's shape: the smallest rectangle of the window's shape that
    /// holds the scene's own frame, so the picture fills the window and
    /// nothing is cut off at its edge. None: the scene's own size.
    pub frame: Option<(f32, f32)>,
    /// Counts redraws: part of the picture's file name.
    pub generation: u64,
}

impl Canvas {
    /// The frame's size in scene units (see [`Canvas::frame`]).
    pub fn size(&self) -> (f32, f32) {
        self.frame.unwrap_or((self.scene.width, self.scene.height))
    }

    /// How far the frame reaches beyond the scene's own frame on each
    /// side, in scene units at this zoom. The view a plugin sets is for
    /// the scene's own frame; the bigger one is centred on it.
    fn margin(&self) -> (f32, f32) {
        if self.scroll {
            return (0.0, 0.0);
        }
        let (w, h) = self.size();
        (
            (w - self.scene.width) / (2.0 * self.view.zoom),
            (h - self.scene.height) / (2.0 * self.view.zoom),
        )
    }

    /// Reshape the frame to `aspect` (width over height), around the
    /// scene's own frame. Returns whether anything changed.
    pub fn set_frame_aspect(&mut self, aspect: f32) -> bool {
        let (bw, bh) = (self.scene.width.max(1.0), self.scene.height.max(1.0));
        let bw = if self.scroll && self.scroll_w > 0.0 {
            self.scroll_w.min(bw)
        } else {
            bw
        };
        let aspect = aspect.clamp(0.2, 8.0);
        let new = if self.scroll {
            (bw, bw / aspect)
        } else if aspect >= bw / bh {
            (bh * aspect, bh)
        } else {
            (bw, bw / aspect)
        };
        let old = self.size();
        if (new.0 - old.0).abs() < 0.5 && (new.1 - old.1).abs() < 0.5 && self.frame.is_some() {
            return false;
        }
        self.frame = Some(new);
        self.clamp_view();
        self.dirty = true;
        true
    }

    /// Keep a scrolling page's view on the page.
    pub fn clamp_view(&mut self) {
        if !self.scroll {
            return;
        }
        let (w, h) = self.size();
        let z = self.view.zoom;
        self.view.x = self.view.x.clamp(0.0, (self.scene.width - w / z).max(0.0));
        self.view.y = self.view.y.clamp(0.0, (self.scene.height - h / z).max(0.0));
    }

    /// The scene as the frame shows it: the view applied.
    pub fn framed(&self) -> Scene {
        let v = self.view;
        let (mx, my) = self.margin();
        // Only what the frame can show is drawn (and cloned): a board of
        // hundreds of cards costs what is on screen, not what exists.
        let ((x0, y0), (x1, y1)) = (self.to_scene(0.0, 0.0), self.to_scene(1.0, 1.0));
        let pad = 32.0;
        let seen: Vec<Op> = self
            .scene
            .ops
            .iter()
            .filter(|op| {
                op.bounds().is_none_or(|[x, y, w, h]| {
                    x <= x1 + pad && x + w >= x0 - pad && y <= y1 + pad && y + h >= y0 - pad
                })
            })
            .cloned()
            .collect();
        let inner = Op::group(seen).transformed(Transform {
            x: -(v.x - mx) * v.zoom,
            y: -(v.y - my) * v.zoom,
            sx: v.zoom,
            sy: v.zoom,
            ..Default::default()
        });
        let (fw, fh) = self.size();
        let mut s = Scene::new(fw, fh);
        s.background = self.scene.background.clone();
        s.ops.push(inner);
        s
    }

    /// A point in the frame (0..1 across and down) as a scene point.
    pub fn to_scene(&self, fx: f32, fy: f32) -> (f32, f32) {
        let v = self.view;
        let (w, h) = self.size();
        let (mx, my) = self.margin();
        (fx * w / v.zoom + v.x - mx, fy * h / v.zoom + v.y - my)
    }

    pub fn zoom_about(&mut self, fx: f32, fy: f32, factor: f32) {
        let (sx, sy) = self.to_scene(fx, fy);
        let zoom = (self.view.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.view.zoom = zoom;
        // Keep the scene point under the pointer where it was.
        let (w, h) = self.size();
        self.view.x = sx - fx * w / zoom;
        self.view.y = sy - fy * h / zoom;
        let (mx, my) = self.margin();
        self.view.x += mx;
        self.view.y += my;
        self.clamp_view();
        self.dirty = true;
    }

    pub fn pan(&mut self, dx_frac: f32, dy_frac: f32) {
        let (w, h) = self.size();
        self.view.x += dx_frac * w / self.view.zoom;
        self.view.y += dy_frac * h / self.view.zoom;
        self.clamp_view();
        self.dirty = true;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Canvases {
    map: BTreeMap<String, Canvas>,
    /// The palette the canvases were last drawn in; a different one draws
    /// them all again.
    pub drawn_in: Option<forge_canvas::Palette>,
    /// Names this set of canvases to the drawing thread (0: not yet).
    pub token: u64,
}

impl Canvases {
    pub fn get(&self, name: &str) -> Option<&Canvas> {
        self.map.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Canvas> {
        self.map.get_mut(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Canvas)> {
        self.map.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut Canvas)> {
        self.map.iter_mut()
    }

    /// The canvas shown in `buffer`, if it is one.
    pub fn name_of(&self, buffer: BufferId) -> Option<&str> {
        self.map
            .iter()
            .find(|(_, c)| c.buffer == buffer)
            .map(|(n, _)| n.as_str())
    }

    pub fn remove_buffer(&mut self, buffer: BufferId) {
        self.map.retain(|_, c| c.buffer != buffer);
    }

    pub fn any_dirty(&self) -> bool {
        self.map.values().any(|c| c.dirty)
    }
}

/// A pointer event on a canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct Pointer {
    /// `press`, `release`, `drag`, `move`, `double` or `scroll`.
    pub kind: &'static str,
    /// 0 none, 1 left, 2 middle, 3 right.
    pub button: u8,
    /// Where in the frame, 0..1 across and down.
    pub fx: f32,
    pub fy: f32,
    /// Wheel travel: positive `dy` is away from the user (scroll up).
    pub dy: f32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Editor {
    /// Show `scene` in canvas `name` for `owner`, making the canvas and its
    /// buffer if they're new. Returns the buffer.
    pub fn canvas_show(&mut self, owner: &str, name: &str, scene: Scene) -> BufferId {
        if let Some(c) = self.canvases.get_mut(name)
            && self.state.buffers.contains_key(&c.buffer)
        {
            c.scene = scene;
            c.dirty = true;
            return c.buffer;
        }
        let id = self.add_buffer(crate::buffer::Buffer::new(buffer_name(name)));
        if let Some(b) = self.state.buffers.get_mut(&id) {
            b.set_major_mode(MODE);
            b.image = Some(ImageState {
                canvas: true,
                ..Default::default()
            });
            b.set_read_only(true);
        }
        self.canvases.map.insert(
            name.to_string(),
            Canvas {
                scene,
                view: View::default(),
                frame: None,
                buffer: id,
                owner: owner.to_string(),
                listener: None,
                last_event: None,
                handles_view: false,
                scroll: false,
                scroll_w: 0.0,
                dirty: true,
                generation: 0,
            },
        );
        id
    }

    /// Turn a pointer event into the canvas's event: scene coordinates and
    /// the shape under the pointer, stored as `last_event` (JSON). Returns
    /// the command to run, if the plugin asked for one.
    pub fn canvas_event(&mut self, name: &str, p: &Pointer) -> Option<String> {
        let c = self.canvases.get_mut(name)?;
        let (x, y) = c.to_scene(p.fx, p.fy);
        let id = c.scene.hit(x, y);
        let json = serde_json::json!({
            "kind": p.kind,
            "button": p.button,
            "x": x,
            "y": y,
            "frame_x": p.fx * c.size().0 - (c.size().0 - c.scene.width) / 2.0,
            "frame_y": p.fy * c.size().1 - (c.size().1 - c.scene.height) / 2.0,
            "dy": p.dy,
            "id": id,
            "shift": p.shift,
            "ctrl": p.ctrl,
            "alt": p.alt,
            "zoom": c.view.zoom,
        });
        c.last_event = Some(json.to_string());
        c.listener.clone()
    }
}

/// A scene as PNG bytes, drawn on the GPU when the `gpu` feature is on and
/// an adapter exists, otherwise by the CPU rasteriser.
pub fn render_png(scene: &Scene, scale: f32) -> Option<Vec<u8>> {
    render_png_waiting(scene, scale, &forge_canvas::Palette::dark()).0
}

static NATIVE_SCENES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The frontend draws scenes itself (CoreGraphics, Compose, cairo, Win2D):
/// canvases are handed over as `.scene.json` files, colours resolved and
/// web pictures swapped for their cached files, instead of as PNGs.
pub fn set_native_scenes(on: bool) {
    NATIVE_SCENES.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn native_scenes() -> bool {
    NATIVE_SCENES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The file-name ending of a scene handed to a frontend that draws.
pub const SCENE_SUFFIX: &str = ".scene.json";

/// The scene as JSON for a frontend that draws it natively, and whether a
/// picture from the web is still on its way.
pub fn scene_json_waiting(
    scene: &Scene,
    palette: &forge_canvas::Palette,
) -> (Option<Vec<u8>>, bool) {
    let colored = scene.resolved(palette);
    let (local, waiting) = localize(&colored);
    let scene = local.as_ref().unwrap_or(&colored);
    (
        serde_json::to_vec(&forge_canvas::display_list(scene)).ok(),
        waiting,
    )
}

/// [`render_png`], also saying whether a picture named by web address is
/// still on its way (so the scene should be drawn again soon).
pub fn render_png_waiting(
    scene: &Scene,
    scale: f32,
    palette: &forge_canvas::Palette,
) -> (Option<Vec<u8>>, bool) {
    let colored = scene.resolved(palette);
    let (local, waiting) = localize(&colored);
    let scene = local.as_ref().unwrap_or(&colored);
    #[cfg(feature = "gpu")]
    if let Some(png) = gpu::render_png(scene, scale) {
        return (Some(png), waiting);
    }
    (
        forge_canvas::raster::render(scene, scale).and_then(|p| p.png().ok()),
        waiting,
    )
}

fn is_web(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

fn has_web_pictures(ops: &[Op]) -> bool {
    ops.iter().any(|op| match op {
        Op::Image { path, .. } => is_web(path),
        Op::Group { ops, .. } => has_web_pictures(ops),
        _ => false,
    })
}

/// Pictures named by web address are swapped for their cached files; one not
/// here yet is left out of the picture (the box behind it shows) and the
/// flag says to try again. None: nothing to swap, draw the scene as it is.
fn localize(scene: &Scene) -> (Option<Scene>, bool) {
    if !has_web_pictures(&scene.ops) {
        return (None, false);
    }
    fn walk(ops: &mut Vec<Op>, waiting: &mut bool) {
        ops.retain_mut(|op| match op {
            Op::Image { path, .. } if is_web(path) => {
                match crate::org_display::remote_picture(path) {
                    crate::org_display::Remote::Ready(file) => {
                        *path = file.display().to_string();
                        true
                    }
                    crate::org_display::Remote::Pending => {
                        *waiting = true;
                        false
                    }
                    crate::org_display::Remote::Missing => false,
                }
            }
            Op::Group { ops, .. } => {
                walk(ops, waiting);
                true
            }
            _ => true,
        });
    }
    let mut copy = scene.clone();
    let mut waiting = false;
    walk(&mut copy.ops, &mut waiting);
    (Some(copy), waiting)
}

/// What draws canvases right now: the adapter's name, or "CPU".
pub fn renderer_name() -> String {
    #[cfg(feature = "gpu")]
    if let Some(name) = gpu::name() {
        return format!("GPU: {name}");
    }
    #[cfg(feature = "gpu")]
    return "CPU (no usable graphics adapter, or FORGE_RENDERER=cpu)".into();
    #[cfg(not(feature = "gpu"))]
    "CPU (built without the gpu feature)".into()
}

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::Mutex;

    use forge_canvas::Scene;
    use forge_canvas_gpu::Renderer;

    /// None until the first use; then the renderer, or None for good when
    /// there is no adapter.
    static RENDERER: Mutex<Option<Option<Renderer>>> = Mutex::new(None);

    fn with<T>(f: impl FnOnce(&mut Renderer) -> T) -> Option<T> {
        let mut slot = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let r = slot.get_or_insert_with(|| {
            if std::env::var("FORGE_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("cpu")) {
                return None;
            }
            Renderer::headless().ok()
        });
        r.as_mut().map(f)
    }

    pub fn render_png(scene: &Scene, scale: f32) -> Option<Vec<u8>> {
        with(|r| r.render_png(scene, scale)).and_then(Result::ok)
    }

    pub fn name() -> Option<String> {
        with(|r| r.adapter.clone())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_native_scene_is_json_with_its_colours_resolved() {
        let mut scene = Scene::new(40.0, 20.0);
        scene.background = Some("@bg".into());
        scene.ops.push(Op::rect(1.0, 2.0, 3.0, 4.0).fill("@card"));
        let (bytes, waiting) = scene_json_waiting(&scene, &forge_canvas::Palette::light());
        assert!(!waiting);
        let text = String::from_utf8(bytes.unwrap()).unwrap();
        assert!(!text.contains('@'), "roles are resolved: {text}");
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["ops"].as_array().unwrap().len(), 1);
        assert!(
            back["bg"]
                .as_str()
                .is_some_and(|c| c.starts_with('#') && c.len() == 9)
        );
    }

    use super::*;

    fn scene() -> Scene {
        let mut s = Scene::new(200.0, 100.0);
        s.push(Op::rect(10.0, 10.0, 50.0, 30.0).fill("#000").with_id("box"));
        s
    }

    #[test]
    fn showing_a_canvas_makes_a_picture_buffer() {
        let mut ed = Editor::new();
        let id = ed.canvas_show("me", "board", scene());
        let b = ed.state.get_buffer(id).unwrap();
        assert_eq!(b.name(), "*Canvas: board*");
        assert_eq!(b.major_mode(), MODE);
        assert!(b.image.as_ref().unwrap().canvas && b.read_only());
        assert!(ed.canvases.get("board").unwrap().dirty);
        // Showing again reuses the buffer and marks it for redrawing.
        ed.canvases.get_mut("board").unwrap().dirty = false;
        assert_eq!(ed.canvas_show("me", "board", scene()), id);
        assert!(ed.canvases.get("board").unwrap().dirty);
        assert_eq!(ed.canvases.name_of(id), Some("board"));
        assert_eq!(ed.canvases.names().collect::<Vec<_>>(), ["board"]);
    }

    #[test]
    fn the_view_moves_the_picture_and_events_undo_it() {
        let mut ed = Editor::new();
        ed.canvas_show("me", "board", scene());
        let c = ed.canvases.get_mut("board").unwrap();
        c.view = View {
            x: 100.0,
            y: 0.0,
            zoom: 2.0,
        };
        // The frame's centre is scene (100 + 100/2, 0 + 100/2/1)...
        assert_eq!(c.to_scene(0.5, 0.5), (150.0, 25.0));
        let framed = c.framed();
        assert_eq!(framed.width, 200.0);
        let p = Pointer {
            kind: "press",
            button: 1,
            fx: 0.0,
            fy: 0.0,
            dy: 0.0,
            shift: false,
            ctrl: false,
            alt: true,
        };
        c.listener = Some("my-command".into());
        // ...so the top-left of the frame is scene (100, 0): outside the box.
        assert_eq!(ed.canvas_event("board", &p).as_deref(), Some("my-command"));
        let ev: serde_json::Value = serde_json::from_str(
            ed.canvases
                .get("board")
                .unwrap()
                .last_event
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ev["x"], 100.0);
        assert!(ev["id"].is_null() && ev["alt"] == true && ev["zoom"] == 2.0);
        // At the default view, the box is hit where it is.
        ed.canvases.get_mut("board").unwrap().view = View::default();
        ed.canvas_event(
            "board",
            &Pointer {
                fx: 30.0 / 200.0,
                fy: 20.0 / 100.0,
                ..p
            },
        );
        let ev: serde_json::Value = serde_json::from_str(
            ed.canvases
                .get("board")
                .unwrap()
                .last_event
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ev["id"], "box");
        assert!(ed.canvas_event("nope", &p).is_none());
    }

    #[test]
    fn framing_draws_the_scene_under_the_view() {
        let mut ed = Editor::new();
        ed.canvas_show("me", "board", scene());
        let c = ed.canvases.get_mut("board").unwrap();
        c.view = View {
            x: 10.0,
            y: 10.0,
            zoom: 2.0,
        };
        let f = c.framed();
        // The box's corner (10, 10) is now the frame's origin.
        assert_eq!(f.bounds(), Some([0.0, 0.0, 100.0, 60.0]));
        c.zoom_about(0.5, 0.5, 2.0);
        assert_eq!(c.view.zoom, 4.0);
        c.pan(0.5, 0.0);
        assert!(c.dirty);
        c.view.zoom = 1.0;
        c.zoom_about(0.0, 0.0, 0.0001);
        assert_eq!(c.view.zoom, MIN_ZOOM);
    }
}

#[cfg(test)]
mod frame_shape_tests {
    use super::*;

    fn canvas(w: f32, h: f32) -> Canvas {
        Canvas {
            scene: Scene::new(w, h),
            view: View::default(),
            buffer: BufferId(0),
            owner: String::new(),
            listener: None,
            last_event: None,
            handles_view: false,
            scroll: false,
            scroll_w: 0.0,
            dirty: false,
            frame: None,
            generation: 0,
        }
    }

    #[test]
    fn a_scrolling_page_fits_its_width_and_stays_on_the_page() {
        let mut c = canvas(800.0, 6000.0);
        c.scroll = true;
        // A wide window: the frame is as wide as the page, not as tall.
        assert!(c.set_frame_aspect(2.0));
        assert_eq!(c.size(), (800.0, 400.0));
        assert_eq!(c.to_scene(0.0, 0.0), (0.0, 0.0));
        // The wheel moves down a screenful at a time and stops at the end.
        c.pan(0.0, 1.0);
        assert_eq!(c.view.y, 400.0);
        c.pan(0.0, 100.0);
        assert_eq!(c.view.y, 5600.0);
        c.pan(0.0, -1000.0);
        assert_eq!(c.view.y, 0.0);
    }

    #[test]
    fn only_what_the_frame_shows_is_drawn() {
        let mut c = canvas(1200.0, 100_000.0);
        c.scroll = true;
        for i in 0..2000 {
            let y = i as f32 * 50.0;
            c.scene.ops.push(
                Op::group(vec![
                    Op::rect(0.0, 0.0, 200.0, 40.0),
                    Op::text(8.0, 8.0, "card", 14.0, "#fff"),
                ])
                .transformed(Transform::translate(10.0, y)),
            );
        }
        c.set_frame_aspect(1.5);
        let shown = |c: &Canvas| match &c.framed().ops[0] {
            Op::Group { ops, .. } => ops.len(),
            _ => 0,
        };
        let top = shown(&c);
        assert!(top > 5 && top < 40, "{top} of 2000 at the top");
        c.view.y = 50_000.0;
        let mid = shown(&c);
        assert!(mid > 5 && mid < 40, "{mid} of 2000 in the middle");
        // And it is quick: a page of this size draws in well under a second.
        let t = std::time::Instant::now();
        assert!(render_png(&c.framed(), 2.0).is_some());
        assert!(t.elapsed().as_millis() < 10_000, "{:?}", t.elapsed());
    }

    #[test]
    fn a_page_wider_than_the_window_scrolls_sideways() {
        let mut c = canvas(1200.0, 3000.0);
        c.scroll = true;
        c.scroll_w = 400.0;
        c.set_frame_aspect(0.5);
        assert_eq!(c.size(), (400.0, 800.0));
        c.pan(1.0, 0.0);
        assert_eq!(c.view.x, 400.0);
        c.pan(10.0, 0.0);
        assert_eq!(c.view.x, 800.0, "stops at the right edge");
    }

    #[test]
    fn a_wide_window_widens_the_frame_around_the_scene_frame() {
        let mut c = canvas(1200.0, 800.0);
        assert!(c.set_frame_aspect(2.0));
        assert_eq!(c.size(), (1600.0, 800.0));
        // The centre of the scene's own frame stays the centre.
        let (x, y) = c.to_scene(0.5, 0.5);
        assert!((x - 600.0).abs() < 0.01 && (y - 400.0).abs() < 0.01);
        // The left edge reaches 200 units past the scene frame's.
        assert!((c.to_scene(0.0, 0.0).0 + 200.0).abs() < 0.01);
        assert!(!c.set_frame_aspect(2.0));
    }

    #[test]
    fn a_tall_window_never_cuts_the_scene_frame() {
        let mut c = canvas(1200.0, 800.0);
        c.set_frame_aspect(0.5);
        assert_eq!(c.size(), (1200.0, 2400.0));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let mut c = canvas(1200.0, 800.0);
        c.set_frame_aspect(2.0);
        let before = c.to_scene(0.25, 0.75);
        c.zoom_about(0.25, 0.75, 2.0);
        let after = c.to_scene(0.25, 0.75);
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
    }
}

// ------------------------------------------------------------------ colours

static DARK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The frontend says whether the system is in dark mode (None: it doesn't
/// know, and Forge assumes dark, as a terminal usually is).
pub fn set_dark_mode(dark: Option<bool>) {
    let v = match dark {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    DARK.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn dark_mode() -> Option<bool> {
    match DARK.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

impl crate::app::Editor {
    /// The colours a canvas is drawn in, from the user's theme: when it
    /// restyles the default face, the page and text colours come from that
    /// (dark or light by how bright the page is); otherwise from the
    /// system's dark or light mode. The theme's code colours become the
    /// accents that tell folders and lanes apart.
    pub fn canvas_palette(&self) -> forge_canvas::Palette {
        use forge_canvas::{Color, Palette};
        let color = |face: &str, bg: bool| {
            self.theme
                .get(face)
                .and_then(|s| if bg { s.bg } else { s.fg })
                .map(|c| Color::new(c.0, c.1, c.2))
        };
        let dark = dark_mode().unwrap_or(true);
        let base = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        let accents: Vec<Option<Color>> = [
            "function",
            "type",
            "string",
            "error",
            "keyword",
            "constant",
            "builtin",
            "attribute",
        ]
        .iter()
        .map(|f| color(f, false))
        .collect();
        let mut p = match (color("default", true), color("default", false)) {
            (None, None) => base.with_accents(&accents),
            (bg, fg) => {
                let parse = |h: &str| Color::parse(h).unwrap_or(Color::BLACK);
                Palette::derive(
                    bg.unwrap_or_else(|| parse(&base.bg)),
                    fg.unwrap_or_else(|| parse(&base.fg)),
                    &accents,
                )
            }
        };
        if let Some(link) = color("link", false) {
            p.tag = link.hex_rgb();
        }
        p
    }
}
