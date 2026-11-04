// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge as a C library: the same grid API the GNOME app use
//! (`forge_ffi::Forge::grid`), for a frontend that can call C but has no
//! UniFFI bindings, such as the WinUI app (C#, P/Invoke).
//!
//! Everything crosses as UTF-8 C strings or plain integers. The grid comes
//! back as JSON (see [`forge_grid_json`]); strings returned by this library
//! belong to it and must be freed with [`forge_string_free`]. A handle is
//! used from one thread at a time except where noted.

use std::ffi::{CStr, CString, c_char};
use std::sync::Arc;

use forge_ffi::{Forge, Grid, Key, KeyInput, MouseKind};
use serde_json::{Value, json};

/// An editor.
pub struct Handle(Arc<Forge>);

fn text<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn give(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Start an editor. `files` is a JSON array of paths to open (or null);
/// `no_config` non-zero reads no configuration. Null if it can't start
/// (the reason is in `forge_last_error`).
///
/// # Safety
/// `files` must be null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_new(files: *const c_char, no_config: i32) -> *mut Handle {
    let files: Vec<String> = text(files).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    match Forge::new_grid(files, no_config != 0) {
        Ok(f) => Box::into_raw(Box::new(Handle(f))),
        Err(e) => {
            *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = e.to_string();
            std::ptr::null_mut()
        }
    }
}

static LAST_ERROR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Why `forge_new` returned null. Free with `forge_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn forge_last_error() -> *mut c_char {
    give(LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

/// # Safety
/// `h` must come from `forge_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_free(h: *mut Handle) {
    if !h.is_null() {
        drop(unsafe { Box::from_raw(h) });
    }
}

/// # Safety
/// `s` must come from this library and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

fn handle<'a>(h: *mut Handle) -> Option<&'a Forge> {
    unsafe { h.as_ref() }.map(|h| &*h.0)
}

/// Key codes for [`forge_key`]'s `kind`.
pub const KEY_CHAR: i32 = 0;
pub const KEY_ENTER: i32 = 1;
pub const KEY_TAB: i32 = 2;
pub const KEY_BACKTAB: i32 = 3;
pub const KEY_BACKSPACE: i32 = 4;
pub const KEY_DELETE: i32 = 5;
pub const KEY_ESCAPE: i32 = 6;
pub const KEY_LEFT: i32 = 7;
pub const KEY_RIGHT: i32 = 8;
pub const KEY_UP: i32 = 9;
pub const KEY_DOWN: i32 = 10;
pub const KEY_HOME: i32 = 11;
pub const KEY_END: i32 = 12;
pub const KEY_PAGE_UP: i32 = 13;
pub const KEY_PAGE_DOWN: i32 = 14;
pub const KEY_INSERT: i32 = 15;
/// Function keys are `KEY_F1 + n - 1` (F1 to F12).
pub const KEY_F1: i32 = 20;

/// A key. For `KEY_CHAR`, `ch` is the Unicode scalar value. `flags`:
/// 1 control, 2 meta (alt), 4 shift.
#[unsafe(no_mangle)]
pub extern "C" fn forge_key(h: *mut Handle, kind: i32, ch: u32, flags: i32) {
    let Some(f) = handle(h) else { return };
    let key = match kind {
        KEY_CHAR => match char::from_u32(ch) {
            Some(c) => Key::Char { c: c.to_string() },
            None => return,
        },
        KEY_ENTER => Key::Enter,
        KEY_TAB => Key::Tab,
        KEY_BACKTAB => Key::BackTab,
        KEY_BACKSPACE => Key::Backspace,
        KEY_DELETE => Key::Delete,
        KEY_ESCAPE => Key::Escape,
        KEY_LEFT => Key::Left,
        KEY_RIGHT => Key::Right,
        KEY_UP => Key::Up,
        KEY_DOWN => Key::Down,
        KEY_HOME => Key::Home,
        KEY_END => Key::End,
        KEY_PAGE_UP => Key::PageUp,
        KEY_PAGE_DOWN => Key::PageDown,
        KEY_INSERT => Key::Insert,
        k if (KEY_F1..KEY_F1 + 12).contains(&k) => Key::F { n: (k - KEY_F1 + 1) as u8 },
        _ => return,
    };
    f.key(KeyInput { key, control: flags & 1 != 0, meta: flags & 2 != 0, shift: flags & 4 != 0 });
}

/// Text from an input method or a paste.
///
/// # Safety
/// `s` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_text(h: *mut Handle, s: *const c_char) {
    if let (Some(f), Some(t)) = (handle(h), text(s)) {
        f.text(t.to_string());
    }
}

/// Mouse kinds: 0 down, 1 drag, 2 up, 3 right-down, 4 scroll up, 5 scroll down.
#[unsafe(no_mangle)]
pub extern "C" fn forge_mouse(h: *mut Handle, kind: i32, col: u16, row: u16) {
    let Some(f) = handle(h) else { return };
    let kind = match kind {
        0 => MouseKind::Down,
        1 => MouseKind::Drag,
        2 => MouseKind::Up,
        3 => MouseKind::RightDown,
        4 => MouseKind::ScrollUp,
        5 => MouseKind::ScrollDown,
        _ => return,
    };
    f.mouse(kind, col, row);
}

/// Run a command by name, as `M-x` would.
///
/// # Safety
/// `name` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_run_command(h: *mut Handle, name: *const c_char) {
    if let (Some(f), Some(n)) = (handle(h), text(name)) {
        f.run_command(n);
    }
}

