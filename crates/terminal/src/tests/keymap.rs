// SPDX-License-Identifier: MIT OR Apache-2.0
//! keymap tests.

use super::*;

#[test]
fn keymap_layers_buffer_local_beats_minor_beats_major_beats_global() {
    let mut app = app_with("one two three");
    let km = &mut app.editor.keymaps;
    km.bind(
        &Scope::Mode("text-mode".into()),
        "C-f",
        "forward-word",
        Origin::Package,
    )
    .unwrap();
    km.bind(
        &Scope::Mode("foo-mode".into()),
        "C-f",
        "end-of-buffer",
        Origin::Package,
    )
    .unwrap();

    app.buf().set_major_mode("text-mode");
    press(&mut app, "C-f");
    assert_eq!(point(&app), Pos::new(0, 3), "major mode beats global");

    app.buf().enable_minor_mode("foo-mode");
    press(&mut app, "C-a C-f");
    assert_eq!(point(&app), Pos::new(0, 13), "minor mode beats major");

    // M-x local-set-key: read a key, then a command.
    press(&mut app, "M-x local-set-key RET C-f");
    assert!(
        app.minibuffer
            .as_ref()
            .unwrap()
            .prompt
            .starts_with("Set key C-f locally")
    );
    press(&mut app, "forward-char RET C-a C-f");
    assert_eq!(point(&app), Pos::new(0, 1), "buffer-local beats minor");

    // Other buffers don't see the local binding (or the modes).
    press(&mut app, "C-x b other RET");
    press(&mut app, "abc C-a C-f");
    assert_eq!(point(&app), Pos::new(0, 1));
    assert!(screen(&mut app, 60, 6).contains("(Fundamental)"));
    press(&mut app, "C-x b RET");
    assert!(screen(&mut app, 60, 6).contains("(Text Foo)"));
}

#[test]
fn describe_key_shows_command_map_and_docs() {
    let mut app = app_with("");
    press(&mut app, "C-h k");
    assert_eq!(app.message.as_deref(), Some("Describe key: "));
    press(&mut app, "C-x");
    assert_eq!(app.message.as_deref(), Some("Describe key: C-x-"));
    press(&mut app, "C-s");
    let help = help_text(&app);
    assert!(
        help.starts_with("C-x C-s runs the command save-buffer (found in global-map)"),
        "{help}"
    );
    assert!(
        help.contains("Key bindings:\n  C-x C-s            (global-map)"),
        "{help}"
    );
    assert!(help.contains("Menu:\n  File > Save"), "{help}");
    assert!(help.contains("Documentation:\n  Save the buffer"), "{help}");
    assert!(help.contains("Save the buffer to its file"));

    // Printable keys self-insert; unbound keys say so.
    press(&mut app, "C-h k a");
    assert!(help_text(&app).contains("a runs the command self-insert-command"));
    press(&mut app, "C-h k C-x z");
    assert_eq!(app.message.as_deref(), Some("C-x z is undefined"));
    // ESC works as Meta while reading keys, too.
    press(&mut app, "C-h c ESC f");
    assert_eq!(
        app.message.as_deref(),
        Some("M-f runs the command forward-word")
    );
}

#[test]
fn describe_key_briefly_function_and_where_is() {
    let mut app = app_with("");
    press(&mut app, "C-h c C-k");
    assert_eq!(
        app.message.as_deref(),
        Some("C-k runs the command kill-line")
    );

    press(&mut app, "C-h w kill-l TAB RET");
    assert_eq!(app.message.as_deref(), Some("kill-line is on C-k"));
    press(&mut app, "C-h w undo RET");
    assert_eq!(app.message.as_deref(), Some("undo is on C-/, C-_, C-x u"));
    press(&mut app, "C-h w describe-mode RET");
    assert_eq!(app.message.as_deref(), Some("describe-mode is on C-h m"));
    press(&mut app, "C-h w global-set-key RET");
    assert_eq!(
        app.message.as_deref(),
        Some("global-set-key is not on any key")
    );

    press(&mut app, "C-h f goto-line RET");
    let help = help_text(&app);
    assert!(help.starts_with("goto-line is an interactive command"));
    assert!(help.contains("M-g g") && help.contains("M-g M-g"), "{help}");
    assert!(help.contains("Edit > Go To > Goto Line..."), "{help}");
}

