// SPDX-License-Identifier: MIT OR Apache-2.0
//! window and buffer habits that are off until asked for: windmove,
//! winner-mode, uniquify and global-auto-revert-mode.

use crate::app::App;
use crate::org_flow::{setting_flag, setting_number, setting_string};
use forge_core::{BufferId, Side, WindowConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "uniquify-buffer-names",
        "Name buffers visiting same-named files after their directories now.",
    )
    .body("Needs the uniquify_buffer_name_style option: forward, reverse, post-forward or post-forward-angle-brackets."),
    forge_docs::CommandDoc::line("windmove-down", "Select the window below the selected one."),
    forge_docs::CommandDoc::line("windmove-left", "Select the window to the left of the selected one."),
    forge_docs::CommandDoc::line("windmove-right", "Select the window to the right of the selected one."),
    forge_docs::CommandDoc::line("windmove-up", "Select the window above the selected one."),
    forge_docs::CommandDoc::new(
        "winner-redo",
        "Go forward again to the window arrangement winner-undo left.",
    )
    .body("Needs the winner_mode option, which records the arrangements."),
    forge_docs::CommandDoc::new(
        "winner-undo",
        "Go back to the previous window arrangement (C-c <left>).",
    )
    .body("Needs the winner_mode option, which records the arrangements."),
];

#[derive(Default)]
pub struct State {
    undo: Vec<WindowConfig>,
    redo: Vec<WindowConfig>,
    last: Option<WindowConfig>,
    /// The names uniquify gave, and what it looked at when it did.
    named: HashMap<BufferId, String>,
    seen: Vec<(BufferId, PathBuf)>,
    style_seen: Option<String>,
    revert_checked: Option<Instant>,
    dired_stamps: HashMap<BufferId, SystemTime>,
}

const WINNER_DEPTH: usize = 200;

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "windmove-left" => windmove(app, Side::Left),
        "windmove-right" => windmove(app, Side::Right),
        "windmove-up" => windmove(app, Side::Up),
        "windmove-down" => windmove(app, Side::Down),
        "winner-undo" => winner(app, true),
        "winner-redo" => winner(app, false),
        "uniquify-buffer-names" => {
            app.extras.seen.clear();
            uniquify(app);
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------- windmove

fn windmove(app: &mut App, side: Side) {
    let target = app.editor.windows.neighbor(side).or_else(|| {
        // windmove-wrap-around: continue from the opposite edge.
        if !setting_flag(app, "windmove_wrap_around") {
            return None;
        }
        let ids = app.editor.windows.ids();
        let me = app.editor.windows.selected_id();
        let mut cur = me;
        let mut found = None;
        // Walk the other way until there is nothing further.
        let back = match side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        };
        for _ in 0..ids.len() {
            app.editor.select_window(cur);
            match app.editor.windows.neighbor(back) {
                Some(n) => {
                    found = Some(n);
                    cur = n;
                }
                None => break,
            }
        }
        app.editor.select_window(me);
        found.filter(|w| *w != me)
    });
    match target {
        Some(id) => app.select_window(id),
        None => {
            let word = match side {
                Side::Left => "left",
                Side::Right => "right",
                Side::Up => "up",
                Side::Down => "down",
            };
            app.error(format!("No window {word} from selected window"));
        }
    }
}

// ------------------------------------------------------------------ winner

fn winner(app: &mut App, back: bool) {
    if !setting_flag(app, "winner_mode") {
        return app.error("Turn on the winner_mode option first");
    }
    let now = app.editor.windows.configuration();
    loop {
        let st = &mut app.extras;
        let next = if back { st.undo.pop() } else { st.redo.pop() };
        let Some(cfg) = next else {
            return app.error(if back {
                "No earlier window configuration"
            } else {
                "No later window configuration"
            });
        };
        if cfg == now {
            continue;
        }
        if back {
            st.redo.push(now);
        } else {
            st.undo.push(now);
        }
        app.editor.restore_windows(&cfg);
        app.extras.last = Some(cfg);
        return;
    }
}

/// After each command: remember the arrangement if it changed.
fn record_winner(app: &mut App) {
    if !setting_flag(app, "winner_mode") {
        app.extras.last = None;
        return;
    }
    let now = app.editor.windows.configuration();
    let st = &mut app.extras;
    match st.last.take() {
        Some(prev) if prev != now => {
            st.undo.push(prev);
            if st.undo.len() > WINNER_DEPTH {
                st.undo.remove(0);
            }
            st.redo.clear();
        }
        _ => {}
    }
    st.last = Some(now);
}

