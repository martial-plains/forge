// SPDX-License-Identifier: MIT OR Apache-2.0
//! commands tests.

use super::*;

#[test]
fn prefix_arguments() {
    let mut app = app_with("abcdefghijklmnopqrstuvwxyz");
    press(&mut app, "C-u C-f");
    assert_eq!(point(&app), Pos::new(0, 4));
    press(&mut app, "C-u C-u C-f");
    assert_eq!(point(&app), Pos::new(0, 20));
    press(&mut app, "M-3 C-b");
    assert_eq!(point(&app), Pos::new(0, 17));
    press(&mut app, "C-u 1 2 C-b");
    assert_eq!(point(&app), Pos::new(0, 5));
    press(&mut app, "M-- C-f");
    assert_eq!(point(&app), Pos::new(0, 4), "M-- reverses");
    press(&mut app, "C-u - 2 C-f");
    assert_eq!(point(&app), Pos::new(0, 2));

    // The echo area shows the argument being typed; C-g drops it.
    press(&mut app, "C-u 5");
    assert_eq!(app.message.as_deref(), Some("C-u 5-"));
    press(&mut app, "C-g C-f");
    assert_eq!(point(&app), Pos::new(0, 3));

    // Self-insert repeats; C-u after digits ends the number (C-u 6 4 C-u 1).
    let mut app = app_with("");
    press(&mut app, "C-u 3 x RET C-u 1 2 C-u 1");
    assert_eq!(text(&app), format!("xxx\n{}", "1".repeat(12)));
    press(&mut app, "C-/");
    assert_eq!(text(&app), "xxx\n", "a repeated insert is one undo step");

    // M-x passes the argument on.
    let mut app = app_with("abcdefgh");
    press(&mut app, "C-u M-x forward-char RET");
    assert_eq!(point(&app), Pos::new(0, 4));
}

#[test]
fn prefix_arguments_for_kills_and_case() {
    let mut app = app_with("one\ntwo\nthree\nfour");
    press(&mut app, "C-f M-2 C-k");
    assert_eq!(text(&app), "othree\nfour");
    press(&mut app, "C-y");
    assert_eq!(text(&app), "one\ntwo\nthree\nfour");
    press(&mut app, "C-p C-e C-u 0 C-k");
    assert_eq!(text(&app), "one\n\nthree\nfour");

    let mut app = app_with("alpha beta gamma");
    press(&mut app, "M-2 M-u");
    assert_eq!(text(&app), "ALPHA BETA gamma");
    press(&mut app, "C-e M-- M-c");
    assert_eq!(text(&app), "ALPHA BETA Gamma");
    assert_eq!(
        point(&app),
        Pos::new(0, 16),
        "negative case commands don't move point"
    );

    // C-u C-y leaves point before the yanked text.
    press(&mut app, "C-a C-k C-u C-y");
    assert_eq!(point(&app), Pos::new(0, 0));
    assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 16)));
}

#[test]
fn grapheme_clusters_move_and_delete_as_one() {
    let mut app = app_with("a👍🏽e\u{301}z");
    press(&mut app, "C-f C-f");
    assert_eq!(point(&app), Pos::new(0, 3));
    press(&mut app, "C-f DEL");
    assert_eq!(text(&app), "a👍🏽z");
    press(&mut app, "C-b C-d");
    assert_eq!(text(&app), "az");
}

#[test]
fn keyboard_macros() {
    let mut app = app_with("a\nb\nc\nd");
    press(&mut app, "C-x (");
    assert!(mode_lines(&mut app, 70, 6)[0].contains("Def"));
    press(&mut app, "C-a > SPC C-n C-x )");
    assert_eq!(app.message.as_deref(), Some("Keyboard macro defined"));
    assert_eq!(text(&app), "> a\nb\nc\nd");
    press(&mut app, "C-x e");
    assert_eq!(text(&app), "> a\n> b\nc\nd");
    press(&mut app, "e");
    assert_eq!(text(&app), "> a\n> b\n> c\nd");

    // C-u 0 C-x e: until something fails (C-n on the last line).
    press(&mut app, "C-u 0 C-x e");
    assert_eq!(text(&app), "> a\n> b\n> c\n> d");

    // A macro with a search in it; <f3>/<f4> work too.
    let mut app = app_with("x=1 x=2 x=3");
    press(&mut app, "<f3> C-s x= RET X <f4>");
    press(&mut app, "<f4> <f4>");
    assert_eq!(text(&app), "x=X1 x=X2 x=X3");
    // C-g abandons a definition.
    press(&mut app, "C-x ( abc C-g");
    assert!(app.editor.kmacro.recording.is_none());
}

