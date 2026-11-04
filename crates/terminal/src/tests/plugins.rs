// SPDX-License-Identifier: MIT OR Apache-2.0
//! plugins tests.

use super::*;

#[test]
fn installed_plugins_start_lazily_and_reload_when_rebuilt() {
    let dir = std::env::temp_dir().join(format!("forge-plugin-lazy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = demo_plugin_file(&dir.join("plugins"));
    assert_eq!(
        forge_core::config::discover_plugins(&dir.join("plugins")),
        vec![path.clone()]
    );
    let start = || {
        let mut app = app_with("");
        app.plugin_cache = Some(dir.join("cache"));
        crate::plugins::load_at_startup(&mut app, &path);
        app
    };
    let host = |app: &App| app.plugin_host.as_ref().unwrap().is_dormant("demo");

    // First start: loads normally (and remembers what it set up).
    let app = start();
    assert!(!host(&app) && app.plugin_host.as_ref().unwrap().get("demo").is_some());

    // Next start: only stand-ins until it's needed.
    let mut app = start();
    assert!(host(&app));
    press(&mut app, "M-x list-plugins RET");
    assert!(text(&app).contains("not started yet"), "{}", text(&app));
    press(&mut app, "q");
    app.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
    assert_eq!(
        app.minibuffer.as_ref().map(|m| m.prompt.as_str()),
        Some("Name: ")
    );
    press(&mut app, "C-g");
    assert!(!host(&app));

    // M-x reload-plugin loads it again from its file.
    press(&mut app, "M-x reload-plugin RET demo RET");
    assert_eq!(
        app.message.as_deref(),
        Some("Loaded plugin demo (1 command)")
    );

    // A rebuilt file reloads by itself once it stops changing.
    let rebuild = || {
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend([0, 3, 1, b'x', b'y']); // one more custom section
        std::fs::write(&path, bytes).unwrap();
    };
    assert!(app.idle_timeout().is_some(), "polls for rebuilds");
    rebuild();
    app.on_idle();
    assert!(!messages_buffer(&app).contains("reloading"));
    app.on_idle();
    assert!(
        messages_buffer(&app).contains("changed: reloading plugin demo"),
        "{}",
        messages_buffer(&app)
    );

    // ...unless that's switched off.
    app.plugin_auto_reload = false;
    assert!(app.idle_timeout().is_none());
    rebuild();
    app.on_idle();
    app.on_idle();
    assert_eq!(messages_buffer(&app).matches("reloading").count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn eval_expression_and_init_rhai_pre_grants() {
    let mut app = app_with("");
    press(&mut app, "M-: 1+2 RET");
    assert_eq!(app.message.as_deref(), Some("3"));
    press(&mut app, "M-: forge.buffer.current().insert(\"hi\") RET");
    assert_eq!(text(&app), "hi");
    press(&mut app, "M-: nope( RET");
    assert!(
        app.message.as_deref().unwrap().contains("init.rhai"),
        "{:?}",
        app.message
    );

    // A plugin asking for the network asks first...
    let dir = std::env::temp_dir().join(format!("forge-pregrant-{}", std::process::id()));
    let path = demo_plugin_file(&dir);
    std::fs::write(dir.join("demo.toml"), "[permissions]\nnetwork = true\n").unwrap();
    let mut app = app_with("");
    crate::plugins::load(&mut app, &path);
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .starts_with("Grant these permissions to demo?")
    );

    // ...unless init.rhai already granted it.
    std::fs::write(
        dir.join("init.rhai"),
        "forge.plugin.grant(\"demo\", #{ network: true });",
    )
    .unwrap();
    let mut app = app_with("");
    app.init_path = Some(dir.join("init.rhai"));
    app.load_init_file();
    crate::plugins::load(&mut app, &path);
    assert!(app.minibuffer.is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("Loaded plugin demo (1 command)")
    );
    press(&mut app, "M-x list-plugins RET");
    assert!(
        text(&app).contains("network: open connections and look up host names  [init.rhai]"),
        "{}",
        text(&app)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn plugin_commands_keys_questions_and_help() {
    let dir = std::env::temp_dir().join(format!("forge-plugin-ui-{}", std::process::id()));
    let path = demo_plugin_file(&dir);
    let mut app = app_with("");
    press(&mut app, "M-x load-plugin RET");
    type_path(&mut app, &path);
    press(&mut app, "RET");
    assert_eq!(
        app.message.as_deref(),
        Some("Loaded plugin demo (1 command)")
    );
    // Loading registers the command's doc in the registry, tagged with
    // its plugin.
    assert!(app.docs.has(forge_docs::Kind::Command, "demo-greet"));

    // The plugin bound <f5>; its question shows in the minibuffer.
    app.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
    assert_eq!(
        app.minibuffer.as_ref().map(|m| m.prompt.as_str()),
        Some("Name: ")
    );
    press(&mut app, "Ada RET");
    assert_eq!(text(&app), "Hi, Ada");
    // What a plugin does in one answer undoes as one step.
    press(&mut app, "C-/");
    assert_eq!(text(&app), "");

    // M-x completes and runs plugin commands; C-g cancels the question.
    press(&mut app, "M-x demo-gr TAB RET C-g");
    assert!(app.minibuffer.is_none());
    assert_eq!(text(&app), "");

    // Help knows where it came from.
    press(&mut app, "C-h f demo-greet RET");
    let help = text(&app);
    assert!(
        help.contains("Greet someone.")
            && help.contains("Provided by plugin demo")
            && help.contains("<f5>"),
        "{help}"
    );

    // Reloading init.rhai keeps the plugin's binding.
    press(&mut app, "q M-x reload-init-file RET C-h c <f5>");
    assert_eq!(
        app.message.as_deref(),
        Some("<f5> runs the command demo-greet")
    );

    press(&mut app, "M-x unload-plugin RET demo RET");
    assert_eq!(app.message.as_deref(), Some("Unloaded plugin demo"));
    press(&mut app, "C-h c <f5>");
    assert_eq!(app.message.as_deref(), Some("<f5> is undefined"));
    assert!(!app.editor.commands.contains("demo-greet"));
    // Unloading drops its doc from the registry too.
    assert!(!app.docs.has(forge_docs::Kind::Command, "demo-greet"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn plugin_events_arrive_after_commands_and_on_idle() {
    let dir = std::env::temp_dir().join(format!("forge-plugin-events-{}", std::process::id()));
    let path = demo_plugin_file(&dir);
    let mut app = app_with("");
    crate::plugins::load(&mut app, &path);

    press(&mut app, "C-x b fresh RET");
    assert!(messages_buffer(&app).contains("[demo] buffer-opened"));

    // Typing doesn't call the plugin per key; the change arrives when idle.
    press(&mut app, "abc");
    assert!(!messages_buffer(&app).contains("buffer-changed"));
    assert_eq!(app.idle_timeout(), Some(crate::plugins::CHANGE_EVENT_DELAY));
    app.on_idle();
    assert_eq!(
        messages_buffer(&app)
            .matches("[demo] buffer-changed")
            .count(),
        1
    );
    // Logging to *Messages* doesn't cause more events.
    app.on_idle();
    assert_eq!(
        messages_buffer(&app)
            .matches("[demo] buffer-changed")
            .count(),
        1
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_open_in_the_major_mode_for_their_extension() {
    let dir = std::env::temp_dir().join(format!("forge-modes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("run"), "#!/usr/bin/env python3\n").unwrap();
    let mut app = app_with("");
    press(&mut app, "C-x C-f");
    type_path(&mut app, &dir.join("main.rs"));
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().major_mode(), "rust-mode");
    assert!(screen(&mut app, 60, 6).contains("(Rust)"));
    // A script without an extension: by its #! line.
    press(&mut app, "C-x C-f");
    type_path(&mut app, &dir.join("run"));
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().major_mode(), "python-mode");
    std::fs::remove_dir_all(&dir).unwrap();
}

// -------------------------------------------- Phase 7: modes and syntax

#[test]
fn ret_and_tab_indent_by_the_syntax_tree() {
    let mut app = rust_app("");
    press(&mut app, "f n SPC m ( ) SPC { RET");
    assert_eq!(text(&app), "fn m() {\n    ");
    press(&mut app, "l e t SPC v SPC = SPC [ RET 1 , RET ] ; RET");
    assert_eq!(
        text(&app),
        "fn m() {\n    let v = [\n        1,\n    ];\n    "
    );
    // A closing brace goes back to its opener's level as it's typed.
    press(&mut app, "}");
    assert_eq!(text(&app), "fn m() {\n    let v = [\n        1,\n    ];\n}");
    // RET on a line of blanks leaves nothing behind.
    let mut app = rust_app("fn m() {\n    x();\n}\n");
    press(&mut app, "C-n C-e RET RET");
    assert_eq!(
        app.buf_ref().line(2),
        "",
        "the blank line in between has no indentation"
    );
    assert_eq!(app.buf_ref().line(3), "    ");
    // C-j: just a newline.
    press(&mut app, "C-j");
    assert_eq!(app.buf_ref().line(4), "");

    // TAB fixes a line wherever point is; M-m goes to the indentation.
    let mut app = rust_app("fn m() {\nx();\n        y();\n}\n");
    press(&mut app, "C-n C-e TAB");
    assert_eq!(app.buf_ref().line(1), "    x();");
    assert_eq!(
        app.buf_ref().point(),
        Pos::new(1, 8),
        "point stays after the text"
    );
    press(&mut app, "C-n TAB");
    assert_eq!(app.buf_ref().line(2), "    y();");
    press(&mut app, "C-e M-m");
    assert_eq!(app.buf_ref().point(), Pos::new(2, 4));
    press(&mut app, "C-/");
    assert_eq!(app.buf_ref().line(2), "        y();");

    // C-M-\ does a region.
    let mut app = rust_app("fn m() {\nif x {\ny();\n}\n}\n");
    press(&mut app, "C-x h C-M-\\");
    assert_eq!(text(&app), "fn m() {\n    if x {\n        y();\n    }\n}\n");
    assert_eq!(app.message.as_deref(), Some("Indenting region...done"));

    // Modes without rules keep the old TAB.
    let mut app = app_with("");
    press(&mut app, "a TAB RET");
    assert_eq!(text(&app), "a   \n");
}

#[test]
fn python_and_electric_indent_off() {
    let mut app = app_with("");
    app.buf().set_major_mode("python-mode");
    press(
        &mut app,
        "d e f SPC f ( ) : RET i f SPC x : RET p a s s RET",
    );
    assert_eq!(text(&app), "def f():\n    if x:\n        pass\n    ");
    press(&mut app, "C-a C-k e l s e :");
    assert_eq!(
        app.buf_ref().line(3),
        "    else:",
        "else: dedents to its if"
    );
    let mut app2 = rust_app("");
    app2.electric_indent = false;
    press(&mut app2, "{ RET }");
    assert_eq!(text(&app2), "{\n}");
}

#[test]
fn line_numbers_and_decoration_messages() {
    let mut app = rust_app("fn a() {}\nlet x = 1;\n");
    app.line_numbers = forge_core::decoration::LineNumbers::Absolute;
    let s = screen(&mut app, 30, 5);
    assert!(s.starts_with(" 1 fn a() {}"), "{s}");
    app.buf().set_option("line-numbers", "off").unwrap();
    assert!(screen(&mut app, 30, 5).starts_with("fn a() {}"));

    // Messages of decorations under point show in the echo area.
    app.buf().set_decorations(
        "lint",
        vec![(
            14,
            15,
            forge_core::decoration::DecoFace::Warning,
            Some("unused x".into()),
        )],
    );
    press(&mut app, "C-n C-f C-f C-f C-f");
    assert_eq!(app.message.as_deref(), Some("unused x"));
    press(&mut app, "C-f");
    assert_eq!(app.message, None);
}

// ------------------------------------------------ Phase 5: plugin permissions

#[test]
fn plugins_ask_before_getting_permissions() {
    let dir = std::env::temp_dir().join(format!("forge-perm-ui-{}", std::process::id()));
    let data = dir.join("templates");
    let path = demo_with_manifest(&dir, &data);
    let mut app = app_with("");
    app.grants = forge_core::GrantStore::load(dir.join("plugin-grants.toml")).unwrap();

    // It asks, showing what the plugin wants.
    press(&mut app, "M-x load-plugin RET");
    type_path(&mut app, &path);
    press(&mut app, "RET");
    assert_eq!(
        app.minibuffer.as_ref().unwrap().prompt,
        "Grant these permissions to demo? (y or n) "
    );
    let t = text(&app);
    assert_eq!(app.buf_ref().name(), "*Plugin Permissions*");
    assert!(t.contains("demo 1.2.0: Greets people"), "{t}");
    assert!(t.contains("network: open connections"), "{t}");
    assert!(
        t.contains(&format!(
            "files: read {} (greeting templates)",
            data.display()
        )),
        "{t}"
    );
    assert!(t.contains("forge:editor/buffers@0.1.0"), "{t}");
    assert!(
        !app.editor.commands.contains("demo-greet"),
        "nothing runs before the answer"
    );

    // y: granted, saved, loaded.
    press(&mut app, "y");
    assert!(app.editor.commands.contains("demo-greet"));
    assert!(app.grants.get("demo").network);
    let saved = std::fs::read_to_string(dir.join("plugin-grants.toml")).unwrap();
    assert!(
        saved.contains("[demo]") && saved.contains("network = true"),
        "{saved}"
    );

    // Loading it again doesn't ask.
    crate::plugins::load(&mut app, &path);
    assert!(app.minibuffer.is_none());
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .starts_with("Loaded plugin demo")
    );

    press(&mut app, "M-x list-plugins RET");
    let t = text(&app);
    assert!(
        t.contains("commands: demo-greet")
            && t.contains("network: open connections")
            && t.contains("memory limit: 128 MB"),
        "{t}"
    );

    // Revoke: it asks again; n loads it without; C-g doesn't load it.
    press(&mut app, "q M-x revoke-plugin-permissions RET demo RET");
    assert!(!app.grants.get("demo").network);
    press(&mut app, "M-x unload-plugin RET demo RET");
    crate::plugins::load(&mut app, &path);
    press(&mut app, "n");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("without the permissions it asked for"),
        "{:?}",
        app.message
    );
    assert!(app.editor.commands.contains("demo-greet"));
    assert!(!app.grants.get("demo").network);
    press(&mut app, "M-x unload-plugin RET demo RET");
    crate::plugins::load(&mut app, &path);
    press(&mut app, "C-g");
    assert_eq!(app.message.as_deref(), Some("Plugin demo not loaded"));
    assert!(!app.editor.commands.contains("demo-greet"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_plugin_without_a_manifest_loads_without_asking() {
    let dir = std::env::temp_dir().join(format!("forge-perm-none-{}", std::process::id()));
    let path = demo_plugin_file(&dir);
    let mut app = app_with("");
    crate::plugins::load(&mut app, &path);
    assert!(app.minibuffer.is_none());
    assert!(app.editor.commands.contains("demo-greet"));
    press(&mut app, "M-x list-plugins RET");
    assert!(text(&app).contains("permissions: none (no files, network or environment)"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn several_plugins_asking_take_turns() {
    let dir = std::env::temp_dir().join(format!("forge-perm-queue-{}", std::process::id()));
    let data = dir.join("t");
    let a = demo_with_manifest(&dir, &data);
    // A second copy under another name, asking the same.
    let b = dir.join("other.wasm");
    std::fs::copy(&a, &b).unwrap();
    std::fs::copy(dir.join("demo.toml"), dir.join("other.toml")).unwrap();
    let mut app = app_with("");
    crate::plugins::load(&mut app, &a);
    crate::plugins::load(&mut app, &b);
    assert_eq!(
        app.minibuffer.as_ref().unwrap().prompt,
        "Grant these permissions to demo? (y or n) "
    );
    // "other" registers the same command name, so its init fails after
    // it's granted: what matters is that it asked second.
    press(&mut app, "y");
    assert_eq!(
        app.minibuffer.as_ref().unwrap().prompt,
        "Grant these permissions to other? (y or n) "
    );
    press(&mut app, "C-g");
    assert!(app.minibuffer.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

// --------------------------------------- Phase 9: built-in core features

#[test]
fn line_numbers_wrapping_and_hl_line_modes() {
    let mut app = app_with("one two three four five six\nshort\n");
    press(&mut app, "M-x display-line-numbers-mode RET");
    assert!(
        screen(&mut app, 40, 5).starts_with(" 1 one two"),
        "{}",
        screen(&mut app, 40, 5)
    );
    press(&mut app, "C-u M-x display-line-numbers-mode RET");
    assert_eq!(
        app.buf_ref().option("line-numbers").as_deref(),
        Some("relative")
    );
    press(&mut app, "M-x display-line-numbers-mode RET");
    assert!(screen(&mut app, 40, 5).starts_with("one two"));

    // visual-line-mode wraps at words; C-n moves by screen row.
    press(&mut app, "M-x visual-line-mode RET");
    let s = screen(&mut app, 12, 6);
    let rows: Vec<&str> = s.lines().take(4).map(str::trim_end).collect();
    assert_eq!(rows, ["one two", "three four", "five six", "short"]);
    press(&mut app, "C-n");
    assert_eq!(point(&app), Pos::new(0, 8), "the second screen row");
    press(&mut app, "C-n C-n");
    assert_eq!(point(&app), Pos::new(1, 0));
    press(&mut app, "M-x toggle-truncate-lines RET");
    assert_eq!(app.buf_ref().option("wrap").as_deref(), Some("off"));

    press(&mut app, "M-x hl-line-mode RET");
    assert_eq!(
        app.message.as_deref(),
        Some("Hl-Line mode enabled in current buffer")
    );
    assert_eq!(app.buf_ref().option("hl-line").as_deref(), Some("true"));
}

#[test]
fn comments_pairs_and_whitespace() {
    let mut app = rust_app("fn a() {\n    x();\n}\n");
    press(&mut app, "C-n M-;");
    assert_eq!(app.buf_ref().line(1), "    x();  // ");
    press(&mut app, "C-a C-x C-;");
    assert_eq!(app.buf_ref().line(1), "    // x();  // ");
    assert_eq!(point(&app).line, 2, "comment-line moves down");
    press(&mut app, "C-p C-x C-;");
    assert_eq!(app.buf_ref().line(1), "    x();  // ");
    // Region: comment, then uncomment.
    press(&mut app, "M-< C-SPC C-n C-n M-;");
    assert_eq!(text(&app), "// fn a() {\n//     x();  // \n}\n");
    press(&mut app, "M-< C-SPC C-n C-n M-;");
    assert_eq!(text(&app), "fn a() {\n    x();  // \n}\n");

    // Trailing whitespace.
    let mut app = app_with("a  \nb\t\n\n\n");
    press(&mut app, "M-x delete-trailing-whitespace RET");
    assert_eq!(text(&app), "a\nb\n");

    // electric-pair-mode.
    let mut app = rust_app("");
    press(&mut app, "M-x electric-pair-mode RET");
    press(&mut app, "f ( x");
    assert_eq!(text(&app), "f(x)");
    press(&mut app, ")");
    assert_eq!(text(&app), "f(x)", "typing ) over the ) moves past it");
    assert_eq!(point(&app), Pos::new(0, 4));
    press(&mut app, "SPC [ DEL");
    assert_eq!(text(&app), "f(x) ", "DEL in [] deletes both");
    press(&mut app, "d o n ' t");
    assert_eq!(text(&app), "f(x) don't", "no pair after a letter");

    // electric-quote-mode: ` and ' become curved quotes, `` and '' double.
    let mut app = app_with("");
    press(&mut app, "M-x electric-quote-mode RET");
    press(&mut app, "`");
    assert_eq!(text(&app), "\u{2018}");
    press(&mut app, "`");
    assert_eq!(text(&app), "\u{201c}", "`` is a left double quote");
    press(&mut app, "DEL DEL");
    press(&mut app, "'");
    assert_eq!(text(&app), "\u{2019}");
    press(&mut app, "'");
    assert_eq!(text(&app), "\u{201d}", "'' is a right double quote");
    press(&mut app, "DEL DEL");
    // C-q types the character as it is.
    press(&mut app, "C-q '");
    assert_eq!(text(&app), "'");
    // A local mode toggles one buffer without the global one.
    press(&mut app, "M-x electric-quote-mode RET");
    assert!(!app.electric_quote);
    press(&mut app, "M-x electric-quote-local-mode RET");
    assert_eq!(
        app.buf_ref().option("electric-quote").as_deref(),
        Some("true")
    );
    press(&mut app, "DEL '");
    assert_eq!(text(&app), "\u{2019}");
    press(&mut app, "M-x electric-quote-local-mode RET");
    press(&mut app, "DEL '");
    assert_eq!(text(&app), "'", "with the mode off, ' is literal");
}

#[test]
fn dabbrev_counts_and_history() {
    let mut app = app_with("forge forget fo");
    press(&mut app, "C-e M-/");
    assert_eq!(text(&app), "forge forget forget");
    press(&mut app, "M-/");
    assert_eq!(text(&app), "forge forget forge");
    press(&mut app, "M-/");
    assert_eq!(text(&app), "forge forget fo", "back to what was typed");
    press(&mut app, "M-x count-words RET");
    assert_eq!(
        app.message.as_deref(),
        Some("Buffer has 1 line, 3 words, and 15 characters")
    );
    // M-= counts the region: mark it first.
    press(&mut app, "M-< C-SPC C-e M-=");
    assert_eq!(
        app.message.as_deref(),
        Some("Region has 1 line, 3 words, and 15 characters")
    );

    // M-p brings back earlier input.
    press(&mut app, "M-x goto-line RET 1 RET");
    press(&mut app, "M-g g M-p");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "1");
    press(&mut app, "M-n");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "");
    press(&mut app, "C-g M-x M-p");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "goto-line");
}

#[test]
fn places_recent_files_and_editorconfig() {
    let dir = std::env::temp_dir().join(format!("forge-p9-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".editorconfig"),
        "root = true\n[*.txt]\nindent_style = tab\nmax_line_length = 60\n",
    )
    .unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let session = dir.join("state/session.toml");
    let start = || {
        let mut app = app_with("");
        app.editor.session = forge_core::session::Session::load(Some(session.clone()));
        app
    };

    let mut app = start();
    press(&mut app, "C-x C-f");
    type_path(&mut app, &dir.join("a.txt"));
    press(&mut app, "RET C-n C-n C-f");
    assert_eq!(app.buf_ref().fill_column, 60, ".editorconfig applied");
    assert_eq!(app.buf_ref().options.indent_tabs, Some(true));
    app.editor.save_session();

    // Next session: the same place, the file in recentf, the history kept.
    drop(app);
    let mut app = start();
    press(&mut app, "M-x recentf-open RET");
    assert_eq!(
        app.minibuffer.as_ref().unwrap().prompt,
        "Open recent file: "
    );
    press(&mut app, "TAB");
    assert!(app.minibuffer.as_ref().unwrap().input.ends_with("a.txt"));
    press(&mut app, "RET");
    assert_eq!(point(&app), Pos::new(2, 1), "save-place");
    press(&mut app, "C-x C-f M-p");
    assert!(
        app.minibuffer.as_ref().unwrap().input.ends_with("a.txt"),
        "savehist"
    );
    press(&mut app, "C-g");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn imenu_and_occur() {
    let mut app = rust_app("struct S;\nfn helper() {}\nfn main() {\n    helper();\n}\n");
    press(&mut app, "M-g i m a TAB RET");
    assert_eq!(point(&app), Pos::new(2, 3));

    press(&mut app, "M-s o helper RET");
    assert_eq!(app.buf_ref().name(), "*Occur*");
    assert_eq!(
        text(&app),
        "2 matches for \"helper\" in buffer: *scratch*\n     2:fn helper() {}\n     4:    helper();\n"
    );
    press(&mut app, "C-n RET");
    assert_eq!(app.buf_ref().name(), "*scratch*");
    assert_eq!(point(&app), Pos::new(3, 0));
}
