// SPDX-License-Identifier: MIT OR Apache-2.0
//! dired tests.

use super::*;

/// An app whose Dired listings use the eza layout these tests were written
/// for (the default is `ls -l`).
fn eza_app() -> App {
    app_with_init("forge.opt[\"dired_listing\"] = \"eza\";\n").0
}

#[test]
fn dired_marks_deletes_and_is_read_only() {
    let dir = std::env::temp_dir().join(format!("forge-dired-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    assert_eq!(app.buf_ref().major_mode(), "dired-mode");
    // Line 0 is the directory, line 1 the column headers, then the files.
    let col = forge_editor::dired::name_col(app.buf_ref()).unwrap();
    assert_eq!(point(&app), Pos::new(2, col));
    assert!(app.buf_ref().line(1).contains("Permissions"));
    assert!(app.buf_ref().decorations().iter().any(|d| {
        d.face
            == forge_core::decoration::DecoFace::Dired(forge_core::decoration::DiredFace::Directory)
    }));
    // m marks a.txt, d flags b.txt for deletion, x deletes it after a y.
    press(&mut app, "m d");
    assert!(app.buf_ref().line(2).starts_with("* "));
    assert!(app.buf_ref().line(3).starts_with("D "));
    press(&mut app, "x y");
    assert!(dir.join("a.txt").exists());
    assert!(!dir.join("b.txt").exists());
    assert!(
        app.buf_ref().line(2).starts_with("* "),
        "marks survive a revert"
    );
    // The buffer is read-only.
    let before = text(&app);
    press(&mut app, "z");
    assert_eq!(text(&app), before);
    // Point stayed on "sub"; RET opens it, ^ goes back to where we were.
    press(&mut app, "RET");
    assert!(app.buf_ref().line(0).ends_with("sub:"));
    press(&mut app, "^");
    assert!(app.buf_ref().line(point(&app).line).ends_with("sub"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_file_on_a_directory_opens_dired() {
    let dir = std::env::temp_dir().join(format!("forge-dired-ff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = eza_app();
    press(&mut app, "C-x C-f");
    type_path(&mut app, &dir);
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().major_mode(), "dired-mode");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vertical_find_file_can_open_a_directory_in_dired() {
    let dir = std::env::temp_dir().join(format!("forge-dired-vertical-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    let typed = format!("{}/", dir.display());

    // C-x C-f on "dir/": the first row is "./", and RET opens Dired there.
    let mut app = eza_app();
    app.editor.completion.vertical = Some((5, "test".into()));
    press(&mut app, "C-x C-f");
    app.minibuffer.as_mut().unwrap().input = typed.clone();
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().major_mode(), "dired-mode");
    assert!(app.buf_ref().line(0).contains("forge-dired-vertical"));

    // M-x dired: C-n RET on the directory candidate opens that one.
    let only_sub =
        std::env::temp_dir().join(format!("forge-dired-vertical2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&only_sub);
    std::fs::create_dir_all(only_sub.join("sub")).unwrap();
    let typed = format!("{}/", only_sub.display());
    let mut app = eza_app();
    app.editor.completion.vertical = Some((5, "test".into()));
    press(&mut app, "M-x dired RET");
    app.minibuffer.as_mut().unwrap().input = typed;
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().major_mode(), "dired-mode");
    assert!(
        app.buf_ref().line(0).contains("forge-dired-vertical2"),
        "{:?}",
        app.buf_ref().line(0)
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&only_sub);
}

#[test]
fn find_file_lists_short_names_with_metadata() {
    let dir = std::env::temp_dir().join(format!("forge-find-meta-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "hello").unwrap();
    std::fs::write(dir.join(".dotfile"), "d").unwrap();
    let current = dir.join("current.txt");
    std::fs::write(&current, "current").unwrap();

    // Visiting a file there, C-x C-f starts with its directory, traditional.
    let mut app = eza_app();
    let (id, _) = app.editor.open_file(&current).unwrap();
    app.switch_to(id);
    app.editor.completion.vertical = Some((10, "test".into()));
    press(&mut app, "C-x C-f");
    let input = app.minibuffer.as_ref().unwrap().input.clone();
    assert!(
        input.ends_with('/'),
        "the directory stays in the prompt: {input:?}"
    );
    assert!(
        input.contains(dir.file_name().unwrap().to_str().unwrap()),
        "the full path is visible: {input:?}"
    );

    // Short names, not full paths — with permissions, size and age.
    let (rows, _) = crate::minibuffer::vertical_view(&mut app).unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert!(names.contains(&"a.txt"), "{names:?}");
    assert!(names.contains(&"sub/"), "{names:?}");
    assert!(
        names.contains(&".dotfile"),
        "dotfiles are listed: {names:?}"
    );
    assert!(
        names.iter().all(|n| match n.find('/') {
            None => true,
            Some(i) => i + 1 == n.len(),
        }),
        "short names, at most a trailing slash: {names:?}"
    );
    for (name, note, _) in &rows {
        if name == "./" {
            continue;
        }
        assert!(
            note.contains("rwx") || note.contains("rw-") || note.contains('r'),
            "{name:?} carries permissions: {note:?}"
        );
        assert!(note.contains("ago"), "{name:?} carries an age: {note:?}");
    }
    press(&mut app, "C-g");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dired_s_toggles_the_sort_order() {
    let dir = std::env::temp_dir().join(format!("forge-dired-s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, secs) in [("a.txt", 1_000_000u64), ("b.txt", 3_000_000)] {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    assert!(app.buf_ref().line(2).ends_with("a.txt"));
    press(&mut app, "s");
    assert!(app.buf_ref().line(0).ends_with("[by date]"));
    assert!(app.buf_ref().line(2).ends_with("b.txt"));
    // g keeps the order; opening the directory again does too.
    press(&mut app, "g");
    assert!(app.buf_ref().line(2).ends_with("b.txt"));
    crate::dired::open(&mut app, &dir);
    assert!(app.buf_ref().line(2).ends_with("b.txt"));
    press(&mut app, "s");
    assert!(!app.buf_ref().line(0).contains("[by date]"));
    assert!(app.buf_ref().line(2).ends_with("a.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dired_lists_subdirectories_inline() {
    let dir = std::env::temp_dir().join(format!("forge-dired-inline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("file.txt"), "f").unwrap();
    std::fs::write(dir.join("sub").join("x.txt"), "x").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    // Lines: 0 dir, 1 columns, 2 file.txt, 3 sub.
    press(&mut app, "n");
    assert!(app.buf_ref().line(3).ends_with("sub"));
    // `i` inserts sub's listing below as an eza tree, in its own section,
    // and goes there (`T` draws everything; `i` draws just this one).
    press(&mut app, "i");
    assert_eq!(app.buf_ref().line(4), "");
    assert!(
        app.buf_ref().line(5).contains("sub:"),
        "{:?}",
        app.buf_ref().line(5)
    );
    assert!(
        app.buf_ref().line(5).contains("[tree]"),
        "the singular insert is a tree: {:?}",
        app.buf_ref().line(5)
    );
    assert!(app.buf_ref().line(6).contains("Permissions"));
    assert!(app.buf_ref().line(7).ends_with("x.txt"));
    assert!(
        app.buf_ref().line(7).contains("└──") || app.buf_ref().line(7).contains("├──"),
        "tree cells in a singular tree: {:?}",
        app.buf_ref().line(7)
    );
    assert_eq!(point(&app).line, 7);
    // Marks work in the subdirectory, and survive g along with the listing.
    press(&mut app, "m");
    assert!(app.buf_ref().line(7).starts_with("* "));
    press(&mut app, "g");
    assert!(app.buf_ref().line(7).starts_with("* "));
    // C-M-n/C-M-p move between sections.
    press(&mut app, "C-M-n");
    assert_eq!(point(&app).line, 7, "no next section, point stays");
    press(&mut app, "C-M-p");
    assert_eq!(point(&app).line, 2, "back in the top listing");
    // D deletes the marked file, which is in the subdirectory.
    press(&mut app, "D");
    assert!(app.minibuffer.is_some());
    press(&mut app, "y");
    assert!(!dir.join("sub").join("x.txt").exists());
    assert!(
        app.buf_ref().line(5).contains("sub:"),
        "the listing stays: {:?}",
        app.buf_ref().line(5)
    );
    // k removes the subdirectory's listing; the directory stays in its parent.
    press(&mut app, "C-M-n");
    press(&mut app, "k");
    let headers = (0..app.buf_ref().line_count())
        .filter(|&l| app.buf_ref().line(l).starts_with("  /"))
        .count();
    assert_eq!(headers, 1);
    assert!(app.buf_ref().line(point(&app).line).ends_with("sub"));
    // `i` on a plain file says so and changes nothing.
    press(&mut app, "C-M-p");
    let before = text(&app);
    press(&mut app, "i");
    assert_eq!(text(&app), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dired_a_g_i_and_c_switch_the_listing_over() {
    let dir = std::env::temp_dir().join(format!("forge-dired-flags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("zsub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join(".dotfile"), "d").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    // eza's defaults: no dotfiles, no inode column, no gradient.
    assert!(!text(&app).contains(".dotfile"));
    assert!(app.buf_ref().line(1).starts_with("  Permissions"));
    assert!(!app.buf_ref().line(1).contains("Inode"));
    // a shows the dotfiles, and says so in the header so g keeps them.
    press(&mut app, "C-c C-a");
    assert!(text(&app).contains(".dotfile"));
    assert!(app.buf_ref().line(0).ends_with("[all]"));
    press(&mut app, "g");
    assert!(text(&app).contains(".dotfile"));
    press(&mut app, "C-c C-a");
    assert!(!text(&app).contains(".dotfile"));
    // G pulls the directory to the top.
    assert!(app.buf_ref().line(2).ends_with(".txt") || app.buf_ref().line(2).ends_with("zsub"));
    press(&mut app, "C-c C-d");
    assert!(app.buf_ref().line(0).ends_with("[dirs first]"));
    assert!(
        app.buf_ref().line(2).ends_with("zsub"),
        "the directory comes first"
    );
    // I adds the inode column in front of the permissions.
    press(&mut app, "I");
    assert!(app.buf_ref().line(0).ends_with("[dirs first, inode]"));
    assert!(app.buf_ref().line(1).contains("Inode"));
    let head = app.buf_ref().line(1).clone();
    let (inode_col, perms_col) = (
        head.find("Inode").unwrap(),
        head.find("Permissions").unwrap(),
    );
    assert!(inode_col < perms_col, "{head:?}");
    // c shades sizes and dates by magnitude.
    press(&mut app, "C-c C-c");
    assert!(
        app.buf_ref()
            .line(0)
            .ends_with("[dirs first, inode, scale]")
    );
    let shaded = app.buf_ref().decorations().iter().any(|d| {
        matches!(
            d.face,
            forge_core::decoration::DecoFace::Dired(forge_core::decoration::DiredFace::Shade(_))
        )
    });
    assert!(shaded, "sizes and dates are shaded against each other");
    // Every part of the layout survives reopening the directory.
    crate::dired::open(&mut app, &dir);
    assert!(
        app.buf_ref()
            .line(0)
            .ends_with("[dirs first, inode, scale]")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dired_t_draws_a_tree_and_says_so_about_the_inline_listings() {
    let dir = std::env::temp_dir().join(format!("forge-dired-T-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
    std::fs::write(dir.join("top.txt"), "t").unwrap();
    std::fs::write(dir.join("sub/inner.txt"), "i").unwrap();
    std::fs::write(dir.join("sub/deep/bottom.txt"), "b").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    assert!(!text(&app).contains('└'));
    press(&mut app, "C-c C-t");
    assert!(app.buf_ref().line(0).ends_with("[tree]"));
    let name_col = app.buf_ref().line(1).find("Name").unwrap();
    let body: Vec<String> = (2..app.buf_ref().line_count())
        .map(|l| {
            app.buf_ref()
                .line(l)
                .chars()
                .skip(name_col)
                .collect::<String>()
        })
        .filter(|s: &String| !s.trim().is_empty())
        .collect();
    // With the Nerd Font icons taken out, the drawing is eza's. "sub" sorts
    // before "top.txt", so neither directory is last and their children hang
    // off a line rather than a blank.
    let clean: Vec<String> = body
        .iter()
        .map(|s| {
            s.chars()
                .filter(|c| !('\u{e000}'..='\u{f8ff}').contains(c))
                .collect()
        })
        .collect();
    // The two spaces are the one the cell ends with and the icon's own.
    assert_eq!(
        clean,
        [
            "├──  sub",
            "│   ├──  deep",
            "│   │   └──  bottom.txt",
            "│   └──  inner.txt",
            "└──  top.txt"
        ]
    );
    // The names under their directories are still real files: RET on one
    // opens it, which is the whole point of drawing the tree.
    let deep_line = (2..app.buf_ref().line_count())
        .find(|&l| app.buf_ref().line(l).ends_with("bottom.txt"))
        .unwrap();
    let name_col = app.buf_ref().line(1).find("Name").unwrap();
    app.buf().set_point(Pos::new(deep_line, name_col));
    press(&mut app, "RET");
    assert_eq!(
        app.buf_ref()
            .path()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned()),
        Some("bottom.txt".to_string())
    );
    // The inline subdirectory commands say why they can't help (`i`
    // inserts one as a tree, so there is nothing to insert when `T`
    // already draws everything). Going back to the listing keeps the
    // tree, which is the point of storing it there.
    crate::dired::open(&mut app, &dir);
    assert!(app.buf_ref().line(0).ends_with("[tree]"));
    press(&mut app, "n");
    press(&mut app, "i");
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("T turns it off"),
        "{:?}",
        app.message
    );
    let headers = (0..app.buf_ref().line_count())
        .filter(|&l| app.buf_ref().line(l).starts_with("  /"))
        .count();
    assert_eq!(headers, 1, "a tree is one listing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_name_in_a_dired_listing_opens_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = std::env::temp_dir().join(format!("forge-dired-click-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("clicked.txt"), "hello").unwrap();
    std::fs::write(dir.join("other.txt"), "hello").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    let ev = |kind, col, row| MouseEvent {
        kind,
        column: col as u16,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    };
    let click = |app: &mut App, line: usize, col: usize| {
        crate::mouse::handle(app, ev(MouseEventKind::Down(MouseButton::Left), col, line));
        crate::mouse::handle(app, ev(MouseEventKind::Up(MouseButton::Left), col, line));
    };
    let name_col = app.buf_ref().line(1).find("Name").unwrap();
    let row_of = |app: &App, name: &str| {
        (2..app.buf_ref().line_count())
            .find(|&l| app.buf_ref().line(l).ends_with(name))
            .unwrap()
    };
    let row = row_of(&app, "clicked.txt");
    click(&mut app, row, name_col + 2);
    assert_eq!(
        app.buf_ref()
            .path()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned()),
        Some("clicked.txt".to_string())
    );
    // A click on the permissions beside the name is not a link: it moves
    // point there and leaves the listing alone.
    crate::dired::open(&mut app, &dir);
    let row = row_of(&app, "other.txt");
    click(&mut app, row, 4);
    assert_eq!(
        app.buf_ref().point(),
        Pos::new(row, 4),
        "point moved to where the click was"
    );
    assert_eq!(
        app.buf_ref().major_mode(),
        "dired-mode",
        "and nothing opened"
    );
    assert!(
        app.buf_ref()
            .line(0)
            .contains(&dir.file_name().unwrap().to_string_lossy().into_owned())
    );
    // A drag out of a name is a selection, not a click.
    crate::dired::open(&mut app, &dir);
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), name_col, 2),
    );
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Drag(MouseButton::Left), name_col + 4, 3),
    );
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Up(MouseButton::Left), name_col + 4, 3),
    );
    assert_eq!(
        app.buf_ref().major_mode(),
        "dired-mode",
        "a drag doesn't open anything"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clicks_binding_is_a_key_a_user_can_take_over() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = std::env::temp_dir().join(format!("forge-click-rebind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hello").unwrap();
    let mut app = eza_app();
    crate::dired::open(&mut app, &dir);
    let ev = |kind, col, row| MouseEvent {
        kind,
        column: col as u16,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    };
    let name_col = app.buf_ref().line(1).find("Name").unwrap();
    let click = |app: &mut App, line: usize, col: usize, button| {
        crate::mouse::handle(app, ev(MouseEventKind::Down(button), col, line));
        crate::mouse::handle(app, ev(MouseEventKind::Up(button), col, line));
    };

    // <mouse-1> visits the name, as RET does; <mouse-2> in the other window.
    click(&mut app, 2, name_col + 1, MouseButton::Left);
    assert_eq!(
        app.buf_ref().path().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new("a.txt"))
    );
    // Rebinding it in the entry's region map takes it away: the click now
    // runs the user's command instead.
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Region(forge_editor::dired::ENTRY_REGION.into()),
            "<mouse-1>",
            "forward-char",
            forge_core::keymap::Origin::User,
        )
        .unwrap();
    crate::dired::open(&mut app, &dir);
    click(&mut app, 2, name_col + 1, MouseButton::Left);
    assert_eq!(app.buf_ref().major_mode(), "dired-mode", "nothing opened");
    assert_eq!(
        point(&app),
        Pos::new(2, name_col + 2),
        "point moved instead"
    );
    // <mouse-2> opens in the other window, so a split grows.
    click(&mut app, 2, name_col + 1, MouseButton::Middle);
    assert_eq!(app.editor.windows.count(), 2, "opened in a new window");
    let _ = std::fs::remove_dir_all(&dir);
}
