// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::declare::{DeferredCode, SkipReason, SystemPackageNeed, UseOutcome, UseReport};
use crate::theme::{FaceStyle, NamedTheme, all_faces, parse_color};
use rhai::NativeCallContext;
use std::cell::OnceCell;
use std::rc::{Rc, Weak};

#[derive(Clone)]
struct Forge;

macro_rules! api {
    ($name:ident) => {
        #[derive(Clone)]
        struct $name;
    };
}

api!(OptApi);
api!(KeymapApi);
api!(CommandApi);
api!(UiApi);
api!(MinibufferApi);
api!(CompletionApi);
api!(BufferApi);
api!(ModeApi);
#[cfg(feature = "tree-sitter")]
api!(GrammarApi);
api!(PluginApi);
api!(ProjectApi);
api!(ProcessApi);
api!(FsApi);
api!(HttpApi);
api!(EnvApi);
api!(SystemApi);
api!(MenuApi);
api!(ThemeApi);
api!(LogApi);

impl RhaiConfig {
    pub fn new() -> RhaiResult<Self> {
        let mut engine = Engine::new();
        engine.set_max_string_size(MEMORY_LIMIT);
        engine.set_max_array_size(MEMORY_LIMIT / 8);
        engine.set_max_map_size(MEMORY_LIMIT / 16);
        engine.set_max_expr_depths(256, 128);
        engine.disable_symbol("eval");
        engine.disable_symbol("import");
        engine.disable_symbol("require");

        let shared = Rc::new(Shared::default());
        let monitor = RhaiMonitor::default();
        let engine_cell = Rc::new(OnceCell::new());

        let progress_shared = shared.clone();
        let progress_interrupt = monitor.interrupt.clone();
        engine.on_progress(move |_| {
            if progress_interrupt.load(Ordering::SeqCst) {
                return Some(Dynamic::from("interrupted"));
            }
            match progress_shared.deadline.get() {
                Some(deadline) if Instant::now() >= deadline => {
                    Some(Dynamic::from("Rhai call exceeded the time limit"))
                }
                _ => None,
            }
        });

        let print_shared = shared.clone();
        engine.on_print(move |text| {
            print_shared.logs.borrow_mut().push(LogLine {
                level: Level::Info,
                message: text.to_string(),
            });
        });

        let mut cfg = RhaiConfig {
            engine: engine_cell.clone(),
            shared,
            monitor,
            scope: RefCell::new(RhaiScope::new()),
        };
        cfg.install_api(&mut engine, Rc::downgrade(&engine_cell));
        let _ = engine_cell.set(engine);
        cfg.scope.borrow_mut().push_constant("forge", Forge);
        Ok(cfg)
    }

    /// A configuration for collecting declarations: closures are recorded
    /// but not run, and nothing may prompt, install or write.
    pub fn collecting() -> RhaiResult<Self> {
        let cfg = Self::new()?;
        cfg.shared.collecting.set(true);
        Ok(cfg)
    }

    fn install_api(&mut self, engine: &mut Engine, engine_cell: Weak<OnceCell<Engine>>) {
        let shared = self.shared.clone();

        engine.register_type_with_name::<Forge>("Forge");
        engine.register_type_with_name::<RhaiBuffer>("Buffer");
        engine.register_type_with_name::<OptApi>("Opt");
        engine.register_type_with_name::<KeymapApi>("Keymap");
        engine.register_type_with_name::<CommandApi>("Command");
        engine.register_type_with_name::<UiApi>("Ui");
        engine.register_type_with_name::<MinibufferApi>("Minibuffer");
        engine.register_type_with_name::<CompletionApi>("Completion");
        engine.register_type_with_name::<BufferApi>("Buffers");
        engine.register_type_with_name::<ModeApi>("Mode");
        #[cfg(feature = "tree-sitter")]
        engine.register_type_with_name::<GrammarApi>("Grammar");
        engine.register_type_with_name::<PluginApi>("Plugin");
        engine.register_type_with_name::<ProjectApi>("Project");
        engine.register_type_with_name::<ProcessApi>("Process");
        engine.register_type_with_name::<FsApi>("Fs");
        engine.register_type_with_name::<HttpApi>("Http");
        engine.register_type_with_name::<EnvApi>("Env");
        engine.register_type_with_name::<SystemApi>("System");
        engine.register_type_with_name::<MenuApi>("Menu");
        engine.register_type_with_name::<ThemeApi>("Theme");
        engine.register_type_with_name::<LogApi>("Log");

        RhaiBuffer::register(engine);

        engine.register_get("opt", |_: &mut Forge| OptApi);
        engine.register_get("keymap", |_: &mut Forge| KeymapApi);
        engine.register_get("command", |_: &mut Forge| CommandApi);
        engine.register_get("ui", |_: &mut Forge| UiApi);
        engine.register_get("minibuffer", |_: &mut Forge| MinibufferApi);
        engine.register_get("completion", |_: &mut Forge| CompletionApi);
        engine.register_get("buffer", |_: &mut Forge| BufferApi);
        engine.register_get("mode", |_: &mut Forge| ModeApi);
        #[cfg(feature = "tree-sitter")]
        engine.register_get("grammar", |_: &mut Forge| GrammarApi);
        engine.register_get("plugin", |_: &mut Forge| PluginApi);
        engine.register_get("project", |_: &mut Forge| ProjectApi);
        engine.register_get("process", |_: &mut Forge| ProcessApi);
        engine.register_get("fs", |_: &mut Forge| FsApi);
        engine.register_get("http", |_: &mut Forge| HttpApi);
        engine.register_get("env", |_: &mut Forge| EnvApi);
        engine.register_get("system", |_: &mut Forge| SystemApi);
        engine.register_get("menu", |_: &mut Forge| MenuApi);
        engine.register_get("theme", |_: &mut Forge| ThemeApi);
        engine.register_get("log", |_: &mut Forge| LogApi);

        engine.register_fn("levels", |_: &mut LogApi| -> Map {
            let mut levels = Map::new();
            for (i, name) in ["DEBUG", "INFO", "WARN", "ERROR"].iter().enumerate() {
                levels.insert((*name).into(), Dynamic::from(i as i64));
            }
            levels
        });

        let s = shared.clone();
        engine.register_fn(
            "on",
            move |_: &mut Forge, event: ImmutableString, func: FnPtr| -> RhaiResult<()> {
                let event = event.to_string();
                if !crate::events::EVENT_NAMES.contains(&event.as_str()) {
                    return Err(err(format!(
                        "unknown event {event:?} (known: {})",
                        crate::events::EVENT_NAMES.join(", ")
                    )));
                }
                let callback = Callback::new(&s, func)?;
                s.handlers.borrow_mut().push((event, callback));
                if !s.tracking.get() {
                    s.with_editor(|ed| {
                        s.tracker.borrow_mut().scan(ed, true);
                        Ok(())
                    })?;
                    s.tracking.set(true);
                }
                Ok(())
            },
        );

        let s = shared.clone();
        engine.register_fn(
            "notify",
            move |_: &mut Forge, msg: ImmutableString| -> RhaiResult<()> { notify(&s, &msg, 1) },
        );
        let s = shared.clone();
        engine.register_fn(
            "notify",
            move |_: &mut Forge, msg: ImmutableString, level: i64| -> RhaiResult<()> {
                notify(&s, &msg, level)
            },
        );

        engine.register_fn("today", |_: &mut Forge| -> String {
            let (d, _) = crate::org_time::now_local();
            d.iso()
        });
        engine.register_fn("stamp", |_: &mut Forge| -> String {
            let (d, (h, m)) = crate::org_time::now_local();
            const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            format!("[{} {} {:02}:{:02}]", d.iso(), DAYS[d.weekday() % 7], h, m)
        });
        let s = shared.clone();
        engine.register_fn(
            "browse",
            move |_: &mut Forge, url: ImmutableString| -> RhaiResult<()> {
                s.effects
                    .borrow_mut()
                    .push(Effect::BrowseUrl(url.to_string()));
                Ok(())
            },
        );
        let s = shared.clone();
        engine.register_fn(
            "load",
            move |_: &mut Forge, path: ImmutableString| -> RhaiResult<()> {
                s.effects
                    .borrow_mut()
                    .push(Effect::LoadRhai(path.to_string()));
                Ok(())
            },
        );
        let s = shared.clone();
        engine.register_fn(
            "kill",
            move |_: &mut Forge, text: ImmutableString| -> RhaiResult<()> {
                s.effects.borrow_mut().push(Effect::Kill(text.to_string()));
                Ok(())
            },
        );
        let s = shared.clone();
        engine.register_fn(
            "message",
            move |_: &mut Forge, msg: ImmutableString| -> RhaiResult<()> {
                s.effects
                    .borrow_mut()
                    .push(Effect::Message(msg.to_string()));
                Ok(())
            },
        );

        let s = shared.clone();
        engine.register_fn(
            "current_message",
            move |_: &mut Forge| -> RhaiResult<Dynamic> {
                s.with_editor(|ed| Ok(ed.echo_area.clone()))
                    .map(|m| m.map(Dynamic::from).unwrap_or(Dynamic::UNIT))
            },
        );

        let s = shared.clone();
        engine.register_fn(
            "messages_buffer",
            move |_: &mut Forge| -> RhaiResult<RhaiBuffer> {
                let id = s.with_editor(|ed| {
                    Ok(match ed.find_buffer("*Messages*") {
                        Some(id) => id,
                        None => {
                            let mut b = Buffer::new("*Messages*");
                            b.set_major_mode("messages-buffer-mode");
                            ed.add_buffer(b)
                        }
                    })
                })?;
                Ok(RhaiBuffer {
                    id,
                    shared: s.clone(),
                })
            },
        );

        let s = shared.clone();
        let temp_engine_cell = engine_cell.clone();
        engine.register_fn(
            "with_temp_message",
            move |_: &mut Forge, text: ImmutableString, func: FnPtr| -> RhaiResult<Dynamic> {
                let saved = s.with_editor(|ed| Ok(ed.echo_area.clone()))?;
                s.with_editor(|ed| {
                    ed.echo_area = Some(text.to_string());
                    Ok(())
                })?;
                let result = call_now(&s, &temp_engine_cell, func, ());
                s.with_editor(|ed| {
                    ed.echo_area = saved;
                    Ok(())
                })?;
                result
            },
        );

        self.install_opt(engine);
        self.install_keymap(engine);
        self.install_command(engine);
        self.install_ui(engine);
        self.install_minibuffer(engine);
        self.install_completion(engine);
        self.install_buffer(engine);
        self.install_mode(engine);
        #[cfg(feature = "tree-sitter")]
        self.install_grammar(engine);
        self.install_plugin(engine, engine_cell.clone());
        self.install_project(engine);
        self.install_process(engine);
        self.install_fs(engine);
        self.install_http(engine);
        self.install_env(engine, engine_cell);
        self.install_system(engine);
        self.install_menu(engine);
        self.install_theme(engine);
    }

    fn install_opt(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_indexer_get(
            move |_: &mut OptApi, name: ImmutableString| -> RhaiResult<Dynamic> {
                opt_get(&s.settings.borrow(), &name)
            },
        );
        let s = self.shared.clone();
        engine.register_indexer_set(
            move |_: &mut OptApi, name: ImmutableString, value: Dynamic| -> RhaiResult<()> {
                opt_set(&mut s.settings.borrow_mut(), &name, value)
            },
        );
    }