#[test]
fn query_replace_asks_about_each_match() {
    let mut app = app_with("foo foo Foo foo FOO");
    press(&mut app, "M-% foo RET bar RET");
    assert_eq!(app.highlight, Some((Pos::new(0, 0), Pos::new(0, 3))));
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("Query replacing foo with bar")
    );
    press(&mut app, "y n");
    assert_eq!(text(&app), "bar foo Foo foo FOO");
    press(&mut app, "!");
    assert_eq!(text(&app), "bar foo Bar bar BAR", "case follows the match");
    assert_eq!(app.message.as_deref(), Some("Replaced 4 occurrences"));
    assert_eq!(app.highlight, None);
    press(&mut app, "C-/");
    assert_eq!(text(&app), "bar foo Bar bar FOO");

    // RET at the FROM prompt reuses the last pair; "." replaces one and stops.
    press(&mut app, "M-< M-% RET .");
    assert_eq!(text(&app), "bar bar Bar bar FOO");
    press(&mut app, "M-< M-% foo RET x RET q");
    assert_eq!(app.message.as_deref(), Some("Replaced 0 occurrences"));
}

#[test]
fn regexp_replace_region_and_word_modes() {
    let mut app = app_with("x=1, y=22, z=333");
    press(&mut app, "C-M-% (\\w)=(\\d+) RET \\2:\\1 RET !");
    assert_eq!(text(&app), "1:x, 22:y, 333:z");

    // replace-string only touches the active region.
    let mut app = app_with("aa aa aa aa");
    press(
        &mut app,
        "M-f C-SPC M-f M-f M-x replace-string RET aa RET b RET",
    );
    assert_eq!(text(&app), "aa b b aa");

    // C-u M-%: whole words only.
    let mut app = app_with("cat catalog cat");
    press(&mut app, "C-u M-% cat RET dog RET !");
    assert_eq!(text(&app), "dog catalog dog");

    // M-% from inside isearch uses the search string.
    let mut app = app_with("one two one two");
    press(&mut app, "C-s two M-% 2 RET !");
    assert_eq!(text(&app), "one 2 one 2");

    // Bad regexps are reported, not panicked on.
    press(&mut app, "C-M-% ( RET x RET");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("Invalid regexp")
    );
}

#[test]
fn rectangles() {
    let mut app = app_with("abcdef\nabcdef\nabcdef");
    press(&mut app, "C-f C-SPC C-n C-n C-f C-f C-x r k");
    assert_eq!(text(&app), "adef\nadef\nadef");
    press(&mut app, "C-e C-x r y");
    assert_eq!(text(&app), "adefbc\nadefbc\nadefbc");

    let mut app = app_with("one\ntwo\nthree");
    press(&mut app, "C-SPC C-n C-n C-x r t >> SPC RET");
    assert_eq!(text(&app), ">> one\n>> two\n>> three");
    press(&mut app, "M-< C-SPC C-n C-n C-x r N");
    assert_eq!(text(&app), "1 >> one\n2 >> two\n3 >> three");
    press(&mut app, "M-< C-f C-f C-SPC C-n C-n C-f C-f C-f C-x r c");
    assert_eq!(text(&app), "1    one\n2    two\n3    three");
}

#[test]
fn registers() {
    let mut app = app_with("hello world");
    press(&mut app, "C-SPC M-f C-x r s a");
    assert_eq!(app.message.as_deref(), Some("Copied to register a"));
    press(&mut app, "C-e C-x r i a");
    assert_eq!(text(&app), "hello worldhello");
    assert_eq!(
        point(&app),
        Pos::new(0, 11),
        "point before the inserted text"
    );

    // A saved position follows edits made before it.
    press(&mut app, "C-a M-f C-x r SPC p M-< >>> SPC C-e C-x r j p");
    assert_eq!(point(&app), Pos::new(0, 9));

    // Numbers.
    press(&mut app, "C-u 4 1 C-x r n n C-u 1 C-x r + n C-e C-x r i n");
    assert!(text(&app).ends_with("hello42"));
    press(&mut app, "C-x r j a");
    assert_eq!(
        app.message.as_deref(),
        Some("Register a doesn't contain a position")
    );
    press(&mut app, "M-x list-registers RET");
    let t = text(&app);
    assert!(
        t.contains("a  text: hello") && t.contains("n  number 42"),
        "{t}"
    );
}

