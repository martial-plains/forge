// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn editor() -> Editor {
    let mut ed = Editor::new();
    ed.create_buffer("*scratch*");
    ed.commands.register_builtin("goto-line", "");
    ed.commands.register_builtin("next-line", "");
    ed
}

fn global(ed: &Editor, keys: &str) -> Option<String> {
    ed.keymaps
        .get(&Scope::Global)
        .and_then(|m| m.get(keys))
        .map(String::from)
}

fn ok(out: &Outcome) {
    assert!(out.errors.is_empty(), "{:?}", out.errors);
}

const ASSERT: &str = r#"
fn assert(cond) { if !cond { throw "assertion failed"; } }
fn assert(cond, msg) { if !cond { throw msg; } }
"#;

fn load(rhai: &RhaiConfig, ed: &mut Editor, src: &str) -> Outcome {
    if src.contains("assert(") {
        let mut src = src.to_string();
        src.push('\n');
        src.push_str(ASSERT);
        rhai.load_str(ed, "init.rhai", &src)
    } else {
        rhai.load_str(ed, "init.rhai", src)
    }
}

fn eval(rhai: &RhaiConfig, ed: &mut Editor, src: &str) -> (Option<String>, Outcome) {
    if src.contains("assert(") {
        rhai.eval(ed, &format!("{ASSERT}\n{src}"))
    } else {
        rhai.eval(ed, src)
    }
}

/// The installed API exposes the namespaces init.rhai is documented to use.
#[test]
fn the_installed_api_exposes_the_namespaces() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let (v, out) = eval(
        &rhai,
        &mut ed,
        r#"
            [
              type_of(forge.opt), type_of(forge.keymap), type_of(forge.command),
              type_of(forge.ui), type_of(forge.buffer), type_of(forge.mode),
              type_of(forge.plugin), type_of(forge.project), type_of(forge.process),
              type_of(forge.fs), type_of(forge.http), type_of(forge.env),
              type_of(forge.system), type_of(forge.menu), type_of(forge.theme),
              type_of(forge.log),
            ]
            "#,
    );
    ok(&out);
    let v = v.unwrap_or_default();
    for name in [
        "Opt", "Keymap", "Command", "Ui", "Buffers", "Mode", "Plugin", "Project", "Process", "Fs",
        "Http", "Env", "System", "Menu", "Theme", "Log",
    ] {
        assert!(v.contains(name), "{name} missing from {v}");
    }
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.opt["mouse"] = true;
            forge.keymap.set("C-c a", "goto-line");
            forge.notify("api");
            assert(forge.keymap.get("C-c a") == "goto-line");
            "#,
    );
    ok(&out);
}

#[test]
fn options_keymaps_and_remaps() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.opt["prefix_help_delay"] = 0.25;
            forge.opt["mouse"] = false;
            assert(forge.opt["mouse"] == false && forge.opt["plugin_auto_reload"] == ());
            forge.opt["line_numbers"] = "relative";
            assert(forge.opt["line_numbers"] == "relative");
            forge.opt["electric_indent"] = false;
            let b = forge.buffer.current();
            b.set_option("indent-width", 2);
            assert(b.option("indent-width") == "2");
            assert(b.indent_line(1) == false, "fundamental-mode has no rules");
            forge.keymap.set("C-c g", "goto-line");
            forge.keymap.set("n", "next-line", #{ mode: "help-mode" });
            forge.keymap.set("C-c z", "no-such-command");
            forge.keymap.remap("kill-line", "kill-region");
            assert(forge.keymap.get("C-c g") == "goto-line");
            forge.keymap.del("C-c g");
            assert(forge.keymap.get("C-c g") == ());
            "#,
    );
    ok(&out);
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.buffer.current().set_option("line-numbers", "sideways");"#,
    );
    assert!(!out.errors.is_empty(), "a bad buffer option is refused");
    assert_eq!(
        rhai.settings(),
        Settings {
            prefix_help_delay: Some(0.25),
            mouse: Some(false),
            plugin_auto_reload: None,
            line_numbers: Some(LineNumbers::Relative),
            electric_indent: Some(false),
            wrap: None,
            package_index: None,
            message_log_max: None,
            save_interprogram_paste_before_kill: None,
            flags: Default::default(),
            numbers: Default::default(),
            strings: Default::default(),
            maps: Default::default(),
            mode_hooks: Vec::new(),
        }
    );
    assert_eq!(
        ed.keymaps
            .get(&Scope::Mode("help-mode".into()))
            .unwrap()
            .get("n"),
        Some("next-line")
    );
    assert_eq!(ed.keymaps.global.remap_of("kill-line"), Some("kill-region"));
    assert_eq!(
        rhai.unknown_bindings(&ed),
        ["C-c z is bound to unknown command no-such-command"]
    );
}

#[test]
fn the_regexp_options_are_accepted() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.opt["sentence_end"] = "[.?!]+ *";
            forge.opt["page_delimiter"] = "^---";
            forge.opt["paragraph_start"] = "^\\*";
            forge.opt["paragraph_separate"] = "[ \t]*$";
            "#,
    );
    ok(&out);
    let strings = &rhai.settings().strings;
    assert_eq!(
        strings.get("sentence_end").map(|v| v[0].as_str()),
        Some("[.?!]+ *")
    );
    assert_eq!(
        strings.get("page_delimiter").map(|v| v[0].as_str()),
        Some("^---")
    );
    assert_eq!(
        strings.get("paragraph_start").map(|v| v[0].as_str()),
        Some("^\\*")
    );
    assert_eq!(
        strings.get("paragraph_separate").map(|v| v[0].as_str()),
        Some("[ \t]*$")
    );
}

#[test]
fn the_display_property_api() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            let b = forge.buffer.current();
            b.insert("hello world");
            b.add_display_text_property(1, 0, 1, 5, ["height", 2.0]);
            let spec = b.get_display_property(1, 0, "height");
            assert(spec != (), "the spec is there");
            assert(spec[0] == "height" && spec[1] == 2.0, "name and value");
            assert(b.get_display_property(1, 6, "height") == (), "outside the range");
            b.add_display_text_property(1, 0, 1, 5, ["raise", 0.5]);
            assert(b.get_display_property(1, 0, "raise") != (), "a second spec");
            b.remove_display_text_property(1, 0, 1, 5, "height");
            assert(b.get_display_property(1, 0, "height") == (), "removed");
            assert(b.get_display_property(1, 0, "raise") != (), "the other stays");
            // An overlay's display property replaces the text property.
            let id = b.add_overlay(1, 0, 1, 5, ["raise", 0.5]);
            assert(b.get_display_property(1, 0, "raise") != ());
            assert(b.get_display_property(1, 0, "height") == ());
            b.remove_overlay(id);
            assert(b.get_display_property(1, 0, "raise") != (), "the text property is back");
            "#,
    );
    ok(&out);
}