#[test]
fn c_c_letter_is_for_the_user() {
    let mut app = app_with("abc");
    // Packages (built-in modes, plugins) can't take it...
    let err = app
        .editor
        .keymaps
        .bind(&Scope::Mode("m".into()), "C-c a", "yank", Origin::Package);
    assert!(err.is_err());
    // ...but the user can, and C-c counts as a prefix while reading the key.
    press(&mut app, "M-x global-set-key RET C-c");
    assert_eq!(app.message.as_deref(), Some("Set key globally: C-c-"));
    press(&mut app, "e move-end-of-line RET");
    assert_eq!(
        app.message.as_deref(),
        Some("C-c e now runs move-end-of-line globally")
    );
    press(&mut app, "C-c e");
    assert_eq!(point(&app), Pos::new(0, 3));
    // C-g cancels setting a key.
    press(&mut app, "M-x global-set-key RET C-g");
    assert!(app.key_reader.is_none());
    assert_eq!(app.message.as_deref(), Some("Quit"));
}

#[test]
fn prefix_hints_after_a_pause() {
    let mut app = app_with("");
    assert_eq!(app.idle_timeout(), None, "no timer without a prefix");
    press(&mut app, "C-x");
    assert!(app.idle_timeout().is_some());
    app.on_idle();
    assert!(app.show_prefix_hints);
    assert_eq!(app.idle_timeout(), None, "hints are already up");
    let hints = app.prefix_hints();
    assert!(hints.contains(&("C-s".to_string(), "save-buffer".to_string())));
    assert!(hints.contains(&("b".to_string(), "switch-to-buffer".to_string())));
    let s = screen(&mut app, 100, 60);
    assert!(
        s.contains("C-x-") && s.contains("save-buffer") && s.contains("switch-to-buffer"),
        "{s}"
    );

    // The next key finishes the sequence and hides the hints.
    press(&mut app, "h");
    assert!(!app.show_prefix_hints);
    assert!(app.pending.is_empty());

    // Sub-prefixes are shown as +prefix; a zero delay turns the timer off.
    press(&mut app, "M-g");
    app.on_idle();
    assert!(
        app.prefix_hints()
            .contains(&("g".to_string(), "goto-line".to_string()))
    );
    press(&mut app, "C-g");
    app.prefix_help_delay = std::time::Duration::ZERO;
    press(&mut app, "C-x");
    assert_eq!(app.idle_timeout(), None);
}

#[test]
fn c_h_after_prefix_lists_keys_immediately() {
    let mut app = app_with("");
    press(&mut app, "C-x C-h");
    assert!(app.show_prefix_hints);
    assert_eq!(
        app.pending,
        vec!["C-x".to_string()],
        "still waiting for the rest of C-x"
    );
    press(&mut app, "h");
    assert_eq!(app.last_command, "mark-whole-buffer");
    // Nested prefixes show +prefix.
    app.editor
        .keymaps
        .bind(&Scope::Global, "C-x 4 f", "find-file", Origin::User)
        .unwrap();
    press(&mut app, "C-x C-h");
    assert!(
        app.prefix_hints()
            .contains(&("4".to_string(), "+prefix".to_string()))
    );
    assert!(screen(&mut app, 80, 20).contains("+prefix"));
}

