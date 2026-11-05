// SPDX-License-Identifier: MIT OR Apache-2.0
//! vc tests.

use super::*;

#[test]
fn vc_mode_line_diff_commit_log_and_annotate() {
    let dir = temp_repo("flow");
    let mut app = app_with("");
    open_file(&mut app, &dir.join("a.txt"));
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Git-main"));
    assert!(mode_lines(&mut app, 90, 6)[0].contains("Git-main"));

    press(&mut app, "M-> four RET C-x C-s");
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Git:main"));

    // C-x v = shows the change.
    press(&mut app, "C-x v =");
    wait_vc(&mut app);
    let diff = buffer_text(&app, "*vc-diff*");
    assert!(diff.contains("+four"), "{diff}");

    // C-x v v opens the commit buffer; C-c C-c commits.
    let file_buf = app.editor.find_buffer_by_path(&dir.join("a.txt")).unwrap();
    app.editor.show_buffer(file_buf);
    press(&mut app, "C-x v v");
    wait_vc(&mut app);
    assert_eq!(app.buf_ref().name(), "*vc-log*");
    assert!(
        app.buf_ref()
            .get_contents()
            .contains("# Committing 1 file to main")
    );
    press(&mut app, "Add SPC four");
    press(&mut app, "C-c C-c");
    wait_vc(&mut app);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Committed: Add four"),
        "{:?}",
        app.message
    );
    assert!(git(&dir, &["log", "-1", "--format=%s"]).trim() == "Add four");
    assert!(app.editor.find_buffer("*vc-log*").is_none());
    let b = app.editor.state.get_buffer(file_buf).unwrap();
    assert_eq!(b.vc_mode.as_deref(), Some("Git-main"));

    // The summary isn't part of the commit, and the message is in the history.
    let msg = git(&dir, &["log", "-1", "--format=%B"]);
    assert!(!msg.contains("Committing"), "{msg}");

    // C-x v l: the log, newest first.
    app.editor.show_buffer(file_buf);
    press(&mut app, "C-x v l");
    wait_vc(&mut app);
    let log = buffer_text(&app, "*vc-change-log*");
    assert!(
        log.contains("Add four") && log.contains("First commit"),
        "{log}"
    );
    // d on the newest entry diffs it with its parent.
    let log_id = app.editor.find_buffer("*vc-change-log*").unwrap();
    app.editor.show_buffer(log_id);
    app.buf().set_point(Pos::new(0, 0));
    press(&mut app, "d");
    wait_vc(&mut app);
    assert!(buffer_text(&app, "*vc-diff*").contains("+four"));

    // C-x v g annotates.
    app.editor.show_buffer(file_buf);
    press(&mut app, "C-x v g");
    wait_vc(&mut app);
    let ann = buffer_text(&app, "*Annotate a.txt*");
    assert!(ann.lines().count() == 4 && ann.contains("Tester"), "{ann}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vc_register_revert_dir_and_branches() {
    let dir = temp_repo("ops");
    let mut app = app_with("");
    std::fs::write(dir.join("new.txt"), "hello\n").unwrap();
    open_file(&mut app, &dir.join("new.txt"));
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Git?main"));
    // C-x v v on an unregistered file registers it.
    press(&mut app, "C-x v v");
    wait_vc(&mut app);
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Git@main"));

    // Revert a change: the diff is shown, then y discards it.
    open_file(&mut app, &dir.join("a.txt"));
    press(&mut app, "M-< changed SPC C-x C-s");
    press(&mut app, "C-x v u");
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .contains("Discard changes")
    );
    press(&mut app, "y");
    wait_vc(&mut app);
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        if cfg!(windows) {
            "one\r\ntwo\r\nthree\r\n"
        } else {
            "one\ntwo\nthree\n"
        }
    );
    assert_eq!(
        text(&app),
        "one\ntwo\nthree\n",
        "the buffer follows the file"
    );

    // vc-dir lists the added file.
    press(&mut app, "C-x v d RET");
    wait_vc(&mut app);
    let d = app.buf_ref().get_contents();
    assert!(
        d.contains("VC backend  : Git") && d.contains("Branch      : main"),
        "buffer {} message {:?}:\n{d}",
        app.buf_ref().name(),
        app.message
    );
    assert!(
        d.contains("Added (1)") && d.contains("new.txt"),
        "buffer {} message {:?}:\n{d}",
        app.buf_ref().name(),
        app.message
    );

    // Branches: make one, and the mode line follows.
    press(&mut app, "C-x v b c topic RET");
    wait_vc(&mut app);
    assert_eq!(git(&dir, &["branch", "--show-current"]).trim(), "topic");
    let a = app.editor.find_buffer_by_path(&dir.join("a.txt")).unwrap();
    assert_eq!(
        app.editor.state.get_buffer(a).unwrap().vc_mode.as_deref(),
        Some("Git-topic")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vc_merge_conflicts_resolve_with_smerge() {
    let dir = temp_repo("merge");
    git(&dir, &["checkout", "-q", "-b", "other"]);
    std::fs::write(dir.join("a.txt"), "one\nTHEIRS\nthree\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "theirs"]);
    git(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("a.txt"), "one\nMINE\nthree\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "mine"]);
    let mut app = app_with("");
    open_file(&mut app, &dir.join("a.txt"));
    press(&mut app, "C-x v m other RET");
    wait_vc(&mut app);
    // The file now has markers; visiting it again turns smerge-mode on.
    let id = app.editor.find_buffer_by_path(&dir.join("a.txt")).unwrap();
    app.editor.show_buffer(id);
    assert!(text(&app).contains("<<<<<<<"), "{}", text(&app));
    assert!(app.buf_ref().minor_mode_enabled("smerge-mode"));
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Git!main"));
    press(&mut app, "C-c ^ n C-c ^ l");
    assert_eq!(text(&app), "one\nTHEIRS\nthree\n");
    assert!(!app.buf_ref().minor_mode_enabled("smerge-mode"));
    press(&mut app, "C-x C-s");
    wait_vc(&mut app);
    assert!(!git(&dir, &["status", "--porcelain"]).contains("UU"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vc_works_with_mercurial_too() {
    if std::process::Command::new("hg")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("forge-vc-hg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let hg = |args: &[&str]| {
        let o = std::process::Command::new("hg")
            .args(args)
            .current_dir(&dir)
            .env("HGUSER", "T <t@x>")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };
    hg(&["init"]);
    std::fs::write(dir.join("f.txt"), "a\n").unwrap();
    hg(&["add", "f.txt"]);
    hg(&["commit", "-m", "first"]);
    let mut app = app_with("");
    open_file(&mut app, &dir.join("f.txt"));
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Hg-default"));
    press(&mut app, "M-> b C-x C-s");
    assert_eq!(app.buf_ref().vc_mode.as_deref(), Some("Hg:default"));
    press(&mut app, "C-x v =");
    wait_vc(&mut app);
    assert!(buffer_text(&app, "*vc-diff*").contains("+b"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn numbered_backups_and_diff_backup() {
    let dir = non_temp_dir("numbered");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("n.txt");
    std::fs::write(&path, "v0\n").unwrap();
    // Seed a numbered backup so the default `Existing` policy uses numbered
    // names. Setting the process-global backup config to `Always` here raced
    // with other tests' `load_init_file` (which resets it to the default),
    // flaking under parallel runs.
    std::fs::write(dir.join("n.txt.~1~"), "older\n").unwrap();
    let mut app = app_with("");
    open_file(&mut app, &path);
    press(&mut app, "M-> v1 C-x C-s");
    assert_eq!(
        std::fs::read_to_string(dir.join("n.txt.~2~")).unwrap(),
        "v0\n"
    );
    press(&mut app, "M-x diff-backup RET");
    assert!(buffer_text(&app, "*Diff*").contains("+v1"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn version_controlled_files_get_no_backups() {
    let dir = non_temp_dir("vc-backup");
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("tracked.txt"), "v0\n").unwrap();
    std::fs::write(dir.join("loose.txt"), "v0\n").unwrap();
    git(&dir, &["add", "tracked.txt"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@x",
            "commit",
            "-qm",
            "c",
        ],
    );
    let mut app = app_with("");
    open_file(&mut app, &dir.join("tracked.txt"));
    press(&mut app, "x C-x C-s");
    assert!(
        !dir.join("tracked.txt~").exists(),
        "the repository is its backup"
    );
    open_file(&mut app, &dir.join("loose.txt"));
    press(&mut app, "x C-x C-s");
    assert!(
        dir.join("loose.txt~").exists(),
        "an unregistered file is backed up"
    );
    // A file in $TMPDIR is not.
    let tmp = std::env::temp_dir().join(format!("forge-tmp-backup-{}.txt", std::process::id()));
    std::fs::write(&tmp, "v0\n").unwrap();
    open_file(&mut app, &tmp);
    press(&mut app, "x C-x C-s");
    let mut b = tmp.clone().into_os_string();
    b.push("~");
    assert!(
        !std::path::Path::new(&b).exists(),
        "temporary files get no backups"
    );
    let _ = std::fs::remove_file(&tmp);
    std::fs::remove_dir_all(dir).unwrap();
}

// ------------------------------------------------------------- C-x 8
