// SPDX-License-Identifier: MIT OR Apache-2.0
//! How long a big file takes to edit and redraw.
//!
//! This used to be a test with wall-clock budgets, which flaked whenever
//! the test binary ran its cases in parallel: the numbers are real, but
//! how much of the machine one thread gets is not. Measuring is the point
//! here, so it lives in a bench, where a busy run reads as a slow number
//! rather than a failure.
//!
//!     cargo bench -p forge-terminal

use forge_core::Editor;
use forge_terminal::app::App;
use forge_terminal::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

/// A 100,000-line buffer, big enough that a redraw has to skip work.
const LINES: usize = 100_000;
/// Editing one page of a big file. The budgets these replaced were 3s and
/// 2s for this many iterations; they are far above what's normal here, and
/// are here to catch an accidental O(n^2), not to measure a regression.
const EDITS: usize = 20;
/// Same, for the redraw.
const DRAWS: usize = 20;

fn app_with_big_buffer() -> App {
    let mut editor = Editor::new();
    editor.create_buffer("*bench*");
    editor
        .state
        .active_buffer_mut()
        .unwrap()
        .set_contents(&"0123456789\n".repeat(LINES));
    App::new(editor)
}

/// Press keys written traditional: "C-x C-s", "M-f", "hello" types letters.
fn press(app: &mut App, keys: &str) {
    for k in keys.split(' ') {
        let (mods, rest) = if let Some(r) = k.strip_prefix("M-") {
            (KeyModifiers::ALT, r)
        } else if let Some(r) = k.strip_prefix("C-") {
            (KeyModifiers::CONTROL, r)
        } else {
            (KeyModifiers::NONE, k)
        };
        let code = match rest {
            "RET" => KeyCode::Enter,
            "ESC" => KeyCode::Esc,
            s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
            s if mods.is_empty() => {
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

/// Time `f`, reporting the mean per iteration.
fn bench(name: &str, iters: usize, mut f: impl FnMut()) {
    // One untimed pass, so the first run's page faults and allocation
    // growth don't land in the numbers.
    f();
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let each = start.elapsed() / iters as u32;
    println!("{name:<28} {:>9.3?} per iteration", each);
}

fn main() {
    let mut app = app_with_big_buffer();

    bench("jump in a big file", EDITS, || {
        press(&mut app, "M-> C-u 5 0 0 0 0 C-p hello C-/");
    });

    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    bench("redisplay a big file", DRAWS, || {
        press(&mut app, "C-v");
        term.draw(|f| {
            ui::render(f, &mut app);
        })
        .unwrap();
    });

    // A baseline to read the two above against: the same redraw with
    // nothing to scroll.
    let mut app = app_with_big_buffer();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    bench("redisplay a big file, still", DRAWS, || {
        term.draw(|f| {
            ui::render(f, &mut app);
        })
        .unwrap();
    });
}