#[test]
fn the_image_options_are_accepted() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.opt["image_animate_loop"] = true;
            "#,
    );
    ok(&out);
    assert_eq!(rhai.settings().flags.get("image_animate_loop"), Some(&true));
}

#[test]
fn a_prompt_scope_names_the_map_it_binds_into() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.keymap.set("q", "goto-line", #{ prompt: "goto-line" });
            forge.keymap.set("q", "next-line", #{ prompt: "dired" });
            forge.keymap.set("C-f", "next-line", #{ transient: "keypad" });
            "#,
    );
    ok(&out);
    let prompt = |name: &str| {
        ed.keymaps
            .get(&Scope::Prompt(name.into()))
            .and_then(|m| m.get("q"))
            .map(String::from)
    };
    assert_eq!(prompt("goto-line").as_deref(), Some("goto-line"));
    assert_eq!(
        prompt("dired").as_deref(),
        Some("next-line"),
        "each prompt has its own map"
    );
    assert_eq!(
        ed.keymaps
            .get(&Scope::Transient("keypad".into()))
            .and_then(|m| m.get("C-f")),
        Some("next-line"),
        "a transient map is still reachable"
    );
}

#[test]
fn a_region_scope_binds_where_its_text_is() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.keymap.set("<mouse-1>", "forward-char", #{ region: "dired-entry" });
            "#,
    );
    ok(&out);
    assert_eq!(
        ed.keymaps
            .get(&Scope::Region("dired-entry".into()))
            .and_then(|m| m.get("<mouse-1>")),
        Some("forward-char")
    );
    assert_eq!(ed.keymaps.region_names(), ["dired-entry"]);
}

#[test]
fn a_project_scope_binds_for_a_directory() {
    let mut ed = editor();
    let dir = std::env::temp_dir().join(format!("forge-rhai-project-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let file = dir.join("src/a.rs");
    std::fs::write(&file, "").unwrap();
    ed.open_file(&file).unwrap();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"
                forge.keymap.set("C-c p", "goto-line", #{{ project: "{}" }});
                forge.notify("root: " + forge.project.root());
                "#,
            dir.display().to_string().replace('\\', "/")
        ),
    );
    ok(&out);
    assert_eq!(
        ed.keymaps
            .get(&Scope::Project(dir.canonicalize().unwrap()))
            .and_then(|m| m.get("C-c p")),
        Some("goto-line")
    );
    // And the project's own init.rhai doesn't have to name itself.
    let out = rhai.load_project_file(
        &mut ed,
        &dir.canonicalize().unwrap(),
        r#"forge.keymap.set("C-c q", "next-line", #{ project: true });"#,
    );
    ok(&out);
    assert_eq!(
        ed.keymaps
            .get(&Scope::Project(dir.canonicalize().unwrap()))
            .and_then(|m| m.get("C-c q")),
        Some("next-line")
    );
    // A project map is matched by the file's path, not by markers:
    // naming a directory in init.rhai is enough, and it can be one with
    // no .forge or .git in it at all.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.keymap.set("C-c z", "next-line", #{ project: "/no/such/project" });"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(
        ed.keymaps
            .project_for(std::path::Path::new("/no/such/project/src/a.rs"))
            .and_then(|(_, m)| m.get("C-c z"))
            .map(String::from),
        Some("next-line".to_string())
    );
    assert!(
        ed.keymaps
            .project_for(std::path::Path::new("/no/such/other/a.rs"))
            .is_none(),
        "but not a file outside it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_layer_needs_its_condition_before_its_bindings_apply() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.keymap.set("C-f", "goto-line", #{ layer: "region" });
            forge.keymap.layer("region", "region-active");
            forge.keymap.layer("quoted", "not(major-derives-from(special-mode))");
            "#,
    );
    ok(&out);
    assert_eq!(
        ed.keymaps
            .get(&Scope::Layer("region".into()))
            .and_then(|m| m.get("C-f")),
        Some("goto-line")
    );
    assert_eq!(
        ed.keymaps.layer_condition("region"),
        Some(&Condition::RegionActive)
    );
    assert_eq!(
        ed.keymaps.layer_condition("quoted"),
        Some(&Condition::Not(Box::new(Condition::MajorDerivesFrom(
            "special-mode".into()
        )))),
        "a condition with a combinator parses too"
    );
    // A layer with no condition never applies, so a bad name in a scope
    // table can't take a key away.
    ed.keymaps
        .bind(
            &Scope::Layer("typo".into()),
            "C-f",
            "goto-line",
            Origin::User,
        )
        .unwrap();
    assert_eq!(ed.keymaps.layer_condition("typo"), None);
}

#[test]
fn a_bad_condition_is_reported_where_it_is_set() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(&rhai, &mut ed, r#"forge.keymap.layer("a", "in-a-barrel");"#);
    assert!(
        out.errors[0].contains("unknown condition"),
        "{:?}",
        out.errors
    );
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.keymap.layer("a", "major-derives-from()");"#,
    );
    assert!(out.errors[0].contains("one mode name"), "{:?}", out.errors);
}

#[test]
fn options_are_checked_where_they_are_set() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        "forge.opt[\"mouse\"] = true;\nforge.opt[\"mosue\"] = false;",
    );
    assert!(
        out.errors[0].contains("unknown option forge.opt[\"mosue\"]")
            && out.errors[0].contains("line 2"),
        "{:?}",
        out.errors
    );
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.opt["prefix_help_delay"] = "soon";"#,
    );
    assert!(
        out.errors[0].contains("must be a number"),
        "{:?}",
        out.errors
    );
    let out = load(&rhai, &mut ed, r#"let x = forge.opt["colour"];"#);
    assert!(out.errors[0].contains("unknown option"), "{:?}", out.errors);
}

