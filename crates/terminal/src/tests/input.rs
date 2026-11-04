// SPDX-License-Identifier: MIT OR Apache-2.0
//! input tests.

use super::*;

#[test]
fn c_x_8_types_characters() {
    let mut app = app_with("");
    press(
        &mut app,
        "C-x 8 ' e C-x 8 \" u C-x 8 1 / 2 C-x 8 a > C-x 8 _ m C-x 8 c",
    );
    assert_eq!(text(&app), "éü½→—©");
    // A prefix argument repeats it, as for a typed character.
    press(&mut app, "C-u 3 C-x 8 0");
    assert_eq!(text(&app), "éü½→—©°°°");
    press(&mut app, "C-x 8 ' q");
    assert_eq!(app.message.as_deref(), Some("C-x 8 ' q is undefined"));
    press(&mut app, "C-x 8 ' C-g");
    assert_eq!(app.message.as_deref(), Some("Quit"));
    assert_eq!(text(&app), "éü½→—©°°°");
    // A keyboard macro replays them.
    press(&mut app, "C-x ( C-x 8 ~ n C-x ) C-x e");
    assert_eq!(text(&app), "éü½→—©°°°ññ");
    // M-x iso-transl-set-language: German's shorter keys.
    press(
        &mut app,
        "M-x iso-transl-set-language RET German RET C-x 8 a",
    );
    assert!(text(&app).ends_with('ä'));
}

#[test]
fn c_x_8_works_in_the_minibuffer_and_isearch() {
    let mut app = app_with("un café");
    press(&mut app, "C-s C-x 8 ' e");
    assert_eq!(point(&app), Pos::new(0, 7));
    press(&mut app, "RET M-x C-x 8 ' e");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "é");
    // C-x 8 RET would need a minibuffer inside the minibuffer.
    press(&mut app, "C-x 8 RET");
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .contains("Command attempted to use minibuffer")
    );
    // C-x and another key is just that, in the minibuffer too.
    press(&mut app, "C-g");
    assert!(app.minibuffer.is_none());
    assert_eq!(text(&app), "un café");
}

#[test]
fn insert_char_by_name_and_code_and_emoji() {
    let mut app = app_with("");
    press(
        &mut app,
        "C-x 8 RET snowman RET C-x 8 RET e9 RET C-x 8 RET #x2192 RET",
    );
    assert_eq!(text(&app), "☃é→");
    press(
        &mut app,
        "C-u 2 C-x 8 RET LATIN SPC SMALL SPC LETTER SPC A SPC WITH SPC GRAVE RET",
    );
    assert_eq!(text(&app), "☃é→àà");
    press(&mut app, "C-x 8 RET nonsense RET");
    assert_eq!(app.message.as_deref(), Some("Invalid character: nonsense"));
    press(&mut app, "C-x 8 e s pizza RET");
    assert_eq!(text(&app), "☃é→àà🍕");
    press(&mut app, "C-x 8 e r pizza RET C-b C-x 8 e d");
    assert_eq!(text(&app), "☃é→àà🍕🍕");
    assert_eq!(
        app.message.as_deref(),
        Some("🍕  pizza (Food & Drink), U+1F355")
    );
    press(&mut app, "C-x 8 e e Food SPC & SPC Drink RET taco RET");
    assert!(text(&app).contains('🌮'));
    // *Emoji*: RET inserts the one at point where emoji-list came from.
    press(&mut app, "C-x 8 e l");
    assert_eq!(app.buf_ref().name(), "*Emoji*");
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().name(), "*scratch*");
    assert!(text(&app).contains('😀'));
}

