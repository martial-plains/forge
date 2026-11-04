// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
#[cfg(unix)]
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
#[cfg(unix)]
fn terminal_char_mode_routes_only_documented_keys_to_forge() {
    let mut app = app_with("");
    crate::term::start(&mut app, "sh");

    assert!(!crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
    ));
    assert!(!crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
    assert!(!crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)
    ));
    assert!(crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT)
    ));
    assert!(crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
    ));

    press(&mut app, "C-x");
    assert!(!crate::term::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT)
    ));
    crate::term::execute(&mut app, "term-kill-subjob");
}

#[test]
fn terminal_key_diagnostic_lists_forge_and_program_keys() {
    let mut app = app_with("");
    press(&mut app, "M-x term-show-keys RET");
    assert_eq!(app.buf_ref().name(), "*term keys*");
    let diagnostic = text(&app);
    assert!(diagnostic.contains("C-x"));
    assert!(diagnostic.contains("C-c"));
    assert!(diagnostic.contains("M-x, M-:, M-!, M-&"));
    assert!(diagnostic.contains("other M- keys such as M-f"));
}
