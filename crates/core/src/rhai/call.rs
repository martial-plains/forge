// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

impl RhaiConfig {
    /// Lend the editor to Rhai for one call, collecting what it left.
    pub(super) fn call<R>(
        &self,
        editor: &mut Editor,
        f: impl FnOnce() -> RhaiResult<R>,
    ) -> (Option<R>, Outcome) {
        *self.shared.editor.borrow_mut() = Some(std::mem::take(editor));
        self.shared.deadline.set(Some(Instant::now() + TIME_LIMIT));
        *self.monitor.since.lock().expect("rhai monitor") = Some(Instant::now());
        self.monitor.interrupt.store(false, Ordering::SeqCst);
        // A panic in a Rhai callback must not lose the editor or leave the
        // monitor thinking Rhai is still running, so the call is guarded
        // and everything the call borrowed is put back before the panic
        // carries on.
        let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(r) => r,
            Err(payload) => {
                self.restore_after_panic(editor);
                std::panic::resume_unwind(payload);
            }
        };
        *self.monitor.since.lock().expect("rhai monitor") = None;
        self.monitor.interrupt.store(false, Ordering::SeqCst);
        self.shared.deadline.set(None);
        *editor = self
            .shared
            .editor
            .borrow_mut()
            .take()
            .expect("the lent editor comes back");