// ---------------------------------------------------------------- uniquify

/// The part of `path` that tells it apart: its last `depth` directories.
fn dirs_of(path: &std::path::Path, depth: usize) -> Vec<String> {
    let mut parts: Vec<String> = path
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let keep = depth.min(parts.len());
    parts.drain(..parts.len() - keep);
    parts
}

fn styled(style: &str, base: &str, dirs: &[String]) -> String {
    match style {
        "reverse" => {
            let mut s = base.to_string();
            for d in dirs.iter().rev() {
                s.push('\\');
                s.push_str(d);
            }
            s
        }
        "post-forward" => format!("{base}|{}", dirs.join("/")),
        "post-forward-angle-brackets" => format!("{base}<{}>", dirs.join("/")),
        _ => format!("{}/{base}", dirs.join("/")),
    }
}

fn uniquify(app: &mut App) {
    let Some(style) = setting_string(app, "uniquify_buffer_name_style").filter(|s| !s.is_empty())
    else {
        if app.extras.style_seen.take().is_some() {
            // Switched off: names go back to the file names.
            let named = std::mem::take(&mut app.extras.named);
            for (id, _) in named {
                rename_to_base(app, id);
            }
            app.extras.seen.clear();
        }
        return;
    };
    let files: Vec<(BufferId, PathBuf)> = app
        .editor
        .buffer_ids()
        .into_iter()
        .filter_map(|id| {
            let p = app.editor.state.get_buffer(id)?.path()?.to_path_buf();
            Some((id, p))
        })
        .collect();
    if app.extras.seen == files && app.extras.style_seen.as_deref() == Some(style.as_str()) {
        return;
    }
    app.extras.seen = files.clone();
    app.extras.style_seen = Some(style.clone());

    let mut groups: HashMap<String, Vec<(BufferId, PathBuf)>> = HashMap::new();
    for (id, p) in files {
        if let Some(n) = p.file_name() {
            groups
                .entry(n.to_string_lossy().into_owned())
                .or_default()
                .push((id, p));
        }
    }
    let min = setting_number(app, "uniquify_min_dir_components")
        .unwrap_or(0.0)
        .max(0.0) as usize;
    for (base, members) in groups {
        // Only buffers still named as we (or Forge) named them are ours.
        let ours: Vec<&(BufferId, PathBuf)> = members
            .iter()
            .filter(|(id, _)| {
                let name = app
                    .editor
                    .state
                    .get_buffer(*id)
                    .map(|b| b.name().to_string());
                name.as_deref() == Some(base.as_str())
                    || name
                        .as_ref()
                        .is_some_and(|n| app.extras.named.get(id) == Some(n))
                    || name
                        .as_ref()
                        .is_some_and(|n| n.starts_with(&format!("{base}<")) && n.ends_with('>'))
            })
            .collect();
        if ours.len() < 2 {
            for (id, _) in &members {
                if app.extras.named.contains_key(id) {
                    rename_to_base(app, *id);
                    app.extras.named.remove(id);
                }
            }
            continue;
        }
        // The fewest directories that make every name different: only the
        // names that clash grow.
        let mut depths = vec![min.max(1); ours.len()];
        let names = loop {
            let names: Vec<String> = ours
                .iter()
                .zip(&depths)
                .map(|((_, p), d)| styled(&style, &base, &dirs_of(p, *d)))
                .collect();
            let mut grew = false;
            for i in 0..names.len() {
                if names.iter().filter(|n| **n == names[i]).count() > 1 && depths[i] < 64 {
                    depths[i] += 1;
                    grew = true;
                }
            }
            if !grew {
                break names;
            }
        };
        for ((id, _), name) in ours.iter().zip(names) {
            let current = app
                .editor
                .state
                .get_buffer(*id)
                .map(|b| b.name().to_string());
            if current.as_deref() != Some(name.as_str()) {
                match app.editor.find_buffer(&name) {
                    Some(other) if other != *id => continue,
                    _ => {}
                }
                if let Some(b) = app.editor.state.get_buffer_mut(*id) {
                    b.set_name(name.clone());
                }
            }
            app.extras.named.insert(*id, name);
        }
    }
    app.extras
        .named
        .retain(|id, _| app.editor.state.get_buffer(*id).is_some());
}