#[test]
fn what_cursor_position_describe_char_and_describe_key() {
    let mut app = app_with("é\n");
    press(&mut app, "C-x =");
    assert_eq!(
        app.message.as_deref(),
        Some("Char: é (233, #o351, #xe9, file ...) point=1 of 2 (0%) column=0")
    );
    press(&mut app, "C-u C-x =");
    let help = help_text(&app);
    assert!(
        help.contains("name: LATIN SMALL LETTER E WITH ACUTE"),
        "{help}"
    );
    assert!(help.contains("or type \"C-x 8 ' e\""), "{help}");
    assert!(
        app.message.is_none(),
        "the C-u C-x- echo is gone: {:?}",
        app.message
    );
    press(&mut app, "q C-h k C-x 8 ' e");
    assert!(help_text(&app).starts_with("C-x 8 ' e is translated to é"));
    press(&mut app, "q C-h c C-x 8 RET");
    assert_eq!(
        app.message.as_deref(),
        Some("C-x 8 RET runs the command insert-char")
    );
    press(&mut app, "C-x 8 C-h");
    let help = help_text(&app);
    assert!(help.starts_with("Key translations starting with C-x 8:"));
    assert!(
        help.contains("C-x 8 ' e       é  LATIN SMALL LETTER E WITH ACUTE"),
        "{help}"
    );
    assert!(help.contains("C-x 8 RET       insert-char"));
}

#[test]
fn insert_char_completes_every_unicode_name_quickly() {
    let mut app = app_with("");
    app.editor.completion.vertical = Some((5, "test".into()));
    // Warm the one-time Unicode table (1.1M codepoints) outside the timing:
    // what must stay fast is filtering per key, which was once quadratic.
    press(&mut app, "C-x 8 RET");
    let _ = screen(&mut app, 80, 10);
    let start = std::time::Instant::now();
    press(&mut app, "snowm");
    let shown = screen(&mut app, 80, 10);
    // Each candidate shows the character and its code (marginalia-style).
    assert!(
        shown.contains("SNOWMAN") && shown.contains("☃  U+2603"),
        "{shown}"
    );
    press(&mut app, "DEL DEL");
    let _ = screen(&mut app, 80, 10);
    // Some 40,000 names: once a quadratic step per key, now well under a
    // second even unoptimized (10s leaves room for parallel debug runs).
    assert!(start.elapsed().as_secs() < 10, "took {:?}", start.elapsed());
    press(&mut app, "C-g");
}

// ------------------------------------------------------- input methods

#[test]
fn c_x_backslash_is_an_input_method_for_one_character() {
    let mut app = app_with("");
    press(&mut app, "C-x \\");
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .starts_with("Transient input method")
    );
    press(&mut app, "TeX RET \\alph");
    // The keys so far show, underlined, but aren't an edit yet.
    assert_eq!(text(&app), "\\alph");
    assert!(!app.buf_ref().is_modified());
    assert_eq!(im_title(&app).as_deref(), Some("\\"));
    press(&mut app, "a");
    // One character, and the input method is off again.
    assert_eq!(text(&app), "α");
    assert_eq!(im_title(&app), None);
    press(&mut app, "SPC \\to");
    assert_eq!(text(&app), "α \\to");
    // Next time it doesn't ask: C-x \ is TeX now (C-u asks again).
    press(&mut app, "SPC C-x \\ \\to SPC");
    assert_eq!(text(&app), "α \\to → ");
    press(&mut app, "C-u C-x \\");
    assert!(app.minibuffer.is_some());
    press(&mut app, "C-g");
}

#[test]
fn c_x_backslash_puts_back_the_input_method_that_was_on() {
    let mut app = app_with("");
    press(&mut app, "C-\\ latin-postfix RET cafe' SPC");
    assert_eq!(text(&app), "café ");
    assert_eq!(im_title(&app).as_deref(), Some("L<"));
    press(&mut app, "C-x \\ TeX RET \\pi SPC");
    assert_eq!(text(&app), "café π ");
    assert_eq!(
        im_title(&app).as_deref(),
        Some("L<"),
        "latin-postfix is back"
    );
    press(&mut app, "e' SPC C-\\ e' SPC");
    assert_eq!(text(&app), "café π é e' ");
    assert_eq!(im_title(&app), None);
}

