// SPDX-License-Identifier: MIT OR Apache-2.0
//! Image mode, the "Viewing Image Files" manual node: visiting an image
//! file selects Image mode, which toggles between the picture and the
//! file's raw text or hex, transforms the picture (scale, rotate, flip,
//! save) with the `image` crate, steps through an animation, and visits
//! the next or previous image file.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::buffer::{ImageDisplay, ImageState, ImageTransform};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A question Image mode asks.
pub enum Ask {
    /// image-transform-set-percent: a percentage.
    Percent { default: f64 },
    /// image-transform-set-scale: a scale factor.
    Scale { default: f64 },
    /// image-goto-frame: a 1-based frame number.
    Frame { max: usize },
    /// image-save: where to write the picture.
    SavePath { default: String },
}

/// The mode's name, as the registry knows it.
pub const MODE: &str = forge_core::buffer::IMAGE_MODE;

/// The buffer's Image-mode state, if the buffer is one.
pub fn state(app: &mut App) -> Option<&mut ImageState> {
    if app.buf_ref().major_mode() != MODE {
        return None;
    }
    app.buf().image.as_mut()
}

/// Is `path` an image file? (by extension, like Forge's
/// `image-file-name-extensions`).
pub fn is_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    forge_core::org_display::DEFAULT_IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Every image file in `dir`, sorted by name.
pub fn image_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    out.sort();
    out
}

fn temp_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("forge-image-{}-{n}.png", std::process::id()))
}

fn is_temp(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("forge-image-"))
}

/// The decoded frames of the animation last shown: decoding a GIF takes
/// every frame, so stepping through one decodes it once, not once a step.
struct FrameCache {
    path: PathBuf,
    stamp: Option<std::time::SystemTime>,
    frames: Vec<image::DynamicImage>,
}

static FRAMES: std::sync::Mutex<Option<FrameCache>> = std::sync::Mutex::new(None);

fn stamp_of(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn is_gif(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
}

/// Every frame of a GIF, cached; empty for anything else.
fn gif_frames(path: &Path) -> Vec<image::DynamicImage> {
    use image::AnimationDecoder;
    if !is_gif(path) {
        return Vec::new();
    }
    let stamp = stamp_of(path);
    if let Ok(g) = FRAMES.lock()
        && let Some(c) = g.as_ref()
        && c.path == path
        && c.stamp == stamp
    {
        return c.frames.clone();
    }
    let frames: Vec<image::DynamicImage> = std::fs::File::open(path)
        .ok()
        .and_then(|f| image::codecs::gif::GifDecoder::new(std::io::BufReader::new(f)).ok())
        .and_then(|d| d.into_frames().collect_frames().ok())
        .map(|fs| fs.into_iter().map(|f| f.into_buffer().into()).collect())
        .unwrap_or_default();
    if let Ok(mut g) = FRAMES.lock() {
        *g = Some(FrameCache {
            path: path.to_path_buf(),
            stamp,
            frames: frames.clone(),
        });
    }
    frames
}

/// Decode a still picture, turned upright by its EXIF orientation.
fn open_still(path: &Path) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Some(img)
}

/// An SVG rasterised by whichever converter is installed.
fn open_svg(path: &Path) -> Option<image::DynamicImage> {
    let out = temp_path();
    let attempts: [(&str, Vec<String>); 3] = [
        (
            "rsvg-convert",
            vec![
                "-o".into(),
                out.display().to_string(),
                path.display().to_string(),
            ],
        ),
        (
            "magick",
            vec![
                "-background".into(),
                "none".into(),
                "-density".into(),
                "192".into(),
                path.display().to_string(),
                out.display().to_string(),
            ],
        ),
        (
            "inkscape",
            vec![
                path.display().to_string(),
                "--export-type=png".into(),
                format!("--export-filename={}", out.display()),
            ],
        ),
    ];
    for (prog, args) in attempts {
        let ok = std::process::Command::new(prog)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok && let Ok(img) = image::open(&out) {
            let _ = std::fs::remove_file(&out);
            return Some(img);
        }
    }
    let _ = std::fs::remove_file(&out);
    None
}

fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// A frame of an animated picture, or the whole picture.
fn open_frame(path: &Path, index: usize) -> Option<image::DynamicImage> {
    if is_svg(path) {
        return open_svg(path);
    }
    let frames = gif_frames(path);
    if !frames.is_empty() {
        return frames.into_iter().nth(index);
    }
    open_still(path).or_else(|| image::open(path).ok())
}

/// How many frames an animated picture has (1 for a still one).
fn frame_count(path: &Path) -> usize {
    gif_frames(path).len().max(1)
}

/// The natural size of the picture.
fn size_of(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap_or((0, 0))
}

/// Forget the render a buffer left in the temp directory.
pub fn forget_render(rendered: Option<&Path>) {
    if let Some(p) = rendered.filter(|p| is_temp(p)) {
        let _ = std::fs::remove_file(p);
    }
}

/// Apply a transform: scale, rotate, flip.
fn apply(img: image::DynamicImage, t: &ImageTransform) -> image::DynamicImage {
    let mut img = img;
    if (t.percent - 100.0).abs() > f64::EPSILON {
        let s = t.percent / 100.0;
        let nw = ((img.width() as f64 * s).round() as u32).max(1);
        let nh = ((img.height() as f64 * s).round() as u32).max(1);
        img = img.resize(nw, nh, image::imageops::FilterType::Triangle);
    }
    img = match t.rotation % 360 {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    };
    if t.flip_h {
        img = img.fliph();
    }
    if t.flip_v {
        img = img.flipv();
    }
    img
}

/// Render the picture (with its transform and frame) to a temp PNG.
fn render(path: &Path, st: &ImageState) -> Option<PathBuf> {
    let Some(img) = open_frame(path, st.frame) else {
        // An SVG with no converter installed: the macOS app draws it
        // natively, so hand it the file untransformed.
        return is_svg(path).then(|| path.to_path_buf());
    };
    let img = apply(img, &st.transform);
    let tmp = temp_path();
    img.save(&tmp).ok()?;
    Some(tmp)
}

/// Re-render the picture and remember the file to draw. The frontend
/// fits the picture to the window (it never upscales).
pub fn refresh(app: &mut App) {
    let Some(st) = app.buf_ref().image.clone() else {
        return;
    };
    let Some(path) = app.buf_ref().path().map(|p| p.to_path_buf()) else {
        return;
    };
    let old = st.rendered.clone();
    let result = render(&path, &st);
    let count = frame_count(&path).max(1);
    if let Some(s) = state(app) {
        s.rendered = result;
        s.frame_count = count;
        s.size = size_of(&path);
        if count > 1 {
            s.frame = s.frame.min(count - 1);
        }
    }
    if let Some(o) = old.filter(|o| is_temp(o)) {
        let _ = std::fs::remove_file(o);
    }
}

/// How long until an animating picture's next frame, or None.
pub fn animation_delay(app: &App) -> Option<Duration> {
    let st = app.buf_ref().image.as_ref()?;
    if !st.animating || st.speed <= 0.0 || st.frame_count <= 1 {
        return None;
    }
    let ms = (100.0 / st.speed).clamp(10.0, 10_000.0);
    Some(Duration::from_millis(ms as u64))
}

/// Advance an animating picture by one frame.
pub fn on_idle(app: &mut App) {
    let Some(st) = app.buf_ref().image.clone() else {
        return;
    };
    if !st.animating || st.frame_count <= 1 {
        return;
    }
    let next = if st.frame + 1 < st.frame_count {
        st.frame + 1
    } else if st.looping {
        0
    } else {
        if let Some(s) = state(app) {
            s.animating = false;
        }
        return;
    };
    if let Some(s) = state(app) {
        s.frame = next;
    }
    refresh(app);
}

fn set_display(app: &mut App, d: ImageDisplay) {
    let prev = state(app).map(|s| s.display);
    if d == ImageDisplay::Hex && prev != Some(ImageDisplay::Hex) {
        // Remember the raw text and show the file's bytes as hex.
        let raw = app.buf_ref().get_contents();
        let hex: String = match app.buf_ref().path() {
            Some(p) => std::fs::read(p)
                .unwrap_or_default()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            None => String::new(),
        };
        let len = app.buf_ref().char_count();
        if let Some(s) = state(app) {
            s.raw = Some(raw);
        }
        app.buf().replace_offsets(0, len, &hex);
    } else if prev == Some(ImageDisplay::Hex) {
        let raw = state(app).and_then(|s| s.raw.clone());
        if let Some(raw) = raw {
            let len = app.buf_ref().char_count();
            app.buf().replace_offsets(0, len, &raw);
        }
    }
    if let Some(s) = state(app) {
        s.display = d;
    }
    refresh(app);
}