#[test]
fn message_log_max_and_inhibit_message_options() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        "forge.opt[\"message_log_max\"] = 50;\nforge.opt[\"inhibit_message\"] = true;",
    ));
    let st = rhai.settings();
    assert_eq!(
        st.message_log_max,
        Some(crate::messages::MessageLogMax::Lines(50))
    );
    assert_eq!(st.flags.get("inhibit_message"), Some(&true));
    // Reading gives the value back the way it was set.
    let (out, _) = eval(&rhai, &mut ed, r#"type_of(forge.opt["message_log_max"])"#);
    assert!(
        out.unwrap_or_default().contains("i64"),
        "message_log_max reads back as a number"
    );
    ok(&load(
        &rhai,
        &mut ed,
        r#"forge.opt["message_log_max"] = true;"#,
    ));
    assert_eq!(
        rhai.settings().message_log_max,
        Some(crate::messages::MessageLogMax::Unlimited)
    );
    ok(&load(
        &rhai,
        &mut ed,
        r#"forge.opt["message_log_max"] = false;"#,
    ));
    assert_eq!(
        rhai.settings().message_log_max,
        Some(crate::messages::MessageLogMax::Off)
    );
    let out = load(&rhai, &mut ed, r#"forge.opt["message_log_max"] = "lots";"#);
    assert!(
        out.errors[0].contains("no limit") && out.errors[0].contains("number of lines"),
        "{:?}",
        out.errors
    );
}

#[test]
fn messages_buffer_and_with_temp_message() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ed.echo_area = Some("ready".into());
    let out = load(
        &rhai,
        &mut ed,
        r#"
            let b = forge.messages_buffer();
            assert(b.id() == forge.buffer.find("*Messages*").id(), "found by name");
            assert(forge.messages_buffer().id() == b.id(), "the same buffer each time");
            assert(b.name() == "*Messages*", "named *Messages*");
            assert(b.major_mode() == "messages-buffer-mode", "special buffer");

            let seen = ();
            forge.with_temp_message("working...", || {
                seen = forge.current_message();
                [1, 2]
            });
            assert(seen == "working...", "shown while the body runs");
            assert(forge.current_message() == "ready", "restored afterwards");
            "#,
    );
    ok(&out);
    // The body's results pass through, and the temp message is not logged.
    let (out, _) = eval(
        &rhai,
        &mut ed,
        r#"forge.with_temp_message("x", || [1, 2])[1]"#,
    );
    let shown = out.unwrap_or_default();
    assert!(shown.contains('2'), "{shown:?}");
    assert_eq!(ed.echo_area.as_deref(), Some("ready"));

    let id = ed.find_buffer("*Messages*").unwrap();
    assert_eq!(ed.state.buffers.get(&id).unwrap().name(), "*Messages*");
    assert_eq!(
        ed.state.buffers.get(&id).unwrap().major_mode(),
        "messages-buffer-mode"
    );
}

#[test]
fn with_temp_message_restores_the_message_after_an_error() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ed.echo_area = Some("ready".into());
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.with_temp_message("working...", || { throw "boom"; });"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("boom")),
        "{:?}",
        out.errors
    );
    assert_eq!(ed.echo_area.as_deref(), Some("ready"));
}

#[test]
fn keymap_set_takes_a_function() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            forge.keymap.set("C-c t", |n| {
                let count = if n == () { 1 } else { n };
                forge.buffer.current().insert("t" + count);
            }, #{ desc: "Insert t." });
            "#,
    ));
    let cmd = global(&ed, "C-c t").unwrap();
    assert_eq!(cmd, "<rhai:C-c_t>");
    assert_eq!(ed.commands.get(&cmd).unwrap().doc, "Insert t.");
    ok(&rhai.run_command(&mut ed, &cmd, Some(3)));
    assert_eq!(ed.state.active_buffer().unwrap().get_contents(), "t3");
}

#[test]
fn errors_name_the_file_and_line() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        "forge.keymap.set(\"C-c g\", \"goto-line\");\nforge.keymap.set(\"C-foo\", \"x\");\nforge.keymap.set(\"C-c h\", \"goto-line\");",
    );
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    assert!(
        out.errors[0].contains("C-foo") && out.errors[0].contains("line 2"),
        "{:?}",
        out.errors
    );
    assert!(global(&ed, "C-c g").is_some() && global(&ed, "C-c h").is_none());
    let out = load(&rhai, &mut ed, "@@@");
    assert!(!out.errors.is_empty());
    assert!(out.errors[0].contains("line 1"), "{:?}", out.errors);
}

#[test]
fn sandbox_has_no_files_or_os() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let (v, out) = eval(&rhai, &mut ed, r#""ok".to_upper() + (2 + 1).to_string()"#);
    ok(&out);
    assert_eq!(v.as_deref(), Some("OK3"));
    // No raw files, no loading code, no host OS.
    for src in [
        r#"eval("1 + 1")"#,
        r#"import "x""#,
        r#"require("x")"#,
        r#"os.time()"#,
        r#"io.open("/etc/passwd")"#,
    ] {
        let out = load(&rhai, &mut ed, src);
        assert!(!out.errors.is_empty(), "{src} should fail");
    }
}

#[test]
fn runaway_code_is_stopped() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let t = Instant::now();
    let out = load(&rhai, &mut ed, "while true { }");
    assert!(out.errors[0].contains("terminated"), "{:?}", out.errors);
    assert!(t.elapsed() < TIME_LIMIT + Duration::from_secs(1));
    let (v, _) = eval(&rhai, &mut ed, "1 + 1");
    assert_eq!(v.as_deref(), Some("2"));
}

#[test]
fn a_watchdog_can_stop_rhai_from_another_thread() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let m = rhai.monitor();
    let t = std::thread::spawn(move || {
        while m.busy().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        m.interrupt();
    });
    let out = load(&rhai, &mut ed, "while true { }");
    t.join().unwrap();
    assert!(out.errors[0].contains("terminated"), "{:?}", out.errors);
    assert!(rhai.monitor().busy().is_none());
}

