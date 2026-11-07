// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge core editor engine.

pub mod agenda_faces;
pub mod app;
pub mod auto_fill;
pub mod background;
pub mod backup;
pub mod buffer;
mod buffer_ext;
pub mod canvas;
pub mod clipboard;
pub mod collect;
pub mod commands;
pub mod completion;
pub mod config;
pub mod context_menu;
pub mod declare;
pub mod decoration;
pub mod display;
pub mod display_spec;
pub mod editing;
pub mod editorconfig;
pub mod env;
pub mod epa;
pub mod events;
pub mod export;
pub mod few;
pub mod filelock;
pub mod fileops;
pub mod fill;
pub mod fold;
pub mod fsutil;
#[cfg(feature = "tree-sitter")]
pub mod grammar_install;
pub mod help_faces;
pub mod http;
pub mod icons;
pub mod init_review;
pub mod keymap;
pub mod kill_ring;
pub mod kmacro;
pub mod links;
pub mod lockfile;
pub mod menu;
pub mod messages;
pub mod mode_menus;
pub mod modes;
pub mod no_littering;
pub mod org;
pub mod org_agenda;
pub mod org_blocks;
pub mod org_calc;
pub mod org_columns;
pub mod org_crypt;
pub mod org_dblock;
pub mod org_display;
pub mod org_ext;
pub mod org_footnote;
pub mod org_publish;
pub mod org_table;
pub mod org_time;
pub mod org_work;
pub mod package;
pub mod package_doctor;
pub mod package_journal;
pub mod package_log;
pub mod package_source;
pub mod permissions;
pub mod plugin;
pub mod plugin_bisect;
pub mod process;
pub mod project;
pub mod rect;
pub mod registers;
pub mod replace;
pub mod rhai;
pub mod session;
pub mod syntax;
pub mod text_nav;
pub mod theme;
pub mod toolbar;
#[cfg(feature = "tree-sitter")]
pub mod treesit;
pub mod width;
pub mod window;
pub mod xref;

pub use app::{BufferId, Editor, EditorState};
pub use buffer::{Buffer, Edit, Pos};
pub use clipboard::{SaveInterprogramPasteBeforeKill, SelectionTarget};
pub use commands::{CommandInfo, CommandRegistry, CommandSource};
pub use declare::{
    CollectedDeclaration, CollectedDeclarations, CollectionError, DeclarationState, DeferredCode,
    PluginUse, SkipReason, SystemPackageNeed, UseOutcome, UseReport,
};
pub use keymap::{ActiveKeymaps, Keymap, Keymaps, Lookup, Origin, Scope};
pub use kill_ring::KillRing;
pub use kmacro::Kmacro;
pub use links::{LinkSpan, LinkTarget};
pub use lockfile::{LockCapabilities, LockEntry, LockFile, Snapshot, SnapshotChange};
pub use messages::MessageLogMax;
pub use modes::{ModeDef, ModeKind, ModeRegistry};
pub use package_journal::{Journal, JournalState, Recovery};
pub use permissions::{
    CapabilityChange, CapabilityDiff, GrantBinding, GrantStore, Grants, ImportMismatch, Manifest,
    PermissionKind, Setting, SettingKind, format_grant_time, uncovered_imports,
};
pub use plugin::host::{Outcome, PluginFile, PluginHost, PluginRequest};
pub use plugin::registry::PluginRegistry;
pub use plugin::runtime::{Effect, LoadedPlugin, PluginMonitor, PluginRuntime, Question};
pub use registers::Register;
pub use rhai::{RhaiConfig, RhaiMonitor};
pub use window::{Direction, FrameId, Rect, Side, TabId, Window, WindowConfig, WindowId, Windows};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub enum EditorError {
    PluginLoadError(String),
    ModeActivationError(String),
    BufferError(String),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PluginLoadError(e) => write!(f, "Plugin load error: {e}"),
            Self::ModeActivationError(e) => write!(f, "Mode activation error: {e}"),
            Self::BufferError(e) => write!(f, "Buffer error: {e}"),
        }
    }
}

impl std::error::Error for EditorError {}

pub type Result<T> = std::result::Result<T, EditorError>;