/// C-c C-c: toggle between the picture and its raw text.
pub fn toggle_display(app: &mut App) {
    match state(app).map(|s| s.display) {
        Some(ImageDisplay::Image) => set_display(app, ImageDisplay::Text),
        Some(ImageDisplay::Hex) => set_display(app, ImageDisplay::Text),
        Some(ImageDisplay::Text) => set_display(app, ImageDisplay::Image),
        None => app.error("Not in Image mode"),
    }
}

/// C-c C-x: toggle between the picture and its hex.
pub fn toggle_hex_display(app: &mut App) {
    match state(app).map(|s| s.display) {
        Some(ImageDisplay::Image) => set_display(app, ImageDisplay::Hex),
        Some(ImageDisplay::Hex) => set_display(app, ImageDisplay::Image),
        Some(ImageDisplay::Text) => set_display(app, ImageDisplay::Hex),
        None => app.error("Not in Image mode"),
    }
}

fn reset(app: &mut App) {
    if let Some(s) = state(app) {
        s.transform = ImageTransform::default();
    }
    refresh(app);
}

/// s w: fit the picture to the window (the frontend already fits, so
/// this clears any manual transform).
pub fn fit_to_window(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    reset(app);
    app.msg("Fitted to window");
}

/// s p: scale to a percentage of the original.
pub fn set_percent(app: &mut App, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if app.has_arg() {
        let pct = n as f64;
        if pct <= 0.0 {
            return app.error("Percentage must be positive");
        }
        if let Some(s) = state(app) {
            s.transform.percent = pct;
        }
        refresh(app);
        app.msg(format!("Scaled to {pct}%"));
    } else {
        let default = state(app).map(|s| s.transform.percent).unwrap_or(100.0);
        app.prompt("Percentage: ", Purpose::ImageAsk(Ask::Percent { default }));
    }
}

/// s s: scale by a factor.
pub fn set_scale(app: &mut App, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if app.has_arg() {
        let f = n as f64;
        if f <= 0.0 {
            return app.error("Scale must be positive");
        }
        if let Some(s) = state(app) {
            s.transform.percent = f * 100.0;
        }
        refresh(app);
        app.msg(format!("Scaled by {f}"));
    } else {
        let default = state(app)
            .map(|s| s.transform.percent / 100.0)
            .unwrap_or(1.0);
        app.prompt("Scale: ", Purpose::ImageAsk(Ask::Scale { default }));
    }
}

/// s 0 / s o: reset the transform.
pub fn reset_transform(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    reset(app);
    app.msg("Transform reset");
}

/// i r: rotate 90 degrees clockwise (counter-clockwise with a negative
/// prefix argument).
pub fn rotate(app: &mut App, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let ccw = app.has_arg() && n < 0;
    if let Some(s) = state(app) {
        let delta = if ccw { 270 } else { 90 };
        s.transform.rotation = (s.transform.rotation + delta) % 360;
    }
    refresh(app);
}

/// i h: flip horizontally.
pub fn flip_horizontally(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if let Some(s) = state(app) {
        s.transform.flip_h = !s.transform.flip_h;
    }
    refresh(app);
}

/// i v: flip vertically.
pub fn flip_vertically(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if let Some(s) = state(app) {
        s.transform.flip_v = !s.transform.flip_v;
    }
    refresh(app);
}

/// i + / i -: increase or decrease the size. The prefix N multiplies by
/// 1 +/- N/10; without one it is 2 (20%).
pub fn adjust_size(app: &mut App, sign: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let inc = app.count().unsigned_abs().max(2) as f64;
    let factor = 1.0 + sign as f64 * inc / 10.0;
    if factor <= 0.0 {
        return app.error("Size must stay positive");
    }
    if let Some(s) = state(app) {
        s.transform.percent = (s.transform.percent * factor).clamp(1.0, 100_000.0);
    }
    refresh(app);
}

/// i o: save the picture to a file.
pub fn save(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let default = app
        .buf_ref()
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    app.prompt(
        "Save image as: ",
        Purpose::ImageAsk(Ask::SavePath { default }),
    );
}

