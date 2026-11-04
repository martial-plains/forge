// SPDX-License-Identifier: MIT OR Apache-2.0
//! Frames: separate top-level displays sharing the same buffers.
//!
//! C-x 5 2 makes a frame, C-x 5 0 deletes one, C-x 5 o switches, C-x 5 1
//! keeps only the current one. C-x 5 f / b / d open a file, buffer or
//! Dired listing in another frame. On a terminal the selected frame fills
//! the screen and the others stay in the background.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::BufferId;

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "make-frame" | "make-frame-command" => make_frame(app),
        "delete-frame" => delete_frame(app),
        "delete-other-frames" => {
            app.editor.delete_other_frames();
            app.msg("Deleted the other frames");
        }
        "other-frame" => app.editor.other_frame(1),
        "undelete-frame" => undelete_frame(app),
        "select-frame-by-name" => {
            let candidates: Vec<String> = app
                .editor
                .windows
                .frame_ids()
                .into_iter()
                .filter_map(|id| app.editor.windows.frame_name(id).map(str::to_string))
                .collect();
            app.prompt(
                "Select frame (name): ",
                Purpose::Choose {
                    what: crate::minibuffer::Choice::SelectFrame,
                    candidates,
                },
            );
        }
        "set-frame-name" => {
            let id = app.editor.windows.selected_frame_id();
            let default = app
                .editor
                .windows
                .frame_name(id)
                .unwrap_or("frame")
                .to_string();
            app.prompt("Set frame name: ", Purpose::SetFrameName { default });
        }
        "make-frame-on-display" => {
            app.prompt(
                "Make frame on display: ",
                Purpose::MakeFrameOnDisplay { monitor: false },
            );
        }
        "make-frame-on-monitor" => {
            app.prompt(
                "Make frame on monitor: ",
                Purpose::MakeFrameOnDisplay { monitor: true },
            );
        }
        "frames-on-display" => print_frames_on_display(app),
        "find-file-other-frame" => {
            let input = crate::minibuffer::initial_find_file_input(app);
            app.prompt(
                "Find file in other frame: ",
                Purpose::FindFile {
                    other_window: false,
                    other_frame: true,
                    other_tab: false,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = input;
            }
        }
        "switch-to-buffer-other-frame" => {
            let default = app
                .other_buffer()
                .and_then(|id| app.editor.state.get_buffer(id))
                .map(|b| b.name().to_string());
            let prompt = match &default {
                Some(d) => format!("Switch to buffer in other frame (default {d}): "),
                None => "Switch to buffer in other frame: ".to_string(),
            };
            app.prompt(
                prompt,
                Purpose::SwitchBuffer {
                    default,
                    other_window: false,
                    other_frame: true,
                    other_tab: false,
                },
            );
        }
        "dired-other-frame" => {
            let dir = crate::dired::start_dir(app);
            app.prompt(
                "Dired in other frame (directory): ",
                Purpose::Dired(crate::dired::DiredAction::OpenOtherFrame),
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        _ => return false,
    }
    true
}

fn make_frame(app: &mut App) {
    let id = app.editor.make_frame();
    let name = app
        .editor
        .windows
        .frame_name(id)
        .unwrap_or("frame")
        .to_string();
    app.msg(format!("Created frame {name}"));
}

fn delete_frame(app: &mut App) {
    if app.editor.windows.frame_count() == 1 {
        return app.error("Attempt to delete the sole frame");
    }
    let id = app.editor.windows.selected_frame_id();
    match app.editor.delete_frame(id) {
        Ok(()) => app.msg("Deleted frame"),
        Err(e) => app.error(e),
    }
}

/// C-x 5 u: bring back a frame deleted with `delete-frame`. The prefix
/// argument says which: 1 (the default) is the most recently deleted.
fn undelete_frame(app: &mut App) {
    let arg = app.count();
    if !(1..=16).contains(&arg) {
        return app.error("The prefix argument must be between 1 and 16");
    }
    match app.editor.undelete_frame(arg as usize) {
        Some(id) => {
            let name = app
                .editor
                .windows
                .frame_name(id)
                .unwrap_or("frame")
                .to_string();
            app.msg(format!("Undeleted frame {name}"));
        }
        None => app.error("No frame was undeleted"),
    }
}

/// Show `buffer` in another frame: an existing one already showing it,
/// else a new frame (C-x 5 f / b and their kin).
pub fn show_in_other_frame(app: &mut App, buffer: BufferId) {
    match app.editor.windows.frame_showing(buffer) {
        Some(id) if id != app.editor.windows.selected_frame_id() => app.editor.select_frame(id),
        Some(_) => {
            app.editor.make_frame();
        }
        None => {
            app.editor.make_frame();
        }
    }
    app.switch_to(buffer);
}

/// The name of the one display Forge runs on. frames name their
/// display; Forge has a single screen, so there is always exactly one.
pub fn display_name() -> String {
    std::env::var("DISPLAY")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "forge".to_string())
}

/// select-frame-by-name: make the named frame the selected one. Answers
/// with the name of a frame (usually from TAB completion).
pub fn select_by_name(app: &mut App, name: &str) {
    let ids = app.editor.windows.frame_ids();
    for id in ids {
        if app.editor.windows.frame_name(id) == Some(name) {
            app.editor.select_frame(id);
            app.msg(format!("Selected frame {name}"));
            return;
        }
    }
    app.error(format!("No frame named {name}"));
}

/// set-frame-name: give the selected frame a new name. Names need not be
/// unique, so this never fails.
pub fn rename_selected(app: &mut App, name: &str) {
    let id = app.editor.windows.selected_frame_id();
    app.editor.windows.set_frame_name(id, name);
    app.msg(format!("Frame renamed to {name}"));
}

/// make-frame-on-display / -on-monitor answered: make a frame, choosing
/// what to reject. On a display, an unknown display is
/// an error; Forge has one, so only its own name works. A monitor name is
/// never checked, since Forge doesn't model monitors.
pub fn make_frame_on_display(app: &mut App, monitor: bool, answer: &str) {
    let answer = answer.trim();
    if !monitor && !answer.is_empty() && answer != display_name() {
        return app.error(format!("No display named {answer}"));
    }
    let id = app.editor.make_frame();
    let name = app
        .editor
        .windows
        .frame_name(id)
        .unwrap_or("frame")
        .to_string();
    app.msg(format!("Created frame {name}"));
}

/// frames-on-display: say which frames are on the current display. Forge
/// returns a list; as a command we print the names in the echo area.
pub fn print_frames_on_display(app: &mut App) {
    let names: Vec<String> = app
        .editor
        .windows
        .frame_ids()
        .into_iter()
        .filter_map(|id| app.editor.windows.frame_name(id).map(str::to_string))
        .collect();
    let display = display_name();
    if names.is_empty() {
        app.msg(format!("No frames on display {display}"));
    } else {
        app.msg(format!(
            "{} frame(s) on display {display}: {}",
            names.len(),
            names.join(", ")
        ));
    }
}
