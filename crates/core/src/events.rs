// SPDX-License-Identifier: MIT OR Apache-2.0
//! Working out what happened to buffers since last time, for plugin and
//! Lua event handlers (buffer-opened, buffer-changed, ...).

use crate::app::{BufferId, Editor};
use crate::plugin::runtime::PluginEvent;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snap {
    version: u64,
    saves: u64,
    modes: (String, Vec<String>),
}

/// Remembers each buffer's state to work out what happened since.
#[derive(Debug, Default)]
pub(crate) struct EventTracker {
    pub(crate) snaps: HashMap<BufferId, Snap>,
    /// Buffers changed since the last idle delivery.
    pub(crate) changed: BTreeSet<BufferId>,
    /// Buffers that never produce events (*Messages*: plugins logging
    /// from an event handler mustn't trigger themselves).
    pub(crate) ignored: BTreeSet<BufferId>,
}

impl EventTracker {
    fn snap(b: &crate::Buffer) -> Snap {
        Snap {
            version: b.version(),
            saves: b.save_count(),
            modes: (b.major_mode().to_string(), b.minor_modes().to_vec()),
        }
    }

    /// Events since the last scan (buffer-changed is only queued). With
    /// `quiet`, just catch up without reporting anything.
    pub(crate) fn scan(&mut self, editor: &Editor, quiet: bool) -> Vec<PluginEvent> {
        let mut events = Vec::new();
        for id in editor.buffer_ids() {
            if self.ignored.contains(&id) {
                continue;
            }
            let b = editor.state.get_buffer(id).expect("listed");
            let now = Self::snap(b);
            match self.snaps.insert(id, now.clone()) {
                None => events.push(PluginEvent::BufferOpened(id.0)),
                Some(old) => {
                    if old.version != now.version && !quiet {
                        self.changed.insert(id);
                    }
                    if old.saves != now.saves {
                        events.push(PluginEvent::BufferSaved(id.0));
                    }
                    if old.modes != now.modes {
                        events.push(PluginEvent::ModeChanged(id.0));
                    }
                }
            }
        }
        let gone: Vec<BufferId> = self
            .snaps
            .keys()
            .filter(|id| !editor.state.buffers.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            self.snaps.remove(&id);
            self.changed.remove(&id);
            events.push(PluginEvent::BufferKilled(id.0));
        }
        if quiet { Vec::new() } else { events }
    }

    pub(crate) fn take_changed(&mut self) -> Vec<PluginEvent> {
        std::mem::take(&mut self.changed)
            .into_iter()
            .map(|id| PluginEvent::BufferChanged(id.0))
            .collect()
    }
}

/// The name of an event, as Lua and the docs spell it.
pub fn event_name(e: &PluginEvent) -> (&'static str, u32) {
    match e {
        PluginEvent::BufferOpened(id) => ("buffer-opened", *id),
        PluginEvent::BufferKilled(id) => ("buffer-killed", *id),
        PluginEvent::BufferChanged(id) => ("buffer-changed", *id),
        PluginEvent::BufferSaved(id) => ("buffer-saved", *id),
        PluginEvent::ModeChanged(id) => ("mode-changed", *id),
    }
}

pub const EVENT_NAMES: [&str; 5] = [
    "buffer-opened",
    "buffer-killed",
    "buffer-changed",
    "buffer-saved",
    "mode-changed",
];
