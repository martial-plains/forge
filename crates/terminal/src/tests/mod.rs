// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end tests: feed key presses through App exactly as the terminal would.

use crate::app::App;
use crate::ui;
use forge_core::{Editor, Pos};
use forge_core::{Origin, Scope};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

mod clipboard;
mod commands;
mod completion;
mod context_menu;
mod dired;
mod editing;
mod files;
mod hooks;
mod input;
mod keymap;
mod menu;
mod messages;
mod mouse;
mod packaging;
mod plugins;
mod rhai;
mod vc;
mod windows;

fn app_with(text: &str) -> App {
    let mut editor = Editor::new();
    editor.create_buffer("*scratch*");
    editor.state.active_buffer_mut().unwrap().set_contents(text);
    let mut app = App::new(editor);
    // Tests must not read or write the machine's clipboard: what is on it
    // would change what a yank pastes, and tests run side by side.
    app.set_clipboard_provider(Box::new(forge_editor::clipboard::InMemoryClipboard::new()));
    app
}

/// Press keys written traditional: "C-x C-s", "M-f", "hello" types letters,
/// "RET", "DEL", "ESC", "<left>".
fn press(app: &mut App, keys: &str) {
    for k in keys.split(' ') {
        let (mods, rest) = if let Some(r) = k.strip_prefix("C-M-") {
            (KeyModifiers::CONTROL | KeyModifiers::ALT, r)
        } else if let Some(r) = k.strip_prefix("C-") {
            (KeyModifiers::CONTROL, r)
        } else if let Some(r) = k.strip_prefix("M-") {
            (KeyModifiers::ALT, r)
        } else if let Some(r) = k.strip_prefix("S-") {
            (KeyModifiers::SHIFT, r)
        } else if let Some(r) = k.strip_prefix("H-") {
            (KeyModifiers::HYPER, r)
        } else {
            (KeyModifiers::NONE, k)
        };
        let code = match rest {
            "RET" => KeyCode::Enter,
            "DEL" => KeyCode::Backspace,
            "ESC" => KeyCode::Esc,
            "TAB" => KeyCode::Tab,
            "SPC" => KeyCode::Char(' '),
            "<left>" => KeyCode::Left,
            "<right>" => KeyCode::Right,
            "<up>" => KeyCode::Up,
            "<down>" => KeyCode::Down,
            s if s.starts_with("<f") && s.ends_with('>') => {
                KeyCode::F(s[2..s.len() - 1].parse().unwrap())
            }
            s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
            s if mods.is_empty() => {
                // A word: type it letter by letter.
                for c in s.chars() {
                    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                }
                continue;
            }
            s => panic!("can't press {s}"),
        };
        app.handle_key(KeyEvent::new(code, mods));
    }
}

fn text(app: &App) -> String {
    app.buf_ref().get_contents()
}

fn point(app: &App) -> Pos {
    app.buf_ref().point()
}

fn help_text(app: &App) -> String {
    assert_eq!(app.buf_ref().name(), "*Help*");
    text(app)
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        ui::render(f, app);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mode_lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
    screen(app, w, h)
        .lines()
        .filter(|l| l.starts_with("-U:"))
        .map(String::from)
        .collect()
}

/// A base directory for test files that is not under a temporary
/// directory (files there get no backups). The checkout's own
/// target/ is used unless the checkout itself sits in one, as in a sandbox
/// under /tmp; then a directory under $HOME or the current user's cache is.
fn non_temp_base() -> std::path::PathBuf {
    let temps = forge_core::backup::default_temporary_dirs();
    let ok = |p: &std::path::Path| {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        !temps.iter().any(|t| p.starts_with(t))
    };
    let mut cands =
        vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-files")];
    if let Some(h) = std::env::var_os("HOME") {
        cands.push(std::path::PathBuf::from(h).join(".cache/forge-test-files"));
    }
    cands.push(std::path::PathBuf::from("/var/tmp-forge-test-files"));
    for c in cands {
        if std::fs::create_dir_all(&c).is_ok() && ok(&c) {
            return c;
        }
    }
    panic!("no directory outside the temporary directories for test files");
}

