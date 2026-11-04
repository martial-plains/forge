// SPDX-License-Identifier: MIT OR Apache-2.0
//! Inline pictures for terminals that draw them (the Kitty graphics
//! protocol: kitty, Ghostty, WezTerm). Org previews and the text
//! browser's cache draw over the blank rows the display reserves.
//!
//! The terminal UI redraws every frame, so placements sync every frame
//! too: anything whose picture, size or place changed is deleted and
//! placed again, and an unchanged screen sends nothing. Transmitted
//! pictures are cached by file, so scrolling only moves placements.
//! Terminals without graphics keep the `[image]` placeholders.

use crate::caps::GraphicsProto;
use std::collections::HashMap;
use std::io::Write;

/// One picture slot from the UI, in screen cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub path: String,
    /// The image's display properties (scale, rotation, slice, ...).
    pub descriptor: Option<forge_core::display::ImageDescriptor>,
}

/// A cell's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPx {
    pub w: u32,
    pub h: u32,
}

/// When the terminal won't say (no `TIOCGWINSZ` pixels): the usual cell,
/// half as wide as tall.
pub const FALLBACK_CELL: CellPx = CellPx { w: 1, h: 2 };

/// Fit (columns, rows) for an `iw`x`ih` picture in a `w`x`h`-cell slot,
/// keeping the aspect. Pictures never scale up: a small picture sits at
/// its natural size in the slot's corner.
pub fn fit_slot(iw: u32, ih: u32, w: u16, h: u16, cell: CellPx) -> (u16, u16) {
    let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
    let (cw, ch) = (cell.w.max(1) as f64, cell.h.max(1) as f64);
    let scale = ((w as f64 * cw) / iw).min((h as f64 * ch) / ih).min(1.0);
    let dw = iw * scale;
    let dh = ih * scale;
    let c = (dw / cw).ceil() as u16;
    let r = (dh / ch).ceil() as u16;
    (c.clamp(1, w.max(1)), r.clamp(1, h.max(1)))
}

/// Pixels to transmit for a `c`x`r`-cell placement: the placement's
/// pixels, capped so a fullscreen picture isn't a huge transfer.
pub fn fit_pixels(c: u16, r: u16, cell: CellPx) -> (u32, u32) {
    const MAX_SIDE: u32 = 1600;
    let (mut w, mut h) = ((c as u32) * cell.w.max(1), (r as u32) * cell.h.max(1));
    w = w.max(1);
    h = h.max(1);
    let m = w.max(h);
    if m > MAX_SIDE {
        w = (w * MAX_SIDE / m).max(1);
        h = (h * MAX_SIDE / m).max(1);
    }
    (w, h)
}

/// This process's cell size from the terminal, if it answers.
pub fn cell_size() -> Option<CellPx> {
    let (cols, rows, xp, yp) = winsize()?;
    if cols == 0 || rows == 0 || xp == 0 || yp == 0 {
        return None;
    }
    Some(CellPx {
        w: (xp as u32) / (cols as u32).max(1),
        h: (yp as u32) / (rows as u32).max(1),
    })
}

#[cfg(unix)]
fn winsize() -> Option<(u16, u16, u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    Some((ws.ws_row, ws.ws_col, ws.ws_xpixel, ws.ws_ypixel))
}

#[cfg(not(unix))]
fn winsize() -> Option<(u16, u16, u16, u16)> {
    None
}

#[derive(Debug, Clone)]
struct Cached {
    id: u32,
    /// File identity: (mtime secs, mtime nanos, length).
    stamp: (u64, u32, u64),
    w: u32,
    h: u32,
    seen: u64,
    /// Already transmitted to the terminal (persists across frames).
    sent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    pid: u32,
    /// (image id, x, y, cols, rows).
    key: (u32, u16, u16, u16, u16),
}

/// Per-frame graphics state, kept in the event loop.
pub struct GfxState {
    proto: GraphicsProto,
    tmux: bool,
    next_id: u32,
    clock: u64,
    cache: HashMap<(String, String), Cached>,
    last: Vec<Placed>,
}