#[test]
fn translating_keys_like_quail() {
    let mut app = app_with("");
    press(&mut app, "C-x RET C-\\ latin-postfix RET");
    // DEL takes back a key; a key that can't follow ends the translation
    // and is read again; RET ends it too, and makes its newline.
    press(&mut app, "nai DEL i\" ve RET");
    assert_eq!(text(&app), "naïve\n");
    // The one character C-x \ translates can be one it leaves alone (x):
    // that was its character.
    press(&mut app, "C-x \\ TeX RET x SPC");
    assert_eq!(text(&app), "naïve\nx ");
    // Several candidates: a digit picks (latin-prefix's '': ´ or ').
    press(&mut app, "C-x RET C-\\ latin-prefix RET ' '");
    assert!(
        app.message.as_deref().unwrap().contains("1.[´] 2.'"),
        "{:?}",
        app.message
    );
    press(&mut app, "2");
    assert!(text(&app).ends_with('\''));
    // Not in read-only buffers, and not after a prefix key.
    app.buf().set_read_only(true);
    press(&mut app, "~");
    assert!(app.im.translation.is_none(), "~ went past the input method");
    app.buf().set_read_only(false);
    press(&mut app, "C-x 8 ' e");
    assert!(text(&app).ends_with("'~é"), "{:?}", text(&app));
    assert!(app.im.translation.is_none());
}

#[test]
fn describe_and_list_input_methods() {
    let mut app = app_with("");
    press(&mut app, "C-h I latin-postfix RET");
    let help = help_text(&app);
    assert!(
        help.starts_with("Input method: latin-postfix (`L<' in mode line) for Latin"),
        "{help}"
    );
    assert!(help.contains("e'\t\té"), "{help}");
    press(&mut app, "q M-x list-input-methods RET");
    assert_eq!(app.buf_ref().name(), "*Input Methods*");
    assert!(text(&app).contains("  TeX (`\\' in mode line)"));
}

#[test]
fn tab_lists_quail_completions() {
    let mut app = app_with("");
    press(&mut app, "C-x RET C-\\ japanese-hiragana RET k TAB");
    // Listed in another window; typing goes on where it was.
    let list = quail_completions(&app);
    assert!(
        list.starts_with("Possible completion and corresponding characters are:\n k: -\n"),
        "{list}"
    );
    assert!(list.contains("   ka:(1/1) 1.か\n"), "{list}");
    assert_eq!(app.buf_ref().name(), "*scratch*");
    assert!(app.editor.windows.iter().any(|w| app.editor.state.get_buffer(w.buffer).unwrap().name() == "*Quail Completions*"));
    assert_eq!(text(&app), "k", "still translating");
    press(&mut app, "a");
    assert_eq!(text(&app), "か");
}

#[test]
fn a_click_in_quail_completions_chooses() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("");
    app.im.completion_everywhere = true;
    press(&mut app, "C-x RET C-\\ latin-prefix RET ' TAB");
    let shown = screen(&mut app, 60, 24);
    let (row, line) = shown
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("1.Á"))
        .expect("on screen");
    let col = line.chars().position(|c| c == 'Á').unwrap() as u16;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        crate::mouse::handle(
            &mut app,
            MouseEvent {
                kind,
                column: col,
                row: row as u16,
                modifiers: KeyModifiers::NONE,
            },
        );
    }
    assert!(app.im.translation.is_none());
    assert_eq!(app.buf_ref().name(), "*scratch*");
    assert_eq!(text(&app), "Á");
}

#[test]
fn tab_in_simple_methods_is_a_tab_unless_asked() {
    let mut app = app_with("");
    // TeX is a simple method: TAB ends the translation and is a TAB, as
    //.
    press(&mut app, "C-x RET C-\\ TeX RET \\al");
    press(&mut app, "TAB");
    let after_tab = text(&app);
    assert!(
        after_tab.len() > "\\al".len() && after_tab.starts_with("\\al"),
        "{after_tab:?}"
    );
    assert!(app.im.translation.is_none());
    assert!(
        app.editor
            .find_buffer(crate::input_method::COMPLETIONS)
            .is_none()
    );
    // forge.opt.input_method_completion_everywhere = true lists them.
    app.im.completion_everywhere = true;
    press(&mut app, "\\al TAB");
    assert!(quail_completions(&app).contains("       \\alpha:(1/1) 1.α\n"));
    press(&mut app, "pha");
    assert_eq!(text(&app), format!("{after_tab}α"));
}
