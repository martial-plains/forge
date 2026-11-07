// SPDX-License-Identifier: MIT OR Apache-2.0
//! packaging tests.

use super::*;
use std::sync::MutexGuard;

fn package_env() -> MutexGuard<'static, ()> {
    super::env_lock()
}

struct EnvGuard {
    data: Option<std::ffi::OsString>,
    config: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(data: &std::path::Path, config: &std::path::Path) -> Self {
        let data_old = std::env::var_os("FORGE_DATA_DIR");
        let config_old = std::env::var_os("FORGE_CONFIG_DIR");
        // SAFETY: the shared test environment lock serializes these changes.
        unsafe {
            std::env::set_var("FORGE_DATA_DIR", data);
            std::env::set_var("FORGE_CONFIG_DIR", config);
        }
        Self {
            data: data_old,
            config: config_old,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the shared test environment lock serializes these changes.
        unsafe {
            match &self.data {
                Some(value) => std::env::set_var("FORGE_DATA_DIR", value),
                None => std::env::remove_var("FORGE_DATA_DIR"),
            }
            match &self.config {
                Some(value) => std::env::set_var("FORGE_CONFIG_DIR", value),
                None => std::env::remove_var("FORGE_CONFIG_DIR"),
            }
        }
    }
}

fn buffer_has_face(app: &App, name: &str, face: forge_core::decoration::PackageFace) -> bool {
    let id = app
        .editor
        .find_buffer(name)
        .unwrap_or_else(|| panic!("no buffer {name}"));
    app.editor
        .state
        .get_buffer(id)
        .unwrap()
        .decorations()
        .iter()
        .any(|d| d.face == forge_core::decoration::DecoFace::Package(face))
}

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

fn incompatible_plugin_file(dir: &std::path::Path) -> std::path::PathBuf {
    use wit_component::{ComponentEncoder, StringEncoding};
    let mut wit = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/wit-0.1.0/forge-editor.wit"
    ))
    .unwrap();
    wit = wit.replace(
        "package forge:editor@0.1.0;",
        "package forge:editor@0.1.12;",
    );
    wit = wit.replace(
        "    log: func(level: level, message: string);",
        "    log: func(level: level, message: string);\n    flush: func();",
    );
    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve.push_str("forge-editor.wit", &wit).unwrap();
    let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
    let mut module = wat::parse_str(include_str!(
        "../../../../tests/fixtures/incompatible-plugin.wat"
    ))
    .unwrap();
    wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .validate(true)
        .encode()
        .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("incompatible.wasm");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn package_compatibility_preflight_lists_and_refuses_unless_forced() {
    let dir = std::env::temp_dir().join(format!("forge-compat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let wasm = incompatible_plugin_file(&dir.join("repo"));
    let entry = forge_core::package::entry_for(&wasm, None).unwrap();
    std::fs::write(dir.join("repo/index.toml"), entry).unwrap();
    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![dir.join("repo/index.toml").display().to_string()];
    app.packages.plugin_dir = Some(dir.join("plugins"));

    press(&mut app, "M-x list-packages RET");
    let listing = text(&app);
    assert!(
        listing.contains("incompatible")
            && listing.contains("needs a newer Forge (missing `flush`)"),
        "{listing}"
    );

    press(&mut app, "M-x package-install RET incompatible RET");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("needs a newer Forge (missing `flush`)"),
        "{:?}",
        app.message
    );
    assert!(!dir.join("plugins/incompatible.wasm").exists());

    press(&mut app, "C-u M-x package-install RET incompatible RET");
    assert!(dir.join("plugins/incompatible.wasm").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_screens_fit_eighty_and_two_hundred_columns() {
    let _env = package_env();
    let dir = std::env::temp_dir().join(format!("forge-pkg-wide-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let wasm = demo_plugin_file(&dir.join("repo"));
    std::fs::write(
        dir.join("repo/index.toml"),
        forge_core::package::entry_for(&wasm, None).unwrap(),
    )
    .unwrap();
    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![dir.join("repo/index.toml").display().to_string()];
    app.packages.plugin_dir = Some(dir.join("plugins"));

    press(&mut app, "M-x package-install RET d TAB RET");
    let init = dir.join("init.rhai");
    std::fs::write(
        &init,
        r#"use_package("declared-demo", #{
            git: "https://example.test/declared-demo.git",
            commands: ["demo-command"]
        });
        use_package("a-very-long-declared-package-name", #{
            git: "https://example.test/long.git"
        });"#,
    )
    .unwrap();
    app.init_path = Some(init);
    app.load_init_file();

    // The package browser lays out at the width of the last redisplay.
    let _ = screen(&mut app, 80, 24);
    press(&mut app, "M-x package-manager RET");
    assert_eq!(point(&app).line, 5, "the first package row is selected");
    let narrow = screen(&mut app, 80, 24);
    let _ = screen(&mut app, 200, 24);
    press(&mut app, "M-x package-manager RET");
    let wide = screen(&mut app, 200, 24);

    for (label, shown, width) in [("80", narrow.as_str(), 80), ("200", wide.as_str(), 200)] {
        assert!(shown.contains("Forge Packages"), "{label}:\n{shown}");
        assert!(shown.contains("demo"), "{label}:\n{shown}");
        assert!(shown.contains("declared-demo"), "{label}:\n{shown}");
        assert!(shown.contains("loaded"), "{label}:\n{shown}");
        assert!(shown.contains("not installed"), "{label}:\n{shown}");
        assert!(
            shown.contains("3 packages - 0 updates"),
            "counts keep the separator readable at {label} columns:\n{shown}"
        );
        assert!(
            shown.contains("...") || shown.contains('…'),
            "long names end in an ellipsis at {label} columns:\n{shown}"
        );
        assert!(
            shown.contains("RET install or upgrade") && shown.contains("q quit"),
            "the footer is visible at {label} columns:\n{shown}"
        );
        for line in shown.lines() {
            assert!(
                unicode_width::UnicodeWidthStr::width(line.trim_end()) <= width,
                "{label}-column line is wider than the screen: {line:?}"
            );
        }
    }
    assert!(
        wide.contains("demo.wasm"),
        "wide layout shows sources:\n{wide}"
    );
    assert!(
        !narrow
            .lines()
            .any(|line| line.contains("Forge Packages") && line.contains("demo.wasm")),
        "narrow layout keeps the source column quiet:\n{narrow}"
    );

    let _ = screen(&mut app, 80, 24);
    press(&mut app, "M-x list-packages RET");
    assert_eq!(point(&app).line, 4, "the first package-list row has point");
    let list_narrow = screen(&mut app, 80, 24);
    let _ = screen(&mut app, 200, 24);
    press(&mut app, "M-x list-packages RET");
    let list_wide = screen(&mut app, 200, 24);
    for (label, shown, width) in [
        ("80", list_narrow.as_str(), 80),
        ("200", list_wide.as_str(), 200),
    ] {
        assert!(shown.contains("Packages"), "{label}:\n{shown}");
        assert!(shown.contains("demo"), "{label}:\n{shown}");
        assert!(
            shown.contains("q quit"),
            "the list footer is visible at {label} columns:\n{shown}"
        );
        for line in shown.lines() {
            assert!(
                unicode_width::UnicodeWidthStr::width(line.trim_end()) <= width,
                "{label}-column list line is wider than the screen: {line:?}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
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
fn package_allowed_sources_deny_indexes_before_reading_them() {
    let dir = std::env::temp_dir().join(format!("forge-allow-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("missing/index.toml");
    let mut app = app_with("");
    app.packages.allowed_sources = Some(
        forge_core::package_source::SourceAllowlist::new(&["github.com".to_string()]).unwrap(),
    );
    app.packages.indexes = vec![missing.display().to_string()];
    press(&mut app, "M-x package-refresh-contents RET");
    let message = app.message.as_deref().unwrap_or("");
    assert!(message.contains("not allowed"), "{message}");
    assert!(!message.contains("No such file"), "{message}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_allowed_sources_do_not_inherit_from_an_allowed_index() {
    let dir = std::env::temp_dir().join(format!("forge-allow-pkg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let wasm = demo_plugin_file(&dir.join("repo"));
    let index = dir.join("repo/index.toml");
    std::fs::write(&index, forge_core::package::entry_for(&wasm, None).unwrap()).unwrap();
    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![index.display().to_string()];
    app.packages.plugin_dir = Some(dir.join("plugins"));
    app.packages.allowed_sources = Some(
        forge_core::package_source::SourceAllowlist::new(&[index.display().to_string()]).unwrap(),
    );

    press(&mut app, "M-x list-packages RET");
    assert!(
        !app.message.as_deref().unwrap_or("").contains("not allowed"),
        "{:?}",
        app.message
    );
    press(&mut app, "M-x package-install RET demo RET");
    let message = app.message.as_deref().unwrap_or("");
    assert!(message.contains("Can't install demo"), "{message}");
    assert!(message.contains("not allowed"), "{message}");
    assert!(!dir.join("plugins/demo.wasm").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_allowed_sources_stop_redirects_before_contacting_the_target() {
    let source = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let source_port = source.local_addr().unwrap().port();
    let target = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    target.set_nonblocking(true).unwrap();
    let target_port = target.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = source.accept().unwrap();
        let mut request = [0; 1024];
        let _ = std::io::Read::read(&mut stream, &mut request);
        use std::io::Write;
        write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/index.toml\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });

    let mut app = app_with("");
    app.message = None;
    app.packages.indexes = vec![format!("http://127.0.0.1:{source_port}/index.toml")];
    app.packages.allowed_sources = Some(
        forge_core::package_source::SourceAllowlist::new(&[format!("127.0.0.1:{source_port}")])
            .unwrap(),
    );
    press(&mut app, "M-x package-refresh-contents RET");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("not allowed"))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.on_idle();
    }
    let message = app.message.as_deref().unwrap_or("");
    assert!(message.contains("redirect"), "{message}");
    assert!(message.contains("not allowed"), "{message}");
    assert!(
        target.accept().is_err(),
        "the disallowed redirect target was contacted"
    );
    server.join().unwrap();
}

#[test]
fn package_allowed_sources_deny_git_and_dry_run_before_network() {
    let dir = std::env::temp_dir().join(format!("forge-allow-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = dir.join("repo");
    let mut app = app_with("");
    app.packages.allowed_sources = Some(
        forge_core::package_source::SourceAllowlist::new(&["github.com".to_string()]).unwrap(),
    );
    crate::package_vc::install(&mut app, "demo", repo.to_str().unwrap(), None, false);
    let message = app.message.as_deref().unwrap_or("");
    assert!(message.contains("Git source"), "{message}");
    assert!(message.contains("not allowed"), "{message}");
    assert!(!dir.join("plugins/demo.wasm").exists());

    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let mut db = forge_core::package::InstalledDb::load(&plugins);
    db.packages.insert(
        "demo".into(),
        forge_core::package::Installed {
            version: "0.1.0".into(),
            sha256: String::new(),
            url: String::new(),
            git: Some("https://evil.test/repo".into()),
            rev: None,
            commit: Some("aaaaaaa".into()),
            build: None,
            recipe: None,
        },
    );
    db.save().unwrap();
    app.packages.plugin_dir = Some(plugins);
    let error = crate::package_vc::dry_run_lines(&app).unwrap_err();
    assert!(error.contains("not allowed"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn packages_from_version_control() {
    let dir = std::env::temp_dir().join(format!("forge-vc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let _env = EnvGuard::set(&dir.join("data with spaces"), &dir.join("config"));
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
        });
        use_package("frozen-demo", #{
            git: "https://example.test/frozen-demo.git",
            freeze: true
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
        text(&app).contains("3 packages - 0 updates"),
        "{}",
        text(&app)
    );
    assert!(
        text(&app)
            .lines()
            .any(|line| line.starts_with("| declared-demo") && line.contains("not installed")),
        "first plugin should be selected:\n{}",
        text(&app)
    );
    assert!(
        app.buf_ref().decorations().iter().any(|d| {
            d.face
                == forge_core::decoration::DecoFace::Package(
                    forge_core::decoration::PackageFace::Selected,
                )
        }),
        "the selected row has its own face"
    );
    assert!(
        app.buf_ref().decorations().iter().any(|d| {
            d.face
                == forge_core::decoration::DecoFace::Package(
                    forge_core::decoration::PackageFace::Neutral,
                )
        }),
        "status labels use the neutral badge"
    );
    assert!(
        app.buf_ref().decorations().iter().any(|d| {
            d.face
                == forge_core::decoration::DecoFace::Package(
                    forge_core::decoration::PackageFace::Inactive,
                )
        }),
        "frozen rows use the inactive surface"
    );
    assert!(
        text(&app)
            .lines()
            .any(|line| line.contains("frozen-demo") && line.contains("frozen")),
        "frozen state stays labeled:\n{}",
        text(&app)
    );
    press(&mut app, "j");
    assert!(
        text(&app)
            .lines()
            .any(|line| line.starts_with("| demo") && line.contains("loaded")),
        "j should select the next plugin:\n{}",
        text(&app)
    );
    press(&mut app, "k");
    press(&mut app, "f");
    answer(&mut app, "declared-");
    assert!(
        text(&app).contains("Filter: declared-")
            && text(&app).contains("1 package - 0 updates")
            && !text(&app).lines().any(|line| line.starts_with("demo ")),
        "filter should limit the browser and update its counts:\n{}",
        text(&app)
    );
    press(&mut app, "F");
    assert!(text(&app).contains("demo"), "{}", text(&app));
    assert!(
        text(&app).contains("3 packages - 0 updates"),
        "{}",
        text(&app)
    );
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
    assert!(
        plan.contains("demo:") && plan.contains(&format!("build: {changed_build}")),
        "dry run should detect the remote build command: {plan}"
    );
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

    press(&mut app, "M-x package-log RET");
    let log = buffer_text(&app, crate::package_log::BUFFER);
    assert!(
        log.contains("Package Operations")
            && log.contains("install")
            && log.contains("ok")
            && log.contains("ms"),
        "{log}"
    );
    assert!(
        buffer_has_face(
            &app,
            crate::package_log::BUFFER,
            forge_core::decoration::PackageFace::Title,
        ) && buffer_has_face(
            &app,
            crate::package_log::BUFFER,
            forge_core::decoration::PackageFace::Heading,
        ) && buffer_has_face(
            &app,
            crate::package_log::BUFFER,
            forge_core::decoration::PackageFace::Ok,
        ),
        "the log has a title, column headings, and status badges"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn vc_upgrades_warn_about_history_rewrites_and_moved_tags() {
    let dir = std::env::temp_dir().join(format!("forge-vc-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let _env = EnvGuard::set(&dir.join("data"), &dir.join("config"));
    let repo = dir.join("forge-demo");
    demo_plugin_file(&repo.join("dist"));
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
        String::from_utf8_lossy(&ok.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "one"]);
    git(&["tag", "v1"]);

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
    press(&mut app, "M-x package-vc-install RET");
    answer(&mut app, &format!("{}@v1", repo.display()));
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Installed"))
    });
    let first = git(&["rev-parse", "HEAD"]);

    // A normal fast-forward still moves a pinned tag: warn, but don't claim
    // the history was rewritten.
    std::fs::write(repo.join("next.txt"), "next\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "next"]);
    git(&["tag", "-f", "v1"]);
    press(&mut app, "C-u M-x package-vc-upgrade-all RET");
    let plan = buffer_text(&app, "*Package Upgrade Plan*");
    assert!(
        plan.contains("tag v1 now points") && plan.contains("was moved"),
        "{plan}"
    );
    assert!(!plan.contains("not a descendant"), "{plan}");

    // Rewrite the branch with an unrelated root and move the tag there.
    git(&["checkout", "-q", "--orphan", "rewritten"]);
    git(&["commit", "-q", "-m", "rewritten"]);
    git(&["tag", "-f", "v1"]);
    let rewritten = git(&["rev-parse", "HEAD"]);

    press(&mut app, "C-u M-x package-vc-upgrade-all RET");
    let plan = buffer_text(&app, "*Package Upgrade Plan*");
    assert!(
        plan.contains("not a descendant")
            && plan.contains("rewritten or force-pushed")
            && plan.contains("tag v1 now points"),
        "{plan}"
    );
    assert!(
        buffer_has_face(
            &app,
            "*Package Upgrade Plan*",
            forge_core::decoration::PackageFace::Title,
        ) && buffer_has_face(
            &app,
            "*Package Upgrade Plan*",
            forge_core::decoration::PackageFace::Warning,
        ),
        "the dry-run plan has a title and warning badge"
    );
    assert_eq!(
        git(&["rev-parse", "HEAD"]),
        rewritten,
        "the dry run must not move the real checkout"
    );
    let installed = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert_eq!(
        installed.packages["demo"].commit.as_deref(),
        Some(first.as_str()),
        "the dry run must not change the installed record"
    );

    // The interactive upgrade review shows both warnings before asking.
    press(&mut app, "M-x package-vc-upgrade RET demo RET");
    wait(&mut app, &|a| a.minibuffer.is_some());
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("changelog")),
        "{:?}",
        app.message
    );
    let review = buffer_text(&app, crate::package_vc::BUFFER);
    assert!(
        review.contains("not a descendant") && review.contains("tag v1 now points"),
        "{review}"
    );
    assert!(
        buffer_has_face(
            &app,
            crate::package_vc::BUFFER,
            forge_core::decoration::PackageFace::Heading,
        ) && buffer_has_face(
            &app,
            crate::package_vc::BUFFER,
            forge_core::decoration::PackageFace::Warning,
        ),
        "the upgrade review headings and warnings are styled"
    );
    press(&mut app, "n");
    assert_eq!(app.message.as_deref(), Some("Not upgrading demo"));

    // Unattended upgrades stop on the rewrite, leaving the installed plugin.
    app.unattended = true;
    press(&mut app, "M-x package-vc-upgrade RET demo RET");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.contains("history was rewritten"))
    });
    let message = app.message.as_deref().unwrap_or("");
    assert!(
        message.contains("history was rewritten") && message.contains("no changes were made"),
        "{:?}",
        app.message
    );
    let installed = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert_eq!(
        installed.packages["demo"].commit.as_deref(),
        Some(first.as_str())
    );

    // An explicit declaration opt-in lets unattended mode cross the rewrite.
    let init = dir.join("init.rhai");
    std::fs::write(
        &init,
        format!(
            "use_package(\"demo\", #{{ git: \"{}\", allow_history_rewrite: true }});",
            repo.display()
        ),
    )
    .unwrap();
    app.init_path = Some(init);
    app.load_init_file();
    app.unattended = true;
    press(&mut app, "M-x package-vc-upgrade RET demo RET");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Installed"))
    });
    let installed = forge_core::package::InstalledDb::load(&dir.join("plugins"));
    assert_eq!(
        installed.packages["demo"].commit.as_deref(),
        Some(rewritten.as_str()),
        "{:?}",
        app.message
    );
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_upgrade_with_new_capabilities_asks_and_blocks_unattended() {
    let dir = std::env::temp_dir().join(format!("forge-capability-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let config = dir.join("config");
    let plugins = dir.join("plugins");
    let _env = EnvGuard::set(&dir.join("data"), &config);
    std::fs::create_dir_all(&config).unwrap();
    let repo = dir.join("repo");
    let old = std::fs::read(demo_plugin_file(&dir.join("old"))).unwrap();
    let old_hash = forge_core::package::sha256_hex(&old);
    let new = include_str!("../../../../tests/fixtures/demo-plugin.wat")
        .replace("demo-greet", "demo-great");
    let new_path = plugin_file_from_wat(&repo, "demo", &new);
    std::fs::write(
        repo.join("demo.toml"),
        "version = \"2.0.0\"\n\
         [permissions]\n\
         network = true\n\
         [[permissions.directories]]\n\
         path = \"~/.cache/forge-demo\"\n\
         write = true\n",
    )
    .unwrap();
    let new = std::fs::read(&new_path).unwrap();
    let index = format!(
        "[[package]]\nname = \"demo\"\nversion = \"2.0.0\"\nurl = \"demo.wasm\"\nsha256 = \"{}\"\n",
        forge_core::package::sha256_hex(&new)
    );
    std::fs::write(repo.join("index.toml"), index).unwrap();

    let mut installed = forge_core::package::InstalledDb::load(&plugins);
    installed
        .install(
            &forge_core::package::Package {
                name: "demo".into(),
                version: "1.0.0".into(),
                description: String::new(),
                url: "old.wasm".into(),
                sha256: old_hash.clone(),
                git: None,
                rev: None,
                release_notes: None,
            },
            &old,
        )
        .unwrap();
    let mut lock = forge_core::LockFile::load(&config).unwrap();
    lock.set(
        "demo",
        forge_core::LockEntry {
            sha256: old_hash.clone(),
            artifact_sha256: Some(old_hash.clone()),
            version: Some("1.0.0".into()),
            ..Default::default()
        },
    );
    lock.save().unwrap();

    fn snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
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

    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![repo.join("index.toml").display().to_string()];
    app.packages.plugin_dir = Some(plugins.clone());

    press(&mut app, "M-x package-upgrade-all RET");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("Apply demo 1.0.0 -> 2.0.0")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "y");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("new capabilities")),
        "message: {:?}",
        app.message
    );
    let review = buffer_text(&app, "*Package Upgrade Review*");
    assert!(
        review.contains("network")
            && review.contains("~/.cache/forge-demo")
            && review.contains("read and write")
            && review.contains("New capabilities"),
        "{review}"
    );
    assert!(
        buffer_has_face(
            &app,
            "*Package Upgrade Review*",
            forge_core::decoration::PackageFace::Title,
        ) && buffer_has_face(
            &app,
            "*Package Upgrade Review*",
            forge_core::decoration::PackageFace::Heading,
        ),
        "the capability review has styled title and headings"
    );
    press(&mut app, "n");
    assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), old);
    assert_eq!(lock.get("demo").unwrap().sha256, old_hash);

    let plugins_before = snapshot(&plugins);
    let config_before = snapshot(&config);
    app.unattended = true;
    press(&mut app, "M-x package-upgrade-all RET");
    let message = app.message.as_deref().unwrap_or("");
    assert!(
        message.contains("new capabilities") && message.contains("No changes were made"),
        "message: {:?}",
        app.message
    );
    assert_eq!(snapshot(&plugins), plugins_before);
    assert_eq!(snapshot(&config), config_before);

    app.unattended = false;
    press(&mut app, "M-x package-upgrade-all RET");
    press(&mut app, "y");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("new capabilities")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "y");
    assert_eq!(
        std::fs::read(plugins.join("demo.wasm")).unwrap(),
        new,
        "message: {:?}",
        app.message
    );
    assert_eq!(app.message.as_deref(), Some("Installed demo 2.0.0"));
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn vc_upgrade_with_new_capabilities_asks_and_blocks_unattended() {
    let dir = std::env::temp_dir().join(format!("forge-vc-capability-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let _env = EnvGuard::set(&dir.join("data"), &dir.join("config"));
    let repo = dir.join("forge-demo");
    let wasm = demo_plugin_file(&repo.join("dist"));
    let manifest = repo.join("dist/demo.toml");
    std::fs::write(&manifest, "version = \"1.0.0\"\ndescription = \"Demo\"\n").unwrap();
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
    git(&["commit", "-q", "-m", "one"]);
    git(&["tag", "v1"]);

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
    press(&mut app, "M-x package-vc-install RET");
    answer(&mut app, &format!("{}@v1", repo.display()));
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Installed"))
    });
    let old = std::fs::read(&wasm).unwrap();
    assert!(dir.join("plugins/demo.wasm").exists());

    std::fs::write(
        &manifest,
        "version = \"2.0.0\"\ndescription = \"Demo\"\n[permissions]\nnetwork = true\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "two"]);
    git(&["tag", "-f", "v1"]);

    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| a.minibuffer.is_some());
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("changelog")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "y");
    wait(&mut app, &|a| {
        a.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("new capabilities"))
    });
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("new capabilities")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "n");
    assert_eq!(std::fs::read(dir.join("plugins/demo.wasm")).unwrap(), old);

    fn snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
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

    let plugins_before = snapshot(&dir.join("plugins"));
    let config_before = snapshot(&dir.join("config"));
    app.unattended = true;
    press(&mut app, "M-x package-vc-upgrade-all RET");
    wait(&mut app, &|a| {
        a.message
            .as_deref()
            .is_some_and(|m| m.contains("new capabilities"))
    });
    let message = app.message.as_deref().unwrap_or("");
    assert!(
        message.contains("new capabilities") && message.contains("No changes were made"),
        "message: {:?}",
        app.message
    );
    assert_eq!(snapshot(&dir.join("plugins")), plugins_before);
    assert_eq!(snapshot(&dir.join("config")), config_before);
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn startup_recovers_an_interrupted_package_update() {
    let dir = std::env::temp_dir().join(format!("forge-recover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let config = dir.join("config");
    let plugins = dir.join("plugins");
    let data = dir.join("data");
    let _env = EnvGuard::set(&data, &config);
    std::fs::create_dir_all(&config).unwrap();
    let old = b"\0asm\x0d\0\x01\0old".to_vec();
    let new = b"\0asm\x0d\0\x01\0new".to_vec();
    let old_hash = forge_core::package::sha256_hex(&old);
    let new_hash = forge_core::package::sha256_hex(&new);
    let package = |sha: &str| forge_core::package::Package {
        name: "demo".into(),
        version: "1.0.0".into(),
        description: String::new(),
        url: "demo.wasm".into(),
        sha256: sha.into(),
        git: None,
        rev: None,
        release_notes: None,
    };
    let mut installed = forge_core::package::InstalledDb::load(&plugins);
    installed.install(&package(&old_hash), &old).unwrap();
    let mut lock = forge_core::LockFile::load(&config).unwrap();
    lock.set(
        "demo",
        forge_core::LockEntry {
            sha256: old_hash.clone(),
            artifact_sha256: Some(old_hash.clone()),
            version: Some("1.0.0".into()),
            ..Default::default()
        },
    );
    lock.save().unwrap();
    let mut journal =
        forge_core::package_journal::Journal::new(&data, "upgrade", &config, &plugins);
    journal.add("demo").unwrap();
    drop(journal);
    std::fs::write(plugins.join("demo.wasm"), &new).unwrap();
    let mut half_applied = forge_core::package::InstalledDb::load(&plugins);
    half_applied.install(&package(&new_hash), &new).unwrap();

    let app = forge_editor::start(&forge_editor::StartOptions {
        no_config: true,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), old);
    assert_eq!(
        forge_core::package::InstalledDb::load(&plugins).packages["demo"].version,
        "1.0.0"
    );
    assert_eq!(
        forge_core::LockFile::load(&config)
            .unwrap()
            .get("demo")
            .and_then(|entry| entry.artifact_sha256.as_deref()),
        Some(old_hash.as_str())
    );
    assert!(!data.join("package-journal.toml").exists());
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_upgrade_all_rolls_back_the_whole_batch() {
    let dir = std::env::temp_dir().join(format!("forge-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _env_lock = package_env();
    let _env = EnvGuard::set(&dir.join("data"), &dir.join("config"));
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let repo = dir.join("repo");
    let old = std::fs::read(demo_plugin_file(&dir.join("old"))).unwrap();
    let old_hash = forge_core::package::sha256_hex(&old);
    let new_good = include_str!("../../../../tests/fixtures/demo-plugin.wat")
        .replace("demo-greet", "demo-great");
    let good_path = plugin_file_from_wat(&repo, "one-new", &new_good);
    let good = std::fs::read(&good_path).unwrap();
    let failing = r#"
        (module
          (memory (export "memory") 1)
          (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
            (i32.const 0))
          (func (export "init") (result i32)
            (unreachable))
          (func (export "forge:editor/handler@0.1.0#run-command")
            (param i32 i32 i32 i64) (result i32)
            (i32.const 0))
          (func (export "forge:editor/handler@0.1.0#on-event") (param i32 i32))
          (func (export "forge:editor/handler@0.1.0#on-answer") (param i32 i32 i32 i32)))
    "#;
    let bad_path = plugin_file_from_wat(&repo, "two-new", failing);
    let bad = std::fs::read(&bad_path).unwrap();
    let index = format!(
        "[[package]]\nname = \"one\"\nversion = \"2.0.0\"\nurl = \"one-new.wasm\"\nsha256 = \"{}\"\n\n\
         [[package]]\nname = \"two\"\nversion = \"2.0.0\"\nurl = \"two-new.wasm\"\nsha256 = \"{}\"\n",
        forge_core::package::sha256_hex(&good),
        forge_core::package::sha256_hex(&bad)
    );
    std::fs::write(repo.join("index.toml"), index).unwrap();

    let plugins = dir.join("plugins");
    let mut installed = forge_core::package::InstalledDb::load(&plugins);
    for name in ["one", "two"] {
        installed
            .install(
                &forge_core::package::Package {
                    name: name.into(),
                    version: "1.0.0".into(),
                    description: String::new(),
                    url: format!("{name}-old.wasm"),
                    sha256: old_hash.clone(),
                    git: None,
                    rev: None,
                    release_notes: None,
                },
                &old,
            )
            .unwrap();
    }
    let mut lock = forge_core::LockFile::load(&dir.join("config")).unwrap();
    for name in ["one", "two"] {
        lock.set(
            name,
            forge_core::LockEntry {
                sha256: old_hash.clone(),
                artifact_sha256: Some(old_hash.clone()),
                version: Some("1.0.0".into()),
                ..Default::default()
            },
        );
    }
    lock.save().unwrap();

    let mut app = app_with("");
    app.plugin_cache = None;
    app.packages.indexes = vec![repo.join("index.toml").display().to_string()];
    app.packages.plugin_dir = Some(plugins.clone());

    press(&mut app, "M-x package-upgrade-all RET");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("Apply one 1.0.0 -> 2.0.0")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "y");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.contains("Apply two 1.0.0 -> 2.0.0")),
        "message: {:?}",
        app.message
    );
    press(&mut app, "y");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Can't upgrade two"),
        "message: {:?}",
        app.message
    );

    assert_eq!(std::fs::read(plugins.join("one.wasm")).unwrap(), old);
    assert_eq!(std::fs::read(plugins.join("two.wasm")).unwrap(), old);
    let restored = forge_core::package::InstalledDb::load(&plugins);
    assert_eq!(restored.packages["one"].version, "1.0.0");
    assert_eq!(restored.packages["two"].version, "1.0.0");
    let lock = forge_core::LockFile::load(&dir.join("config")).unwrap();
    assert_eq!(
        lock.get("one").and_then(|e| e.artifact_sha256.as_deref()),
        Some(old_hash.as_str())
    );
    assert!(
        !dir.join("data/package-journal.toml").exists(),
        "a completed rollback removes the journal"
    );
    assert_eq!(
        app.editor.commands.plugin_for("demo-greet"),
        Some("one"),
        "the first package is running its restored artifact again"
    );
    assert_eq!(app.editor.commands.plugin_for("demo-great"), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_doctor_uses_accessible_badges() {
    let _env_lock = package_env();
    let dir = std::env::temp_dir().join(format!("forge-doctor-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let _env = EnvGuard::set(&dir.join("data"), &dir.join("config"));
    let mut app = app_with("");
    app.init_errors.push("synthetic init problem".into());

    press(&mut app, "M-x package-doctor RET");
    let doctor = buffer_text(&app, crate::package_doctor::BUFFER);
    assert!(
        doctor.contains("Package Doctor")
            && doctor.contains("error")
            && doctor.contains("synthetic init problem"),
        "{doctor}"
    );
    assert!(
        buffer_has_face(
            &app,
            crate::package_doctor::BUFFER,
            forge_core::decoration::PackageFace::Title,
        ) && buffer_has_face(
            &app,
            crate::package_doctor::BUFFER,
            forge_core::decoration::PackageFace::Error,
        ) && buffer_has_face(
            &app,
            crate::package_doctor::BUFFER,
            forge_core::decoration::PackageFace::Heading,
        ),
        "the doctor has a title and severity badge"
    );
    for line in screen(&mut app, 80, 24).lines() {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.trim_end()) <= 80,
            "80-column doctor line is too wide: {line:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn smoke_test_uses_the_doctor_check_seam() {
    use forge_editor::packages::SmokeTestCheck;

    struct FailingCheck;
    impl SmokeTestCheck for FailingCheck {
        fn check(
            &self,
            _app: &App,
            _editor: &forge_core::Editor,
            name: &str,
        ) -> Result<(), String> {
            assert_eq!(name, "demo");
            Err("doctor found a problem".into())
        }
    }

    let dir = std::env::temp_dir().join(format!("forge-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = demo_plugin_file(&dir);
    let bytes = std::fs::read(path).unwrap();
    let app = app_with("");
    let error =
        forge_editor::packages::smoke_test_component_with(&app, "demo", &bytes, &FailingCheck)
            .unwrap_err();
    assert_eq!(error, "doctor found a problem");
    forge_editor::packages::smoke_test_component_with(
        &app,
        "demo",
        &bytes,
        &forge_editor::packages::NoSmokeTestCheck,
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