        let mut out = Outcome {
            effects: std::mem::take(&mut *self.shared.effects.borrow_mut()),
            logs: self
                .shared
                .logs
                .borrow_mut()
                .drain(..)
                .map(|l| (SOURCE.to_string(), l))
                .collect(),
            ..Default::default()
        };
        match r {
            Ok(v) => (Some(v), out),
            Err(e) => {
                out.errors.push(format!("{SOURCE}: {}", describe(&e)));
                (None, out)
            }
        }
    }

    /// Put back what a call borrowed before it panicked: the editor, the
    /// time limit and the monitor. Nothing here may panic, since the
    /// panic it is cleaning up after is already on its way.
    fn restore_after_panic(&self, editor: &mut Editor) {
        if let Ok(mut since) = self.monitor.since.lock() {
            *since = None;
        }
        self.monitor.interrupt.store(false, Ordering::SeqCst);
        self.shared.deadline.set(None);
        if let Ok(mut lent) = self.shared.editor.try_borrow_mut()
            && let Some(back) = lent.take()
        {
            *editor = back;
        }
    }

    /// Group candidates for `category` with the callback the script set
    /// (`forge.completion.group`). Empty when none is registered.
    pub fn group(
        &self,
        editor: &mut Editor,
        category: &str,
        candidates: &[String],
    ) -> (Vec<String>, Outcome) {
        let Some(callback) = self.shared.groups.borrow().get(category).cloned() else {
            return (Vec::new(), Outcome::default());
        };
        let candidates: rhai::Array = candidates.iter().cloned().map(Dynamic::from).collect();
        let (groups, out) = self.call(editor, || {
            let d = callback.call_dynamic(self.engine(), (Dynamic::from(candidates.clone()),))?;
            let list = d
                .try_cast::<Vec<Dynamic>>()
                .ok_or_else(|| err("the group callback must return an array of strings"))?;
            list.iter()
                .map(|v| as_string(v, "group name"))
                .collect::<RhaiResult<Vec<_>>>()
        });
        (groups.unwrap_or_default(), out)
    }

    /// Run a Rhai file (init.rhai).
    pub fn load_file(&self, editor: &mut Editor, path: &Path) -> Outcome {
        match std::fs::read_to_string(path) {
            Ok(src) => {
                self.shared.in_init.set(true);
                let out = self.load_str(editor, &path.display().to_string(), &src);
                self.shared.in_init.set(false);
                out
            }
            Err(e) => {
                let mut out = Outcome::default();
                out.errors.push(format!("{}: {e}", path.display()));
                out
            }
        }
    }

    /// Run Rhai source.
    pub fn load_str(&self, editor: &mut Editor, chunk_name: &str, src: &str) -> Outcome {
        self.call(editor, || {
            let ast = self
                .engine()
                .compile_with_scope(&self.scope.borrow(), src)
                .map_err(|e| err(e.to_string()))?;
            let _ = chunk_name;
            self.run_ast(&ast)
        })
        .1
    }

    pub(super) fn run_ast(&self, ast: &AST) -> RhaiResult<Dynamic> {
        self.shared.ast.replace(Some(ast.clone()));
        let mut scope = self.scope.borrow_mut();
        let result = self
            .engine()
            .eval_ast_with_scope::<Dynamic>(&mut scope, ast);
        drop(scope);
        self.shared.ast.replace(None);
        result
    }

    /// Run a project's `init.rhai` (see [`crate::project`]). While it runs,
    /// `#{ project: true }` means this project, so the file can bind its
    /// own keys without writing its own path out.
    pub fn load_project_file(&self, editor: &mut Editor, root: &Path, src: &str) -> Outcome {
        *self.shared.project.borrow_mut() = Some(root.to_path_buf());
        let out = self.load_str(editor, &root.display().to_string(), src);
        *self.shared.project.borrow_mut() = None;
        out
    }

    /// The project whose init.rhai is running, else the one the active
    /// buffer is in: what `forge.project.root()` answers, and what
    /// `#{ project: true }` means.
    pub fn project_root(&self) -> Option<std::path::PathBuf> {
        self.shared.project_root()
    }

    /// Bindings made so far to commands that don't exist. Checked after
    /// plugins have loaded (a plugin may define them), and kept either way.
    pub fn unknown_bindings(&self, editor: &Editor) -> Vec<String> {
        self.shared
            .bound
            .borrow_mut()
            .drain(..)
            .filter(|(_, c)| !editor.commands.contains(c))
            .map(|(keys, c)| format!("{keys} is bound to unknown command {c}"))
            .collect()
    }

    /// What init.rhai granted with forge.plugin.grant.
    pub fn plugin_grants(&self) -> std::collections::BTreeMap<String, crate::permissions::Grants> {
        self.shared.grants.borrow().clone()
    }

    /// What init.rhai declared with use_package, in the order declared.
    pub fn plugin_uses(&self) -> Vec<crate::declare::PluginUse> {
        self.shared.uses.borrow().clone()
    }

    /// One line per use_package: loaded, deferred, or why it was skipped.
    pub fn use_package_report(&self) -> Vec<String> {
        self.shared.use_log.borrow().clone()
    }

    /// Run a command defined with `forge.command.create` or bound as a function.
    pub fn run_command(&self, editor: &mut Editor, name: &str, prefix: Option<i64>) -> Outcome {
        let callback = self.shared.commands.borrow().get(name).cloned();
        match callback {
            Some(callback) => {
                let arg = prefix.map(Dynamic::from).unwrap_or(Dynamic::UNIT);
                self.call(editor, || callback.call_one_unit(self.engine(), arg))
                    .1
            }
            None => {
                let mut out = Outcome::default();
                out.errors.push(format!("{name} isn't a Rhai command"));
                out
            }
        }
    }

    /// The user answered (None: cancelled) a question Rhai asked.
    pub fn answer(&self, editor: &mut Editor, request: u32, answer: Option<&str>) -> Outcome {
        let pending = self.shared.answers.borrow_mut().remove(&request);
        let Some(pending) = pending else {
            return Outcome::default();
        };
        self.call(editor, || {
            let value = answer
                .map(|s| Dynamic::from(s.to_string()))
                .unwrap_or(Dynamic::UNIT);
            match pending {
                Pending::Text(cb) => cb.call_one_unit(self.engine(), value),
                Pending::Confirm(cb) => {
                    cb.call_one_unit(self.engine(), Dynamic::from(answer == Some("y")))
                }
                Pending::Select { callback, items } => {
                    let idx = answer
                        .and_then(|a| items.iter().position(|i| i == a))
                        .map(|i| Dynamic::from(i as i64 + 1))
                        .unwrap_or(Dynamic::UNIT);
                    match callback.call_unit(self.engine(), (value.clone(), idx.clone())) {
                        Err(e) if is_arity_error(&e) => {
                            match callback.call_unit(self.engine(), (value,)) {
                                Err(e) if is_arity_error(&e) => {
                                    callback.call_unit(self.engine(), ())
                                }
                                other => other,
                            }
                        }
                        other => other,
                    }
                }
                Pending::Callback(cb) => cb.call_one_unit(self.engine(), value),
            }
        })
        .1
    }

    /// The user answered (None: cancelled) a multiple-value question
    /// (`forge.ui.select` with multiple): the callback gets the list as an
    /// array, or () when it was cancelled.
    pub fn answer_list(
        &self,
        editor: &mut Editor,
        request: u32,
        answer: Option<&[String]>,
    ) -> Outcome {
        let pending = self.shared.answers.borrow_mut().remove(&request);
        let Some(pending) = pending else {
            return Outcome::default();
        };
        let value = match answer {
            Some(items) => {
                Dynamic::from_array(items.iter().cloned().map(Dynamic::from).collect::<Vec<_>>())
            }
            None => Dynamic::UNIT,
        };
        self.call(editor, || match pending {
            Pending::Text(cb) => cb.call_one_unit(self.engine(), value),
            Pending::Callback(cb) => cb.call_one_unit(self.engine(), value),
            Pending::Select { callback, .. } => callback.call_one_unit(self.engine(), value),
            Pending::Confirm(cb) => cb.call_one_unit(self.engine(), value),
        })
        .1
    }

    fn take_pending(&self, request: u32) -> Option<Pending> {
        self.shared.answers.borrow_mut().remove(&request)
    }

    /// A process finished: call its callback with
    /// `#{ code, stdout, stderr }`.
    pub fn answer_process(
        &self,
        editor: &mut Editor,
        request: u32,
        code: Option<i32>,
        stdout: String,
        stderr: String,
    ) -> Outcome {
        let Some(pending) = self.take_pending(request) else {
            return Outcome::default();
        };
        let mut map = Map::new();
        map.insert(
            "code".into(),
            code.map(|c| Dynamic::from(c as i64))
                .unwrap_or(Dynamic::UNIT),
        );
        map.insert("stdout".into(), Dynamic::from(stdout));
        map.insert("stderr".into(), Dynamic::from(stderr));
        self.call(editor, || {
            pending_callback(pending).call_values_unit(self.engine(), vec![Dynamic::from_map(map)])
        })
        .1
    }

    /// A file operation finished: call its callback. Reads answer
    /// `(text, err)`, writes `(err)`, lists `(entries, err)` and checks
    /// `(exists)`, with units where the other side has nothing to say.
    pub fn answer_fs(
        &self,
        editor: &mut Editor,
        request: u32,
        answer: crate::process::FsAnswer,
    ) -> Outcome {
        use crate::process::FsAnswer;
        let Some(pending) = self.take_pending(request) else {
            return Outcome::default();
        };
        let args = match answer {
            FsAnswer::Read(result) => match result {
                Ok(bytes) => vec![
                    Dynamic::from(String::from_utf8_lossy(&bytes).into_owned()),
                    Dynamic::UNIT,
                ],
                Err(e) => vec![Dynamic::UNIT, Dynamic::from(e)],
            },
            FsAnswer::Written(result) => match result {
                Ok(()) => vec![Dynamic::UNIT],
                Err(e) => vec![Dynamic::from(e)],
            },
            FsAnswer::Listed(result) => match result {
                Ok(entries) => {
                    let list = entries
                        .iter()
                        .map(|e| {
                            let mut map = Map::new();
                            map.insert("name".into(), Dynamic::from(e.name.clone()));
                            map.insert("dir".into(), Dynamic::from(e.dir));
                            Dynamic::from_map(map)
                        })
                        .collect::<Vec<_>>();
                    vec![Dynamic::from_array(list), Dynamic::UNIT]
                }
                Err(e) => vec![Dynamic::UNIT, Dynamic::from(e)],
            },
            FsAnswer::Exists(yes) => vec![Dynamic::from(yes)],
        };
        self.call(editor, || {
            pending_callback(pending).call_values_unit(self.engine(), args)
        })
        .1
    }

    /// A web request finished: call its callback. On success it gets
    /// `(#{ status, headers, body }, ())`, on failure `((), err)`.
    pub fn answer_http(
        &self,
        editor: &mut Editor,
        request: u32,
        answer: crate::http::Answer,
    ) -> Outcome {
        let Some(pending) = self.take_pending(request) else {
            return Outcome::default();
        };
        let args = match answer {
            Ok(r) => {
                let mut map = Map::new();
                map.insert("status".into(), Dynamic::from(r.status as i64));
                let mut headers = Map::new();
                for (name, value) in &r.headers {
                    headers.insert(name.as_str().into(), Dynamic::from(value.clone()));
                }
                map.insert("headers".into(), Dynamic::from_map(headers));
                map.insert(
                    "body".into(),
                    Dynamic::from(String::from_utf8_lossy(&r.body).into_owned()),
                );
                vec![Dynamic::from_map(map), Dynamic::UNIT]
            }
            Err(e) => vec![Dynamic::UNIT, Dynamic::from(e)],
        };
        self.call(editor, || {
            pending_callback(pending).call_values_unit(self.engine(), args)
        })
        .1
    }
}

fn pending_callback(pending: Pending) -> Callback {
    match pending {
        Pending::Callback(cb)
        | Pending::Text(cb)
        | Pending::Confirm(cb)
        | Pending::Select { callback: cb, .. } => cb,
    }
}