#[test]
fn commands_and_ui_questions() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            forge.command.create("greet", |n| {
              forge.ui.input(#{ prompt: "Name: ", "default": "you" }, |name| {
                if name != () {
                  let count = if n == () { 1 } else { n };
                  for i in 0..count {
                    forge.buffer.current().insert("Hi, " + name + "\n");
                  }
                  print("greeted " + name);
                }
              });
            }, #{ desc: "Insert a greeting." });
            forge.command.create("pick", || {
              forge.ui.select(["red", "green"], #{ prompt: "Colour: " }, |item, i| {
                forge.notify(item.to_string() + " " + i.to_string());
              });
            });
            "#,
    ));
    assert_eq!(ed.commands.get("greet").unwrap().doc, "Insert a greeting.");

    let out = rhai.run_command(&mut ed, "greet", Some(2));
    let request = match &out.effects[..] {
        [
            Effect::Ask {
                plugin,
                request,
                question: Question::Text { prompt, initial },
            },
        ] => {
            assert_eq!(
                (plugin.as_str(), prompt.as_str(), initial.as_str()),
                (SOURCE, "Name: ", "you")
            );
            *request
        }
        other => panic!("{other:?}"),
    };
    let out = rhai.answer(&mut ed, request, Some("Ada"));
    ok(&out);
    assert_eq!(
        ed.state.active_buffer().unwrap().get_contents(),
        "Hi, Ada\nHi, Ada\n"
    );
    assert_eq!(out.logs[0].1.message, "greeted Ada");
    assert!(
        rhai.answer(&mut ed, request, Some("x")).effects.is_empty(),
        "answered once only"
    );

    let out = rhai.run_command(&mut ed, "pick", None);
    let request = match &out.effects[..] {
        [
            Effect::Ask {
                request,
                question:
                    Question::Completing {
                        candidates,
                        require_match,
                        ..
                    },
                ..
            },
        ] => {
            assert_eq!(candidates, &["red", "green"]);
            assert!(require_match);
            *request
        }
        other => panic!("{other:?}"),
    };
    let out = rhai.answer(&mut ed, request, Some("green"));
    assert_eq!(out.effects, [Effect::Message("green 2".into())]);
}

#[test]
fn ui_select_can_read_several_values() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            forge.command.create("pick-many", || {
              forge.ui.select(["red", "green", "blue"],
                #{ prompt: "Colours: ", multiple: true },
                |items| {
                  if items == () {
                    forge.notify("cancelled");
                  } else {
                    let text = "";
                    for item in items {
                      if text != "" { text += "|"; }
                      text += item;
                    }
                    forge.notify(text);
                  }
                });
            });
            "#,
    ));

    let out = rhai.run_command(&mut ed, "pick-many", None);
    let request = match &out.effects[..] {
        [
            Effect::Ask {
                request,
                question:
                    Question::Completing {
                        candidates,
                        require_match,
                        multiple,
                        ..
                    },
                ..
            },
        ] => {
            assert_eq!(candidates, &["red", "green", "blue"]);
            assert!(require_match);
            assert!(multiple, "the prompt reads several values");
            *request
        }
        other => panic!("{other:?}"),
    };

    // The list arrives as a Rhai array.
    let out = rhai.answer_list(
        &mut ed,
        request,
        Some(&["red".to_string(), "blue".to_string()]),
    );
    assert_eq!(out.effects, [Effect::Message("red|blue".into())]);

    // Cancelling (no answer) answers with (), not an error.
    let out = rhai.run_command(&mut ed, "pick-many", None);
    let request = match &out.effects[..] {
        [Effect::Ask { request, .. }] => *request,
        other => panic!("{other:?}"),
    };
    let out = rhai.answer_list(&mut ed, request, None);
    assert_eq!(out.effects, [Effect::Message("cancelled".into())]);
}

#[test]
fn notify_levels() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.notify("hello");
            forge.notify("careful", forge.log.levels()["WARN"]);
            "#,
    );
    ok(&out);
    assert_eq!(out.effects, [Effect::Message("hello".into())]);
    assert_eq!(
        out.logs[0].1,
        LogLine {
            level: Level::Warn,
            message: "careful".into()
        }
    );
}

#[test]
fn buffer_api_uses_mode_line_numbers() {
    let mut ed = editor();
    ed.state
        .active_buffer_mut()
        .unwrap()
        .set_contents("one\ntwo\nthree");
    let rhai = RhaiConfig::new().unwrap();
    let (v, out) = eval(
        &rhai,
        &mut ed,
        r#"
            let b = forge.buffer.current();
            b.set_point(2, 1);
            let p = b.point();
            b.replace(3, 0, 3, 5, "THREE");
            assert(b.search_backward("one"));
            assert(forge.buffer.list().len() == 1 && forge.buffer.find("*scratch*").id() == b.id());
            b.name() + " " + p[0] + ":" + p[1] + " " + b.line(3) + " " + b.line_count()
            "#,
    );
    ok(&out);
    assert_eq!(v.as_deref(), Some("*scratch* 2:1 THREE 3"));
    assert_eq!(ed.state.active_buffer().unwrap().point(), Pos::new(0, 0));
}

#[test]
fn modes_and_the_run_queue() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.mode.define_major("notes-mode", #{ parent: "fundamental-mode", extensions: ["notes"] });
            forge.mode.define_minor("zen-mode");
            forge.keymap.set("C-c C-n", "next-line", #{ mode: "notes-mode" });
            forge.command.run("goto-line");
            "#,
    );
    ok(&out);
    assert_eq!(ed.modes.major_for_file("todo.notes"), Some("notes-mode"));
    assert!(ed.modes.is_minor("zen-mode"));
    assert_eq!(out.effects, [Effect::RunCommand("goto-line".into())]);
    let out = load(&rhai, &mut ed, r#"forge.command.run("nope");"#);
    assert!(out.errors[0].contains("no command named nope"));
}

#[test]
fn plugin_config_from_init_rhai_is_json() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r##"forge.plugin.config("om", #{ star: ["a", "b"], off: false, n: 2, list: #{ "-": "x\"y" } });"##,
    ));
    assert_eq!(
        ed.plugin_config["om"],
        r#"{"list":{"-":"x\"y"},"n":2,"off":false,"star":["a","b"]}"#
    );
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.plugin.config("om", #{ f: || 1 });"#,
    );
    assert!(out.errors[0].contains("plugin config"), "{:?}", out.errors);
}