    fn install_keymap(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "set",
            move |_: &mut KeymapApi, keys: ImmutableString, rhs: Dynamic| -> RhaiResult<()> {
                keymap_set(&s, &keys, rhs, None)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set",
            move |_: &mut KeymapApi,
                  keys: ImmutableString,
                  rhs: Dynamic,
                  opts: Map|
                  -> RhaiResult<()> { keymap_set(&s, &keys, rhs, Some(opts)) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "del",
            move |_: &mut KeymapApi, keys: ImmutableString| -> RhaiResult<()> {
                keymap_del(&s, &keys, None)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "del",
            move |_: &mut KeymapApi, keys: ImmutableString, opts: Map| -> RhaiResult<()> {
                keymap_del(&s, &keys, Some(opts))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "remap",
            move |_: &mut KeymapApi,
                  from: ImmutableString,
                  to: ImmutableString|
                  -> RhaiResult<()> { keymap_remap(&s, &from, &to, None) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "remap",
            move |_: &mut KeymapApi,
                  from: ImmutableString,
                  to: ImmutableString,
                  opts: Map|
                  -> RhaiResult<()> { keymap_remap(&s, &from, &to, Some(opts)) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "layer",
            move |_: &mut KeymapApi,
                  name: ImmutableString,
                  when: ImmutableString|
                  -> RhaiResult<()> {
                let condition = Condition::parse(&when).map_err(|e| e.to_string())?;
                s.with_editor(|ed| {
                    ed.keymaps.add_layer(&name, condition);
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "remove_layer",
            move |_: &mut KeymapApi, name: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| {
                    ed.keymaps.remove_layer(&name);
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "get",
            move |_: &mut KeymapApi, keys: ImmutableString| -> RhaiResult<Dynamic> {
                let seq = parse_keys(&keys).map_err(|e| err(e.to_string()))?;
                s.with_editor(|ed| {
                    Ok(match ed.active_keymaps().lookup(&seq) {
                        crate::keymap::Lookup::Command { command, .. } => Dynamic::from(command),
                        _ => Dynamic::UNIT,
                    })
                })
            },
        );
    }

    fn install_command(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "create",
            move |_: &mut CommandApi, name: ImmutableString, func: FnPtr| -> RhaiResult<()> {
                command_create(&s, &name, func, None)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "create",
            move |_: &mut CommandApi,
                  name: ImmutableString,
                  func: FnPtr,
                  opts: Map|
                  -> RhaiResult<()> { command_create(&s, &name, func, Some(opts)) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "run",
            move |_: &mut CommandApi, name: ImmutableString| -> RhaiResult<()> {
                command_run(&s, &name, None)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "run",
            move |_: &mut CommandApi, name: ImmutableString, arg: i64| -> RhaiResult<()> {
                command_run(&s, &name, Some(arg))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "exists",
            move |_: &mut CommandApi, name: ImmutableString| -> RhaiResult<bool> {
                s.with_editor(|ed| Ok(ed.commands.contains(&name)))
            },
        );
    }

    fn install_ui(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "input",
            move |_: &mut UiApi, then: FnPtr| -> RhaiResult<u32> { ui_input(&s, None, then) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "input",
            move |_: &mut UiApi, opts: Map, then: FnPtr| -> RhaiResult<u32> {
                ui_input(&s, Some(opts), then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "select",
            move |_: &mut UiApi, items: Vec<Dynamic>, then: FnPtr| -> RhaiResult<u32> {
                ui_select(&s, items, None, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "select",
            move |_: &mut UiApi, items: Vec<Dynamic>, opts: Map, then: FnPtr| -> RhaiResult<u32> {
                ui_select(&s, items, Some(opts), then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "confirm",
            move |_: &mut UiApi, prompt: ImmutableString, then: FnPtr| -> RhaiResult<u32> {
                let callback = Callback::new(&s, then)?;
                Ok(s.ask(
                    Question::YOrN {
                        prompt: prompt.to_string(),
                    },
                    Pending::Confirm(callback),
                ))
            },
        );
    }

    fn install_minibuffer(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "set_input",
            move |_: &mut MinibufferApi, text: ImmutableString| -> RhaiResult<()> {
                let open = s.with_editor(|ed| {
                    if ed.prompt_open {
                        ed.prompt_input = text.to_string();
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
                if open {
                    s.effects
                        .borrow_mut()
                        .push(Effect::SetPromptInput(text.to_string()));
                }
                Ok(())
            },
        );
        let s = self.shared.clone();
        engine.register_fn("active", move |_: &mut MinibufferApi| -> RhaiResult<bool> {
            s.with_editor(|ed| Ok(ed.prompt_open))
        });
        let s = self.shared.clone();
        engine.register_fn(
            "input",
            move |_: &mut MinibufferApi| -> RhaiResult<String> {
                s.with_editor(|ed| Ok(ed.prompt_input.clone()))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "history_kind",
            move |_: &mut MinibufferApi| -> RhaiResult<Dynamic> {
                s.with_editor(|ed| {
                    Ok(ed
                        .prompt_history
                        .clone()
                        .map(Dynamic::from)
                        .unwrap_or(Dynamic::UNIT))
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "on_open",
            move |_: &mut MinibufferApi, command: ImmutableString| -> RhaiResult<()> {
                let name = SOURCE;
                s.with_editor(|ed| {
                    if !ed.commands.contains(&command) {
                        return Err(format!("no command named {command}"));
                    }
                    ed.completion
                        .minibuffer_open_commands
                        .retain(|(p, _)| p != name);
                    ed.completion
                        .minibuffer_open_commands
                        .push((name.to_string(), command.to_string()));
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn("suspend", move |_: &mut MinibufferApi| -> RhaiResult<()> {
            let open = s.with_editor(|ed| Ok(ed.prompt_open))?;
            if open {
                s.effects.borrow_mut().push(Effect::SuspendPrompt);
            }
            Ok(())
        });
    }

    fn install_completion(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "group",
            move |_: &mut CompletionApi,
                  category: ImmutableString,
                  then: FnPtr|
                  -> RhaiResult<()> {
                let callback = Callback::new(&s, then)?;
                s.groups.borrow_mut().insert(category.to_string(), callback);
                s.with_editor(|ed| {
                    ed.completion
                        .groupers
                        .insert(category.to_string(), SOURCE.to_string());
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set_group_format",
            move |_: &mut CompletionApi, format: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| {
                    ed.completion.group_format = Some((format.to_string(), SOURCE.to_string()));
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set_layout",
            move |_: &mut CompletionApi, layout: ImmutableString, lines: i64| -> RhaiResult<()> {
                let layout = crate::completion::Layout::from_name(&layout).map_err(err)?;
                s.with_editor(|ed| {
                    ed.completion.layout =
                        Some((layout, lines.clamp(1, 50) as u32, SOURCE.to_string()));
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set_row_labels",
            move |_: &mut CompletionApi, labels: Vec<Dynamic>| -> RhaiResult<()> {
                let labels = labels
                    .iter()
                    .map(|l| as_string(l, "row label"))
                    .collect::<RhaiResult<Vec<_>>>()?;
                s.with_editor(|ed| {
                    ed.completion.row_labels = if labels.is_empty() {
                        None
                    } else {
                        Some((labels.clone(), SOURCE.to_string()))
                    };
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "jump_to_row",
            move |_: &mut CompletionApi, row: i64, accept: bool| -> RhaiResult<()> {
                let open = s.with_editor(|ed| Ok(ed.prompt_open))?;
                if open {
                    s.effects.borrow_mut().push(Effect::JumpToRow {
                        row: row.max(0) as u32,
                        accept,
                    });
                }
                Ok(())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "sessions",
            move |_: &mut CompletionApi| -> RhaiResult<rhai::Array> {
                let sessions = s.with_editor(|ed| Ok(ed.completion_sessions.clone()))?;
                Ok(sessions
                    .into_iter()
                    .map(|session| {
                        let mut m = Map::new();
                        m.insert("category".into(), Dynamic::from(session.category));
                        m.insert("prompt".into(), Dynamic::from(session.prompt));
                        m.insert("input".into(), Dynamic::from(session.input));
                        m.insert("total".into(), Dynamic::from(session.total as i64));
                        Dynamic::from_map(m)
                    })
                    .collect())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "resume",
            move |_: &mut CompletionApi, index: i64| -> RhaiResult<()> {
                s.effects
                    .borrow_mut()
                    .push(Effect::ResumeSession(index.max(0) as u32));
                Ok(())
            },
        );
    }

    fn install_buffer(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "current",
            move |_: &mut BufferApi| -> RhaiResult<RhaiBuffer> {
                let id = s.with_editor(|ed| {
                    ed.state
                        .active_buffer
                        .ok_or_else(|| "no current buffer".to_string())
                })?;
                Ok(RhaiBuffer {
                    id,
                    shared: s.clone(),
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "find",
            move |_: &mut BufferApi, name: ImmutableString| -> RhaiResult<Dynamic> {
                let id = s.with_editor(|ed| Ok(ed.find_buffer(&name)))?;
                Ok(id
                    .map(|id| {
                        Dynamic::from(RhaiBuffer {
                            id,
                            shared: s.clone(),
                        })
                    })
                    .unwrap_or(Dynamic::UNIT))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "list",
            move |_: &mut BufferApi| -> RhaiResult<Vec<Dynamic>> {
                let ids = s.with_editor(|ed| Ok(ed.buffer_ids()))?;
                Ok(ids
                    .into_iter()
                    .map(|id| {
                        Dynamic::from(RhaiBuffer {
                            id,
                            shared: s.clone(),
                        })
                    })
                    .collect())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "create",
            move |_: &mut BufferApi, name: ImmutableString| -> RhaiResult<RhaiBuffer> {
                let id = s.with_editor(|ed| Ok(ed.add_buffer(Buffer::new(name.to_string()))))?;
                Ok(RhaiBuffer {
                    id,
                    shared: s.clone(),
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "open_quiet",
            move |_: &mut BufferApi, path: ImmutableString| -> RhaiResult<RhaiBuffer> {
                use crate::permissions::expand_home;
                let path = expand_home(&path);
                let id = s.with_editor(|ed| {
                    let (id, _) = ed
                        .open_file(&path)
                        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
                    Ok(id)
                })?;
                Ok(RhaiBuffer {
                    id,
                    shared: s.clone(),
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "open",
            move |_: &mut BufferApi, path: ImmutableString| -> RhaiResult<RhaiBuffer> {
                use crate::permissions::expand_home;
                let path = expand_home(&path);
                let id = s.with_editor(|ed| {
                    let (id, _) = ed
                        .open_file(&path)
                        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
                    ed.show_buffer(id);
                    Ok(id)
                })?;
                Ok(RhaiBuffer {
                    id,
                    shared: s.clone(),
                })
            },
        );
    }

    fn install_mode(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "define_major",
            move |_: &mut ModeApi, name: ImmutableString| -> RhaiResult<()> {
                define_major(&s, &name, None)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "define_major",
            move |_: &mut ModeApi, name: ImmutableString, opts: Map| -> RhaiResult<()> {
                define_major(&s, &name, Some(opts))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "hook",
            move |_: &mut ModeApi, mode: ImmutableString, what: Dynamic| -> RhaiResult<()> {
                let command = match what.clone().try_cast::<ImmutableString>() {
                    Some(c) => c.to_string(),
                    None => match what.try_cast::<FnPtr>() {
                        Some(f) => {
                            let n = s.next_hook.get() + 1;
                            s.next_hook.set(n);
                            let name = format!("{mode}-hook-{n}");
                            let desc = format!("A function init.rhai added to {mode}'s hook.");
                            s.with_editor(|ed| {
                                ed.commands
                                    .register_config(&name, &desc)
                                    .map_err(|e| e.to_string())
                            })?;
                            let callback = Callback::new(&s, f)?;
                            s.commands.borrow_mut().insert(name.clone(), callback);
                            name
                        }
                        None => {
                            return Err(err("forge.mode.hook(mode, command name or function)"));
                        }
                    },
                };
                s.settings
                    .borrow_mut()
                    .mode_hooks
                    .push((mode.to_string(), command));
                Ok(())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "define_minor",
            move |_: &mut ModeApi, name: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| {
                    ed.modes
                        .define_minor(&name, Some(SOURCE))
                        .map_err(|e| e.to_string())
                })
            },
        );
    }

    #[cfg(feature = "tree-sitter")]
    fn install_grammar(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "register",
            move |_: &mut GrammarApi,
                  name: ImmutableString,
                  path: ImmutableString|
                  -> RhaiResult<String> { register_grammar_api(&s, name, path, None) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "register",
            move |_: &mut GrammarApi,
                  name: ImmutableString,
                  path: ImmutableString,
                  opts: Map|
                  -> RhaiResult<String> {
                register_grammar_api(&s, name, path, Some(opts))
            },
        );
    }

    fn install_plugin(&self, engine: &mut Engine, engine_cell: Weak<OnceCell<Engine>>) {
        let s = self.shared.clone();
        engine.register_fn(
            "grant",
            move |_: &mut PluginApi, name: ImmutableString, spec: Map| -> RhaiResult<()> {
                s.deny_while_collecting("forge.plugin.grant")?;
                let g = parse_grants(&spec)?;
                s.grants
                    .borrow_mut()
                    .entry(name.to_string())
                    .or_default()
                    .merge(&g);
                Ok(())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "config",
            move |_: &mut PluginApi, name: ImmutableString, spec: Dynamic| -> RhaiResult<()> {
                let json = rhai_to_json(&spec, 0)?;
                s.with_editor(|ed| {
                    ed.plugin_config.insert(name.to_string(), json);
                    Ok(())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "use",
            move |_: &mut PluginApi, name: ImmutableString, spec: Map| -> RhaiResult<()> {
                use_spec(&s, name.to_string(), &spec)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "declare",
            move |_: &mut PluginApi, name: ImmutableString, spec: Map| -> RhaiResult<()> {
                use_spec(&s, name.to_string(), &spec)
            },
        );
        let s = self.shared.clone();
        let cell = engine_cell.clone();
        engine.register_fn(
            "use_package",
            move |ctx: NativeCallContext, name: ImmutableString, spec: Map| -> RhaiResult<()> {
                use_package_spec(
                    &s,
                    &cell,
                    name.to_string(),
                    &spec,
                    ctx.call_position().line(),
                )
            },
        );
        let s = self.shared.clone();
        engine.register_fn("use_package_report", move || -> Vec<Dynamic> {
            s.use_log
                .borrow()
                .iter()
                .map(|report| Dynamic::from(report.message.clone()))
                .collect()
        });
        let s = self.shared.clone();
        let cell = engine_cell;
        engine.register_fn(
            "use_package",
            move |ctx: NativeCallContext,
                  _: &mut PluginApi,
                  name: ImmutableString,
                  spec: Map|
                  -> RhaiResult<()> {
                use_package_spec(
                    &s,
                    &cell,
                    name.to_string(),
                    &spec,
                    ctx.call_position().line(),
                )
            },
        );
    }

    fn install_project(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn("root", move |_: &mut ProjectApi| -> RhaiResult<Dynamic> {
            Ok(s.project_root()
                .map(|p| Dynamic::from(p.display().to_string()))
                .unwrap_or(Dynamic::UNIT))
        });
    }

    fn install_process(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "run",
            move |_: &mut ProcessApi, cmd: Dynamic, then: FnPtr| -> RhaiResult<u32> {
                process_run(&s, cmd, None, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "run",
            move |_: &mut ProcessApi, cmd: Dynamic, opts: Map, then: FnPtr| -> RhaiResult<u32> {
                process_run(&s, cmd, Some(opts), then)
            },
        );
    }

    fn install_fs(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "read",
            move |_: &mut FsApi, path: ImmutableString, then: FnPtr| -> RhaiResult<u32> {
                fs_op(&s, path.to_string(), FsKind::Read, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "write",
            move |_: &mut FsApi,
                  path: ImmutableString,
                  text: ImmutableString,
                  then: FnPtr|
                  -> RhaiResult<u32> {
                s.deny_while_collecting("forge.fs.write")?;
                if s.project.borrow().is_some() {
                    return Err(err(
                        "forge.fs.write isn't available in project init.rhai files",
                    ));
                }
                let request = s.reserve(Pending::Callback(Callback::new(&s, then)?));
                s.effects.borrow_mut().push(Effect::Fs {
                    request,
                    op: crate::process::FsOp::Write(
                        crate::permissions::expand_home(&path),
                        text.as_bytes().to_vec(),
                    ),
                });
                Ok(request)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "list",
            move |_: &mut FsApi, path: ImmutableString, then: FnPtr| -> RhaiResult<u32> {
                fs_op(&s, path.to_string(), FsKind::List, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "exists",
            move |_: &mut FsApi, path: ImmutableString, then: FnPtr| -> RhaiResult<u32> {
                fs_op(&s, path.to_string(), FsKind::Exists, then)
            },
        );
    }

    fn install_http(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "get",
            move |_: &mut HttpApi, url: ImmutableString, then: FnPtr| -> RhaiResult<u32> {
                http_get(&s, url.to_string(), None, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "get",
            move |_: &mut HttpApi,
                  url: ImmutableString,
                  headers: Map,
                  then: FnPtr|
                  -> RhaiResult<u32> {
                http_get(&s, url.to_string(), Some(headers), then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "request",
            move |_: &mut HttpApi,
                  method: ImmutableString,
                  url: ImmutableString,
                  opts: Map,
                  then: FnPtr|
                  -> RhaiResult<u32> {
                http_request(&s, method.to_string(), url.to_string(), Some(opts), then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "request",
            move |_: &mut HttpApi,
                  method: ImmutableString,
                  url: ImmutableString,
                  then: FnPtr|
                  -> RhaiResult<u32> {
                http_request(&s, method.to_string(), url.to_string(), None, then)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "cancel",
            move |_: &mut HttpApi, id: i64| -> RhaiResult<()> {
                s.answers.borrow_mut().remove(&(id as u32));
                Ok(())
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "cancel",
            move |_: &mut HttpApi, id: u32| -> RhaiResult<()> {
                s.answers.borrow_mut().remove(&id);
                Ok(())
            },
        );
    }

    fn install_env(&self, engine: &mut Engine, engine_cell: Weak<OnceCell<Engine>>) {
        let s = self.shared.clone();
        engine.register_fn(
            "get",
            move |_: &mut EnvApi, name: ImmutableString| -> RhaiResult<Dynamic> {
                s.with_editor(|ed| {
                    Ok(ed
                        .environment
                        .get(&name)
                        .map(|v| Dynamic::from(v.to_string()))
                        .unwrap_or(Dynamic::UNIT))
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set",
            move |_: &mut EnvApi, name: ImmutableString, value: Dynamic| -> RhaiResult<()> {
                env_set(&s, &name, value, false)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "set",
            move |_: &mut EnvApi,
                  name: ImmutableString,
                  value: Dynamic,
                  substitute: bool|
                  -> RhaiResult<()> { env_set(&s, &name, value, substitute) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "substitute",
            move |_: &mut EnvApi, text: ImmutableString| -> RhaiResult<String> {
                s.with_editor(|ed| Ok(ed.environment.substitute(&text)))
            },
        );
        let s = self.shared.clone();
        engine.register_fn("all", move |_: &mut EnvApi| -> RhaiResult<Vec<Dynamic>> {
            s.with_editor(|ed| {
                Ok(ed
                    .environment
                    .as_list()
                    .into_iter()
                    .map(Dynamic::from)
                    .collect())
            })
        });
        let s = self.shared.clone();
        engine.register_fn(
            "initial",
            move |_: &mut EnvApi| -> RhaiResult<Vec<Dynamic>> {
                s.with_editor(|ed| {
                    Ok(ed
                        .environment
                        .initial()
                        .iter()
                        .map(|(k, v)| Dynamic::from(format!("{k}={v}")))
                        .collect())
                })
            },
        );
        engine.register_fn("path_separator", |_: &mut EnvApi| -> String {
            crate::env::path_separator().to_string()
        });
        engine.register_fn(
            "parse_path",
            |_: &mut EnvApi, path: ImmutableString| -> Vec<Dynamic> {
                crate::env::parse_path(&path)
                    .into_iter()
                    .map(|part| match part {
                        Some(dir) => Dynamic::from(dir),
                        None => Dynamic::from(false),
                    })
                    .collect()
            },
        );
        let s = self.shared.clone();
        let cell = engine_cell.clone();
        engine.register_fn(
            "with",
            move |_: &mut EnvApi, vars: Map, body: FnPtr| -> RhaiResult<Dynamic> {
                env_with(&s, &cell, vars, body)
            },
        );
        let s = self.shared.clone();
        let cell = engine_cell.clone();
        engine.register_fn(
            "with_env",
            move |_: &mut EnvApi, vars: Map, body: FnPtr| -> RhaiResult<Dynamic> {
                env_with(&s, &cell, vars, body)
            },
        );
    }

    fn install_system(&self, engine: &mut Engine) {
        use crate::env;
        engine.register_fn("type_name", |_: &mut SystemApi| -> String {
            env::system_type().to_string()
        });
        engine.register_fn("kind", |_: &mut SystemApi| -> String {
            env::system_type().to_string()
        });
        engine.register_fn("configuration", |_: &mut SystemApi| -> String {
            env::system_configuration().to_string()
        });
        engine.register_fn("name", |_: &mut SystemApi| -> String { env::system_name() });
        engine.register_fn("pid", |_: &mut SystemApi| -> i64 { env::pid() as i64 });
        engine.register_fn("null_device", |_: &mut SystemApi| -> String {
            env::null_device().to_string()
        });
        engine.register_fn("invocation_name", |_: &mut SystemApi| -> String {
            env::invocation_name()
        });
        engine.register_fn("invocation_directory", |_: &mut SystemApi| -> Dynamic {
            env::invocation_directory()
                .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
                .unwrap_or(Dynamic::UNIT)
        });
        engine.register_fn("installation_directory", |_: &mut SystemApi| -> Dynamic {
            env::installation_directory()
                .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
                .unwrap_or(Dynamic::UNIT)
        });
        engine.register_fn("load_average", |_: &mut SystemApi| -> Dynamic {
            load_average(false)
        });
        engine.register_fn(
            "load_average",
            |_: &mut SystemApi, use_float: bool| -> Dynamic { load_average(use_float) },
        );
        engine.register_fn("tty_erase_char", |_: &mut SystemApi| -> Dynamic {
            env::tty_erase_char()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        });
    }

    fn install_menu(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "define",
            move |_: &mut MenuApi, title: ImmutableString, items: Dynamic| -> RhaiResult<()> {
                let items = menu_items(items)?;
                s.with_editor(|ed| {
                    ed.menus
                        .define(&title, items, SOURCE)
                        .map_err(|e| e.to_string())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "extend",
            move |_: &mut MenuApi, title: ImmutableString, items: Dynamic| -> RhaiResult<()> {
                let items = menu_items(items)?;
                s.with_editor(|ed| {
                    ed.menus
                        .extend(&title, items, SOURCE)
                        .map_err(|e| e.to_string())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "extend_submenu",
            move |_: &mut MenuApi,
                  menu: ImmutableString,
                  label: ImmutableString,
                  items: Dynamic|
                  -> RhaiResult<()> {
                let items = menu_items(items)?;
                s.with_editor(|ed| {
                    ed.menus
                        .extend_submenu(&menu, &label, items, SOURCE)
                        .map_err(|e| e.to_string())
                })
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "remove",
            move |_: &mut MenuApi, title: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| ed.menus.remove(&title, SOURCE).map_err(|e| e.to_string()))
            },
        );
    }

    fn install_theme(&self, engine: &mut Engine) {
        let s = self.shared.clone();
        engine.register_fn(
            "set",
            move |_: &mut ThemeApi, face: ImmutableString, style: Map| -> RhaiResult<()> {
                let style = style_arg(style)?;
                s.with_editor(|ed| ed.theme.set(&face, style).map_err(|e| e.to_string()))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "get",
            move |_: &mut ThemeApi, face: ImmutableString| -> RhaiResult<Dynamic> {
                let found = s.with_editor(|ed| Ok(ed.theme.get(&face).cloned()))?;
                let Some(found) = found else {
                    return Ok(Dynamic::UNIT);
                };
                Ok(Dynamic::from(style_map(&found)))
            },
        );
        let s = self.shared.clone();
        engine.register_fn("reset", move |_: &mut ThemeApi| -> RhaiResult<()> {
            s.with_editor(|ed| {
                ed.theme.clear();
                Ok(())
            })
        });
        let s = self.shared.clone();
        engine.register_fn(
            "reset",
            move |_: &mut ThemeApi, face: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| {
                    ed.theme.reset(&face);
                    Ok(())
                })
            },
        );
        engine.register_fn("faces", |_: &mut ThemeApi| -> Vec<Dynamic> {
            all_faces()
                .iter()
                .map(|s| Dynamic::from(s.to_string()))
                .collect()
        });
        let s = self.shared.clone();
        engine.register_fn(
            "define",
            move |_: &mut ThemeApi, name: ImmutableString, faces: Map| -> RhaiResult<()> {
                theme_define(&s, &name, "", faces)
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "define",
            move |_: &mut ThemeApi,
                  name: ImmutableString,
                  description: ImmutableString,
                  faces: Map|
                  -> RhaiResult<()> { theme_define(&s, &name, &description, faces) },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "load",
            move |_: &mut ThemeApi, name: ImmutableString| -> RhaiResult<()> {
                s.with_editor(|ed| ed.load_theme(&name).map_err(|e| e.to_string()))
            },
        );
        let s = self.shared.clone();
        engine.register_fn("disable", move |_: &mut ThemeApi| -> RhaiResult<()> {
            s.with_editor(|ed| {
                ed.disable_theme();
                Ok(())
            })
        });
        let s = self.shared.clone();
        engine.register_fn(
            "names",
            move |_: &mut ThemeApi| -> RhaiResult<Vec<Dynamic>> {
                s.with_editor(|ed| Ok(ed.themes.names().into_iter().map(Dynamic::from).collect()))
            },
        );
        let s = self.shared.clone();
        engine.register_fn(
            "describe",
            move |_: &mut ThemeApi, name: ImmutableString| -> RhaiResult<Dynamic> {
                s.with_editor(|ed| Ok(ed.themes.description(&name).map(str::to_string)))
                    .map(|d| d.map(Dynamic::from).unwrap_or(Dynamic::UNIT))
            },
        );
        let s = self.shared.clone();
        engine.register_fn("active", move |_: &mut ThemeApi| -> RhaiResult<Dynamic> {
            s.with_editor(|ed| Ok(ed.themes.current().map(str::to_string)))
                .map(|d| d.map(Dynamic::from).unwrap_or(Dynamic::UNIT))
        });
    }
}

const USE_PACKAGE_KEYS: &[&str] = &[
    // What to install and when to load it.
    "git",
    "vc",
    "index",
    "url",
    "sha256",
    "ensure",
    "ensure_system_package",
    "defer",
    "demand",
    "after",
    "commands",
    "modes",
    "extensions",
    "mode",
    // Whether to use it at all.
    "disabled",
    "when",
    "if",
    "unless",
    "requires",
    // Code and settings.
    "preface",
    "init",
    "config",
    "custom",
    "catch",
    // Keys, hooks, the mode line.
    "bind",
    "bind_mode",
    "bind_prompt",
    "remap",
    "hook",
    "delight",
    "grant",
    // Carried over from use-package.
    "pin",
    "freeze",
    "version",
    "load_path",
    "no_require",
    // Forge extensions.
    "build_env",
    "build_timeout_secs",
    "allow_history_rewrite",
    "magic",
    "magic_fallback",
    "interpreter",
    "custom_face",
    "diminish",
    "autoload",
    "bind_star",
];

const USE_PACKAGE_DECLARATION_KEYS: &[&str] = &[
    "git",
    "rev",
    "index",
    "url",
    "sha256",
    "grant",
    "hook",
    "on",
    "after",
    "version",
    "freeze",
    "allow_history_rewrite",
];

/// Is a `when`/`if`/`unless` value true? A function is called, except while
/// collecting declarations: then it is left unevaluated (`None`).
fn use_condition(
    s: &std::rc::Rc<Shared>,
    engine_cell: &Weak<OnceCell<Engine>>,
    value: &Dynamic,
    what: &str,
) -> RhaiResult<Option<bool>> {
    if let Some(f) = value.clone().try_cast::<FnPtr>() {
        if s.collecting.get() {
            return Ok(None);
        }
        let r = call_now(s, engine_cell, f, ())?;
        as_bool(&r, what).map(Some)
    } else {
        as_bool(value, what).map(Some)
    }
}

/// Run user code from a declaration; with `catch` an error is logged
/// instead of stopping the declaration.
fn use_code(
    s: &std::rc::Rc<Shared>,
    engine_cell: &Weak<OnceCell<Engine>>,
    name: &str,
    key: &str,
    value: &Dynamic,
    catch: bool,
) -> RhaiResult<()> {
    let func = value.clone().try_cast::<FnPtr>().ok_or_else(|| {
        err(format!(
            "use_package: {key} is a function, like || {{ ... }}"
        ))
    })?;
    match call_now(s, engine_cell, func, ()) {
        Ok(_) => Ok(()),
        Err(e) if catch => {
            s.use_log.borrow_mut().push(UseReport::note(
                name.to_string(),
                None,
                format!("{name}: {key} failed: {e}"),
            ));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The command that installs a system package with whatever package
/// manager this machine has.
fn system_install_command(package: &str) -> Option<String> {
    let have = |exe: &str| exe_on_path(exe);
    let quoted = format!("'{}'", package.replace('\'', "'\\''"));
    let (manager, args) = if cfg!(target_os = "macos") && have("brew") {
        ("brew", "install")
    } else if have("apt-get") {
        ("sudo apt-get", "install -y")
    } else if have("dnf") {
        ("sudo dnf", "install -y")
    } else if have("pacman") {
        ("sudo pacman", "-S --noconfirm")
    } else if have("zypper") {
        ("sudo zypper", "install -y")
    } else if have("apk") {
        ("sudo apk", "add")
    } else if have("brew") {
        ("brew", "install")
    } else if have("winget") {
        ("winget", "install")
    } else {
        return None;
    };
    Some(format!("{manager} {args} {quoted}"))
}

fn exe_on_path(exe: &str) -> bool {
    if exe.contains('/') || exe.contains('\\') {
        return std::path::Path::new(exe).exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(exe).is_file() || (cfg!(windows) && dir.join(format!("{exe}.exe")).is_file())
    })
}

/// `ensure_system_package`: a name ("rg"), a list, or a map of
/// executable (or a file path) to a package name or an install command.
/// Anything missing is installed in the background unless `spawn` is false;
/// returns what was wanted either way.
fn ensure_system_packages(value: &Dynamic, spawn: bool) -> RhaiResult<Vec<SystemPackageNeed>> {
    let mut wanted: Vec<(String, Option<String>)> = Vec::new();
    if let Some(m) = value.clone().try_cast::<Map>() {
        for (exe, how) in &m {
            wanted.push((
                exe.to_string(),
                Some(as_string(how, "ensure_system_package")?),
            ));
        }
    } else {
        for exe in name_or_names(value, "ensure_system_package")? {
            wanted.push((exe, None));
        }
    }
    let mut needs = Vec::new();
    for (exe, how) in wanted {
        if exe_on_path(&exe) {
            needs.push(SystemPackageNeed {
                executable: exe,
                command: None,
                present: true,
            });
            continue;
        }
        let command = match how {
            Some(h) if h.contains(char::is_whitespace) => Some(h),
            Some(package) => system_install_command(&package),
            None => system_install_command(&exe),
        };
        if spawn
            && !cfg!(test)
            && let Some(command) = &command
        {
            let (shell, flag) = if cfg!(windows) {
                ("cmd", "/C")
            } else {
                ("sh", "-c")
            };
            let _ = std::process::Command::new(shell)
                .args([flag, command])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        needs.push(SystemPackageNeed {
            executable: exe,
            command,
            present: false,
        });
    }
    Ok(needs)
}

/// `use_package(name, spec)`: a `forge.plugin.declare` plus the keys and
/// code a use-package-style declaration wants.
///
/// Whether: `disabled`, `when`/`if` (a boolean or a function), `unless`,
/// `requires` (plugins that must be declared first). Code: `preface` and
/// `init` run now, `config` is a map of plugin settings or a function run
/// after the declaration, `custom` sets `forge.opt` values, `catch` turns
/// an error in any of that into a line in `use_package_report()`. Keys:
/// `bind` (and `bind_mode` per mode) binds keys, adding commands nobody
/// has defined yet to `commands` so the key loads the plugin lazily;
/// `mode` maps file patterns to a major mode; `delight` changes a minor
/// mode's mode-line text; `defer`/`demand` choose lazy or eager loading.
fn use_package_spec(
    s: &std::rc::Rc<Shared>,
    engine_cell: &Weak<OnceCell<Engine>>,
    name: String,
    spec: &Map,
    line: Option<usize>,
) -> RhaiResult<()> {
    for key in spec.keys() {
        if !USE_PACKAGE_KEYS.contains(&key.as_str()) {
            let where_ = line.map(|n| format!(" (line {n})")).unwrap_or_default();
            let message = format!("{name}: skipped, unknown key {key:?}{where_}");
            s.use_log.borrow_mut().push(UseReport {
                name: name.clone(),
                line,
                message,
                outcome: UseOutcome::Error {
                    message: format!("unknown key {key:?}"),
                },
                deferred: Vec::new(),
                system_packages: Vec::new(),
            });
            return Err(err(format!(
                "unknown key {key:?} in use_package (known: {})",
                USE_PACKAGE_KEYS.join(", ")
            )));
        }
    }
    // spellings: autoload is commands, diminish is delight, bind*
    // is bind (Forge has no override map), pin is the git tag/branch/commit.
    let mut spec = spec.clone();
    let pinned = spec.contains_key("pin");
    for (from, to) in [
        ("autoload", "commands"),
        ("diminish", "delight"),
        ("bind_star", "bind"),
    ] {
        if let Some(v) = spec.remove(from) {
            if spec.contains_key(to) {
                if to == "commands" {
                    let mut on = Map::new();
                    merge_package_names(&mut on, "commands", spec.get("commands"))?;
                    merge_package_names(&mut on, "commands", Some(&v))?;
                    spec.insert(to.into(), on.remove("commands").unwrap_or_default());
                } else if let (Some(a), Some(b)) = (
                    spec.get(to).and_then(|x| x.clone().try_cast::<Map>()),
                    v.clone().try_cast::<Map>(),
                ) {
                    let mut a = a;
                    a.extend(b);
                    spec.insert(to.into(), Dynamic::from(a));
                } else {
                    return Err(err(format!("use_package: give {from} or {to}, not both")));
                }
            } else {
                spec.insert(to.into(), v);
            }
        }
    }
    if let Some(pin) = spec.remove("pin")
        && !spec.contains_key("rev")
    {
        spec.insert("rev".into(), pin);
    }
    let spec = &spec;
    let catch = match spec.get("catch") {
        Some(v) => as_bool(v, "catch")?,
        None => false,
    };
    let collecting = s.collecting.get();
    let mut deferred = Vec::new();
    let mut system_packages = Vec::new();
    let mut conditional = None;
    let skip = |why: &str, reason: SkipReason| {
        let message = format!("{name}: skipped, {why}");
        s.use_log.borrow_mut().push(UseReport {
            name: name.clone(),
            line,
            message,
            outcome: UseOutcome::Skipped { reason },
            deferred: Vec::new(),
            system_packages: Vec::new(),
        });
    };

    // Whether to use it at all.
    if let Some(v) = spec.get("disabled")
        && as_bool(v, "disabled")?
    {
        skip("disabled", SkipReason::Disabled);
        return Ok(());
    }
    for key in ["when", "if"] {
        if let Some(v) = spec.get(key) {
            match use_condition(s, engine_cell, v, key)? {
                Some(true) => {}
                Some(false) => {
                    skip(
                        &format!("{key} was false"),
                        if key == "when" {
                            SkipReason::WhenFalse
                        } else {
                            SkipReason::IfFalse
                        },
                    );
                    return Ok(());
                }
                None => {
                    conditional = Some(key.to_string());
                    if !deferred.contains(&DeferredCode::Condition) {
                        deferred.push(DeferredCode::Condition);
                    }
                }
            }
        }
    }
    if let Some(v) = spec.get("unless") {
        match use_condition(s, engine_cell, v, "unless")? {
            Some(false) => {}
            Some(true) => {
                skip("unless was true", SkipReason::UnlessTrue);
                return Ok(());
            }
            None => {
                conditional = Some("unless".to_string());
                if !deferred.contains(&DeferredCode::Condition) {
                    deferred.push(DeferredCode::Condition);
                }
            }
        }
    }
    let requires = match spec.get("requires") {
        Some(v) => name_or_names(v, "requires")?,
        None => Vec::new(),
    };
    {
        let declared: Vec<String> = s.uses.borrow().iter().map(|u| u.name.clone()).collect();
        for need in &requires {
            if !declared.contains(need) {
                let where_ = line.map(|n| format!(" (line {n})")).unwrap_or_default();
                let message =
                    format!("{name}: skipped, requires {need}, which isn't declared{where_}");
                s.use_log.borrow_mut().push(UseReport {
                    name: name.clone(),
                    line,
                    message,
                    outcome: UseOutcome::Skipped {
                        reason: SkipReason::MissingRequires(need.clone()),
                    },
                    deferred: Vec::new(),
                    system_packages: Vec::new(),
                });
                return Ok(());
            }
        }
    }
    if let Some(v) = spec.get("ensure")
        && v.is::<bool>()
        && !as_bool(v, "ensure")?
        && spec.get("git").is_none()
        && spec.get("vc").is_none()
        && spec.get("url").is_none()
        && spec.get("index").is_none()
    {
        // ensure: false with nothing to fetch: a built-in or hand-installed one.
        s.use_log.borrow_mut().push(UseReport::note(
            name.clone(),
            line,
            format!("{name}: not installed by Forge (ensure: false)"),
        ));
    }

    if let Some(v) = spec.get("ensure_system_package") {
        system_packages = ensure_system_packages(v, !collecting)?;
        for need in &system_packages {
            let Some(command) = &need.command else {
                continue;
            };
            let message = if collecting {
                format!("{name}: would install a system package: {command}")
            } else {
                format!("{name}: installing a system package: {command}")
            };
            s.use_log
                .borrow_mut()
                .push(UseReport::note(name.clone(), line, message));
        }
    }

    if let Some(v) = spec.get("preface") {
        if collecting {
            if !v.is::<FnPtr>() {
                return Err(err("use_package: preface is a function, like || { ... }"));
            }
            deferred.push(DeferredCode::Preface);
        } else {
            use_code(s, engine_cell, &name, "preface", v, catch)?;
        }
    }
    if let Some(init) = spec.get("init") {
        if collecting {
            if !init.is::<FnPtr>() {
                return Err(err("use_package: init is a function, like || { ... }"));
            }
            deferred.push(DeferredCode::Init);
        } else {
            use_code(s, engine_cell, &name, "init", init, catch)?;
        }
    }
    if let Some(custom) = spec.get("custom") {
        for (opt, value) in as_map(custom.clone(), "custom")? {
            opt_set(&mut s.settings.borrow_mut(), &opt, value)?;
        }
    }

    for key in ["interpreter", "magic", "magic_fallback"] {
        let Some(v) = spec.get(key) else { continue };
        for (pattern, mode) in &as_map(v.clone(), key)? {
            let mode = as_string(mode, key)?;
            s.with_editor(|ed| {
                if key == "interpreter" {
                    ed.modes.add_interpreter(pattern, &mode)
                } else {
                    ed.modes.add_magic(pattern, &mode, key == "magic_fallback")
                }
                .map_err(|e| e.to_string())
            })?;
        }
    }
    if let Some(faces) = spec.get("custom_face") {
        for (face, style) in &as_map(faces.clone(), "custom_face")? {
            let style = style_arg(as_map(style.clone(), "face style")?)?;
            s.with_editor(|ed| ed.theme.set(face, style).map_err(|e| e.to_string()))?;
        }
    }

    let mut declaration = Map::new();
    for key in USE_PACKAGE_DECLARATION_KEYS {
        if let Some(value) = spec.get(*key) {
            declaration.insert((*key).into(), value.clone());
        }
    }
    let frozen = match spec.get("freeze") {
        Some(value) => as_bool(value, "freeze")?,
        None => false,
    };
    if pinned || frozen {
        declaration.insert("freeze".into(), Dynamic::from(true));
    }
    if let Some(allow) = spec.get("allow_history_rewrite") {
        let allow = as_bool(allow, "allow_history_rewrite")?;
        declaration.insert("allow_history_rewrite".into(), Dynamic::from(allow));
    }
    // vc: a url, or #{ url, rev, make, lisp_dir, features } as :vc.
    let (mut git_dir, mut build, mut features) = (None, None, Vec::new());
    if let Some(vc) = spec.get("vc") {
        if let Some(m) = vc.clone().try_cast::<Map>() {
            for k in m.keys() {
                if !["url", "rev", "make", "lisp_dir", "features"].contains(&k.as_str()) {
                    return Err(err(format!(
                        "unknown key {k:?} in vc (known: url, rev, make, lisp_dir, features)"
                    )));
                }
            }
            let url = map_string(&m, "url")?.ok_or_else(|| err("vc needs a url"))?;
            declaration.insert("git".into(), Dynamic::from(url));
            if let Some(rev) = map_string(&m, "rev")?
                && !declaration.contains_key("rev")
                && !spec.contains_key("pin")
            {
                declaration.insert("rev".into(), Dynamic::from(rev));
            }
            build = map_string(&m, "make")?;
            git_dir = map_string(&m, "lisp_dir")?;
            if let Some(f) = m.get("features") {
                features = name_or_names(f, "features")?;
            }
        } else {
            declaration.insert("git".into(), vc.clone());
        }
    }
    let config_fn = match spec.get("config") {
        Some(c) if c.is::<FnPtr>() => {
            if collecting {
                deferred.push(DeferredCode::Config);
                None
            } else {
                Some(c.clone())
            }
        }
        Some(c) => {
            declaration.insert("config".into(), c.clone());
            None
        }
        None => None,
    };

    // Lazy loading is `commands`, `modes` and `extensions` (use_package has
    // no `on`; forge.plugin.declare does).
    let mut on = Map::new();
    merge_package_names(&mut on, "commands", spec.get("commands"))?;
    merge_package_names(&mut on, "modes", spec.get("modes"))?;
    merge_package_names(&mut on, "extensions", spec.get("extensions"))?;

    // `mode`: "\\.rs$" style patterns are not needed; an extension, a list
    // of them, or a map of extension to major mode.
    let mut associations: Vec<(String, String)> = Vec::new();
    if let Some(m) = spec.get("mode") {
        if let Some(map) = m.clone().try_cast::<Map>() {
            for (ext, mode) in &map {
                associations.push((ext.to_string(), as_string(mode, "mode")?));
            }
        } else {
            return Err(err(
                "use_package: mode is a map of file extension to major mode, like #{ rs: \"rust-mode\" }",
            ));
        }
        for (ext, _) in &associations {
            add_package_name(&mut on, "extensions", ext.trim_start_matches('.'))?;
        }
    }

    // bind: keys in every buffer; bind_mode: keys in one major mode.
    let mut binds: Vec<(String, String, Option<(&str, String)>)> = Vec::new();
    if let Some(bind) = spec.get("bind") {
        for (keys, command) in &as_map(bind.clone(), "bind")? {
            binds.push((keys.to_string(), as_string(command, "bind command")?, None));
        }
    }
    if let Some(per_mode) = spec.get("bind_mode") {
        for (mode, table) in &as_map(per_mode.clone(), "bind_mode")? {
            for (keys, command) in &as_map(table.clone(), "bind_mode keys")? {
                binds.push((
                    keys.to_string(),
                    as_string(command, "bind command")?,
                    Some(("mode", mode.to_string())),
                ));
            }
        }
    }
    // bind_prompt: keys in a prompt's map ("prompt" is every minibuffer
    // prompt, "isearch" during Isearch, "completion-list" in the list), as
    // `:map minibuffer-local-map` / `:map isearch-mode-map`.
    if let Some(per_prompt) = spec.get("bind_prompt") {
        for (prompt, table) in &as_map(per_prompt.clone(), "bind_prompt")? {
            for (keys, command) in &as_map(table.clone(), "bind_prompt keys")? {
                binds.push((
                    keys.to_string(),
                    as_string(command, "bind command")?,
                    Some(("prompt", prompt.to_string())),
                ));
            }
        }
    }
    for (_, command, _) in &binds {
        if !s.with_editor(|ed| Ok(ed.commands.contains(command)))? {
            add_package_name(&mut on, "commands", command)?;
        }
    }

    // defer: lazy even with nothing to trigger it on (it still starts when
    // something it provides is used). demand: eager whatever `on` says.
    let demand = match spec.get("demand") {
        Some(v) => as_bool(v, "demand")?,
        None => false,
    };
    if demand {
        on = Map::new();
    }
    if !on.is_empty() {
        declaration.insert("on".into(), Dynamic::from(on));
    }

    use_spec(s, name.clone(), &declaration)?;
    {
        let load_path = match spec.get("load_path") {
            Some(v) => name_or_names(v, "load_path")?,
            None => Vec::new(),
        };
        let no_require = match spec.get("no_require") {
            Some(v) => as_bool(v, "no_require")?,
            None => false,
        };
        let build_env = match spec.get("build_env") {
            Some(v) => name_or_names(v, "build_env")?,
            None => Vec::new(),
        };
        let build_timeout_secs = match spec.get("build_timeout_secs") {
            Some(v) => {
                let secs = as_int(v, "build_timeout_secs")?;
                if secs <= 0 {
                    return Err(err("build_timeout_secs must be positive"));
                }
                Some(secs as u64)
            }
            None => None,
        };
        if let Some(u) = s.uses.borrow_mut().iter_mut().find(|u| u.name == name) {
            u.load_path = load_path;
            u.no_require = no_require;
            u.git_dir = git_dir;
            u.build = build;
            u.features = features;
            u.build_env = build_env;
            u.build_timeout_secs = build_timeout_secs;
            u.requires = requires;
            u.line = line;
        }
    }
    let lazy = s
        .uses
        .borrow()
        .iter()
        .find(|u| u.name == name)
        .is_some_and(|u| u.lazy());
    let message = if let Some(key) = &conditional {
        format!("{name}: conditional on {key}, not evaluated")
    } else {
        format!(
            "{name}: {}",
            if lazy {
                "deferred until used"
            } else {
                "loads at startup"
            }
        )
    };
    s.use_log.borrow_mut().push(UseReport {
        name: name.clone(),
        line,
        message,
        outcome: match &conditional {
            Some(key) => UseOutcome::Conditional { key: key.clone() },
            None => UseOutcome::Collected { lazy },
        },
        deferred,
        system_packages,
    });

    for (ext, mode) in &associations {
        s.with_editor(|ed| ed.modes.associate(ext, mode).map_err(|e| e.to_string()))?;
    }
    for (keys, command, scope) in binds {
        let opts = scope.map(|(kind, name)| {
            let mut o = Map::new();
            o.insert(kind.into(), Dynamic::from(name));
            o
        });
        keymap_set(s, &keys, Dynamic::from(command), opts)?;
    }
    // remap: [remap command] -- whatever is bound to `from` runs `to`.
    if let Some(remap) = spec.get("remap") {
        for (from, to) in &as_map(remap.clone(), "remap")? {
            keymap_remap(s, from, &as_string(to, "remap")?, None)?;
        }
    }
    if let Some(delight) = spec.get("delight") {
        let table = if let Some(map) = delight.clone().try_cast::<Map>() {
            map.into_iter()
                .map(|(m, l)| Ok((m.to_string(), as_string(&l, "delight")?)))
                .collect::<RhaiResult<Vec<_>>>()?
        } else {
            // A mode name alone: hide it from the mode line.
            name_or_names(delight, "delight")?
                .into_iter()
                .map(|m| (m, String::new()))
                .collect()
        };
        for (mode, lighter) in table {
            s.with_editor(|ed| {
                ed.modes
                    .set_lighter(&mode, &lighter, None)
                    .map_err(|e| e.to_string())
            })?;
        }
    }
    if let Some(f) = config_fn {
        use_code(s, engine_cell, &name, "config", &f, catch)?;
    }
    Ok(())
}

fn package_names(on: &Map, key: &str) -> RhaiResult<Vec<String>> {
    match on.get(key) {
        Some(value) => name_or_names(value, key),
        None => Ok(Vec::new()),
    }
}

fn set_package_names(on: &mut Map, key: &str, names: Vec<String>) {
    let list = names.into_iter().map(Dynamic::from).collect::<Vec<_>>();
    on.insert(key.into(), Dynamic::from(list));
}

fn merge_package_names(on: &mut Map, key: &str, extra: Option<&Dynamic>) -> RhaiResult<()> {
    let Some(extra) = extra else {
        return Ok(());
    };
    let mut names = package_names(on, key)?;
    for name in name_or_names(extra, key)? {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    set_package_names(on, key, names);
    Ok(())
}

fn add_package_name(on: &mut Map, key: &str, name: &str) -> RhaiResult<()> {
    let mut names = package_names(on, key)?;
    if !names.iter().any(|n| n == name) {
        names.push(name.to_string());
    }
    set_package_names(on, key, names);
    Ok(())
}

fn call_now(
    _shared: &std::rc::Rc<Shared>,
    engine_cell: &Weak<OnceCell<Engine>>,
    func: FnPtr,
    args: impl rhai::FuncArgs,
) -> RhaiResult<Dynamic> {
    let cell = engine_cell
        .upgrade()
        .ok_or_else(|| err("the Rhai engine is gone"))?;
    let engine = cell
        .get()
        .ok_or_else(|| err("the Rhai engine is not ready"))?;
    let ast = _shared
        .ast
        .borrow()
        .clone()
        .ok_or_else(|| err("a callback was made outside a script"))?;
    func.call::<Dynamic>(engine, &ast, args)
}

fn opt_get(settings: &Settings, name: &str) -> RhaiResult<Dynamic> {
    use crate::clipboard::SaveInterprogramPasteBeforeKill;
    use crate::messages::MessageLogMax;
    Ok(match name {
        "prefix_help_delay" => settings
            .prefix_help_delay
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
        "mouse" => settings.mouse.map(Dynamic::from).unwrap_or(Dynamic::UNIT),
        "plugin_auto_reload" => settings
            .plugin_auto_reload
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
        "epa_file_encrypt_to" => Dynamic::from_array(
            crate::epa::default_recipients()
                .into_iter()
                .map(Dynamic::from)
                .collect(),
        ),
        "package_index" => settings
            .package_index
            .as_ref()
            .map(|list| Dynamic::from_array(list.iter().cloned().map(Dynamic::from).collect()))
            .unwrap_or(Dynamic::UNIT),
        "package_index_keys" => settings
            .package_index_keys
            .as_ref()
            .map(|list| Dynamic::from_array(list.iter().cloned().map(Dynamic::from).collect()))
            .unwrap_or(Dynamic::UNIT),
        "package_allowed_sources" => settings
            .package_allowed_sources
            .as_ref()
            .map(|list| Dynamic::from_array(list.iter().cloned().map(Dynamic::from).collect()))
            .unwrap_or(Dynamic::UNIT),
        "message_log_max" => match settings.message_log_max {
            Some(MessageLogMax::Unlimited) => Dynamic::from(true),
            Some(MessageLogMax::Off) => Dynamic::from(false),
            Some(MessageLogMax::Lines(n)) => Dynamic::from(n as i64),
            None => Dynamic::UNIT,
        },
        "save_interprogram_paste_before_kill" => {
            match settings.save_interprogram_paste_before_kill {
                Some(SaveInterprogramPasteBeforeKill::Always) => Dynamic::from(true),
                Some(SaveInterprogramPasteBeforeKill::Off) => Dynamic::from(false),
                Some(SaveInterprogramPasteBeforeKill::Limit(n)) => Dynamic::from(n as i64),
                None => Dynamic::UNIT,
            }
        }
        "electric_indent" => settings
            .electric_indent
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
        "line_numbers" => settings
            .line_numbers
            .map(|l| Dynamic::from(l.name().to_string()))
            .unwrap_or(Dynamic::UNIT),
        "wrap" => settings
            .wrap
            .map(|w| Dynamic::from(w.name().to_string()))
            .unwrap_or(Dynamic::UNIT),
        n if FLAGS.contains(&n) => settings
            .flags
            .get(n)
            .map(|b| Dynamic::from(*b))
            .unwrap_or(Dynamic::UNIT),
        n if NUMBERS.contains(&n) => settings
            .numbers
            .get(n)
            .map(|v| Dynamic::from(*v))
            .unwrap_or(Dynamic::UNIT),
        n if STRINGS.contains(&n) => match settings.strings.get(n) {
            Some(v) if v.len() == 1 => Dynamic::from(v[0].clone()),
            Some(v) => Dynamic::from_array(v.iter().cloned().map(Dynamic::from).collect()),
            None => Dynamic::UNIT,
        },
        n if MAPS.contains(&n) => match settings.maps.get(n) {
            Some(m) => {
                let mut out = Map::new();
                for (k, v) in m {
                    out.insert(k.as_str().into(), Dynamic::from(v.clone()));
                }
                Dynamic::from_map(out)
            }
            None => Dynamic::UNIT,
        },
        _ => return Err(unknown_option(name)),
    })
}

fn opt_set(settings: &mut Settings, name: &str, value: Dynamic) -> RhaiResult<()> {
    use crate::clipboard::SaveInterprogramPasteBeforeKill;
    use crate::messages::MessageLogMax;
    let is_nil = value.is_unit();
    match name {
        "prefix_help_delay" => {
            if value.is::<i64>() && as_int(&value, "prefix_help_delay")? >= 0 {
                settings.prefix_help_delay = Some(as_int(&value, "prefix_help_delay")? as f64);
            } else if value.is::<f64>() {
                let n = value.clone().cast::<f64>();
                if n >= 0.0 && n.is_finite() {
                    settings.prefix_help_delay = Some(n);
                } else {
                    return Err(err(
                        "forge.opt.prefix_help_delay must be a number of seconds, 0 or more",
                    ));
                }
            } else {
                return Err(err(
                    "forge.opt.prefix_help_delay must be a number of seconds, 0 or more",
                ));
            }
        }
        "mouse" => settings.mouse = Some(as_bool(&value, "mouse")?),
        "plugin_auto_reload" => {
            settings.plugin_auto_reload = Some(as_bool(&value, "plugin_auto_reload")?)
        }
        "electric_indent" => settings.electric_indent = Some(as_bool(&value, "electric_indent")?),
        "epa_file_encrypt_to" => {
            let list = if is_nil {
                Vec::new()
            } else if value.is::<ImmutableString>() {
                vec![as_string(&value, "epa_file_encrypt_to")?]
            } else if value.is::<Vec<Dynamic>>() {
                as_array(value, "epa_file_encrypt_to")?
                    .iter()
                    .map(|d| as_string(d, "epa_file_encrypt_to"))
                    .collect::<RhaiResult<Vec<_>>>()?
            } else {
                return Err(err(
                    "forge.opt.epa_file_encrypt_to is a key id or address, or a list of them",
                ));
            };
            crate::epa::set_default_recipients(list);
        }
        "package_index" => {
            if is_nil {
                settings.package_index = None;
            } else if value.is::<ImmutableString>() {
                settings.package_index = Some(vec![as_string(&value, "package_index")?]);
            } else if value.is::<Vec<Dynamic>>() {
                let list = as_array(value, "package_index")?
                    .iter()
                    .map(|d| as_string(d, "package_index"))
                    .collect::<RhaiResult<Vec<_>>>()?;
                settings.package_index = Some(list);
            } else {
                return Err(err(
                    "forge.opt.package_index is a location (path or URL) or a list of them",
                ));
            }
        }
        "package_index_keys" => {
            if is_nil {
                settings.package_index_keys = None;
            } else if value.is::<ImmutableString>() {
                settings.package_index_keys = Some(vec![as_string(&value, "package_index_keys")?]);
            } else if value.is::<Vec<Dynamic>>() {
                let list = as_array(value, "package_index_keys")?
                    .iter()
                    .map(|d| as_string(d, "package_index_keys"))
                    .collect::<RhaiResult<Vec<_>>>()?;
                settings.package_index_keys = Some(list);
            } else {
                return Err(err(
                    "forge.opt.package_index_keys is a Minisign public key or a list of them",
                ));
            }
        }
        "package_allowed_sources" => {
            let list = if is_nil {
                None
            } else if value.is::<ImmutableString>() {
                Some(vec![as_string(&value, "package_allowed_sources")?])
            } else if value.is::<Vec<Dynamic>>() {
                Some(
                    as_array(value, "package_allowed_sources")?
                        .iter()
                        .map(|d| as_string(d, "package_allowed_sources"))
                        .collect::<RhaiResult<Vec<_>>>()?,
                )
            } else {
                return Err(err(
                    "forge.opt.package_allowed_sources is a host, URL, path, or a list of them",
                ));
            };
            crate::package_source::SourceAllowlist::new(list.as_deref().unwrap_or_default())
                .map_err(err)?;
            settings.package_allowed_sources = list;
        }
        "message_log_max" => {
            if is_nil {
                settings.message_log_max = None;
            } else if value.is::<bool>() {
                settings.message_log_max = Some(if as_bool(&value, "message_log_max")? {
                    MessageLogMax::Unlimited
                } else {
                    MessageLogMax::Off
                });
            } else if value.is::<i64>() || value.is::<f64>() {
                let n = if value.is::<i64>() {
                    as_int(&value, "message_log_max")? as f64
                } else {
                    value.clone().cast::<f64>()
                };
                if n >= 0.0 && n.is_finite() {
                    settings.message_log_max = Some(MessageLogMax::Lines(n as usize));
                } else {
                    return Err(err(
                        "forge.opt.message_log_max is true (no limit), false (off) or a number of lines",
                    ));
                }
            } else {
                return Err(err(
                    "forge.opt.message_log_max is true (no limit), false (off) or a number of lines",
                ));
            }
        }
        "save_interprogram_paste_before_kill" => {
            if is_nil {
                settings.save_interprogram_paste_before_kill = None;
            } else if value.is::<bool>() {
                settings.save_interprogram_paste_before_kill = Some(if as_bool(&value, name)? {
                    SaveInterprogramPasteBeforeKill::Always
                } else {
                    SaveInterprogramPasteBeforeKill::Off
                });
            } else if value.is::<i64>() || value.is::<f64>() {
                let n = if value.is::<i64>() {
                    as_int(&value, name)? as f64
                } else {
                    value.clone().cast::<f64>()
                };
                if n >= 0.0 && n.is_finite() {
                    settings.save_interprogram_paste_before_kill =
                        Some(SaveInterprogramPasteBeforeKill::Limit(n as usize));
                } else {
                    return Err(err(
                        "forge.opt.save_interprogram_paste_before_kill is true, false or a maximum character count",
                    ));
                }
            } else {
                return Err(err(
                    "forge.opt.save_interprogram_paste_before_kill is true, false or a maximum character count",
                ));
            }
        }
        "line_numbers" => {
            if value.is::<bool>() {
                settings.line_numbers = Some(if as_bool(&value, name)? {
                    LineNumbers::Absolute
                } else {
                    LineNumbers::Off
                });
            } else if value.is::<ImmutableString>() {
                settings.line_numbers = Some(
                    LineNumbers::from_name(&as_string(&value, name)?)
                        .map_err(|e| err(format!("forge.opt.{e}")))?,
                );
            } else {
                return Err(err(
                    "forge.opt.line_numbers is \"off\", \"absolute\" or \"relative\"",
                ));
            }
        }
        "wrap" => {
            if value.is::<ImmutableString>() {
                settings.wrap = Some(
                    crate::decoration::Wrap::from_name(&as_string(&value, name)?)
                        .map_err(|e| err(format!("forge.opt.{e}")))?,
                );
            } else {
                return Err(err("forge.opt.wrap is \"off\", \"char\" or \"word\""));
            }
        }
        n if FLAGS.contains(&n) => {
            if value.is::<bool>() {
                settings.flags.insert(n.to_string(), as_bool(&value, name)?);
            } else {
                return Err(err(format!("forge.opt.{name} must be true or false")));
            }
        }
        n if NUMBERS.contains(&n) => {
            if is_nil {
                settings.numbers.remove(n);
            } else if value.is::<i64>() {
                let v = as_int(&value, name)?;
                if v >= 0 {
                    settings.numbers.insert(n.to_string(), v as f64);
                } else {
                    return Err(err(format!("forge.opt.{name} must be a number, 0 or more")));
                }
            } else if value.is::<f64>() {
                let v = value.clone().cast::<f64>();
                if v >= 0.0 && v.is_finite() {
                    settings.numbers.insert(n.to_string(), v);
                } else {
                    return Err(err(format!("forge.opt.{name} must be a number, 0 or more")));
                }
            } else {
                return Err(err(format!("forge.opt.{name} must be a number, 0 or more")));
            }
        }
        n if STRINGS.contains(&n) => {
            if is_nil {
                if n == "org_crypt_key" {
                    settings
                        .strings
                        .insert(n.to_string(), vec!["nil".to_string()]);
                } else {
                    settings.strings.remove(n);
                }
            } else if value.is::<bool>() && n == "version_control" {
                settings.strings.insert(
                    n.to_string(),
                    vec![if as_bool(&value, name)? { "t" } else { "nil" }.to_string()],
                );
            } else if value.is::<ImmutableString>() {
                settings
                    .strings
                    .insert(n.to_string(), vec![as_string(&value, name)?]);
            } else if value.is::<Vec<Dynamic>>() {
                let list = as_array(value, name)?
                    .iter()
                    .map(|d| as_string(d, name))
                    .collect::<RhaiResult<Vec<_>>>()?;
                settings.strings.insert(n.to_string(), list);
            } else {
                return Err(err(format!(
                    "forge.opt.{name} is a string or a list of strings"
                )));
            }
        }
        n if MAPS.contains(&n) => {
            if is_nil {
                settings.maps.remove(n);
            } else {
                let map = as_map(value, name)?;
                let mut m = std::collections::BTreeMap::new();
                for (k, v) in &map {
                    m.insert(k.to_string(), as_string(v, "map value")?);
                }
                settings.maps.insert(n.to_string(), m);
            }
        }
        _ => return Err(unknown_option(name)),
    }
    Ok(())
}

fn env_with(
    s: &std::rc::Rc<Shared>,
    engine_cell: &Weak<OnceCell<Engine>>,
    vars: Map,
    body: FnPtr,
) -> RhaiResult<Dynamic> {
    let saved = s.with_editor(|ed| Ok(ed.environment.all().to_vec()))?;
    for (name, value) in &vars {
        let value = as_string(value, "environment value")?;
        s.with_editor(|ed| {
            ed.environment.set(name.as_str(), Some(value));
            Ok(())
        })?;
    }
    let result = call_now(s, engine_cell, body, ());
    s.with_editor(|ed| {
        ed.environment.replace(saved);
        Ok(())
    })?;
    result
}

fn command_create(
    s: &std::rc::Rc<Shared>,
    name: &str,
    func: FnPtr,
    opts: Option<Map>,
) -> RhaiResult<()> {
    let desc = opts
        .as_ref()
        .map(|o| map_string(o, "desc"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    s.with_editor(|ed| {
        ed.commands
            .register_config(name, &desc)
            .map_err(|e| e.to_string())
    })?;
    let callback = Callback::new(s, func)?;
    s.commands.borrow_mut().insert(name.to_string(), callback);
    Ok(())
}

fn notify(s: &std::rc::Rc<Shared>, msg: &str, level: i64) -> RhaiResult<()> {
    let level = match level {
        0 => Level::Debug,
        1 => Level::Info,
        2 => Level::Warn,
        3 => Level::Error,
        n => {
            return Err(err(format!("unknown log level {n} (use forge.log.levels)")));
        }
    };
    if level == Level::Info {
        s.effects
            .borrow_mut()
            .push(Effect::Message(msg.to_string()));
    } else {
        s.logs.borrow_mut().push(LogLine {
            level,
            message: msg.to_string(),
        });
    }
    Ok(())
}

fn keymap_set(
    s: &std::rc::Rc<Shared>,
    keys: &str,
    rhs: Dynamic,
    opts: Option<Map>,
) -> RhaiResult<()> {
    let desc = opts
        .as_ref()
        .map(|o| map_string(o, "desc"))
        .transpose()?
        .flatten();
    let scope = scope_from(opts, s)?;
    let command = if let Some(name) = rhs.clone().try_cast::<ImmutableString>() {
        name.to_string()
    } else if let Some(func) = rhs.try_cast::<FnPtr>() {
        let seq = parse_keys(keys).map_err(|e| err(e.to_string()))?;
        let name = match scope_tag(&scope) {
            Some(tag) => format!("<rhai:{tag}:{}>", seq.join("_")),
            None => format!("<rhai:{}>", seq.join("_")),
        };
        let doc = desc.unwrap_or_else(|| format!("A Rhai function bound to {}.", seq.join(" ")));
        s.with_editor(|ed| {
            ed.commands
                .register_config(&name, &doc)
                .map_err(|e| e.to_string())
        })?;
        let callback = Callback::new(s, func)?;
        s.commands.borrow_mut().insert(name.clone(), callback);
        name
    } else {
        return Err(err(
            "forge.keymap.set: the second argument must be a command name or a function",
        ));
    };
    s.with_editor(|ed| {
        let seq = ed
            .keymaps
            .bind(&scope, keys, &command, Origin::User)
            .map_err(|e| e.to_string())?;
        s.bound.borrow_mut().push((seq.join(" "), command));
        Ok(())
    })
}

fn keymap_del(s: &std::rc::Rc<Shared>, keys: &str, opts: Option<Map>) -> RhaiResult<()> {
    let scope = scope_from(opts, s)?;
    s.with_editor(|ed| {
        ed.keymaps
            .unbind(&scope, keys)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

fn keymap_remap(
    s: &std::rc::Rc<Shared>,
    from: &str,
    to: &str,
    opts: Option<Map>,
) -> RhaiResult<()> {
    let scope = scope_from(opts, s)?;
    s.with_editor(|ed| {
        ed.keymaps.remap(&scope, from, to);
        Ok(())
    })
}

fn command_run(s: &std::rc::Rc<Shared>, name: &str, arg: Option<i64>) -> RhaiResult<()> {
    s.deny_while_collecting("forge.command.run")?;
    if !s.with_editor(|ed| Ok(ed.commands.contains(name)))? {
        if s.in_init.get() {
            s.effects
                .borrow_mut()
                .push(Effect::RunCommandLater(name.to_string(), arg));
            return Ok(());
        }
        return Err(err(format!("no command named {name}")));
    }
    s.effects.borrow_mut().push(match arg {
        Some(n) => Effect::RunCommandWithArg(name.to_string(), n),
        None => Effect::RunCommand(name.to_string()),
    });
    Ok(())
}

fn ui_input(s: &std::rc::Rc<Shared>, opts: Option<Map>, then: FnPtr) -> RhaiResult<u32> {
    s.deny_while_collecting("forge.ui.input")?;
    let (prompt, default) = match opts {
        Some(o) => (map_string(&o, "prompt")?, map_string(&o, "default")?),
        None => (None, None),
    };
    let callback = Callback::new(s, then)?;
    Ok(s.ask(
        Question::Text {
            prompt: prompt.unwrap_or_else(|| "> ".into()),
            initial: default.unwrap_or_default(),
        },
        Pending::Text(callback),
    ))
}

fn ui_select(
    s: &std::rc::Rc<Shared>,
    items: Vec<Dynamic>,
    opts: Option<Map>,
    then: FnPtr,
) -> RhaiResult<u32> {
    s.deny_while_collecting("forge.ui.select")?;
    let items = items
        .iter()
        .map(|d| as_string(d, "select item"))
        .collect::<RhaiResult<Vec<_>>>()?;
    let (prompt, multiple) = match opts {
        Some(o) => (
            map_string(&o, "prompt")?,
            map_bool(&o, "multiple")?.unwrap_or(false),
        ),
        None => (None, false),
    };
    let prompt = prompt.unwrap_or_else(|| "Select: ".into());
    let callback = Callback::new(s, then)?;
    if multiple {
        Ok(s.ask(
            Question::Completing {
                prompt,
                candidates: items,
                require_match: true,
                multiple: true,
                over: false,
            },
            Pending::Text(callback),
        ))
    } else {
        Ok(s.ask(
            Question::Completing {
                prompt,
                candidates: items.clone(),
                require_match: true,
                multiple: false,
                over: false,
            },
            Pending::Select { callback, items },
        ))
    }
}

#[cfg(feature = "tree-sitter")]
fn grammar_strings(opts: &Map, key: &str) -> RhaiResult<Option<Vec<String>>> {
    match map_array(opts, key)? {
        None => Ok(None),
        Some(items) => items
            .iter()
            .map(|v| as_string(v, key))
            .collect::<RhaiResult<Vec<_>>>()
            .map(Some),
    }
}

#[cfg(feature = "tree-sitter")]
fn grammar_query(opts: &Map, key: &str, file_key: &str) -> RhaiResult<Option<String>> {
    if let Some(query) = map_string(opts, key)? {
        return Ok(Some(query));
    }
    if let Some(path) = map_string(opts, file_key)? {
        let text =
            std::fs::read_to_string(&path).map_err(|e| err(format!("reading {path}: {e}")))?;
        return Ok(Some(text));
    }
    Ok(None)
}

#[cfg(feature = "tree-sitter")]
fn register_grammar_api(
    s: &std::rc::Rc<Shared>,
    name: ImmutableString,
    path: ImmutableString,
    opts: Option<Map>,
) -> RhaiResult<String> {
    let mut grammar = crate::syntax::DynamicGrammar::new(name.to_string(), path.to_string());
    let mut parent = None;
    if let Some(opts) = opts {
        if let Some(mode) = map_string(&opts, "mode")? {
            grammar.mode = mode;
        }
        if let Some(items) = grammar_strings(&opts, "extensions")? {
            grammar.extensions = items
                .into_iter()
                .map(|s| s.trim_start_matches('.').to_string())
                .collect();
        }
        if let Some(items) = grammar_strings(&opts, "file_names")? {
            grammar.file_names = items;
        }
        if let Some(items) = grammar_strings(&opts, "interpreters")? {
            grammar.interpreters = items;
        }
        if let Some(symbol) = map_string(&opts, "symbol")? {
            grammar.symbol = Some(symbol);
        }
        if let Some(query) = grammar_query(&opts, "highlights", "highlights_file")? {
            grammar.highlights = query;
        }
        if let Some(query) = grammar_query(&opts, "tags", "tags_file")? {
            grammar.tags = query;
        }
        if let Some(v) = map_get(&opts, "indent_width") {
            let width = as_int(v, "indent_width")?;
            if width < 1 {
                return Err(err("indent_width must be at least 1"));
            }
            grammar.indent_width = width as usize;
        }
        if let Some(tabs) = map_bool(&opts, "tabs")? {
            grammar.indent_tabs = tabs;
        }
        if let Some(comment) = map_string(&opts, "line_comment")? {
            grammar.line_comment = Some(comment);
        }
        if let Some(pairs) = map_string(&opts, "pairs")? {
            grammar.pairs = pairs;
        }
        parent = map_string(&opts, "parent")?;
    }
    let mode = grammar.mode.clone();
    let extensions = grammar.extensions.clone();
    let def = crate::syntax::register_grammar(grammar).map_err(err)?;
    s.with_editor(|ed| {
        ed.modes
            .define_major(&mode, parent.as_deref(), &extensions, Some(SOURCE))
            .map_err(|e| e.to_string())?;
        if let Some(p) = &parent {
            ed.keymaps.set_mode_parent(&mode, p);
        }
        Ok(())
    })?;
    Ok(def.name.to_string())
}

fn define_major(s: &std::rc::Rc<Shared>, name: &str, opts: Option<Map>) -> RhaiResult<()> {
    let (parent, exts) = match opts {
        Some(t) => (map_string(&t, "parent")?, map_array(&t, "extensions")?),
        None => (None, None),
    };
    let exts = exts
        .unwrap_or_default()
        .iter()
        .map(|d| as_string(d, "extension"))
        .collect::<RhaiResult<Vec<_>>>()?;
    s.with_editor(|ed| {
        ed.modes
            .define_major(name, parent.as_deref(), &exts, Some(SOURCE))
            .map_err(|e| e.to_string())?;
        if let Some(p) = &parent {
            ed.keymaps.set_mode_parent(name, p);
        }
        Ok(())
    })
}

fn process_run(
    s: &std::rc::Rc<Shared>,
    cmd: Dynamic,
    opts: Option<Map>,
    then: FnPtr,
) -> RhaiResult<u32> {
    use std::time::Duration;
    s.deny_while_collecting("forge.process.run")?;
    if s.project.borrow().is_some() {
        return Err(err(
            "forge.process.run isn't available in project init.rhai files",
        ));
    }
    let program = if let Some(line) = cmd.clone().try_cast::<ImmutableString>() {
        crate::process::ProcessSpec::Shell(line.to_string())
    } else if let Some(list) = cmd.try_cast::<Vec<Dynamic>>() {
        let argv = list
            .iter()
            .map(|d| as_string(d, "argv"))
            .collect::<RhaiResult<Vec<_>>>()?;
        if argv.is_empty() {
            return Err(err("forge.process.run needs a command"));
        }
        crate::process::ProcessSpec::Argv(argv[0].clone(), argv.into_iter().skip(1).collect())
    } else {
        return Err(err(
            "forge.process.run takes a shell line or [ program, args... ]",
        ));
    };
    let mut timeout = Duration::from_secs(30);
    let mut cwd: Option<String> = None;
    let mut stdin: Option<Vec<u8>> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(opts) = opts {
        for (k, v) in &opts {
            match k.as_str() {
                "timeout" => {
                    let n = as_int(v, "timeout")?;
                    if (1..=300).contains(&n) {
                        timeout = Duration::from_secs(n as u64);
                    } else {
                        return Err(err("timeout is seconds, 1 to 300"));
                    }
                }
                "cwd" => cwd = Some(as_string(v, "cwd")?),
                "stdin" => stdin = Some(as_string(v, "stdin")?.into_bytes()),
                "env" => {
                    let map = as_map(v.clone(), "env")?;
                    for (name, value) in &map {
                        env.push((name.to_string(), as_string(value, "env value")?));
                    }
                }
                other => {
                    return Err(err(format!(
                        "unknown option {other:?} (known: timeout, cwd, stdin, env)"
                    )));
                }
            }
        }
    }
    let dir = match cwd {
        Some(c) => crate::permissions::expand_home(&c),
        None => s.with_editor(|ed| {
            Ok(ed
                .state
                .active_buffer
                .and_then(|id| ed.state.get_buffer(id))
                .and_then(|b| b.path().map(|p| p.to_path_buf()))
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| Path::new(".").to_path_buf()))
        })?,
    };
    let mut resolved = s.with_editor(|ed| Ok(ed.environment.all().to_vec()))?;
    for (k, v) in env {
        match resolved.iter().position(|(ek, _)| ek == &k) {
            Some(i) => resolved[i].1 = v,
            None => resolved.push((k, v)),
        }
    }
    let request = s.reserve(Pending::Callback(Callback::new(s, then)?));
    s.effects.borrow_mut().push(Effect::Process {
        request,
        program,
        dir,
        stdin,
        timeout,
        env: resolved,
    });
    Ok(request)
}

enum FsKind {
    Read,
    List,
    Exists,
}

fn fs_op(s: &std::rc::Rc<Shared>, path: String, kind: FsKind, then: FnPtr) -> RhaiResult<u32> {
    use crate::process::FsOp;
    let path = crate::permissions::expand_home(&path);
    let op = match kind {
        FsKind::Read => FsOp::Read(path),
        FsKind::List => FsOp::List(path),
        FsKind::Exists => FsOp::Exists(path),
    };
    let request = s.reserve(Pending::Callback(Callback::new(s, then)?));
    s.effects.borrow_mut().push(Effect::Fs { request, op });
    Ok(request)
}

fn http_get(
    s: &std::rc::Rc<Shared>,
    url: String,
    headers: Option<Map>,
    then: FnPtr,
) -> RhaiResult<u32> {
    s.deny_while_collecting("forge.http.get")?;
    if s.project.borrow().is_some() {
        return Err(err(
            "forge.http.get isn't available in project init.rhai files",
        ));
    }
    let headers = match headers {
        Some(h) => rhai_headers(&h)?,
        None => Vec::new(),
    };
    let request = s.reserve(Pending::Callback(Callback::new(s, then)?));
    s.effects.borrow_mut().push(Effect::Http {
        request,
        method: "GET".into(),
        url,
        headers,
        body: None,
    });
    Ok(request)
}

fn http_request(
    s: &std::rc::Rc<Shared>,
    method: String,
    url: String,
    opts: Option<Map>,
    then: FnPtr,
) -> RhaiResult<u32> {
    s.deny_while_collecting("forge.http.request")?;
    if s.project.borrow().is_some() {
        return Err(err(
            "forge.http.request isn't available in project init.rhai files",
        ));
    }
    let mut headers = Vec::new();
    let mut body = None;
    if let Some(opts) = opts {
        for (k, v) in &opts {
            match k.as_str() {
                "headers" => headers = rhai_headers(&as_map(v.clone(), "headers")?)?,
                "body" => body = Some(as_string(v, "body")?.into_bytes()),
                other => {
                    return Err(err(format!(
                        "unknown option {other:?} (known: headers, body)"
                    )));
                }
            }
        }
    }
    let request = s.reserve(Pending::Callback(Callback::new(s, then)?));
    s.effects.borrow_mut().push(Effect::Http {
        request,
        method,
        url,
        headers,
        body,
    });
    Ok(request)
}

fn env_set(
    s: &std::rc::Rc<Shared>,
    name: &str,
    value: Dynamic,
    substitute: bool,
) -> RhaiResult<()> {
    s.deny_while_collecting("forge.env.set")?;
    let value = if value.is_unit() {
        None
    } else if let Some(text) = value.clone().try_cast::<ImmutableString>() {
        let text = text.to_string();
        if substitute {
            Some(s.with_editor(|ed| Ok(ed.environment.substitute(&text)))?)
        } else {
            Some(text)
        }
    } else {
        return Err(err(format!(
            "forge.env.set wants a string or (), got {}",
            value.type_name()
        )));
    };
    s.with_editor(|ed| {
        ed.environment.set(name, value);
        Ok(())
    })
}

fn load_average(use_float: bool) -> Dynamic {
    let Some((one, five, fifteen)) = crate::env::load_average() else {
        return Dynamic::UNIT;
    };
    let vals = if use_float {
        vec![
            Dynamic::from(one),
            Dynamic::from(five),
            Dynamic::from(fifteen),
        ]
    } else {
        vec![
            Dynamic::from((one * 100.0).round() as i64),
            Dynamic::from((five * 100.0).round() as i64),
            Dynamic::from((fifteen * 100.0).round() as i64),
        ]
    };
    Dynamic::from_array(vals)
}

fn menu_items(items: Dynamic) -> RhaiResult<Vec<crate::menu::Item>> {
    let list = as_array(items, "menu items")?;
    list.iter()
        .map(|item| {
            let map = as_map(item.clone(), "menu item")?;
            Ok(crate::menu::Item::new(
                map_string(&map, "label")?.ok_or_else(|| err("a menu item needs a label"))?,
                map_string(&map, "command")?.ok_or_else(|| err("a menu item needs a command"))?,
            ))
        })
        .collect()
}

fn style_arg(style: Map) -> RhaiResult<FaceStyle> {
    let mut out = FaceStyle::default();
    for (k, v) in &style {
        match k.as_str() {
            "fg" => out.fg = Some(parse_color(&as_string(v, "fg")?).map_err(|e| e.to_string())?),
            "bg" => out.bg = Some(parse_color(&as_string(v, "bg")?).map_err(|e| e.to_string())?),
            "bold" => out.bold = Some(as_bool(v, "bold")?),
            "italic" => out.italic = Some(as_bool(v, "italic")?),
            "underline" => out.underline = Some(as_bool(v, "underline")?),
            other => {
                return Err(err(format!(
                    "unknown style {other:?} (known: fg, bg, bold, italic, underline)"
                )));
            }
        }
    }
    Ok(out)
}

fn style_map(style: &FaceStyle) -> Map {
    let mut out = Map::new();
    if let Some(c) = style.fg {
        out.insert("fg".into(), Dynamic::from(c.hex()));
    }
    if let Some(c) = style.bg {
        out.insert("bg".into(), Dynamic::from(c.hex()));
    }
    if let Some(b) = style.bold {
        out.insert("bold".into(), Dynamic::from(b));
    }
    if let Some(b) = style.italic {
        out.insert("italic".into(), Dynamic::from(b));
    }
    if let Some(b) = style.underline {
        out.insert("underline".into(), Dynamic::from(b));
    }
    out
}

fn theme_define(
    s: &std::rc::Rc<Shared>,
    name: &str,
    description: &str,
    faces: Map,
) -> RhaiResult<()> {
    let mut list = Vec::new();
    for (face, style) in &faces {
        let style = as_map(style.clone(), "face style")?;
        list.push((face.to_string(), style_arg(style)?));
    }
    s.with_editor(|ed| {
        ed.themes
            .define(
                SOURCE,
                name,
                NamedTheme {
                    description: description.to_string(),
                    faces: list,
                },
            )
            .map_err(|e| e.to_string())
    })
}