fn answer_save(app: &mut App, input: &str) {
    let path = PathBuf::from(input);
    let Some(st) = app.buf_ref().image.clone() else {
        return;
    };
    let Some(src) = st
        .rendered
        .or_else(|| app.buf_ref().path().map(|p| p.to_path_buf()))
    else {
        return app.error("No image to save");
    };
    match std::fs::copy(&src, &path) {
        Ok(_) => app.msg(format!("Saved {}", path.display())),
        Err(e) => app.error(format!("Cannot save {}: {e}", path.display())),
    }
}

/// n / p: visit the next or previous image file in the directory.
pub fn next_file(app: &mut App, backward: bool) {
    let Some(path) = app.buf_ref().path().map(|p| p.to_path_buf()) else {
        return app.error("Not visiting a file");
    };
    let Some(dir) = path.parent().map(Path::to_path_buf) else {
        return app.error("No directory");
    };
    let files = image_files(&dir);
    if files.is_empty() {
        return app.error("No image files in this directory");
    }
    let n = files.len();
    let idx = match files.iter().position(|p| p == &path) {
        Some(i) if backward => (i + n - 1) % n,
        Some(i) => (i + 1) % n,
        None => 0,
    };
    let target = files[idx].clone();
    crate::minibuffer::visit_file(app, &target, false);
    refresh(app);
}

/// m / u: mark or unmark the file in its Dired buffer.
pub fn mark_file(app: &mut App, mark: char) {
    let Some(path) = app.buf_ref().path().map(|p| p.to_path_buf()) else {
        return app.error("Not visiting a file");
    };
    if crate::dired::mark_file(app, &path, mark) {
        app.msg(if mark == '*' { "Marked" } else { "Unmarked" });
    } else {
        app.error("No Dired buffer for this file's directory");
    }
}

/// w: copy the file name to the kill ring.
pub fn copy_file_name_as_kill(app: &mut App) {
    let Some(path) = app.buf_ref().path().map(|p| p.to_path_buf()) else {
        return app.error("Not visiting a file");
    };
    let name = path.display().to_string();
    app.editor.kill_ring.push(name.clone());
    app.msg(format!("Copied {name}"));
}

/// RET: start or stop the animation.
pub fn toggle_animation(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let Some(st) = state(app) else {
        return;
    };
    if st.frame_count <= 1 {
        return app.error("This image is not animated");
    }
    let playing = !st.animating;
    let looping = app.image_animate_loop;
    if let Some(s) = state(app) {
        s.animating = playing;
        s.looping = looping;
    }
    app.msg(if playing { "Playing" } else { "Paused" });
}

/// f / b: step a frame (a numeric argument steps several).
pub fn step_frame(app: &mut App, forward: bool, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let count = state(app).map(|s| s.frame_count).unwrap_or(1);
    if count <= 1 {
        return app.error("This image is not animated");
    }
    let steps = n.unsigned_abs().max(1) as usize;
    let cur = state(app).map(|s| s.frame).unwrap_or(0);
    let next = if forward {
        (cur + steps) % count
    } else {
        (cur + count - steps % count) % count
    };
    if let Some(s) = state(app) {
        s.frame = next;
    }
    refresh(app);
}

/// F: go to a 1-based frame.
pub fn goto_frame(app: &mut App, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let count = state(app).map(|s| s.frame_count).unwrap_or(1);
    if count <= 1 {
        return app.error("This image is not animated");
    }
    if !app.has_arg() {
        app.prompt("Frame: ", Purpose::ImageAsk(Ask::Frame { max: count }));
        return;
    }
    let idx = (n.max(1) as usize).min(count);
    if let Some(s) = state(app) {
        s.frame = idx - 1;
    }
    refresh(app);
}

/// a + / a - / a r / a 0: change the animation speed.
pub fn adjust_speed(app: &mut App, n: i64) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    let cur = state(app).map(|s| s.speed).unwrap_or(1.0);
    let factor = 1.0 + n as f64 / 10.0;
    let speed = (cur * factor).clamp(0.1, 20.0);
    if let Some(s) = state(app) {
        s.speed = speed;
    }
    app.msg(format!("Speed {speed:.1}"));
}

pub fn reverse_speed(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if let Some(s) = state(app) {
        s.speed = -s.speed;
    }
}

