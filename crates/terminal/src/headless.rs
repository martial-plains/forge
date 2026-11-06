// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forge --headless STEP...`: run the whole editor without a terminal,
//! press keys, and print the screen. For testing plugins that change how
//! the editor looks and behaves (vertical completion, popups, modal
//! editing) end to end, in CI or by hand:
//!
//! ```text
//! forge --headless --plugin vertico.wasm --keys "M-x" --type "buf kill" --screen
//! ```
//!
//! Steps run in order:
//!
//! - `--init FILE`: run FILE as init.rhai (nothing else of your
//!   configuration is read), so a configuration can be tried out:
//!   `--init init.rhai --plugin x.wasm` loads the plugin after it, as
//!   starting up does.
//! - `--plugin FILE`: load a plugin, granting what its manifest asks.
//! - `--open FILE`: visit a file.
//! - `--keys "C-x b"`: keys notation, as if typed.
//! - `--type TEXT`: type text, character by character.
//! - `--wait SECONDS`: let idle work happen (plugin events, processes,
//!   auto completion).
//! - `--test CMD`: find the plugin that causes CMD to fail, by disabling
//!   halves of the loaded plugins (the same search as M-x plugin-bisect).
//!   A plugin has a problem when CMD exits nonzero. CMD gets the enabled
//!   plugin names in FORGE_BISECT_ENABLED (comma-separated).
//! - `--screen`: print the screen (see `--size WxH`, default 80x24).
//!
//! At the end it prints the echo area and where point is. Nothing is read
//! from or saved to your configuration.

use crate::app::App;
use crate::{plugins, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

pub enum Step {
    Init(String),
    Plugin(String),
    Open(String),
    Keys(String),
    Type(String),
    Wait(f64),
    Test(String),
    Screen,
}

pub struct Options {
    pub steps: Vec<Step>,
    pub width: u16,
    pub height: u16,
}

/// Run the steps; false if a plugin didn't load or a file didn't open.
pub fn run(opts: Options) -> bool {
    let start = crate::StartOptions {
        no_config: true,
        files: Vec::new(),
        plugin_files: Vec::new(),
        graphical: false,
    };
    let mut app = match crate::start(&start) {
        Ok(a) => a,
        Err(e) => {
            println!("error: {e:#}");
            return false;
        }
    };
    app.message = None;
    // Compiled plugins are cached as usual, so repeated runs are quick.
    app.plugin_cache = forge_core::config::cache_dir();
    let mut ok = true;
    for step in opts.steps {
        match step {
            Step::Init(path) => {
                app.init_path = Some(std::path::PathBuf::from(&path));
                // Its hooks and commands wait for the plugins, as at startup:
                // they run after a --plugin step or a key.
                app.load_init_file();
            }
            Step::Plugin(path) => {
                ok &= load(&mut app, &path);
                app.run_mode_hooks();
            }
            Step::Open(f) if std::path::Path::new(&f).is_dir() => {
                crate::dired::open(&mut app, std::path::Path::new(&f));
            }
            Step::Open(f) => match app.editor.open_file(&f) {
                Ok((id, _)) => {
                    app.switch_to(id);
                    plugins::notice(&mut app, false);
                }
                Err(e) => {
                    println!("error: {f}: {e}");
                    ok = false;
                }
            },
            Step::Keys(k) => app.execute_keys(&k),
            Step::Type(t) => {
                for c in t.chars() {
                    let key = if c == '\n' {
                        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
                    } else {
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                    };
                    app.handle_key(key);
                }
            }
            Step::Wait(secs) => {
                let deadline = Instant::now() + Duration::from_secs_f64(secs);
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                    app.on_idle();
                }
            }
            Step::Test(command) => match crate::plugin_bisect::run_headless(&mut app, &command) {
                Ok(report) => {
                    use forge_core::plugin_bisect::BisectOutcome;
                    match report.outcome {
                        BisectOutcome::Culprit(name) => {
                            println!("plugin-bisect: culprit {name} ({} tests)", report.tests)
                        }
                        BisectOutcome::NoSingleCulprit => {
                            println!("plugin-bisect: no single culprit ({} tests)", report.tests)
                        }
                        BisectOutcome::NoProblem => {
                            println!(
                                "plugin-bisect: no problem with all plugins enabled ({} tests)",
                                report.tests
                            );
                            ok = false;
                        }
                    }
                }
                Err(error) => {
                    println!("error: plugin-bisect: {error}");
                    ok = false;
                }
            },
            Step::Screen => print_screen(&mut app, opts.width, opts.height),
        }
    }
    println!("echo: {}", forge_editor::ui::echo_line(&app).0);
    let b = app.buf_ref();
    let p = b.point();
    println!("buffer: {} at {}:{}", b.name(), p.line + 1, p.col);
    ok
}

fn load(app: &mut App, path: &str) -> bool {
    let file = match forge_core::PluginFile::read(std::path::Path::new(path)) {
        Ok(f) => f,
        Err(e) => {
            println!("error: {e:#}");
            return false;
        }
    };
    // It's your plugin: grant what it asks for (nothing is saved).
    let _ = app.grants.grant(&file.name, &file.requested());
    let name = file.name.clone();
    plugins::load(app, std::path::Path::new(path));
    let loaded = plugins::loaded_names(app).contains(&name);
    if !loaded {
        println!(
            "error: {name} didn't load: {}",
            app.message.as_deref().unwrap_or("")
        );
    }
    loaded
}

/// The screen as text, one line per row (trailing blanks trimmed).
pub fn screen_text(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    term.draw(|f| {
        ui::render(f, app);
    })
    .expect("drawing");
    let buf = term.backend().buffer();
    (0..height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < width {
                let cell = &buf[(x, y)];
                line.push_str(cell.symbol());
                x += (unicode_width::UnicodeWidthStr::width(cell.symbol()) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn print_screen(app: &mut App, width: u16, height: u16) {
    println!("--- screen ---");
    for line in screen_text(app, width, height) {
        println!("|{line}");
    }
    println!("--- end ---");
}
