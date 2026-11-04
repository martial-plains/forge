// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bookmarks: named places, C-x r m / C-x r b.
//!
//! A bookmark is a name for a file and a char offset of point in it,
//! kept in the session (like save-place) so it survives restarts.
//! Registers hold positions too, but only for one character and only
//! for this session; bookmarks have real names and persist.

use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use std::path::Path;

/// Save the default name offered when setting a bookmark: the file's
/// name, or the buffer's when it visits no file.
fn default_name(app: &App) -> String {
    let b = app.buf_ref();
    b.path()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| b.name().to_string())
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "bookmark-set" => {
            if app.buf_ref().path().is_none() {
                app.error("Bookmark needs a buffer visiting a file");
                return true;
            }
            let default = default_name(app);
            app.prompt(
                format!("Set bookmark (default {default}): "),
                Purpose::BookmarkName { default },
            );
        }
        "bookmark-jump" => {
            let names = app.editor.session.bookmark_names();
            if names.is_empty() {
                app.error("No bookmarks are set");
                return true;
            }
            app.prompt(
                "Jump to bookmark: ",
                Purpose::Choose {
                    what: Choice::BookmarkJump,
                    candidates: names,
                },
            );
        }
        "bookmark-delete" => {
            let names = app.editor.session.bookmark_names();
            if names.is_empty() {
                app.error("No bookmarks are set");
                return true;
            }
            app.prompt(
                "Delete bookmark: ",
                Purpose::Choose {
                    what: Choice::BookmarkDelete,
                    candidates: names,
                },
            );
        }
        "list-bookmarks" => {
            let mut text = String::from("Bookmarks\n\n");
            let names = app.editor.session.bookmark_names();
            if names.is_empty() {
                text.push_str("No bookmarks are set. C-x r m sets one here.\n");
            }
            for name in &names {
                let where_ = app
                    .editor
                    .session
                    .bookmark(name)
                    .map(|b| b.path)
                    .unwrap_or_default();
                text.push_str(&format!("{name}  {where_}\n"));
            }
            app.show_special_buffer("*Bookmarks*", "special-mode", &text);
        }
        _ => return false,
    }
    true
}

/// The bookmark name was entered.
pub fn set(app: &mut App, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return app.error("No bookmark name");
    }
    let Some(path) = app.buf_ref().path().map(|p| p.display().to_string()) else {
        return app.error("Bookmark needs a buffer visiting a file");
    };
    let offset = app.buf_ref().point_offset();
    app.editor.session.set_bookmark(name, &path, offset);
    let _ = app.editor.session.save();
    app.msg(format!("Bookmark {name} set in {path}"));
}

/// Go to the named bookmark, opening its file when needed.
pub fn jump(app: &mut App, name: &str) {
    let Some(bm) = app.editor.session.bookmark(name) else {
        return app.error(format!("No bookmark {name}"));
    };
    crate::minibuffer::visit_file(app, Path::new(&bm.path), false);
    let max = {
        let b = app.buf_ref();
        b.pos_to_offset(b.end())
    };
    let b = app.buf();
    let p = b.point();
    b.push_mark(p, false);
    b.set_offset(bm.offset.min(max));
}

/// Forget the named bookmark.
pub fn delete(app: &mut App, name: &str) {
    if app.editor.session.remove_bookmark(name) {
        let _ = app.editor.session.save();
        app.msg(format!("Bookmark {name} deleted"));
    } else {
        app.error(format!("No bookmark {name}"));
    }
}