pub fn reset_speed(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in Image mode");
    }
    if let Some(s) = state(app) {
        s.speed = 1.0;
    }
    app.msg("Speed reset");
}

/// Run an Image-mode command by name.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    let n = app.count();
    match cmd {
        "image-toggle-display" => toggle_display(app),
        "image-toggle-hex-display" => toggle_hex_display(app),
        "image-transform-fit-to-window" => fit_to_window(app),
        "image-transform-set-percent" => set_percent(app, n),
        "image-transform-set-scale" => set_scale(app, n),
        "image-transform-reset-to-initial" | "image-transform-reset-to-original" => {
            reset_transform(app)
        }
        "image-rotate" => rotate(app, n),
        "image-flip-horizontally" => flip_horizontally(app),
        "image-flip-vertically" => flip_vertically(app),
        "image-increase-size" => adjust_size(app, 1),
        "image-decrease-size" => adjust_size(app, -1),
        "image-save" => save(app),
        "image-next-file" => next_file(app, false),
        "image-previous-file" => next_file(app, true),
        "image-mode-mark-file" => mark_file(app, '*'),
        "image-mode-unmark-file" => mark_file(app, ' '),
        "image-mode-copy-file-name-as-kill" => copy_file_name_as_kill(app),
        "image-toggle-animation" => toggle_animation(app),
        "image-next-frame" => step_frame(app, true, n),
        "image-previous-frame" => step_frame(app, false, n),
        "image-goto-frame" => goto_frame(app, n),
        "image-increase-speed" => adjust_speed(app, n),
        "image-decrease-speed" => adjust_speed(app, -n),
        "image-reverse-speed" => reverse_speed(app),
        "image-reset-speed" => reset_speed(app),
        _ => return false,
    }
    true
}

/// The minibuffer's answer to an `Ask` question.
pub fn answer(app: &mut App, ask: Ask, input: &str) {
    match ask {
        Ask::Percent { .. } => match input.trim().parse::<f64>() {
            Ok(pct) if pct > 0.0 => {
                if let Some(s) = state(app) {
                    s.transform.percent = pct;
                }
                refresh(app);
                app.msg(format!("Scaled to {pct}%"));
            }
            _ => app.error("Percentage must be a positive number"),
        },
        Ask::Scale { .. } => match input.trim().parse::<f64>() {
            Ok(f) if f > 0.0 => {
                if let Some(s) = state(app) {
                    s.transform.percent = f * 100.0;
                }
                refresh(app);
                app.msg(format!("Scaled by {f}"));
            }
            _ => app.error("Scale must be a positive number"),
        },
        Ask::Frame { .. } => match input.trim().parse::<usize>() {
            Ok(i) if i >= 1 => {
                let count = state(app).map(|s| s.frame_count).unwrap_or(1);
                if let Some(s) = state(app) {
                    s.frame = (i - 1).min(count - 1);
                }
                refresh(app);
            }
            _ => app.error("Frame must be a positive number"),
        },
        Ask::SavePath { .. } => answer_save(app, input),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_formats_decode_and_gifs_step_without_redecoding() {
        let dir = std::env::temp_dir().join(format!("forge-img-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([1, 2, 3, 255]));
        for ext in ["bmp", "tiff", "ico", "tga", "qoi"] {
            let p = dir.join(format!("a.{ext}"));
            let ok = if ext == "ico" {
                image::DynamicImage::from(image::RgbaImage::from_pixel(
                    8,
                    8,
                    image::Rgba([1, 2, 3, 255]),
                ))
                .save(&p)
            } else {
                img.save(&p)
            };
            assert!(ok.is_ok(), "{ext}");
            assert!(open_frame(&p, 0).is_some(), "{ext} decodes");
        }
        // A two-frame GIF: both frames come from one decode.
        let p = dir.join("anim.gif");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(f);
            for c in [10u8, 200] {
                enc.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    4,
                    4,
                    image::Rgba([c, 0, 0, 255]),
                )))
                .unwrap();
            }
        }
        assert_eq!(frame_count(&p), 2);
        let a = open_frame(&p, 0).unwrap().to_rgba8();
        let b = open_frame(&p, 1).unwrap().to_rgba8();
        assert_ne!(a.get_pixel(0, 0), b.get_pixel(0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