#[test]
fn fill_transpose_join_and_quoted_insert() {
    let mut app = app_with("// the quick brown fox jumps over the lazy dog\n\nnext");
    press(&mut app, "C-u 2 0 C-x f");
    assert_eq!(
        app.message.as_deref(),
        Some("Fill column set to 20 (was 70)")
    );
    press(&mut app, "M-q");
    assert_eq!(
        text(&app),
        "// the quick brown\n// fox jumps over\n// the lazy dog\n\nnext"
    );

    let mut app = app_with("one\ntwo\nthree");
    press(&mut app, "C-n C-x C-t");
    assert_eq!(text(&app), "two\none\nthree");
    assert_eq!(point(&app), Pos::new(2, 0));
    press(&mut app, "M-^");
    assert_eq!(text(&app), "two\none three");

    let mut app = app_with("");
    press(&mut app, "C-q C-l C-q TAB C-q 1 0 1 C-u 3 C-q C-a");
    assert_eq!(text(&app), "\u{c}\tA\u{1}\u{1}\u{1}");
}

#[test]
fn text_commands_and_their_keys() {
    // M-@ marks the next word, leaving point put.
    let mut app = app_with("one two three");
    press(&mut app, "M-@");
    assert_eq!(point(&app), Pos::new(0, 0));
    assert_eq!(text(&app), "one two three");
    press(&mut app, "M-w");
    assert_eq!(app.editor.kill_ring.current(), Some("one"));

    // M-t transposes the word before point with the next one.
    let mut app = app_with("one two three");
    press(&mut app, "C-f C-f C-f C-f");
    press(&mut app, "M-t");
    assert_eq!(text(&app), "two one three");

    // M-a / M-e move by sentence; M-k kills one.
    let mut app = app_with("One two. Three four");
    press(&mut app, "M-e");
    assert_eq!(point(&app), Pos::new(0, 8));
    press(&mut app, "M-e");
    assert_eq!(point(&app), Pos::new(0, 19));
    press(&mut app, "M-a");
    assert_eq!(point(&app), Pos::new(0, 9));
    press(&mut app, "C-a M-k");
    assert_eq!(text(&app), " Three four");

    // M-{ / M-} move by paragraph; M-h marks one.
    let mut app = app_with("one\n\ntwo\n\nthree");
    press(&mut app, "C-n C-n M-{");
    assert_eq!(point(&app), Pos::new(0, 0));
    press(&mut app, "M-}");
    assert_eq!(point(&app), Pos::new(1, 0), "M-}} stops at the blank line");
    press(&mut app, "M-h");
    assert_eq!(
        app.buf_ref().region(),
        Some((Pos::new(1, 0), Pos::new(3, 0)))
    );

    // C-x [ / C-x ] move by page; C-x C-p marks one; C-x l counts it.
    let mut app = app_with("a\nb\n\u{c}c\nd");
    press(&mut app, "C-x ]");
    assert_eq!(point(&app), Pos::new(2, 0));
    press(&mut app, "C-x [");
    assert_eq!(point(&app), Pos::new(0, 0));
    press(&mut app, "C-x C-p");
    assert_eq!(
        app.buf_ref().region(),
        Some((Pos::new(0, 0), Pos::new(2, 0)))
    );
    press(&mut app, "C-x l");
    assert_eq!(app.message.as_deref(), Some("Page has 2 lines (2 + 2 + 0)"));
}

#[test]
fn sentence_end_double_space_changes_sentence_motion() {
    // By default a single space ends a sentence.
    let mut app = app_with("One two. Three");
    press(&mut app, "M-e");
    assert_eq!(point(&app), Pos::new(0, 8));
    // With the option on, it takes two spaces or a line break.
    let mut app = app_with("One two.  Three");
    app.sentence_end.double_space = true;
    press(&mut app, "M-e");
    assert_eq!(point(&app), Pos::new(0, 8));
    press(&mut app, "M-a");
    assert_eq!(point(&app), Pos::new(0, 0));
}

#[test]
fn text_delimiter_options_change_the_boundaries() {
    // page-delimiter: a line of dashes separates pages instead of a formfeed.
    let mut app = app_with("a\n---\nb");
    app.page_delimiter = "^---".to_string();
    press(&mut app, "C-x ]");
    assert_eq!(point(&app), Pos::new(1, 0));
    press(&mut app, "C-x [");
    assert_eq!(point(&app), Pos::new(0, 0));

    // paragraph-start: only lines starting with a bullet start a paragraph.
    let mut app = app_with("* one\ncontinued\n\n* two\nmore");
    app.paragraph_start = "^\\*".to_string();
    press(&mut app, "C-n");
    press(&mut app, "M-{");
    assert_eq!(point(&app), Pos::new(0, 0));
    press(&mut app, "M-}");
    assert_eq!(point(&app), Pos::new(2, 0));
    press(&mut app, "M-h");
    assert_eq!(
        app.buf_ref().region(),
        Some((Pos::new(2, 0), Pos::new(4, 0)))
    );

    // sentence-end: a regexp of your own decides the boundary.
    let mut app = app_with("One two. Three");
    app.sentence_end_pattern = "[.?!]+[\"')]*\r?\n?".to_string();
    press(&mut app, "M-e");
    assert_eq!(point(&app), Pos::new(0, 8));
}