#[test]
fn plugin_grants_from_init_rhai() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            forge.plugin.grant("spell", #{ network: true, environment: true, directories: ["~/dict", #{ path: "/tmp/w", write: true }] });
            forge.plugin.grant("spell", #{ memory_mb: 512 });
            "#,
    ));
    let g = &rhai.plugin_grants()["spell"];
    assert!(g.network);
    assert!(g.environment);
    assert_eq!(g.memory_mb, Some(512));
    assert_eq!(g.directories.len(), 2);
    assert!(g.directories[1].write && !g.directories[0].write);
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.plugin.grant("x", #{ netwrok: true });"#,
    );
    assert!(
        out.errors[0].contains("unknown permission \"netwrok\""),
        "{:?}",
        out.errors
    );
}

#[test]
fn plugin_use_from_init_rhai() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            forge.plugin.declare("org-modern", #{
              git: "https://example.com/you/forge-org-modern",
              rev: "v0.2.0",
              grant: #{ network: true },
              config: #{ progress: false },
              hook: #{ "org-mode": "org-modern-mode" },
              on: #{ commands: ["org-modern-mode"], extensions: "org" },
              after: ["org"],
            });
            "#,
    ));
    let uses = rhai.plugin_uses();
    assert_eq!(uses.len(), 1);
    let u = &uses[0];
    assert_eq!(u.name, "org-modern");
    assert_eq!(u.rev.as_deref(), Some("v0.2.0"));
    assert!(u.grants.network);
    assert_eq!(
        u.hooks,
        [("org-mode".to_string(), "org-modern-mode".into())]
    );
    assert_eq!(u.on_commands, ["org-modern-mode"]);
    assert_eq!(u.on_extensions, ["org"]);
    assert_eq!(u.after, ["org"]);
    assert!(u.lazy());
    assert_eq!(ed.plugin_config["org-modern"], r#"{"progress":false}"#);
    assert!(rhai.plugin_grants()["org-modern"].network);
    // a source is optional: an already-installed plugin can be used by
    // name alone (config, grant, hook and on still apply)
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.plugin.declare("local", #{ config: #{ a: 1 }, on: #{ extensions: "local" } });"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let u = rhai
        .plugin_uses()
        .into_iter()
        .find(|u| u.name == "local")
        .expect("declared");
    assert!(u.git.is_none() && u.index.is_none() && u.url.is_none());
    assert_eq!(u.on_extensions, ["local"]);
    assert_eq!(ed.plugin_config["local"], r#"{"a":1}"#);
    // an unknown key is refused
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.plugin.declare("x", #{ git: "g", revv: "v1" });"#,
    );
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
}

#[test]
fn use_package_is_declare_with_binds_and_init() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            use_package("org-modern", #{
              index: "org-modern",
              commands: ["org-modern-mode", "org-modern-other"],
              modes: ["org-mode"],
              extensions: ["org"],
              hook: #{ "org-mode": "org-modern-mode" },
              bind: #{
                "C-c o": "org-modern-mode",
                "C-c n": "next-line",
              },
              grant: #{ network: true },
              config: #{ progress: false },
              after: ["org"],
              init: || { forge.opt["mouse"] = false; },
            });
            forge.plugin.use_package("second", #{ commands: "second-mode" });
            "#,
    ));
    let uses = rhai.plugin_uses();
    assert_eq!(uses.len(), 2, "{uses:?}");
    let u = &uses[0];
    assert_eq!(u.name, "org-modern");
    assert_eq!(u.index.as_deref(), Some("org-modern"));
    assert_eq!(u.on_commands, ["org-modern-mode", "org-modern-other"]);
    assert_eq!(u.on_modes, ["org-mode"]);
    assert_eq!(u.on_extensions, ["org"]);
    assert_eq!(
        u.hooks,
        [("org-mode".to_string(), "org-modern-mode".into())]
    );
    assert_eq!(u.after, ["org"]);
    assert!(u.grants.network);
    assert_eq!(ed.plugin_config["org-modern"], r#"{"progress":false}"#);
    assert_eq!(rhai.settings().mouse, Some(false), "init ran");
    assert_eq!(global(&ed, "C-c o").as_deref(), Some("org-modern-mode"));
    // A built-in doesn't become an autoload trigger.
    assert_eq!(global(&ed, "C-c n").as_deref(), Some("next-line"));
    assert_eq!(uses[1].on_commands, ["second-mode"]);

    let out = load(&rhai, &mut ed, r#"use_package("x", #{ ensur: true });"#);
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
    let out = load(
        &rhai,
        &mut ed,
        r#"use_package("x", #{ init: #{ mouse: false } });"#,
    );
    assert!(out.errors[0].contains("init"), "{:?}", out.errors);
}

#[test]
fn use_package_conditions_code_and_modes() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    ok(&load(
        &rhai,
        &mut ed,
        r#"
            use_package("off", #{ disabled: true, commands: "off-mode" });
            use_package("no", #{ when: false, commands: "no-mode" });
            use_package("fn-no", #{ when: || false, commands: "fn-no-mode" });
            use_package("un", #{ unless: true, commands: "un-mode" });
            use_package("needs", #{ requires: ["ghost"], commands: "needs-mode" });
            use_package("base", #{ commands: "base-mode" });
            use_package("yes", #{
              requires: "base",
              when: || true,
              preface: || { forge.opt["mouse"] = false; },
              custom: #{ prefix_help_delay: 3 },
              config: || { forge.opt["plugin_auto_reload"] = false; },
              bind_mode: #{ "org-mode": #{ "C-c y": "yes-mode" } },
          bind_prompt: #{ "isearch": #{ "M-e": "yes-mode" } },
          remap: #{ "next-line": "previous-line" },
              mode: #{ yes: "text-mode" },
              "catch": true,
              init: || { throw "boom"; },
            });
            use_package("eager", #{ commands: "eager-mode", demand: true });
            use_package("sys", #{ ensure_system_package: "sh" });
            "#,
    ));
    let names: Vec<_> = rhai.plugin_uses().into_iter().map(|u| u.name).collect();
    assert_eq!(names, ["base", "yes", "eager", "sys"]);
    let s = rhai.settings();
    assert_eq!(s.mouse, Some(false), "preface ran");
    assert_eq!(s.prefix_help_delay, Some(3.0), "custom");
    assert_eq!(s.plugin_auto_reload, Some(false), "config fn ran");
    assert!(!rhai.plugin_uses()[2].lazy(), "demand loads eagerly");
    assert_eq!(ed.modes.major_for_file("a.yes"), Some("text-mode"));
    let report = rhai.use_package_report().join("\n");
    assert!(report.contains("off: skipped, disabled"), "{report}");
    assert!(
        report.contains("needs: skipped, requires ghost"),
        "{report}"
    );
    assert!(
        report.contains("yes: init failed"),
        "catch logged: {report}"
    );
    assert!(report.contains("base: deferred"), "{report}");

    ok(&load(
        &rhai,
        &mut ed,
        r#"
        use_package("fancy", #{
          git: "https://example.com/e", pin: "v1", load_path: "~/dev/e",
          no_require: true,
          autoload: ["fancy-go"], diminish: #{ "tab-bar-mode": " T" },
          bind_star: #{ "C-c e": "fancy-go" },
          interpreter: #{ fancy: "text-mode" },
          magic: #{ "^%FANCY": "text-mode" },
          custom_face: #{ "comment": #{ italic: true } },
        });
        use_package("bounded", #{ index: "theme", version: "^1.2", freeze: true });
        "#,
    ));
    let u = rhai
        .plugin_uses()
        .into_iter()
        .find(|u| u.name == "fancy")
        .unwrap();
    assert_eq!(u.rev.as_deref(), Some("v1"));
    assert!(u.frozen);
    assert_eq!(u.load_path, ["~/dev/e"]);
    assert!(u.no_require);
    assert_eq!(u.on_commands, ["fancy-go"]);
    assert_eq!(global(&ed, "C-c e").as_deref(), Some("fancy-go"));
    assert_eq!(
        ed.modes
            .major_for_content("#!/usr/bin/env fancy")
            .as_deref(),
        Some("text-mode")
    );
    assert_eq!(
        ed.modes.major_for_content("%FANCY\nx").as_deref(),
        Some("text-mode")
    );
    assert_eq!(ed.modes.major_for_content("plain"), None);
    ok(&load(
        &rhai,
        &mut ed,
        r#"use_package("built", #{ vc: #{ url: "https://example.com/b", rev: "main",
             make: "cargo build -p b", lisp_dir: "crates/b", features: ["fast"] } });"#,
    ));
    let b = rhai
        .plugin_uses()
        .into_iter()
        .find(|u| u.name == "built")
        .unwrap();
    assert_eq!(b.git.as_deref(), Some("https://example.com/b"));
    assert_eq!(b.rev.as_deref(), Some("main"));
    assert_eq!(b.build.as_deref(), Some("cargo build -p b"));
    assert_eq!(b.git_dir.as_deref(), Some("crates/b"));
    assert_eq!(b.features, ["fast"]);
    let bounded = rhai
        .plugin_uses()
        .into_iter()
        .find(|u| u.name == "bounded")
        .unwrap();
    assert_eq!(bounded.version.as_deref(), Some("^1.2"));
    assert!(bounded.frozen);
    let out = load(&rhai, &mut ed, r#"use_package("bad", #{ freeze: "yes" });"#);
    assert!(out.errors[0].contains("freeze"), "{:?}", out.errors);
    let out = load(
        &rhai,
        &mut ed,
        r#"use_package("x", #{ vc: #{ url: "u", bogus: 1 } });"#,
    );
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
    let out = load(&rhai, &mut ed, r#"use_package("x", #{ defines: ["a"] });"#);
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
    let out = load(&rhai, &mut ed, r#"use_package("x", #{ bogus: 1 });"#);
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
    let out = load(
        &rhai,
        &mut ed,
        r#"use_package("x", #{ init: || { throw "no"; } });"#,
    );
    assert!(!out.errors.is_empty(), "uncaught init error is an error");
}

#[test]
fn buffer_open_visits_files() {
    let dir = std::env::temp_dir().join(format!("forge-rhai-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), "hi\n").unwrap();
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let path = dir
        .join("note.txt")
        .display()
        .to_string()
        .replace('\\', "/");
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"
            let b = forge.buffer.open("{path}");
            assert(b.name() == "note.txt");
            assert(b.text() == "hi\n");
            "#
        ),
    );
    ok(&out);
    assert!(ed.find_buffer("note.txt").is_some());
    // A missing file opens empty, the way C-x C-f does; only real
    // I/O failures are errors.
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"
            let b = forge.buffer.open("{dir}/fresh.txt");
            assert(b.name() == "fresh.txt" && b.text() == "");
            "#,
            dir = dir.display().to_string().replace('\\', "/"),
        ),
    );
    ok(&out);
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"forge.buffer.open("{}");"#,
            dir.display().to_string().replace('\\', "/")
        ),
    );
    assert!(
        out.errors.iter().any(|e| e.contains("cannot open")),
        "{:?}",
        out.errors
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fs_queues_effects_and_answers_them() {
    use crate::process::{FsAnswer, FsOp};
    let dir = std::env::temp_dir().join(format!("forge-rhai-fs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hello").unwrap();
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let path = dir.display().to_string().replace('\\', "/");
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"
            let got_text = ();
            let got_err = ();
            let req = forge.fs.read("{path}/a.txt", |text, err| {{
              got_text = text;
              got_err = err;
            }});
            "#
        ),
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [Effect::Fs { request, op }] => {
            assert!(matches!(op, FsOp::Read(_)));
            *request
        }
        other => panic!("a read effect: {other:?}"),
    };
    let out = rhai.answer_fs(&mut ed, request, FsAnswer::Read(Ok(b"hello".to_vec())));
    ok(&out);
    let (text, _) = eval(&rhai, &mut ed, "got_text");
    assert_eq!(text.as_deref(), Some("hello"));
    let (err, _) = eval(&rhai, &mut ed, "got_err");
    assert_eq!(err.as_deref(), Some("()"));
    // Writing works, and project files may not.
    let out = load(
        &rhai,
        &mut ed,
        &format!(
            r#"
            let werr = ();
            forge.fs.write("{path}/b.txt", "bye", |err| werr = err);
            "#
        ),
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [Effect::Fs { request, op }] => {
            assert!(matches!(op, FsOp::Write(..)));
            *request
        }
        other => panic!("a write effect: {other:?}"),
    };
    let out = rhai.answer_fs(&mut ed, request, FsAnswer::Written(Ok(())));
    ok(&out);
    let (err, _) = eval(&rhai, &mut ed, "werr");
    assert_eq!(err.as_deref(), Some("()"));
    let out = rhai.load_project_file(
        &mut ed,
        &dir,
        &format!(r#"forge.fs.write("{path}/c.txt", "x", || ());"#),
    );
    assert!(
        out.errors.iter().any(|e| e.contains("project init.rhai")),
        "{:?}",
        out.errors
    );
    assert!(!dir.join("c.txt").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn process_queues_effects_and_answers_them() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            let got = ();
            let req = forge.process.run("echo hi", #{ timeout: 5 }, |r| {
              got = [r.code, r.stdout, r.stderr];
            });
            "#,
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [
            Effect::Process {
                request, program, ..
            },
        ] => {
            assert!(matches!(
                program,
                crate::process::ProcessSpec::Shell(line) if line == "echo hi"
            ));
            *request
        }
        other => panic!("a process effect: {other:?}"),
    };
    let out = rhai.answer_process(&mut ed, request, Some(0), "hi\n".into(), "".into());
    ok(&out);
    let (code, _) = eval(&rhai, &mut ed, "got[0]");
    assert_eq!(code.as_deref(), Some("0"));
    let (text, _) = eval(&rhai, &mut ed, "got[1]");
    assert_eq!(text.as_deref(), Some("hi\n"));
    // Unknown options fail fast, and project files may not run things.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.process.run("x", #{ bogus: 1 }, || ());"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("unknown option")),
        "{:?}",
        out.errors
    );
    let out = rhai.load_project_file(
        &mut ed,
        std::env::temp_dir().as_path(),
        r#"forge.process.run("echo hi", || ());"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("project init.rhai")),
        "{:?}",
        out.errors
    );
}

#[test]
fn http_queues_effects_and_answers_them() {
    use crate::http::Response;
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            let got = ();
            let req = forge.http.get("https://example.com/a", #{ "User-Agent": "forge" }, |r, err| {
              got = [r.status, r.headers["Content-Type"], r.body, err];
            });
            "#,
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [
            Effect::Http {
                request,
                method,
                url,
                headers,
                body,
            },
        ] => {
            assert_eq!(method, "GET");
            assert_eq!(url, "https://example.com/a");
            assert_eq!(headers, &[("User-Agent".to_string(), "forge".to_string())]);
            assert!(body.is_none());
            *request
        }
        other => panic!("an http effect: {other:?}"),
    };
    let out = rhai.answer_http(
        &mut ed,
        request,
        Ok(Response {
            status: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: b"hello".to_vec(),
        }),
    );
    ok(&out);
    let (status, _) = eval(&rhai, &mut ed, "got[0]");
    assert_eq!(status.as_deref(), Some("200"));
    let (ct, _) = eval(&rhai, &mut ed, "got[1]");
    assert_eq!(ct.as_deref(), Some("text/plain"));
    let (body, _) = eval(&rhai, &mut ed, "got[2]");
    assert_eq!(body.as_deref(), Some("hello"));
    let (err, _) = eval(&rhai, &mut ed, "got[3]");
    assert_eq!(err.as_deref(), Some("()"));

    // A failure answers ((), err), and request() takes method/opts.
    let out = load(
        &rhai,
        &mut ed,
        r#"
            let got2 = ();
            let req2 = forge.http.request("POST", "https://example.com/b", #{ body: "data" }, |r, err| {
              got2 = [r, err];
            });
            "#,
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [
            Effect::Http {
                request,
                method,
                body,
                ..
            },
        ] => {
            assert_eq!(method, "POST");
            assert_eq!(body.as_deref(), Some(b"data".as_slice()));
            *request
        }
        other => panic!("a request effect: {other:?}"),
    };
    let out = rhai.answer_http(&mut ed, request, Err("connection refused".into()));
    ok(&out);
    let (r, _) = eval(&rhai, &mut ed, "got2[0]");
    assert_eq!(r.as_deref(), Some("()"));
    let (err, _) = eval(&rhai, &mut ed, "got2[1]");
    assert_eq!(err.as_deref(), Some("connection refused"));

    // cancel() drops the callback, so a later answer is a no-op.
    let out = load(
        &rhai,
        &mut ed,
        r#"
           let cancelled = false;
           let c = forge.http.get("https://example.com/c", || cancelled = true);
           forge.http.cancel(c);
           "#,
    );
    ok(&out);
    let request = match out.effects.as_slice() {
        [Effect::Http { request, .. }] => *request,
        other => panic!("an http effect: {other:?}"),
    };
    let out = rhai.answer_http(&mut ed, request, Err("gone".into()));
    ok(&out);
    let (cancelled, _) = eval(&rhai, &mut ed, "cancelled");
    assert_eq!(cancelled.as_deref(), Some("false"));

    // Unknown options fail fast, and project files may not make requests.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.http.request("GET", "https://x", #{ bogus: 1 }, || ());"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("unknown option")),
        "{:?}",
        out.errors
    );
    let out = rhai.load_project_file(
        &mut ed,
        std::env::temp_dir().as_path(),
        r#"forge.http.get("https://x", || ());"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("project init.rhai")),
        "{:?}",
        out.errors
    );
}