#[test]
fn command_remapping() {
    let mut app = app_with("one two three");
    app.editor
        .keymaps
        .remap(&Scope::Global, "forward-char", "forward-word");
    press(&mut app, "C-f");
    assert_eq!(point(&app), Pos::new(0, 3));
    assert_eq!(app.last_command, "forward-word");
    press(&mut app, "C-h c C-f");
    assert_eq!(
        app.message.as_deref(),
        Some("C-f runs the command forward-word (remapped from forward-char)")
    );
    press(&mut app, "C-h w forward-char RET");
    assert_eq!(
        app.message.as_deref(),
        Some("forward-char is remapped to forward-word, which is on C-f, M-f, <right>, C-<right>")
    );
    press(&mut app, "C-h k C-f");
    assert!(help_text(&app).contains("which was remapped from forward-char"));
    // M-x runs the command itself, not the remapped one.
    press(&mut app, "q C-a M-x forward-char RET");
    assert_eq!(point(&app), Pos::new(0, 1));
}

#[test]
fn shift_is_a_modifier_of_the_key_and_of_the_letter() {
    // Shift+a is a key of its own ("S-a") and the letter it makes is what
    // gets typed, whichever way the terminal reported it: as 'a' with the
    // Shift bit (the kitty keyboard protocol) or as 'A' with it.
    let mut app = app_with("");
    press(&mut app, "S-a S-b S-1");
    assert_eq!(
        text(&app),
        "AB!",
        "and Shift+1 is the character it makes, not the number"
    );
    // A binding in that spelling is what the keypress finds.
    let mut app = app_with("abc");
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Global,
            "S-a",
            "delete-char",
            Origin::User,
        )
        .unwrap();
    app.editor.refresh_key_context();
    press(&mut app, "S-a");
    assert_eq!(text(&app), "bc", "a key written S-a is bound by Shift+a");
    // And Ctrl+I is TAB, whichever name it arrives as.
    let tabbed = {
        let mut app = app_with("ab");
        press(&mut app, "TAB");
        text(&app)
    };
    let mut app = app_with("ab");
    press(&mut app, "C-i");
    assert_eq!(text(&app), tabbed, "Ctrl+I does what TAB does");
    // Binding Ctrl+I separately takes it over.
    app.editor
        .keymaps
        .bind(&forge_core::Scope::Global, "C-i", "kill-line", Origin::User)
        .unwrap();
    app.editor.refresh_key_context();
    // Point is after the indentation the first Ctrl+I inserted, so this
    // kills what was after it, leaving the indentation.
    press(&mut app, "C-i");
    assert_eq!(text(&app), "    ");
    // TAB is unaffected: it is the same key, but it still means itself.
    let mut app = app_with("ab");
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Global,
            "C-i",
            "upcase-word",
            Origin::User,
        )
        .unwrap();
    app.editor.refresh_key_context();
    press(&mut app, "TAB");
    assert_eq!(text(&app), "    ab", "TAB still indents");
    press(&mut app, "C-i");
    assert_eq!(
        text(&app),
        "    AB",
        "and Ctrl+I does what it was bound to, not what TAB is"
    );
}