#[test]
fn image_mode_shows_and_transforms_the_picture() {
    use image::ImageEncoder;
    let dir = std::env::temp_dir().join(format!("forge-image-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    let img = image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 200, 30, 255]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), 40, 20, image::ExtendedColorType::Rgba8)
        .unwrap();
    std::fs::write(&png, &buf).unwrap();

    let mut app = app_with("");
    let (id, _) = app.editor.open_file(&png).unwrap();
    app.switch_to(id);
    // Opening an image file selects Image mode and renders the picture.
    assert_eq!(app.buf_ref().major_mode(), "image-mode");
    assert!(
        app.buf_ref()
            .image
            .as_ref()
            .is_some_and(|s| s.rendered.is_some())
    );
    // The window lays out with the picture and no text rows.
    screen(&mut app, 40, 10);

    // C-c C-c shows the raw text.
    press(&mut app, "C-c C-c");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().display,
        forge_core::buffer::ImageDisplay::Text
    );
    // C-c C-x shows the hex.
    press(&mut app, "C-c C-x");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().display,
        forge_core::buffer::ImageDisplay::Hex
    );
    assert!(app.buf_ref().get_contents().contains("0a"));
    press(&mut app, "C-c C-x");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().display,
        forge_core::buffer::ImageDisplay::Image
    );

    // s s 2 scales by a factor of 2 (200%).
    press(&mut app, "C-u 2 s s");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().transform.percent,
        200.0
    );
    press(&mut app, "s p");
    assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "Percentage: ");
    press(&mut app, "50 RET");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().transform.percent,
        50.0
    );
    press(&mut app, "s 0");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().transform,
        forge_core::buffer::ImageTransform::default()
    );
    press(&mut app, "i r");
    assert_eq!(app.buf_ref().image.as_ref().unwrap().transform.rotation, 90);
    // i h flips horizontally, i v vertically.
    press(&mut app, "i h");
    assert!(app.buf_ref().image.as_ref().unwrap().transform.flip_h);
    press(&mut app, "i v");
    assert!(app.buf_ref().image.as_ref().unwrap().transform.flip_v);
    // i + grows by 20% and i - shrinks back.
    press(&mut app, "i +");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().transform.percent,
        120.0
    );
    // 120% decreased by 20% is 96%.
    press(&mut app, "i -");
    assert_eq!(
        app.buf_ref().image.as_ref().unwrap().transform.percent,
        96.0
    );

    // i o prompts for a file name and saves the rendered picture.
    let saved = std::env::temp_dir().join(format!("forge-image-saved-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&saved);
    press(&mut app, "i o");
    assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "Save image as: ");
    press(&mut app, &saved.display().to_string());
    press(&mut app, "RET");
    assert!(saved.exists(), "the picture was saved");

    // n visits the next image file in the directory (itself, alone here).
    let other = dir.join("other.png");
    std::fs::write(&other, &buf).unwrap();
    press(&mut app, "n");
    assert_eq!(
        app.buf_ref().path().and_then(|p| p.file_name()),
        other.file_name()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_column_indicator_draws_the_chosen_column_and_character() {
    let mut app = app_with("abcdefghijkl\nshort\n");
    // C-x f asks now: answer it.
    press(&mut app, "C-x f");
    assert_eq!(
        app.minibuffer.as_ref().unwrap().prompt,
        "Change fill-column from 70 to: "
    );
    press(&mut app, "5 RET");
    assert_eq!(app.buf_ref().fill_column, 5);

    app.buf()
        .set_option("display-fill-column-indicator-character", "|")
        .unwrap();
    press(&mut app, "M-x display-fill-column-indicator-mode RET");
    let s = screen(&mut app, 20, 5);
    let rows: Vec<&str> = s.lines().collect();
    assert_eq!(&rows[0][5..6], "f", "text at the column hides the line");
    assert_eq!(&rows[1][5..6], "|", "the chosen character is drawn");
}