#[test]
fn env_reads_writes_and_feeds_processes() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.env.set("FORGE_TEST_A", "one");
            assert(forge.env.get("FORGE_TEST_A") == "one");
            // $NAME expands when asked (setenv's substitute).
            forge.env.set("FORGE_TEST_B", "$FORGE_TEST_A/two", true);
            assert(forge.env.get("FORGE_TEST_B") == "one/two");
            assert(forge.env.substitute("${FORGE_TEST_A}!") == "one!");
            // with() sets while the body runs, then puts things back.
            forge.env.with_env(#{ FORGE_TEST_A: "changed" }, || {
              assert(forge.env.get("FORGE_TEST_A") == "changed");
            });
            assert(forge.env.get("FORGE_TEST_A") == "one");
            // parse_path: an empty part is the current directory (false).
            let parts = forge.env.parse_path(forge.env.path_separator() + "/usr/bin");
            assert(parts[0] == false && parts[1] == "/usr/bin");
            // all()/initial() are lists of KEY=VALUE.
            assert(forge.env.all().len() > 0 && forge.env.initial().len() > 0);
            "#,
    );
    ok(&out);

    // A program inherits process-environment, with its own env on top.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.process.run("echo", #{ env: #{ FORGE_TEST_A: "override" } }, || ());"#,
    );
    ok(&out);
    match out.effects.as_slice() {
        [Effect::Process { env, .. }] => {
            let get = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
            assert_eq!(get("FORGE_TEST_A"), Some("override"));
            assert_eq!(get("FORGE_TEST_B"), Some("one/two"));
        }
        other => panic!("a process effect: {other:?}"),
    }

    // Setting () removes the variable.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.env.set("FORGE_TEST_B", ()); assert(forge.env.get("FORGE_TEST_B") == ());"#,
    );
    ok(&out);
}