impl GfxState {
    /// Which protocol this syncs (decided once, from the environment).
    pub fn proto(&self) -> GraphicsProto {
        self.proto
    }

    pub fn new(proto: GraphicsProto) -> Self {
        GfxState {
            proto,
            tmux: std::env::var("TMUX").is_ok_and(|v| !v.is_empty()),
            next_id: 1,
            clock: 0,
            cache: HashMap::new(),
            last: Vec::new(),
        }
    }

    /// Emit one escape sequence (tmux-wrapped when inside tmux).
    fn emit(&self, out: &mut Vec<u8>, esc: &str) {
        if self.tmux {
            out.extend_from_slice(b"\x1bPtmux;");
            out.extend_from_slice(esc.replace('\x1b', "\x1b\x1b").as_bytes());
            out.extend_from_slice(b"\x1b\\");
        } else {
            out.extend_from_slice(esc.as_bytes());
        }
    }

    fn transmit(&mut self, out: &mut Vec<u8>, id: u32, png: &[u8], w: u32, h: u32) {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let mut chunks = b64.as_bytes().chunks(4096).peekable();
        // Empty pictures still need one (empty) chunk.
        if chunks.peek().is_none() {
            self.emit(
                out,
                &format!("\x1b_Ga=t,t=d,f=100,s={w},v={h},i={id},m=0;\x1b\\"),
            );
            return;
        }
        while let Some(chunk) = chunks.next() {
            let more = if chunks.peek().is_none() { 0 } else { 1 };
            let text = String::from_utf8_lossy(chunk);
            self.emit(
                out,
                &format!("\x1b_Ga=t,t=d,f=100,s={w},v={h},i={id},m={more};{text}\x1b\\"),
            );
        }
    }

    fn place(&self, out: &mut Vec<u8>, pid: u32, id: u32, x: u16, y: u16, c: u16, r: u16) {
        self.emit(out, &format!("\x1b[{};{}H", y + 1, x + 1));
        self.emit(out, &format!("\x1b_Ga=p,i={id},p={pid},c={c},r={r}\x1b\\"));
    }

    fn delete_placement(&self, out: &mut Vec<u8>, pid: u32) {
        self.emit(out, &format!("\x1b_Ga=d,d=i,i={pid}\x1b\\"));
    }

    fn delete_image(&self, out: &mut Vec<u8>, id: u32) {
        self.emit(out, &format!("\x1b_Ga=d,d=I,i={id}\x1b\\"));
    }

    /// Forget every placement and picture (quitting).
    pub fn clear(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        if self.last.is_empty() && self.cache.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::new();
        let old: Vec<Placed> = std::mem::take(&mut self.last);
        for p in old {
            self.delete_placement(&mut buf, p.pid);
        }
        for c in self.cache.values() {
            if c.sent {
                self.delete_image(&mut buf, c.id);
            }
        }
        self.cache.clear();
        out.write_all(&buf)?;
        out.flush()
    }
}

/// Read a picture's dimensions without decoding it fully.
fn dimensions(path: &str) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// File identity for the cache, if the file is there.
fn stamp(path: &str) -> Option<((u64, u32, u64), u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(((t.as_secs(), t.subsec_nanos(), m.len()), m.len()))
}