fn rename_to_base(app: &mut App, id: BufferId) {
    let base = app.editor.state.get_buffer(id).and_then(|b| {
        b.path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    if let Some(base) = base
        && app.editor.find_buffer(&base).is_none()
        && let Some(b) = app.editor.state.get_buffer_mut(id)
    {
        b.set_name(base);
    }
}

// ------------------------------------------------------------- auto-revert

/// `global-auto-revert-mode`: every file buffer that has not been edited
/// follows its file, not only the ones on screen; with the non-file option
/// Dired listings follow their directories too.
fn auto_revert(app: &mut App) {
    if !setting_flag(app, "global_auto_revert_mode") {
        app.extras.revert_checked = None;
        return;
    }
    let every = setting_number(app, "auto_revert_interval")
        .unwrap_or(5.0)
        .max(0.0);
    if app
        .extras
        .revert_checked
        .is_some_and(|t| t.elapsed().as_secs_f64() < every)
    {
        return;
    }
    app.extras.revert_checked = Some(Instant::now());
    for id in app.editor.buffer_ids() {
        let Some(b) = app.editor.state.get_buffer(id) else {
            continue;
        };
        if b.path().is_some() && !b.is_modified() && b.changed_on_disk() {
            app.check_file(id);
        }
    }
    if !setting_flag(app, "global_auto_revert_non_file_buffers") {
        return;
    }
    let me = app.editor.windows.selected_id();
    let shown: Vec<_> = app
        .editor
        .windows
        .iter()
        .map(|w| (w.id, w.buffer))
        .collect();
    for (win, id) in shown {
        let Some((_, at)) = app
            .editor
            .state
            .get_buffer(id)
            .and_then(crate::dired::watch_stamp)
        else {
            continue;
        };
        match app.extras.dired_stamps.insert(id, at) {
            Some(old) if old != at => {
                app.editor.select_window(win);
                crate::dired::refresh(app);
            }
            _ => {}
        }
    }
    app.editor.select_window(me);
    app.extras
        .dired_stamps
        .retain(|id, _| app.editor.state.get_buffer(*id).is_some());
}

pub fn after_command(app: &mut App) {
    record_winner(app);
    uniquify(app);
}

pub fn on_idle(app: &mut App) {
    uniquify(app);
    auto_revert(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Rect};

    fn app_with_options(src: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("one");
        b.set_contents("one\ntwo\n");
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        let path = std::env::temp_dir().join(format!(
            "forge-extras-init-{}-{}.rhai",
            std::process::id(),
            src.len()
        ));
        std::fs::write(&path, src).unwrap();
        app.init_path = Some(path);
        app.load_init_file();
        app
    }

    /// Lay the windows out so they have places on the screen.
    fn lay_out(app: &mut App) {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let _ = forge_core::display::redisplay(&mut app.editor, area, &Default::default());
    }

    #[test]
    fn windmove_goes_to_the_window_in_that_direction() {
        let mut app = app_with_options("");
        let first = app.editor.windows.selected_id();
        app.run_command("split-window-right");
        lay_out(&mut app);
        let right = app.editor.windows.nth_from_selected(1);
        app.run_command("windmove-right");
        assert_eq!(app.editor.windows.selected_id(), right);
        app.run_command("windmove-left");
        assert_eq!(app.editor.windows.selected_id(), first);
        // Nothing further left: an error, and no change.
        app.run_command("windmove-left");
        assert_eq!(app.editor.windows.selected_id(), first);
        app.run_command("split-window-below");
        lay_out(&mut app);
        let below = app
            .editor
            .windows
            .neighbor(Side::Down)
            .expect("a window below");
        app.run_command("windmove-down");
        assert_eq!(app.editor.windows.selected_id(), below);
        app.run_command("windmove-up");
        assert_eq!(app.editor.windows.selected_id(), first);
    }

    #[test]
    fn windmove_wraps_around_when_asked() {
        let mut app = app_with_options("forge.opt[\"windmove_wrap_around\"] = true;\n");
        let first = app.editor.windows.selected_id();
        app.run_command("split-window-right");
        lay_out(&mut app);
        let right = app.editor.windows.nth_from_selected(1);
        app.run_command("windmove-left");
        assert_eq!(app.editor.windows.selected_id(), right);
        app.run_command("windmove-left");
        assert_eq!(app.editor.windows.selected_id(), first);
    }

    #[test]
    fn winner_undoes_and_redoes_window_changes() {
        let mut app = app_with_options("forge.opt[\"winner_mode\"] = true;\n");
        lay_out(&mut app);
        app.run_command("other-window"); // a command, so the first state is recorded
        assert_eq!(app.editor.windows.count(), 1);
        app.run_command("split-window-right");
        assert_eq!(app.editor.windows.count(), 2);
        app.run_command("split-window-below");
        assert_eq!(app.editor.windows.count(), 3);
        app.run_command("winner-undo");
        assert_eq!(app.editor.windows.count(), 2);
        app.run_command("winner-undo");
        assert_eq!(app.editor.windows.count(), 1);
        app.run_command("winner-redo");
        assert_eq!(app.editor.windows.count(), 2);
        app.run_command("winner-redo");
        assert_eq!(app.editor.windows.count(), 3);
        // Undo after a window was deleted brings it back.
        app.run_command("delete-other-windows");
        assert_eq!(app.editor.windows.count(), 1);
        app.run_command("winner-undo");
        assert_eq!(app.editor.windows.count(), 3);
    }

    #[test]
    fn winner_needs_its_option() {
        let mut app = app_with_options("");
        app.run_command("split-window-right");
        app.run_command("winner-undo");
        assert_eq!(app.editor.windows.count(), 2, "nothing was recorded");
    }

    fn file_buffer(app: &mut App, path: &std::path::Path) -> BufferId {
        let b = Buffer::from_file(path).unwrap();
        app.editor.add_buffer(b)
    }

    #[test]
    fn uniquify_names_same_named_files_after_their_directories() {
        let root = std::env::temp_dir().join(format!("forge-uniq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["alpha/src", "beta/src", "gamma"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("main.rs"), "x").unwrap();
        }
        let mut app = app_with_options(
            "forge.opt[\"uniquify_buffer_name_style\"] = \"post-forward-angle-brackets\";\n",
        );
        let a = file_buffer(&mut app, &root.join("alpha/src/main.rs"));
        let b = file_buffer(&mut app, &root.join("beta/src/main.rs"));
        app.run_command("uniquify-buffer-names");
        let name = |app: &App, id| app.editor.state.get_buffer(id).unwrap().name().to_string();
        assert_eq!(name(&app, a), "main.rs<alpha/src>");
        assert_eq!(name(&app, b), "main.rs<beta/src>");
        // One more file with the name; only one directory tells gamma apart.
        let c = file_buffer(&mut app, &root.join("gamma/main.rs"));
        app.run_command("uniquify-buffer-names");
        assert_eq!(name(&app, c), "main.rs<gamma>");
        assert_ne!(name(&app, a), name(&app, b));
        // Killing all but one gives the survivor its plain name back.
        app.editor.kill_buffer(b, None);
        app.editor.kill_buffer(c, None);
        app.run_command("uniquify-buffer-names");
        assert_eq!(name(&app, a), "main.rs");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uniquify_styles() {
        let dirs = vec!["a".to_string(), "b".to_string()];
        assert_eq!(styled("forward", "f.c", &dirs), "a/b/f.c");
        assert_eq!(styled("reverse", "f.c", &dirs), "f.c\\b\\a");
        assert_eq!(styled("post-forward", "f.c", &dirs), "f.c|a/b");
        assert_eq!(
            styled("post-forward-angle-brackets", "f.c", &dirs),
            "f.c<a/b>"
        );
    }

    #[test]
    fn global_auto_revert_follows_files_that_are_not_on_screen() {
        let dir = std::env::temp_dir().join(format!("forge-grevert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "old\n").unwrap();
        let mut app = app_with_options(
            "forge.opt[\"global_auto_revert_mode\"] = true;\nforge.opt[\"auto_revert_interval\"] = 0;\n",
        );
        let id = file_buffer(&mut app, &file); // in a buffer, shown in no window
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, "new text\n").unwrap();
        app.on_idle();
        assert_eq!(
            app.editor.state.get_buffer(id).unwrap().get_contents(),
            "new text\n"
        );
        // An edited buffer is left alone.
        {
            let b = app.editor.state.get_buffer_mut(id).unwrap();
            b.insert_str("mine ");
            assert!(b.is_modified());
        }
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, "theirs\n").unwrap();
        app.on_idle();
        assert!(
            app.editor
                .state
                .get_buffer(id)
                .unwrap()
                .get_contents()
                .starts_with("mine ")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn vc_follow_symlinks_opens_the_real_file() {
        let dir = std::env::temp_dir().join(format!("forge-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(dir.join("repo/conf.txt"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("repo/conf.txt"), dir.join("home/conf.txt")).unwrap();
        std::fs::write(dir.join("plain.txt"), "p").unwrap();
        std::os::unix::fs::symlink(dir.join("plain.txt"), dir.join("home/plain.txt")).unwrap();
        let mut app = app_with_options("forge.opt[\"vc_follow_symlinks\"] = true;\n");
        crate::minibuffer::visit_file(&mut app, &dir.join("home/conf.txt"), false);
        let real = std::fs::canonicalize(dir.join("repo/conf.txt")).unwrap();
        assert_eq!(app.buf_ref().path().map(|p| p.to_path_buf()), Some(real));
        // A link to something outside version control is visited as is.
        crate::minibuffer::visit_file(&mut app, &dir.join("home/plain.txt"), false);
        assert!(
            app.buf_ref()
                .path()
                .is_some_and(|p| p.ends_with("home/plain.txt") || p.ends_with("plain.txt"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn margin_options_keep_blank_columns_beside_the_text() {
        let mut app = app_with_options(
            "forge.opt[\"left_margin_width\"] = 4;\nforge.opt[\"right_margin_width\"] = 3;\n",
        );
        let opts = crate::ui::display_options(&app);
        assert_eq!((opts.left_margin, opts.right_margin), (4, 3));
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 6,
        };
        let plain = forge_core::display::redisplay(&mut app.editor, area, &Default::default());
        let with = forge_core::display::redisplay(&mut app.editor, area, &opts);
        let row = |v: &Vec<forge_core::display::WindowView>| -> String {
            v[0].rows[0].iter().map(|s| s.text.clone()).collect()
        };
        assert!(row(&plain).starts_with("one"), "{:?}", row(&plain));
        assert!(row(&with).starts_with("    one"), "{:?}", row(&with));
    }
}

#[cfg(test)]
mod config_dir_tests {
    use super::*;

    /// Loads the config in FORGE_TEST_CONFIG (a directory with init.rhai)
    /// and reports anything that went wrong. Skipped when it is not set.
    #[test]
    fn a_config_directory_loads_without_errors() {
        let Ok(dir) = std::env::var("FORGE_TEST_CONFIG") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        // SAFETY: only this test sets it.
        unsafe { std::env::set_var("FORGE_CONFIG_DIR", &dir) };
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(forge_core::Buffer::new("scratch"));
        app.editor.show_buffer(id);
        app.init_path = Some(dir.join("init.rhai"));
        app.load_init_file();
        let log = app
            .editor
            .find_buffer("*Messages*")
            .and_then(|id| app.editor.state.get_buffer(id))
            .map(|b| b.get_contents())
            .unwrap_or_default();
        assert!(!log.to_lowercase().contains("error"), "{log}");
        assert_eq!(app.editor.themes.current(), Some("catppuccin-frappe"));
        for cmd in [
            "vc-dir",
            "vc-push",
            "undo-redo",
            "switch-to-buffer",
            "yank-pop",
            "manual",
            "next-error",
            "previous-error",
            "winner-undo",
            "winner-redo",
            "windmove-left",
            "beginning-of-buffer",
        ] {
            assert!(
                app.editor.commands.get(cmd).is_some(),
                "{cmd} is not a command"
            );
        }
        for (key, want) in [
            ("winner_mode", true),
            ("global_auto_revert_mode", true),
            ("vc_follow_symlinks", true),
            ("delete_trailing_whitespace_on_save", true),
        ] {
            assert_eq!(crate::org_flow::setting_flag(&app, key), want, "{key}");
        }
        assert_eq!(
            crate::org_flow::setting_string(&app, "uniquify_buffer_name_style").as_deref(),
            Some("post-forward-angle-brackets")
        );
    }
}
