// SPDX-License-Identifier: MIT OR Apache-2.0
//! The watchdog thread: keeps the editor responsive while a plugin or
//! init.rhai runs. Driven by [`forge_editor::watch::Watch`], which is the
//! state it shares with the main loop.

use forge_editor::watch::Watch;
use ratatui::crossterm::{cursor, queue, style, terminal};
use std::io::Write;
use std::time::Duration;

/// How long a call runs before the busy message appears.
pub const SHOW_AFTER: Duration = Duration::from_millis(150);
const CHECK_EVERY: Duration = Duration::from_millis(25);

/// Start the watchdog thread (only with a real terminal).
pub fn spawn(watch: Watch) {
    std::thread::Builder::new()
        .name("forge-watchdog".into())
        .spawn(move || {
            let mut shown = false;
            loop {
                std::thread::sleep(CHECK_EVERY);
                match watch.busy() {
                    Some((name, d)) if d >= SHOW_AFTER => {
                        if !shown {
                            draw_busy(&name);
                            watch.mark_dirty();
                            shown = true;
                        }
                    }
                    Some(_) => {}
                    None => shown = false,
                }
            }
        })
        .expect("spawn the watchdog");
}

fn draw_busy(name: &str) {
    let Ok((w, h)) = terminal::size() else { return };
    let text: String = format!("{name} is busy... (C-g to stop)")
        .chars()
        .take(w as usize)
        .collect();
    let mut out = std::io::stdout().lock();
    let _ = queue!(
        out,
        cursor::SavePosition,
        cursor::MoveTo(0, h.saturating_sub(1)),
        terminal::Clear(terminal::ClearType::CurrentLine),
        style::Print(text),
        cursor::RestorePosition
    );
    let _ = out.flush();
}
