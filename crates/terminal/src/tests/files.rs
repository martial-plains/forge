// SPDX-License-Identifier: MIT OR Apache-2.0
//! files tests.

use super::*;

#[test]
fn backups_auto_save_revert_and_recover() {
    let dir = non_temp_dir("files-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "original\n").unwrap();
    let mut app = app_with("");
    let open = |app: &mut App| {
        press(app, "C-x C-f");
        type_path(app, &path);
        press(app, "RET");
    };
    open(&mut app);

    // Idle time auto-saves to #f.txt#; saving makes f.txt~ and removes #f.txt#.
    press(&mut app, "edited SPC");
    assert!(app.idle_timeout().is_some());
    // Not before the idle time is up, however often the loop wakes.
    app.on_idle();
    assert!(
        !dir.join("#f.txt#").exists(),
        "only after auto_save_idle of no input"
    );
    idle_past_auto_save(&mut app);
    app.on_idle();
    assert_eq!(
        std::fs::read_to_string(dir.join("#f.txt#")).unwrap(),
        "edited original\n"
    );
    press(&mut app, "C-x C-s");
    let _ = std::fs::remove_file(dir.join("src/a.c~"));
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt~")).unwrap(),
        "original\n"
    );
    assert!(!dir.join("#f.txt#").exists());

    // Another program rewrites the file: an unmodified buffer follows it.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, "changed elsewhere\n").unwrap();
    app.on_focus();
    assert_eq!(text(&app), "changed elsewhere\n");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("Reverted f.txt")
    );

    // A modified buffer isn't touched, and saving asks first.
    press(&mut app, "M-< mine SPC");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, "theirs\n").unwrap();
    app.on_focus();
    assert!(app.message.as_deref().unwrap().contains("changed on disk"));
    assert!(mode_lines(&mut app, 70, 6)[0].starts_with("-U:!!-"));
    press(&mut app, "C-x C-s");
    let _ = std::fs::remove_file(dir.join("src/a.c~"));
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .contains("save anyway?")
    );
    press(&mut app, "n M-x revert-buffer RET y");
    assert_eq!(text(&app), "theirs\n");
    press(&mut app, "C-/");
    assert_eq!(text(&app), "mine changed elsewhere\n", "revert is undoable");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[cfg(unix)]
fn a_locked_file_is_asked_about_at_the_first_change() {
    let dir = std::env::temp_dir().join(format!("forge-lock-prompt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("locked.txt");
    std::fs::write(&path, "text\n").unwrap();
    let lock_path = dir.join(".#locked.txt");
    // Another editor (say: pid 1 is alive) holds the lock.
    let theirs = format!("other@{}.1", forge_core::filelock::local_hostname());
    std::os::unix::fs::symlink(&theirs, &lock_path).unwrap();

    let mut app = app_with("");
    press(&mut app, "C-x C-f");
    type_path(&mut app, &path);
    press(&mut app, "RET");
    assert!(app.minibuffer.is_none(), "visiting asks nothing");

    // q: the change is given up.
    press(&mut app, "x");
    let prompt = app.minibuffer.as_ref().unwrap().prompt.clone();
    assert!(
        prompt.contains("locked by other@") && prompt.contains("(s, q, p, ?)"),
        "{prompt}"
    );
    press(&mut app, "q");
    assert_eq!(text(&app), "text\n");
    assert!(!app.buf_ref().is_modified());
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("File is locked")
    );

    // p: edit without the lock, which stays theirs.
    press(&mut app, "y");
    press(&mut app, "p");
    assert_eq!(text(&app), "ytext\n");
    assert_eq!(forge_core::filelock::read_lock(&lock_path).unwrap(), theirs);

    // Back to unmodified and changed again: asked again; s steals it.
    press(&mut app, "C-/");
    assert!(!app.buf_ref().is_modified());
    press(&mut app, "z");
    press(&mut app, "s");
    assert!(app.minibuffer.is_none());
    assert_eq!(
        forge_core::filelock::read_lock(&lock_path).unwrap(),
        forge_core::filelock::our_lock_text()
    );
    press(&mut app, "C-x C-s");
    assert!(std::fs::symlink_metadata(&lock_path).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn our_own_lock_comes_and_goes_with_modification() {
    let dir = std::env::temp_dir().join(format!("forge-lock-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mine.txt");
    std::fs::write(&path, "text\n").unwrap();
    let lock_path = dir.join(".#mine.txt");
    let mut app = app_with("");
    press(&mut app, "C-x C-f");
    type_path(&mut app, &path);
    press(&mut app, "RET");
    assert!(
        std::fs::symlink_metadata(&lock_path).is_err(),
        "no lock just for visiting"
    );
    press(&mut app, "a");
    assert!(
        std::fs::symlink_metadata(&lock_path).is_ok(),
        "locked at the first change"
    );
    press(&mut app, "C-/");
    assert!(
        std::fs::symlink_metadata(&lock_path).is_err(),
        "undone to the saved text: unlocked"
    );
    press(&mut app, "b");
    drop(app);
    assert!(
        std::fs::symlink_metadata(&lock_path).is_err(),
        "exiting unlocks"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn undo_restores_point_in_a_big_file() {
    let mut app = app_with(&"0123456789\n".repeat(100_000));
    press(&mut app, "M-> C-u 5 0 0 0 0 C-p hello C-/");
    assert_eq!(point(&app), Pos::new(50_000, 0));
    // C-/ took back the "hello" that was typed there.
    assert_eq!(text(&app).lines().nth(50_000), Some("0123456789"));
}

#[test]
fn recover_this_file_after_a_crash() {
    let dir = std::env::temp_dir().join(format!("forge-recover-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.txt");
    std::fs::write(&path, "on disk\n").unwrap();
    let open = |app: &mut App| {
        press(app, "C-x C-f");
        type_path(app, &path);
        press(app, "RET");
    };

    // Session one edits, auto-saves, then "crashes" (the app is dropped).
    let mut app = app_with("");
    open(&mut app);
    press(&mut app, "unsaved SPC");
    idle_past_auto_save(&mut app);
    app.on_idle();
    assert!(dir.join("#r.txt#").exists());
    drop(app);

    // Session two finds the newer auto-save and recovers it.
    let mut app = app_with("");
    open(&mut app);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("recover-this-file"),
        "{:?}",
        app.message
    );
    press(&mut app, "M-x recover-this-file RET y");
    assert_eq!(text(&app), "unsaved on disk\n");
    assert!(app.buf_ref().is_modified());
    assert!(dir.join("#r.txt#").exists(), "kept until saved, ");
    press(&mut app, "C-x C-s");
    assert!(!dir.join("#r.txt#").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ------------------------------------------------------------------- mouse