/// A fresh directory outside $TMPDIR (whose files get no backups, as in
/// Forge), under target/.
fn non_temp_dir(name: &str) -> std::path::PathBuf {
    let d = non_temp_base().join(format!("forge-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// Pretend no key has been pressed for longer than the auto-save timeout.
fn idle_past_auto_save(app: &mut App) {
    app.last_input =
        std::time::Instant::now() - app.auto_save_idle - std::time::Duration::from_secs(1);
}

fn type_path(app: &mut App, path: &std::path::Path) {
    // C-x C-f starts with the buffer's directory (traditional); an
    // absolute path replaces it, as C-a C-k would.
    if let Some(mb) = app.minibuffer.as_mut() {
        mb.input.clear();
    }
    for c in path.to_str().unwrap().chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn messages_buffer(app: &App) -> String {
    let id = app
        .editor
        .find_buffer("*Messages*")
        .expect("*Messages* exists");
    app.editor.state.get_buffer(id).unwrap().get_contents()
}

/// Menu tests check a buffer list laid out for the buffers they make, and
/// *Messages* buffer (which the echo-area log creates when a
/// command says something) would shift every row after it.
fn without_messages(app: &mut App) {
    if let Some(id) = app.editor.find_buffer("*Messages*") {
        app.kill_buffer(id);
    }
}

fn rust_app(text: &str) -> App {
    let mut app = app_with(text);
    app.buf().set_major_mode("rust-mode");
    app
}

/// demo.wasm plus a manifest asking for network and a directory.
fn demo_with_manifest(dir: &std::path::Path, data_dir: &std::path::Path) -> std::path::PathBuf {
    let path = demo_plugin_file(dir);
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(
        dir.join("demo.toml"),
        format!(
            r#"description = "Greets people"
version = "1.2.0"
[permissions]
network = true
[[permissions.directories]]
path = "{}"
why = "greeting templates"
"#,
            data_dir.display()
        ),
    )
    .unwrap();
    path
}

/// Run idle work until no process is left (or 10 s).
fn finish_jobs(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.jobs.any_running() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.on_idle();
    }
    app.on_idle();
}

fn answer(app: &mut App, input: &str) {
    app.minibuffer.as_mut().expect("a prompt").input = input.to_string();
    press(app, "RET");
}

/// An app with a popup open on "alp": alpha, alpine, alphabet (by length).
fn popup_app(words: &str) -> App {
    let mut app = app_with(words);
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    press(&mut app, "M->");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    app
}

fn popup_names(app: &App) -> Vec<String> {
    crate::capf::popup(app)
        .map(|p| p.rows.into_iter().map(|r| r.text).collect())
        .unwrap_or_default()
}

fn popup_selected(app: &App) -> Option<String> {
    crate::capf::popup(app).and_then(|p| p.rows.into_iter().find(|r| r.selected).map(|r| r.text))
}

/// Set the popup's policy the way a completion plugin does
/// (completion.set-options).
fn set_policy(app: &mut App, f: impl FnOnce(&mut forge_core::completion::Policy)) {
    let mut p = app.editor.completion.policy();
    f(&mut p);
    app.editor.completion.policy = Some((p, "test".into()));
}

fn answer_typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with one commit of a.txt, on branch main.
fn temp_repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-vc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.name", "Tester"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-q", "-m", "First commit"]);
    dir
}

fn wait_vc(app: &mut App) {
    assert!(
        crate::vc::runner::wait_all(app, std::time::Duration::from_secs(30)),
        "VC commands still running"
    );
}

fn open_file(app: &mut App, path: &std::path::Path) {
    press(app, "C-x C-f");
    type_path(app, path);
    press(app, "RET");
}

fn buffer_text(app: &App, name: &str) -> String {
    let id = app
        .editor
        .find_buffer(name)
        .unwrap_or_else(|| panic!("no buffer {name}"));
    app.editor.state.get_buffer(id).unwrap().get_contents()
}

fn im_title(app: &App) -> Option<String> {
    app.buf_ref().input_method_title.clone()
}

fn quail_completions(app: &App) -> String {
    let id = app
        .editor
        .find_buffer(crate::input_method::COMPLETIONS)
        .expect("listed");
    app.editor.state.get_buffer(id).unwrap().get_contents()
}

/// An app running `init` as its init.rhai.
fn app_with_init(init: &str) -> (App, std::path::PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("forge-hooks-{}-{}", std::process::id(), init.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("init.rhai");
    std::fs::write(&path, init).unwrap();
    let mut app = app_with("");
    app.init_path = Some(path);
    app.load_init_file();
    (app, dir)
}

fn mouse_ev(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, column: u16, row: u16) {
    crate::mouse::handle(
        app,
        ratatui::crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );
}

fn right_click(app: &mut App, column: u16, row: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    mouse_ev(app, MouseEventKind::Down(MouseButton::Right), column, row);
    mouse_ev(app, MouseEventKind::Up(MouseButton::Right), column, row);
}

fn left_click(app: &mut App, column: u16, row: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    mouse_ev(app, MouseEventKind::Moved, column, row);
    mouse_ev(app, MouseEventKind::Down(MouseButton::Left), column, row);
    mouse_ev(app, MouseEventKind::Up(MouseButton::Left), column, row);
}

/// Build tests/fixtures/demo-plugin.wat into a component file.
pub(crate) fn demo_plugin_file(dir: &std::path::Path) -> std::path::PathBuf {
    use wit_component::{ComponentEncoder, StringEncoding};
    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/wit-0.1.0/forge-editor.wit"
        ))
        .unwrap();
    let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
    let mut module =
        wat::parse_str(include_str!("../../../../tests/fixtures/demo-plugin.wat")).unwrap();
    wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("demo.wasm");
    std::fs::write(&path, bytes).unwrap();
    path
}
