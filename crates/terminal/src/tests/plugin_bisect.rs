// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end tests for scripted plugin-bisect (`--headless --test`).

use super::*;
use std::sync::{Mutex, MutexGuard};

static ENV: Mutex<()> = Mutex::new(());

struct EnvGuard {
    data: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(data: &std::path::Path) -> (Self, MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|error| error.into_inner());
        let old = std::env::var_os("FORGE_DATA_DIR");
        // SAFETY: ENV serializes the tests that change it.
        unsafe { std::env::set_var("FORGE_DATA_DIR", data) };
        (Self { data: old }, guard)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: ENV serializes the tests that change it.
        unsafe {
            match &self.data {
                Some(value) => std::env::set_var("FORGE_DATA_DIR", value),
                None => std::env::remove_var("FORGE_DATA_DIR"),
            }
        }
    }
}

/// A component that only runs init: enough to load, unload and load again.
const EMPTY_PLUGIN: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (i32.const 1024))
  (func (export "init") (result i32)
    (i32.const 0))
  (func (export "forge:editor/handler@0.1.0#run-command")
    (param i32 i32 i32 i64) (result i32)
    (i32.const 0))
  (func (export "forge:editor/handler@0.1.0#on-event") (param i32 i32))
  (func (export "forge:editor/handler@0.1.0#on-answer") (param i32 i32 i32 i32)))
"#;

#[test]
fn headless_bisect_finds_one_culprit_and_restores_the_plugins() {
    let root = std::env::temp_dir().join(format!(
        "forge-plugin-bisect-terminal-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let plugins_dir = root.join("plugins");
    let (_guard, _lock) = EnvGuard::set(&data);

    let names = ["a", "b", "bad", "c", "d", "e", "f", "g"];
    let paths: Vec<std::path::PathBuf> = names
        .iter()
        .map(|name| plugin_file_from_wat(&plugins_dir, name, EMPTY_PLUGIN))
        .collect();
    let mut app = app_with("");
    for path in &paths {
        crate::plugins::load(&mut app, path);
    }
    let before = crate::plugins::loaded_names(&app);
    assert_eq!(before.len(), names.len(), "{before:?}");
    assert_eq!(
        std::fs::read_dir(&plugins_dir).unwrap().count(),
        names.len()
    );

    let command = if cfg!(windows) {
        "echo %FORGE_BISECT_ENABLED% | findstr /C:\"bad\" >NUL && exit /b 1 || exit /b 0"
    } else {
        "case \",$FORGE_BISECT_ENABLED,\" in *,bad,*) exit 1;; *) exit 0;; esac"
    };
    let report = crate::plugin_bisect::run_headless(&mut app, command).unwrap();
    assert_eq!(
        report.outcome,
        forge_core::plugin_bisect::BisectOutcome::Culprit("bad".into())
    );
    assert!(report.tests <= 6, "{} tests", report.tests);
    assert_eq!(crate::plugins::loaded_names(&app), before, "restored");
    assert!(!forge_core::plugin_bisect::BisectJournal::path(&data).exists());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn headless_bisect_reports_two_interacting_culprits() {
    let root = std::env::temp_dir().join(format!(
        "forge-plugin-bisect-interacting-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let plugins_dir = root.join("plugins");
    let (_guard, _lock) = EnvGuard::set(&data);

    let names = ["a", "b", "c", "d"];
    let paths: Vec<std::path::PathBuf> = names
        .iter()
        .map(|name| plugin_file_from_wat(&plugins_dir, name, EMPTY_PLUGIN))
        .collect();
    let mut app = app_with("");
    for path in &paths {
        crate::plugins::load(&mut app, path);
    }

    let command = if cfg!(windows) {
        "echo %FORGE_BISECT_ENABLED% | findstr /C:\"a\" >NUL && echo %FORGE_BISECT_ENABLED% | findstr /C:\"b\" >NUL && exit /b 1 || exit /b 0"
    } else {
        "case \",$FORGE_BISECT_ENABLED,\" in *,a,*) case \",$FORGE_BISECT_ENABLED,\" in *,b,*) exit 1;; esac;; esac; exit 0"
    };
    let report = crate::plugin_bisect::run_headless(&mut app, command).unwrap();
    assert_eq!(
        report.outcome,
        forge_core::plugin_bisect::BisectOutcome::NoSingleCulprit
    );
    assert_eq!(crate::plugins::loaded_names(&app).len(), names.len());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn interactive_bisect_asks_and_restores_after_answering() {
    let root = std::env::temp_dir().join(format!(
        "forge-plugin-bisect-interactive-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let plugins_dir = root.join("plugins");
    let (_guard, _lock) = EnvGuard::set(&data);

    let names = ["a", "b", "bad", "c", "d", "e", "f", "g"];
    let paths: Vec<std::path::PathBuf> = names
        .iter()
        .map(|name| plugin_file_from_wat(&plugins_dir, name, EMPTY_PLUGIN))
        .collect();
    let mut app = app_with("");
    for path in &paths {
        crate::plugins::load(&mut app, path);
    }
    let before = crate::plugins::loaded_names(&app);

    press(&mut app, "M-x plugin-bisect RET");
    assert!(
        app.minibuffer
            .as_ref()
            .is_some_and(|mb| mb.prompt.starts_with("Plugin bisect 1:")),
        "{:?}",
        app.minibuffer.as_ref().map(|mb| &mb.prompt)
    );
    for _ in 0..20 {
        if app.minibuffer.is_none() {
            break;
        }
        let has_bad = crate::plugins::loaded_names(&app).contains(&"bad".to_string());
        press(&mut app, if has_bad { "y" } else { "n" });
    }
    assert!(app.minibuffer.is_none(), "the search ended");
    assert_eq!(
        app.message.as_deref(),
        Some("Plugin bisect: the culprit is bad (6 tests)")
    );
    assert_eq!(crate::plugins::loaded_names(&app), before, "restored");
    assert!(!forge_core::plugin_bisect::BisectJournal::path(&data).exists());

    // C-g puts the original set back too.
    press(&mut app, "M-x plugin-bisect RET");
    assert!(app.minibuffer.is_some());
    press(&mut app, "C-g");
    assert!(app.minibuffer.is_none());
    assert_eq!(
        crate::plugins::loaded_names(&app),
        before,
        "restored after C-g"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("aborted"),
        "{:?}",
        app.message
    );

    let _ = std::fs::remove_dir_all(&root);
}
