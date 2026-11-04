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
    answer(
        &mut app,
        "printf 'src/a.c:2:9: error: undeclared\\n  --> src/b.c:1:5\\n'; exit 2",
    );
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
    answer(&mut app, "printf '// top\\n'");
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
    assert!(grep.contains("src/a.c:3:int y = needle;"), "{grep}");
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

    // Over HTTP, if python3 can serve the directory.
    let port = 20000 + (std::process::id() % 20000) as u16;
    let wasm = demo_plugin_file(&dir.join("repo"));
    std::fs::write(
        dir.join("repo/index.toml"),
        forge_core::package::entry_for(&wasm, None).unwrap(),
    )
    .unwrap();
    let server = std::process::Command::new("python3")
        .args([
            "-m",
            "http.server",
            &port.to_string(),
            "--bind",
            "127.0.0.1",
            "--directory",
        ])
        .arg(dir.join("repo"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut server) = server {
        let up = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        assert!(up, "python3 -m http.server didn't start");
        if up {
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
        }
        let _ = server.kill();
    }
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
    unsafe { std::env::set_var("FORGE_DATA_DIR", dir.join("data")) };
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

    // A prebuilt component: nothing to run, no question.
    press(&mut app, "M-x package-vc-install RET");
    answer(&mut app, &format!("{}@v1", repo.display()));
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
    assert!(dir.join("data/vc/demo/.git").exists());
    let db = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert_eq!(db.packages["demo"].rev.as_deref(), Some("v1"));

    // A build command is asked about first.
    std::fs::rename(&wasm, repo.join("demo.bin")).unwrap();
    std::fs::write(
        repo.join("forge-package.toml"),
        "build = \"cp demo.bin \\\"$OUT\\\"\"\n",
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
            .contains("with `cp demo.bin"),
        "{:?}",
        app.message
    );
    press(&mut app, "y");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Installed"))
    });
    let db = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert!(
        db.packages["demo"]
            .build
            .as_deref()
            .unwrap()
            .starts_with("cp demo.bin")
    );

    // Nothing new: up to date. Declining a build leaves things as they were.
    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.contains("up to date"))
    });
    std::fs::write(
        repo.join("forge-package.toml"),
        "build = \"cp demo.bin \\\"$OUT\\\" && true\"\n",
    )
    .unwrap();
    git(&["commit", "-q", "-am", "new build"]);
    git(&["tag", "-f", "v1"]);
    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| a.minibuffer.is_some());
    press(&mut app, "n");
    assert_eq!(app.message.as_deref(), Some("Not building demo"));

    press(&mut app, "M-x package-delete RET demo RET");
    assert!(!dir.join("data/vc/demo").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