/// Decode an image descriptor, apply its transforms, crop to the placement
/// (never up) and PNG-encode. Returns the PNG and its pixel dimensions.
fn render_png(
    path: &str,
    tw: u32,
    th: u32,
    descriptor: Option<&forge_core::display::ImageDescriptor>,
) -> Option<(Vec<u8>, u32, u32)> {
    use image::ImageEncoder;
    let img = image::open(path).ok()?;
    // Slice first, then scale, then rotate/flip.
    let mut img = match descriptor.and_then(|d| d.slice_rect((img.width(), img.height()))) {
        Some((x, y, w, h)) => img.crop_imm(x, y, w, h),
        None => img,
    };
    let d = descriptor.cloned().unwrap_or_default();
    let (mut nw, mut nh) = (img.width() as f64, img.height() as f64);
    // :width / :height, keeping the aspect ratio when only one is given.
    match (d.width, d.height) {
        (Some(w), Some(h)) => {
            nw = w;
            nh = h;
        }
        (Some(w), None) => {
            nh *= w / nw;
            nw = w;
        }
        (None, Some(h)) => {
            nw *= h / nh;
            nh = h;
        }
        (None, None) => {}
    }
    if let Some(s) = d.scale {
        nw *= s;
        nh *= s;
    }
    // :max-width / :max-height, keeping the aspect ratio.
    if let Some(mw) = d.max_width
        && nw > mw
    {
        nh *= mw / nw;
        nw = mw;
    }
    if let Some(mh) = d.max_height
        && nh > mh
    {
        nw *= mh / nh;
        nh = mh;
    }
    nw = nw.max(1.0);
    nh = nh.max(1.0);
    if (img.width() as f64 - nw).abs() > 0.5 || (img.height() as f64 - nh).abs() > 0.5 {
        img = img.resize(nw as u32, nh as u32, image::imageops::FilterType::Triangle);
    }
    img = match d.rotation.rem_euclid(360) {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    };
    if d.flip_h {
        img = img.fliph();
    }
    // :margin adds transparent pixels around the image.
    if d.margin != (0, 0) {
        let (mx, my) = (d.margin.0.max(0) as u32, d.margin.1.max(0) as u32);
        let (w, h) = (img.width() + mx * 2, img.height() + my * 2);
        let mut out = image::RgbaImage::new(w, h);
        image::imageops::overlay(&mut out, &img.to_rgba8(), mx as i64, my as i64);
        img = image::DynamicImage::ImageRgba8(out);
    }
    let img = if img.width() > tw || img.height() > th {
        image::DynamicImage::ImageRgba8(image::imageops::resize(
            &img,
            tw,
            th,
            image::imageops::FilterType::Triangle,
        ))
    } else {
        img.to_rgba8().into()
    };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&rgba.into_raw(), w, h, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some((png, w, h))
}