/// C-g for a busy plugin. Safe from any thread.
#[unsafe(no_mangle)]
pub extern "C" fn forge_interrupt(h: *mut Handle) {
    if let Some(f) = handle(h) {
        f.interrupt();
    }
}

/// Say that this frontend draws canvases itself: they arrive in the grid's
/// pictures as paths ending in `.scene.json` (a display list; see
/// crates/canvas/src/native.rs) instead of PNGs. Call it before the first grid.
#[unsafe(no_mangle)]
pub extern "C" fn forge_set_native_scenes(h: *mut Handle, on: i32) {
    if let Some(f) = handle(h) {
        f.set_native_scenes(on != 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_save_session(h: *mut Handle) {
    if let Some(f) = handle(h) {
        f.save_session();
    }
}

/// The answer to a `dialog` in the grid: the path picked, or null if the
/// person cancelled.
///
/// # Safety
/// `path` must be null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn forge_dialog_result(h: *mut Handle, id: u32, path: *const c_char) {
    if let Some(f) = handle(h) {
        f.dialog_result(id, text(path).map(str::to_string));
    }
}

/// The kill ring's top (for the system clipboard), or null if empty. Free
/// with `forge_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn forge_kill_top(h: *mut Handle) -> *mut c_char {
    handle(h).and_then(|f| pollster::block_on(f.kill_top())).map_or(std::ptr::null_mut(), give)
}

pub fn grid_value(g: &Grid) -> Value {
    json!({
        "cols": g.cols,
        "rows": g.rows,
        "fg": g.fg,
        "bg": g.bg,
        "cursor": [g.cursor_col, g.cursor_row],
        "title": g.title,
        "modified": g.modified,
        "busy": g.busy,
        "quit": g.quit,
        "crashed": g.crashed,
        // [row, col, cols, text, fg, bg, flags] per run
        "runs": g.runs.iter().map(|r| json!([r.row, r.col, r.cols, r.text, r.fg, r.bg, r.flags])).collect::<Vec<_>>(),
        "pictures": g.pictures.iter().map(|p| json!({"col": p.col, "row": p.row, "cols": p.cols, "rows": p.rows, "path": p.path})).collect::<Vec<_>>(),
        "dialog": g.dialog.as_ref().map(|d| json!({"id": d.id, "kind": d.kind, "title": d.title, "start": d.start, "name": d.name})),
    })
}

/// The screen, `cols` by `rows` cells, as JSON:
///
/// ```json
/// {"cols":80,"rows":24,"fg":13481716,"bg":1973806,"cursor":[0,1],
///  "title":"a.txt","modified":false,"busy":null,"quit":false,"crashed":null,
///  "runs":[[row,col,cols,"text",fg,bg,flags],...],
///  "pictures":[{"col":0,"row":0,"cols":10,"rows":5,"path":"..."}],
///  "dialog":{"id":1,"kind":"open_file|open_folder|save_as","title":"..","start":"..","name":".."}|null}
/// ```
///
/// Colours are `0xRRGGBB`; `flags` is 1 bold, 2 italic, 4 underline,
/// 8 strike. Blocks until the editor answers (a busy editor answers with
/// the last picture, `busy` set), so call it off the UI thread. Free the
/// result with `forge_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn forge_grid_json(h: *mut Handle, cols: u16, rows: u16, dark: i32) -> *mut c_char {
    let Some(f) = handle(h) else { return std::ptr::null_mut() };
    let g = pollster::block_on(f.grid(cols, rows, dark != 0));
    give(grid_value(&g).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_c_api_runs_an_editor() {
        let dir = std::env::temp_dir().join(format!("forge-capi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.txt");
        std::fs::write(&file, "from c\n").unwrap();
        let files = CString::new(serde_json::to_string(&[file.display().to_string()]).unwrap()).unwrap();
        let h = unsafe { forge_new(files.as_ptr(), 1) };
        assert!(!h.is_null());
        for c in "ab".chars() {
            forge_key(h, KEY_CHAR, c as u32, 0);
        }
        let json = forge_grid_json(h, 60, 12, 1);
        let s = unsafe { CStr::from_ptr(json) }.to_str().unwrap().to_string();
        unsafe { forge_string_free(json) };
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["cols"], 60);
        assert_eq!(v["title"], "c.txt");
        assert_eq!(v["modified"], true, "the typed text changed the buffer");
        let all: String = v["runs"].as_array().unwrap().iter().map(|r| r[3].as_str().unwrap()).collect::<Vec<_>>().join("|");
        assert!(all.contains("abfrom c"), "{all}");
        // A picker request comes through the grid and is answered by id.
        let cmd = CString::new("find-file-dialog").unwrap();
        unsafe { forge_run_command(h, cmd.as_ptr()) };
        let json = forge_grid_json(h, 60, 12, 1);
        let v: Value = serde_json::from_str(unsafe { CStr::from_ptr(json) }.to_str().unwrap()).unwrap();
        unsafe { forge_string_free(json) };
        assert_eq!(v["dialog"]["kind"], "open_file");
        let id = v["dialog"]["id"].as_u64().unwrap() as u32;
        unsafe { forge_dialog_result(h, id, std::ptr::null()) };
        let json = forge_grid_json(h, 60, 12, 1);
        let v: Value = serde_json::from_str(unsafe { CStr::from_ptr(json) }.to_str().unwrap()).unwrap();
        unsafe { forge_string_free(json) };
        assert!(v["dialog"].is_null(), "cancelled");
        unsafe { forge_free(h) };
        let _ = std::fs::remove_dir_all(dir);
    }
}
