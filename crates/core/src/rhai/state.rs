// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

impl RhaiConfig {
    pub fn monitor(&self) -> RhaiMonitor {
        self.monitor.clone()
    }

    /// buffer-changed events wait for idle time and a handler wants them.
    pub fn changes_pending(&self) -> bool {
        !self.shared.tracker.borrow().changed.is_empty()
            && self
                .shared
                .handlers
                .borrow()
                .iter()
                .any(|(e, _)| e == "buffer-changed")
    }

    /// Run forge.on handlers for what changed since last time (`idle`:
    /// include buffer-changed). Changes the handlers make aren't
    /// announced again.
    pub fn notice_changes(&self, editor: &mut Editor, idle: bool) -> Outcome {
        let mut out = Outcome::default();
        if self.shared.handlers.borrow().is_empty() {
            return out;
        }
        {
            let mut t = self.shared.tracker.borrow_mut();
            // *Messages* never makes events (handlers log there).
            if let Some(m) = editor.find_buffer("*Messages*") {
                t.ignored.insert(m);
                t.snaps.remove(&m);
                t.changed.remove(&m);
            }
            if !self.shared.tracking.get() {
                t.scan(editor, true);
                self.shared.tracking.set(true);
                return out;
            }
        }
        let mut events = self.shared.tracker.borrow_mut().scan(editor, false);
        if idle {
            events.extend(self.shared.tracker.borrow_mut().take_changed());
        }
        for e in events {
            let (name, id) = crate::events::event_name(&e);
            let funcs: Vec<Callback> = self
                .shared
                .handlers
                .borrow()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, cb)| cb.clone())
                .collect();
            for cb in funcs {
                let buf = RhaiBuffer {
                    id: BufferId(id),
                    shared: self.shared.clone(),
                };
                let (_, more) = self.call(editor, || cb.call_unit(self.engine(), (buf,)));
                out.effects.extend(more.effects);
                out.logs.extend(more.logs);
                out.errors.extend(more.errors);
            }
        }
        self.shared.tracker.borrow_mut().scan(editor, true);
        out
    }

    pub fn settings(&self) -> Settings {
        self.shared.settings.borrow().clone()
    }

    /// Evaluate an expression or statement, for tests and (later) M-:.
    pub fn eval(&self, editor: &mut Editor, src: &str) -> (Option<String>, Outcome) {
        self.call(editor, || {
            let ast = match self.engine().compile_with_scope(&self.scope.borrow(), src) {
                Ok(ast) => ast,
                Err(e) => return Err(err(e.to_string())),
            };
            self.run_ast(&ast).map(|v| pretty(&v))
        })
    }
}