/// Sync the screen's pictures: delete what moved or went away, transmit
/// what's new, place what's here. An unchanged screen sends nothing.
pub fn sync(out: &mut impl Write, st: &mut GfxState, slots: &[Slot]) -> std::io::Result<()> {
    if st.proto != GraphicsProto::Kitty {
        return st.clear(out);
    }
    let cell = cell_size().unwrap_or(FALLBACK_CELL);
    st.clock += 1;
    // What should be on screen: one placement per slot with a picture.
    struct Want {
        key: (u32, u16, u16, u16, u16),
        path: String,
        id: u32,
        x: u16,
        y: u16,
        c: u16,
        r: u16,
        descriptor: Option<forge_core::display::ImageDescriptor>,
    }
    let mut want: Vec<Want> = Vec::new();
    let mut garbage: Vec<u32> = Vec::new();
    for s in slots {
        if s.w == 0 || s.h == 0 {
            continue;
        }
        let Some((stamp, _)) = stamp(&s.path) else {
            continue;
        };
        let key = (s.path.clone(), format!("{:?}", s.descriptor));
        let hit = match st.cache.get_mut(&key) {
            Some(c) if c.stamp == stamp => {
                c.seen = st.clock;
                Some((c.id, c.w, c.h))
            }
            _ => None,
        };
        let (id, iw, ih) = match hit {
            Some(h) => h,
            None => {
                let natural = dimensions(&s.path);
                let (iw, ih) = match (&natural, &s.descriptor) {
                    (Some(n), Some(d)) => d.target_size(*n),
                    (Some(n), None) => *n,
                    _ => continue,
                };
                if let Some(old) = st.cache.get(&key)
                    && old.sent
                {
                    garbage.push(old.id);
                }
                let id = st.next_id;
                st.next_id += 1;
                st.cache.insert(
                    key,
                    Cached {
                        id,
                        stamp,
                        w: iw,
                        h: ih,
                        seen: st.clock,
                        sent: false,
                    },
                );
                (id, iw, ih)
            }
        };
        let (c, r) = fit_slot(iw, ih, s.w, s.h, cell);
        want.push(Want {
            key: (id, s.x, s.y, c, r),
            path: s.path.clone(),
            id,
            x: s.x,
            y: s.y,
            c,
            r,
            descriptor: s.descriptor.clone(),
        });
    }
    let have: Vec<(u32, u16, u16, u16, u16)> = st.last.iter().map(|p| p.key).collect();
    if have == want.iter().map(|w| w.key).collect::<Vec<_>>() {
        return Ok(());
    }
    let mut buf: Vec<u8> = Vec::new();
    // Cursor save/restore around the block (ratatui repositions after).
    st.emit(&mut buf, "\x1b7");
    for id in garbage {
        st.delete_image(&mut buf, id);
    }
    for p in std::mem::take(&mut st.last) {
        st.delete_placement(&mut buf, p.pid);
    }
    // Transmit pictures placed for the first time (once per file).
    let mut sent_this_frame: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for w in &want {
        if !sent_this_frame.insert(w.id) {
            continue;
        }
        let ckey = (w.path.clone(), format!("{:?}", w.descriptor));
        if !st.cache.get(&ckey).is_some_and(|c| !c.sent) {
            continue;
        }
        let (tw, th) = fit_pixels(w.c, w.r, cell);
        if let Some((png, iw, ih)) = render_png(&w.path, tw, th, w.descriptor.as_ref()) {
            st.transmit(&mut buf, w.id, &png, iw.max(1), ih.max(1));
            if let Some(c) = st.cache.get_mut(&ckey) {
                c.sent = true;
            }
        }
    }
    for w in &want {
        let pid = st.next_id;
        st.next_id += 1;
        st.place(&mut buf, pid, w.id, w.x, w.y, w.c, w.r);
        st.last.push(Placed { pid, key: w.key });
    }
    // Drop transmitted pictures no screen shows anymore (bounded memory
    // both here and in the terminal).
    if st.cache.len() > 64 {
        let mut ids: Vec<((String, String), u64)> =
            st.cache.iter().map(|(k, c)| (k.clone(), c.seen)).collect();
        ids.sort_by_key(|(_, s)| *s);
        for (k, _) in ids.into_iter().take(st.cache.len() - 64) {
            if let Some(c) = st.cache.remove(&k)
                && c.sent
            {
                st.delete_image(&mut buf, c.id);
            }
        }
    }
    st.emit(&mut buf, "\x1b8");
    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellPx = CellPx { w: 8, h: 16 };

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        // 8x16 cells: a 40x8 slot is 320x128 px.
        // 160x80 at scale 1 -> 20 cols x 5 rows.
        assert_eq!(fit_slot(160, 80, 40, 8, CELL), (20, 5));
        // A wide panorama spans the width, fitting the rows.
        assert_eq!(fit_slot(400, 100, 40, 8, CELL), (40, 5));
        // A tall portrait fits the rows.
        assert_eq!(fit_slot(100, 400, 40, 8, CELL), (4, 8));
        // A small icon stays natural, top-left.
        assert_eq!(fit_slot(16, 16, 40, 8, CELL), (2, 1));
        // Degenerate inputs still place a cell.
        assert_eq!(fit_slot(0, 0, 40, 8, CELL), (1, 1));
        assert_eq!(fit_slot(100, 100, 0, 0, CELL), (1, 1));
    }

    #[test]
    fn fit_pixels_caps_huge_transfers() {
        assert_eq!(fit_pixels(40, 20, CELL), (320, 320));
        let (w, h) = fit_pixels(400, 400, CELL);
        assert!(w.max(h) <= 1600);
    }

    /// A small PNG on disk for sync tests.
    fn test_png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> String {
        use image::ImageEncoder;
        let path = dir.join(name);
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        std::fs::write(&path, &buf).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("forge-gfx-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn kitty_state() -> GfxState {
        let mut st = GfxState::new(GraphicsProto::Kitty);
        st.tmux = false;
        st
    }

    fn slot(path: &str) -> Slot {
        Slot {
            x: 0,
            y: 0,
            w: 40,
            h: 8,
            path: path.to_string(),
            descriptor: None,
        }
    }

    #[test]
    fn sync_transmits_once_then_only_moves() {
        let dir = tmpdir("sync");
        let path = test_png(&dir, "a.png", 160, 80);
        let mut st = kitty_state();
        // First frame: transmit (a=t) and place (a=p).
        let mut out = Vec::new();
        sync(&mut out, &mut st, &[slot(&path)]).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("a=t"), "{text}");
        assert!(text.contains("a=p"), "{text}");
        assert!(text.contains("f=100"), "{text}");
        assert!(text.contains(",m=0;"), "final chunk: {text}");
        // The payload decodes to a PNG (all transmit chunks concatenated).
        let mut payload = String::new();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("\x1b_G") {
            rest = &rest[i + 4..];
            if let Some(j) = rest.find(';') {
                let after = &rest[j + 1..];
                if let Some(k) = after.find("\x1b\\") {
                    payload.push_str(&after[..k]);
                    rest = &after[k..];
                    continue;
                }
            }
            break;
        }
        let raw =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &payload).unwrap();
        assert_eq!(&raw[..8], b"\x89PNG\r\n\x1a\n");
        // Same screen again: silence.
        let mut out2 = Vec::new();
        sync(&mut out2, &mut st, &[slot(&path)]).unwrap();
        assert!(out2.is_empty());
        // Moved one row: delete the placement, place again, no transmit.
        let mut moved = slot(&path);
        moved.y = 1;
        let mut out3 = Vec::new();
        sync(&mut out3, &mut st, &[moved]).unwrap();
        let text3 = String::from_utf8_lossy(&out3).into_owned();
        assert!(text3.contains("a=d"), "{text3}");
        assert!(text3.contains("a=p"), "{text3}");
        assert!(!text3.contains("a=t"), "no retransmit: {text3}");
        // Gone: delete, nothing placed.
        let mut out4 = Vec::new();
        sync(&mut out4, &mut st, &[]).unwrap();
        let text4 = String::from_utf8_lossy(&out4).into_owned();
        assert!(text4.contains("a=d"), "{text4}");
        assert!(!text4.contains("a=p"), "{text4}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slice_crops_the_picture() {
        let dir = tmpdir("slice");
        let path = test_png(&dir, "a.png", 40, 20);
        let mut st = kitty_state();
        let mut out = Vec::new();
        let mut s = slot(&path);
        s.descriptor = Some(forge_core::display::ImageDescriptor {
            slice: Some([5, 5, 10, 10].map(forge_core::display::SliceValue::px)),
            ..Default::default()
        });
        sync(&mut out, &mut st, &[s]).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("a=t"), "{text}");
        // The transmitted picture is the 10x10 slice, not the 40x20 source.
        assert!(text.contains("s=10,v=10"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_stay_placeholders() {
        let mut st = kitty_state();
        let mut out = Vec::new();
        sync(&mut out, &mut st, &[slot("/nonexistent/forge.png")]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn off_clears_leftovers() {
        let dir = tmpdir("off");
        let path = test_png(&dir, "a.png", 32, 32);
        let mut st = kitty_state();
        let mut out = Vec::new();
        sync(&mut out, &mut st, &[slot(&path)]).unwrap();
        assert!(!out.is_empty());
        st.proto = GraphicsProto::Off;
        let mut out2 = Vec::new();
        sync(&mut out2, &mut st, &[slot(&path)]).unwrap();
        let text = String::from_utf8_lossy(&out2).into_owned();
        assert!(text.contains("a=d"), "{text}");
        // And then silence.
        let mut out3 = Vec::new();
        sync(&mut out3, &mut st, &[]).unwrap();
        assert!(out3.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tmux_wraps_every_escape() {
        let dir = tmpdir("tmux");
        let path = test_png(&dir, "a.png", 32, 32);
        let mut st = kitty_state();
        st.tmux = true;
        let mut out = Vec::new();
        sync(&mut out, &mut st, &[slot(&path)]).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.starts_with("\x1bPtmux;"), "{text}");
        // Every graphics escape is doubled inside the wrapper.
        assert!(text.contains("\x1b\x1b_G"), "{text}");
        assert_eq!(
            text.matches("\x1b_G").count(),
            text.matches("\x1b\x1b_G").count(),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
