// SPDX-License-Identifier: MIT OR Apache-2.0
//! packaging tests.

use super::*;

#[test]
fn compile_next_error_shell_commands_and_projects() {
    let dir = std::env::temp_dir().join(format!("forge-compile-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".project"), "").unwrap();
    std::fs::write(dir.join("src/a.c"), "int x;\nint y = needle;\n").unwrap();
    std::fs::write(dir.join("src/b.c"), "int z;\n").unwrap();
    let mut app = app_with("");
    let (id, _) = app.editor.open_file(dir.join("src/a.c")).unwrap();
    app.switch_to(id);

    // M-x compile: runs in the project root, output streams in.
    press(&mut app, "M-x compile RET");
    let command = if cfg!(windows) {
        "echo src/a.c:2:9: error: undeclared & echo src/b.c:1:5: error: second & exit /b 2"
    } else {
        "printf 'src/a.c:2:9: error: undeclared\\n  --> src/b.c:1:5\\n'; exit 2"
    };
    answer(&mut app, command);
    finish_jobs(&mut app);
    let out = app
        .editor
        .state
        .get_buffer(app.editor.find_buffer("*compilation*").unwrap())
        .unwrap()
        .get_contents();
    assert!(
        out.contains("src/a.c:2:9: error") && out.contains("exited abnormally with code 2"),
        "{out}"
    );
    assert_eq!(app.editor.windows.count(), 2, "shown in another window");
    assert_eq!(app.buf_ref().name(), "a.c", "the selected window stays");

    press(&mut app, "M-g n");
    assert_eq!((app.buf_ref().name(), point(&app)), ("a.c", Pos::new(1, 8)));
    assert_eq!(
        app.message.as_deref(),
        Some("src/a.c:2:9: error: undeclared")
    );
    press(&mut app, "M-g n");
    assert_eq!((app.buf_ref().name(), point(&app)), ("b.c", Pos::new(0, 4)));
    press(&mut app, "M-g n");
    assert_eq!(app.message.as_deref(), Some("No more locations"));
    press(&mut app, "M-g p");
    assert_eq!(app.buf_ref().name(), "a.c");

    // M-!: one line to the echo area; C-u M-! inserts.
    press(&mut app, "M-!");
    answer(&mut app, "echo hi there");
    finish_jobs(&mut app);
    assert_eq!(app.message.as_deref(), Some("hi there"));
    press(&mut app, "M-< C-u M-!");
    let command = if cfg!(windows) {
        "echo // top"
    } else {
        "printf '// top\\n'"
    };
    answer(&mut app, command);
    finish_jobs(&mut app);
    assert!(text(&app).starts_with("// top\nint x;"), "{}", text(&app));
    press(&mut app, "C-x C-s");
    let _ = std::fs::remove_file(dir.join("src/a.c~"));

    // Project files and grep.
    press(&mut app, "C-x p f b . TAB RET");
    assert_eq!(app.buf_ref().name(), "b.c");
    press(&mut app, "C-x p g");
    answer(&mut app, "needle");
    finish_jobs(&mut app);
    let grep = app
        .editor
        .state
        .get_buffer(app.editor.find_buffer("*grep*").unwrap())
        .unwrap()
        .get_contents();
    assert!(
        grep.replace('\\', "/")
            .contains("src/a.c:3:int y = needle;"),
        "{grep}"
    );
    press(&mut app, "M-g n");
    assert_eq!((app.buf_ref().name(), point(&app).line), ("a.c", 2));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn packages_install_list_and_delete() {
    let dir = std::env::temp_dir().join(format!("forge-pkg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let wasm = demo_plugin_file(&dir.join("repo"));
    let entry = forge_core::package::entry_for(&wasm, None).unwrap();
    assert!(
        entry.contains("name = \"demo\"") && entry.contains("url = \"demo.wasm\""),
        "{entry}"
    );
    std::fs::write(dir.join("repo/index.toml"), &entry).unwrap();
    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![dir.join("repo/index.toml").display().to_string()];
    app.packages.plugin_dir = Some(dir.join("plugins"));

    press(&mut app, "M-x package-install RET d TAB RET");
    assert_eq!(
        app.message.as_deref(),
        Some("Installed demo 0.0.0"),
        "{:?}",
        app.message
    );
    assert!(dir.join("plugins/demo.wasm").exists());
    assert!(crate::plugins::loaded_names(&app).contains(&"demo".to_string()));

    press(&mut app, "M-x list-packages RET");
    assert_eq!(app.buf_ref().name(), "*Packages*");
    assert!(
        text(&app).contains("demo") && text(&app).contains("installed"),
        "{}",
        text(&app)
    );

    // A tampered file isn't installed.
    std::fs::write(&wasm, b"\0asm\x0d\0\x01\0changed").unwrap();
    press(&mut app, "d");
    assert_eq!(app.message.as_deref(), Some("Deleted demo"));
    assert!(!dir.join("plugins/demo.wasm").exists());
    assert!(!crate::plugins::loaded_names(&app).contains(&"demo".to_string()));
    press(&mut app, "RET");
    assert!(
        app.message.as_deref().unwrap_or("").contains("sha256"),
        "{:?}",
        app.message
    );

    // Serve the index and component locally without external tooling.
    let wasm = demo_plugin_file(&dir.join("repo"));
    std::fs::write(
        dir.join("repo/index.toml"),
        forge_core::package::entry_for(&wasm, None).unwrap(),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let repo = dir.join("repo");
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let n = std::io::Read::read(&mut stream, &mut request).unwrap();
            let path = String::from_utf8_lossy(&request[..n])
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            let body = match path.as_str() {
                "/index.toml" => std::fs::read(repo.join("index.toml")).unwrap(),
                "/demo.wasm" => std::fs::read(repo.join("demo.wasm")).unwrap(),
                _ => panic!("unexpected package URL path: {path}"),
            };
            use std::io::Write;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    app.packages.indexes = vec![format!("http://127.0.0.1:{port}/index.toml")];
    app.packages.index = None;
    press(&mut app, "M-x package-install RET");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.minibuffer.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.on_idle();
    }
    press(&mut app, "demo RET");
    while !dir.join("plugins/demo.wasm").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.on_idle();
    }
    assert_eq!(
        app.message.as_deref(),
        Some("Installed demo 0.0.0"),
        "{:?}",
        app.message
    );
    server.join().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn packages_from_version_control() {
    let dir = std::env::temp_dir().join(format!("forge-vc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("forge-demo");
    let wasm = demo_plugin_file(&repo.join("dist"));
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "tag.gpgSign=false",
            ])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "prebuilt"]);
    git(&["tag", "v1"]);
    // SAFETY: tests that read FORGE_DATA_DIR are only this one.
    unsafe { std::env::set_var("FORGE_DATA_DIR", dir.join("data with spaces")) };
    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.plugin_dir = Some(dir.join("plugins"));
    let wait = |app: &mut App, done: &dyn Fn(&App) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !done(app) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.on_idle();
        }
    };
    fn tree_snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        fn visit(
            root: &std::path::Path,
            path: &std::path::Path,
            files: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
        ) {
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    visit(root, &path, files);
                } else if entry.file_type().is_ok_and(|kind| kind.is_file())
                    && let Ok(bytes) = std::fs::read(&path)
                    && let Ok(relative) = path.strip_prefix(root)
                {
                    files.push((relative.to_path_buf(), bytes));
                }
            }
        }
        let mut files = Vec::new();
        visit(root, root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    // A prebuilt component: nothing to run, no question.
    press(&mut app, "M-x package-vc-install RET");
    answer(&mut app, &format!("{}@v1", repo.display()));
    assert_eq!(app.buf_ref().name(), crate::package_vc::BUFFER);
    assert!(
        buffer_text(&app, crate::package_vc::BUFFER).contains("$ git clone"),
        "{}",
        buffer_text(&app, crate::package_vc::BUFFER)
    );
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Installed"))
    });
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("Installed demo 0.0.0 ("),
        "{:?}",
        app.message
    );
    assert!(crate::plugins::loaded_names(&app).contains(&"demo".to_string()));
    assert!(dir.join("data with spaces/vc/demo/.git").exists());
    let db = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert_eq!(db.packages["demo"].rev.as_deref(), Some("v1"));

    let init = dir.join("init.rhai");
    std::fs::write(
        &init,
        r#"use_package("declared-demo", #{
            git: "https://example.test/declared-demo.git",
            commands: ["demo-command"]
        });"#,
    )
    .unwrap();
    app.init_path = Some(init);
    app.load_init_file();
    press(&mut app, "M-x package-manager RET");
    assert!(
        text(&app).contains("demo")
            && text(&app).contains("loaded")
            && text(&app).contains("declared-demo")
            && text(&app).contains("not installed"),
        "{}",
        text(&app)
    );
    assert!(
        text(&app).contains("declared-demo >"),
        "first plugin should be selected:\n{}",
        text(&app)
    );
    press(&mut app, "j");
    assert!(
        text(&app)
            .lines()
            .any(|line| line.starts_with("demo ") && line.contains(" >  loaded")),
        "j should select the next plugin:\n{}",
        text(&app)
    );
    press(&mut app, "k");
    press(&mut app, "f");
    answer(&mut app, "declared-");
    assert!(
        text(&app).contains("Filter: declared-")
            && !text(&app).lines().any(|line| line.starts_with("demo ")),
        "filter should limit the browser:\n{}",
        text(&app)
    );
    press(&mut app, "F");
    assert!(text(&app).contains("demo"), "{}", text(&app));
    press(&mut app, "v");
    assert!(
        text(&app).contains("declared-demo.git")
            && text(&app).contains("demo-command")
            && text(&app).contains("load: lazy"),
        "{}",
        text(&app)
    );
    press(&mut app, "M-x package-manager RET");
    press(&mut app, "b");
    assert_eq!(app.buf_ref().name(), crate::package_vc::BUFFER);
    assert!(text(&app).contains("Installed demo"), "{}", text(&app));

    // A build command is asked about first.
    std::fs::rename(&wasm, repo.join("demo.bin")).unwrap();
    let build = if cfg!(windows) {
        "copy /Y demo.bin \"%OUT%\" >nul"
    } else {
        "cp demo.bin \"$OUT\""
    };
    std::fs::write(
        repo.join("forge-package.toml"),
        format!("build = {build:?}\n"),
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "built"]);
    git(&["tag", "-f", "v1"]);
    press(&mut app, "M-x package-vc-upgrade RET demo RET");
    wait(&mut app, &|a| a.minibuffer.is_some());
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .contains("changelog"),
        "{:?}",
        app.message
    );
    press(&mut app, "y");
    wait(&mut app, &|a| {
        a.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains(&format!("with `{build}")))
    });
    assert!(
        app.minibuffer.is_some(),
        "message: {:?}\n{}",
        app.message,
        buffer_text(&app, "*Package Build*")
    );
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .contains(&format!("with `{build}")),
        "{:?}",
        app.message
    );
    assert!(
        buffer_text(&app, crate::package_vc::BUFFER).contains("Build requires approval"),
        "{}",
        buffer_text(&app, crate::package_vc::BUFFER)
    );
    press(&mut app, "y");
    let plugins = dir.join("plugins");
    wait(&mut app, &|_| {
        forge_core::package::InstalledDb::load(&plugins)
            .packages
            .get("demo")
            .is_some_and(|p| p.build.as_deref() == Some(build))
    });
    let db = forge_core::package::InstalledDb::load(&plugins);
    assert_eq!(
        db.packages["demo"].build.as_deref(),
        Some(build),
        "stored build command: {:?}\n{}",
        app.message,
        buffer_text(&app, "*Package Build*")
    );

    // Nothing new: up to date. Declining a build leaves things as they were.
    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.contains("up to date"))
    });
    let changed_build = if cfg!(windows) {
        "copy /Y demo.bin \"%OUT%\" >nul && ver >nul"
    } else {
        "cp demo.bin \"$OUT\" && true"
    };
    std::fs::write(
        repo.join("forge-package.toml"),
        format!("build = {changed_build:?}\n"),
    )
    .unwrap();
    git(&["commit", "-q", "-am", "new build"]);
    git(&["tag", "-f", "v1"]);
    let plugins_before = tree_snapshot(&dir.join("plugins"));
    let data_before = tree_snapshot(&dir.join("data with spaces"));
    press(&mut app, "C-u M-x package-vc-upgrade-all RET");
    let plan = buffer_text(&app, "*Package Upgrade Plan*");
    assert!(plan.contains("demo:") && plan.contains("build:"), "{plan}");
    assert_eq!(tree_snapshot(&dir.join("plugins")), plugins_before);
    assert_eq!(tree_snapshot(&dir.join("data with spaces")), data_before);

    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| a.minibuffer.is_some());
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .contains("changelog"),
        "{:?}",
        app.message
    );
    press(&mut app, "y");
    wait(&mut app, &|a| a.minibuffer.is_some());
    press(&mut app, "n");
    assert_eq!(app.message.as_deref(), Some("Not building demo"));

    press(&mut app, "M-x package-delete RET demo RET");
    assert!(!dir.join("data with spaces/vc/demo").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
