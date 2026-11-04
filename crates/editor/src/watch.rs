// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the editor shares with whatever is watching a plugin or init.rhai
//! call while it runs.
//!
//! A call runs on the frontend's thread (it edits the editor directly), so
//! while it runs the frontend can't read keys or redraw. The terminal's
//! watchdog thread drives this state: it shows "NAME is busy... (C-g to
//! stop)", reads keys itself, and stops a call that never ends. The macOS
//! app asks the same state what is busy and to interrupt it.

use forge_core::{PluginMonitor, RhaiMonitor};
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the watchdog shares with the main loop.
#[derive(Clone, Default)]
pub struct Watch {
    plugins: Arc<Mutex<Option<PluginMonitor>>>,
    rhai: Arc<Mutex<Option<RhaiMonitor>>>,
    /// Keys read while something was busy, for the main loop.
    stolen: Arc<Mutex<VecDeque<Event>>>,
    /// The watchdog drew on the screen: redraw everything.
    dirty: Arc<AtomicBool>,
}

impl Watch {
    pub fn set_plugin_monitor(&self, m: PluginMonitor) {
        *self.plugins.lock().expect("watch lock") = Some(m);
    }

    pub fn set_rhai_monitor(&self, m: Option<RhaiMonitor>) {
        *self.rhai.lock().expect("watch lock") = m;
    }

    /// What's running and for how long.
    pub fn busy(&self) -> Option<(String, Duration)> {
        if let Some(b) = self
            .plugins
            .lock()
            .expect("watch lock")
            .as_ref()
            .and_then(|m| m.busy())
        {
            return Some(b);
        }
        let rhai = self.rhai.lock().expect("watch lock");
        rhai.as_ref()
            .and_then(|m| m.busy())
            .map(|d| (forge_core::rhai::SOURCE.to_string(), d))
    }

    /// Stop whatever is running (C-g).
    pub fn interrupt(&self) {
        if let Some(m) = self.plugins.lock().expect("watch lock").as_ref() {
            m.interrupt();
        }
        if let Some(m) = self.rhai.lock().expect("watch lock").as_ref() {
            m.interrupt();
        }
    }

    /// Keys typed while something was busy.
    pub fn take_stolen(&self) -> Vec<Event> {
        self.stolen.lock().expect("watch lock").drain(..).collect()
    }

    /// Ask for a full redraw: something drew over the editor.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }

    /// Handle one event read while busy: C-g interrupts, the rest waits.
    pub fn handle_busy_event(&self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if k.code == KeyCode::Char('g') && k.modifiers.contains(KeyModifiers::CONTROL) {
                    self.interrupt();
                } else {
                    self.stolen
                        .lock()
                        .expect("watch lock")
                        .push_back(Event::Key(k));
                }
            }
            Event::Key(_) => {}
            other => self.stolen.lock().expect("watch lock").push_back(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    #[test]
    fn busy_keys_are_kept_and_c_g_interrupts() {
        let w = Watch::default();
        w.handle_busy_event(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        w.handle_busy_event(Event::Key(KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
        )));
        w.handle_busy_event(Event::Key(KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
        )));
        let kept: Vec<String> = w
            .take_stolen()
            .into_iter()
            .map(|e| match e {
                Event::Key(k) => format!("{:?}", k.code),
                _ => String::new(),
            })
            .collect();
        assert_eq!(kept, ["Char('a')", "Char('b')"]);
        assert!(w.busy().is_none());
    }
}