#[test]
fn system_reports_where_it_runs() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"
            assert(forge.system.type_name().len() > 0);
            assert(forge.system.configuration().len() > 0);
            assert(forge.system.name().len() > 0);
            assert(forge.system.pid() > 0);
            assert(forge.system.null_device() == "/dev/null"
                || forge.system.null_device() == "NUL");
            assert(forge.system.invocation_name().len() > 0);
            assert(forge.system.installation_directory() == ());
            "#,
    );
    ok(&out);
}

#[test]
fn menus_from_rhai_come_and_go_with_reload() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    // Unknown menus fail the way unknown commands do: loudly.
    let out = load(
        &rhai,
        &mut ed,
        r#"forge.menu.extend("Missing", [#{ label: "X", command: "y" }]);"#,
    );
    assert!(
        out.errors.iter().any(|e| e.contains("no Missing menu")),
        "{:?}",
        out.errors
    );
    let out = load(
        &rhai,
        &mut ed,
        r#"
            forge.menu.define("Notes", [#{ label: "Greet", command: "next-line" }, #{ label: "", command: "" }]);
            forge.menu.extend_submenu("Notes", "More", [#{ label: "Again", command: "goto-line" }]);
            "#,
    );
    ok(&out);
    let notes = ed.menus.index_of("Notes").expect("the menu");
    assert_eq!(ed.menus.menu(notes).unwrap().items.len(), 3);
    // Reloading (menus.remove_plugin) takes Rhai's back out.
    ed.menus.remove_plugin(crate::rhai::SOURCE);
    assert!(ed.menus.index_of("Notes").is_none());
}

#[test]
fn themes_from_rhai_restyle_get_and_reset() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r##"
            assert(forge.theme.get("comment") == ());
            forge.theme.set("comment", #{ fg: "#6a9955", italic: true });
            assert(forge.theme.get("comment").fg == "#6a9955");
            assert(forge.theme.get("comment").italic == true);
            assert(forge.theme.get("comment").bg == ());
            assert(forge.theme.faces().len() > 30, "every face listed");
            forge.theme.reset("comment");
            assert(forge.theme.get("comment") == ());
            "##,
    );
    ok(&out);
    assert!(ed.theme.is_empty());
    let out = load(&rhai, &mut ed, r#"forge.theme.set("nope", #{});"#);
    assert!(!out.errors.is_empty(), "unknown faces fail");
    let out = load(
        &rhai,
        &mut ed,
        r##"forge.theme.set("comment", #{ fg: "chartreuse" });"##,
    );
    assert!(!out.errors.is_empty(), "bad colors fail");
}

#[test]
fn named_themes_from_rhai_define_load_describe_and_disable() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r##"
            forge.theme.define("solar", "warm", #{
                comment: #{ fg: "#6a9955", italic: true },
                keyword: #{ fg: "blue", bold: true },
            });
            forge.theme.define("plain", #{ comment: #{ fg: "#888888" } });
            assert(forge.theme.describe("solar") == "warm");
            assert(forge.theme.describe("plain") == "");
            assert(forge.theme.active() == ());
            let names = forge.theme.names();
            assert(names[0] == "plain" && names[1] == "solar");

            forge.theme.load("solar");
            assert(forge.theme.active() == "solar");
            assert(forge.theme.get("comment").fg == "#6a9955");
            assert(forge.theme.get("comment").italic == true);
            assert(forge.theme.get("keyword").fg != ());

            // Loading another theme replaces the first entirely.
            forge.theme.load("plain");
            assert(forge.theme.active() == "plain");
            assert(forge.theme.get("keyword") == ());
            assert(forge.theme.get("comment").fg == "#888888");

            forge.theme.disable();
            assert(forge.theme.active() == ());
            assert(forge.theme.get("comment") == ());
            "##,
    );
    ok(&out);
    assert_eq!(ed.themes.names(), vec!["plain", "solar"]);
    assert!(ed.theme.is_empty(), "disabled");
    assert_eq!(ed.themes.current(), None);
    let out = load(&rhai, &mut ed, r#"forge.theme.load("missing");"#);
    assert!(!out.errors.is_empty(), "unknown theme fails");
    let out = load(
        &rhai,
        &mut ed,
        r##"forge.theme.define("bad", #{ nope: #{ fg: "red" } });"##,
    );
    assert!(!out.errors.is_empty(), "unknown face fails");
}

#[test]
fn eval_pretty_prints_values() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let (v, _) = eval(&rhai, &mut ed, "[1, 2]");
    assert_eq!(v.as_deref(), Some("[1, 2]"));
    let (v, _) = eval(&rhai, &mut ed, r#"#{ a: "x", n: 1 }"#);
    assert_eq!(v.as_deref(), Some(r#"#{a: "x", n: 1}"#));
    let (v, _) = eval(&rhai, &mut ed, "1");
    assert_eq!(v.as_deref(), Some("1"));
    let (v, _) = eval(&rhai, &mut ed, r#""a""#);
    assert_eq!(v.as_deref(), Some("a"));
    let (v, _) = eval(&rhai, &mut ed, "()");
    assert_eq!(v.as_deref(), Some("()"));
    let (v, _) = eval(&rhai, &mut ed, "let x = 1;");
    assert_eq!(v.as_deref(), Some("()"), "statements answer unit");
    // Depth stops cycles from recursing forever.
    let (v, _) = eval(&rhai, &mut ed, "[[[[1]]]]");
    assert!(v.as_deref().is_some_and(|s| s.contains("[...]")), "{v:?}");
}

#[test]
fn use_package_has_no_on_key() {
    let mut ed = editor();
    let rhai = RhaiConfig::new().unwrap();
    let out = load(
        &rhai,
        &mut ed,
        r#"use_package("x", #{ on: #{ commands: "x-mode" } });"#,
    );
    assert!(out.errors[0].contains("unknown key"), "{:?}", out.errors);
}