#[test]
fn a_projects_bindings_are_the_projects_own_and_say_so() {
    let dir = std::env::temp_dir().join(format!("forge-project-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("a.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();

    let mut app = app_with("");
    let root = dir.canonicalize().unwrap();
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Project(root.clone()),
            "C-c C-p",
            "goto-line",
            Origin::Package,
        )
        .unwrap();
    let id = app.editor.open_file(&file).unwrap().0;
    app.switch_to(id);
    app.editor.refresh_key_context();
    assert!(
        app.editor
            .active_keymaps_now()
            .where_is("goto-line")
            .contains(&"C-c C-p".to_string()),
        "the file inside the project has the project's keys"
    );
    press(&mut app, "C-h b");
    assert!(
        help_text(&app).contains(&format!("Project ({})", root.display())),
        "and C-h b says which keymap they came from: {}",
        help_text(&app)
    );
    press(&mut app, "q");
    press(&mut app, "M-x list-projects RET");
    let listed = text(&app);
    assert!(listed.contains(&root.display().to_string()), "{listed}");
    assert!(listed.contains("C-c C-p"), "{listed}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_projects_init_rhai_asks_before_it_runs() {
    // A checkout with a .forge/init.rhai of its own. The code is the
    // directory's, so Forge asks before running it.
    let dir = std::env::temp_dir().join(format!("forge-project-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".forge")).unwrap();
    std::fs::write(
        dir.join(".forge/init.rhai"),
        r#"forge.keymap.set("C-c p", "forward-char", #{ project: true });"#,
    )
    .unwrap();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("a.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let init = dir.join("user-init.rhai");
    std::fs::write(&init, "").unwrap();
    let grants = dir.join("grants.toml");
    let scope = forge_core::Scope::Project(dir.canonicalize().unwrap());

    // Saying no leaves the project's keys out of the buffer.
    {
        let mut app = app_with("");
        // The user's own init.rhai first, so there is a Rhai state to run
        // the project's in.
        app.init_path = Some(init.clone());
        app.load_init_file();
        app.grants = forge_core::GrantStore::load(grants.clone()).unwrap();
        let id = app.editor.open_file(&file).unwrap().0;
        app.switch_to(id);
        let prompt = app
            .minibuffer
            .as_ref()
            .expect("the question is up")
            .prompt
            .clone();
        assert!(prompt.contains(".forge/init.rhai"), "{prompt}");
        assert!(prompt.contains("(y or n)"), "{prompt}");
        press(&mut app, "n");
        assert!(app.minibuffer.is_none());
        assert!(
            app.editor.keymaps.get(&scope).is_none(),
            "nothing ran, so nothing is bound"
        );
        assert!(
            !grants.exists(),
            "and saying no isn't remembered across sessions"
        );
    }

    // Saying yes runs it: the binding applies to the file, and the answer
    // is saved so the next session doesn't ask.
    {
        let mut app = app_with("");
        app.init_path = Some(init.clone());
        app.load_init_file();
        app.grants = forge_core::GrantStore::load(grants.clone()).unwrap();
        let id = app.editor.open_file(&file).unwrap().0;
        app.switch_to(id);
        press(&mut app, "y");
        assert_eq!(
            app.editor.keymaps.get(&scope).and_then(|m| m.get("C-c p")),
            Some("forward-char"),
            "the project's own bindings, for this project"
        );
        assert!(
            std::fs::read_to_string(&grants)
                .unwrap()
                .contains("[projects."),
            "and the answer was saved, in the projects section: {}",
            std::fs::read_to_string(&grants).unwrap()
        );
        // C-c p is the project's here: where_is says so.
        app.editor.refresh_key_context();
        assert!(
            app.editor
                .active_keymaps_now()
                .where_is("forward-char")
                .contains(&"C-c p".to_string()),
            "the key is found through the project's map"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_rhai_settings_bindings_commands_and_reload() {
    let dir = std::env::temp_dir().join(format!("forge-init-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("init.rhai");
    std::fs::write(
        &path,
        r#"
        forge.opt["prefix_help_delay"] = 0.25;
        forge.keymap.set("C-c l", "goto-line");
        forge.keymap.set("C-t", "undefined");
        forge.keymap.del("C-x C-b");
        forge.keymap.remap("kill-line", "kill-word");
        forge.keymap.set("x", "quit-window", #{ mode: "help-mode" });
        forge.command.create("shout", || {
          let b = forge.buffer.current();
          let lines = b.line_count();
          b.replace(1, 0, lines, b.line(lines).len(), b.text().to_upper());
        }, #{ desc: "Upcase the buffer." });
        forge.keymap.set("C-c s", "shout");
        forge.keymap.set("C-c z", "no-such-command");
        "#,
    )
    .unwrap();

    let mut app = app_with("ab cd");
    app.init_path = Some(path.clone());
    app.load_init_file();
    // Reported once plugins had their chance to define the command.
    assert!(app.message.is_none(), "{:?}", app.message);
    app.report_unknown_bindings();
    assert_eq!(app.prefix_help_delay, std::time::Duration::from_millis(250));
    assert!(
        app.message.as_deref().unwrap().contains("no-such-command"),
        "{:?}",
        app.message
    );

    press(&mut app, "C-t");
    assert_eq!(app.message.as_deref(), Some("C-t is undefined"));
    press(&mut app, "C-k");
    assert_eq!(text(&app), " cd", "C-k remapped to kill-word");
    press(&mut app, "C-x C-b");
    assert_eq!(app.message.as_deref(), Some("C-x C-b is undefined"));
    press(&mut app, "C-c s");
    assert_eq!(text(&app), " CD");
    press(&mut app, "C-/");
    assert_eq!(text(&app), " cd", "a Rhai command undoes in one step");
    press(&mut app, "C-c l");
    assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "Goto line: ");
    press(&mut app, "C-g C-h f shout RET");
    assert!(text(&app).contains("Defined in init.rhai"));
    press(&mut app, "x");
    assert_eq!(
        app.buf_ref().name(),
        "*scratch*",
        "help-mode key from init.rhai"
    );

    // Reloading picks up edits; removed bindings and commands go away.
    press(&mut app, "M-x local-set-key RET <right> backward-char RET");
    std::fs::write(&path, "forge.keymap.set(\"C-c l\", \"list-buffers\");").unwrap();
    press(&mut app, "M-x reload-init-file RET");
    assert!(
        app.message.as_deref().unwrap().starts_with("Loaded"),
        "{:?}",
        app.message
    );
    press(&mut app, "C-h c C-t");
    assert_eq!(
        app.message.as_deref(),
        Some("C-t runs the command transpose-chars")
    );
    press(&mut app, "C-h c C-c l");
    assert_eq!(
        app.message.as_deref(),
        Some("C-c l runs the command list-buffers")
    );
    press(&mut app, "C-h c <right>");
    assert_eq!(
        app.message.as_deref(),
        Some("<right> runs the command backward-char")
    );
    assert!(!app.editor.commands.contains("shout"));

    // An error names the line; the defaults still work.
    std::fs::write(
        &path,
        "forge.keymap.set(\"C-c l\", \"goto-line\");\nforge.keymap.set(\"C-foo\", \"x\");",
    )
    .unwrap();
    press(&mut app, "M-x reload-init-file RET");
    let m = app.message.clone().unwrap();
    assert!(
        m.contains("init.rhai") && m.contains("line 2") && m.contains("C-foo"),
        "{m}"
    );
    press(&mut app, "C-h c C-x C-s");
    assert_eq!(
        app.message.as_deref(),
        Some("C-x C-s runs the command save-buffer")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_commands_can_ask_questions() {
    let dir = std::env::temp_dir().join(format!("forge-init-ask-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("init.rhai");
    std::fs::write(&path, include_str!("../../../../docs/init.example.rhai")).unwrap();
    let mut app = app_with("");
    app.init_path = Some(path);
    app.load_init_file();
    assert!(
        app.message.is_none(),
        "the example has no problems: {:?}",
        app.message
    );

    press(&mut app, "C-u 2 C-c H");
    assert_eq!(app.minibuffer.as_ref().unwrap().prompt, "Greet whom? ");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "world");
    press(&mut app, "DEL DEL DEL DEL DEL Ada RET");
    assert_eq!(text(&app), "Hello, Ada!\nHello, Ada!\n");

    press(&mut app, "M-< C-c d");
    assert_eq!(text(&app), "Hello, Ada!\nHello, Ada!\nHello, Ada!\n");
    press(&mut app, "M-x count-rhai-words RET");
    assert_eq!(app.message.as_deref(), Some("6 words"));
    // C-g on a Rhai question just cancels.
    press(&mut app, "C-c H C-g");
    assert!(app.minibuffer.is_none());
    assert_eq!(
        app.editor.modes.major_for_file("todo.notes"),
        Some("notes-mode")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_old_keys_toml_points_to_init_rhai() {
    let dir = std::env::temp_dir().join(format!("forge-init-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keys.toml"), "[global]\n").unwrap();
    let mut app = app_with("");
    app.init_path = Some(dir.join("init.rhai"));
    app.load_init_file();
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("is no longer read"),
        "{:?}",
        app.message
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn help_buffers_use_special_mode_keys() {
    let mut app = app_with("text");
    press(&mut app, "C-h b");
    let help = help_text(&app);
    assert!(help.contains("Global bindings (global-map)"));
    assert!(help.contains("C-c followed by a letter is reserved"));
    assert!(screen(&mut app, 70, 8).contains("(Help)"));
    // In *Help*, C-h b lists the special-mode keys first.
    press(&mut app, "C-h b");
    assert!(help_text(&app).contains("Major mode bindings (special-mode-map)"));
    press(&mut app, "q");
    assert_eq!(app.buf_ref().name(), "*scratch*");
    assert_eq!(text(&app), "text", "q in *Help* doesn't insert anywhere");

    press(&mut app, "C-h m");
    assert!(help_text(&app).starts_with("Fundamental mode (fundamental-mode)"));
}

#[test]
fn describe_bindings_hides_shadowed_keys() {
    let mut app = app_with("");
    app.editor
        .keymaps
        .bind(
            &Scope::Mode("foo-mode".into()),
            "C-k",
            "kill-region",
            Origin::Package,
        )
        .unwrap();
    app.buf().enable_minor_mode("foo-mode");
    press(&mut app, "C-h b");
    let help = help_text(&app);
    let minor = help
        .find("Minor mode bindings (foo-mode-map)")
        .expect("minor section");
    let global = help.find("Global bindings").expect("global section");
    assert!(minor < global, "higher-priority maps first");
    assert!(help[minor..global].contains("  C-k                kill-region"));
    assert!(
        !help[global..].contains("kill-line"),
        "shadowed global C-k is left out"
    );
}

#[test]
fn describe_bindings_names_a_layers_condition() {
    let mut app = app_with("");
    app.editor.keymaps.add_layer(
        "region",
        forge_core::keymap::Condition::Not(Box::new(
            forge_core::keymap::Condition::MajorDerivesFrom("special-mode".into()),
        )),
    );
    app.editor
        .keymaps
        .bind(
            &Scope::Layer("region".into()),
            "C-k",
            "kill-region",
            Origin::Package,
        )
        .unwrap();
    press(&mut app, "C-h b");
    let help = help_text(&app);
    assert!(
        help.contains("Conditional layer (region), when not(major-derives-from(special-mode))"),
        "the condition is what says when the bindings apply: {help}"
    );
}

#[test]
fn describe_bindings_names_the_region_a_text_is_in() {
    let mut app = app_with("hello\n");
    app.editor
        .keymaps
        .bind(
            &Scope::Region("mine".into()),
            "C-k",
            "kill-region",
            Origin::Package,
        )
        .unwrap();
    // With point outside it the map is down, so nothing is listed.
    press(&mut app, "C-h b");
    assert!(
        !help_text(&app).contains("Region (mine)"),
        "a region map that isn't up isn't a layer of the buffer"
    );
    // Attach it to some text and put point there.
    app.editor
        .state
        .active_buffer_mut()
        .unwrap()
        .set_decoration_keymap("test", 0, 5, "mine");
    app.buf().set_point(Pos::new(0, 2));
    // A key bound in it answers where the text is, not everywhere.
    assert_eq!(
        app.editor.active_keymaps().lookup(&["C-k".to_string()]),
        forge_core::keymap::Lookup::Command {
            command: "kill-region".to_string(),
            scope: Scope::Region("mine".into()),
            remapped_from: None,
        },
        "the region map is above the global one"
    );
    // Move off the text and it's a global key again.
    app.buf().set_point(Pos::new(0, 5));
    assert_eq!(
        app.editor.active_keymaps().lookup(&["C-k".to_string()]),
        forge_core::keymap::Lookup::Command {
            command: "kill-line".to_string(),
            scope: Scope::Global,
            remapped_from: None,
        }
    );
    app.buf().set_point(Pos::new(0, 2));
    press(&mut app, "C-h b");
    assert!(
        help_text(&app).contains("Region (mine)"),
        "the region's keys are part of the buffer's keys"
    );
}

// ------------------------------------------------------ Phase 2: editing depth
