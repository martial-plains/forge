// SPDX-License-Identifier: MIT OR Apache-2.0
//! Collect package declarations without running closures, loading plugins or
//! touching the machine.

use crate::declare::{
    CollectedDeclaration, CollectedDeclarations, CollectionError, DeclarationState, UseOutcome,
    UseReport,
};
use crate::{Editor, RhaiConfig};
use std::path::Path;

/// Evaluate one init.rhai file.
pub fn collect_from_init(path: &Path) -> CollectedDeclarations {
    let mut out = CollectedDeclarations {
        init_path: Some(path.to_path_buf()),
        ..Default::default()
    };
    let rhai = match RhaiConfig::collecting() {
        Ok(rhai) => rhai,
        Err(error) => {
            out.errors.push(CollectionError::new(
                None,
                None,
                format!("can't start Rhai: {error}"),
            ));
            return out;
        }
    };
    let mut editor = scratch_editor();
    let outcome = rhai.load_file(&mut editor, path);
    finish(&rhai, &outcome, &mut out);
    out
}

/// Evaluate Rhai source, for tests and embedding.
pub fn collect_str(name: &str, source: &str) -> CollectedDeclarations {
    let mut out = CollectedDeclarations::default();
    let rhai = match RhaiConfig::collecting() {
        Ok(rhai) => rhai,
        Err(error) => {
            out.errors.push(CollectionError::new(
                None,
                None,
                format!("can't start Rhai: {error}"),
            ));
            return out;
        }
    };
    let mut editor = scratch_editor();
    let outcome = rhai.load_str(&mut editor, name, source);
    finish(&rhai, &outcome, &mut out);
    out
}

/// The user's init.rhai plus theme files, if it exists.
pub fn collect_default() -> CollectedDeclarations {
    let path = crate::config::init_file();
    let mut out = CollectedDeclarations {
        init_path: path.clone(),
        ..Default::default()
    };
    let Some(path) = path else {
        return out;
    };
    let rhai = match RhaiConfig::collecting() {
        Ok(rhai) => rhai,
        Err(error) => {
            out.errors.push(CollectionError::new(
                None,
                None,
                format!("can't start Rhai: {error}"),
            ));
            return out;
        }
    };
    let mut editor = scratch_editor();
    if path.exists() {
        let outcome = rhai.load_file(&mut editor, &path);
        finish(&rhai, &outcome, &mut out);
    }
    for theme in crate::config::theme_files() {
        let outcome = rhai.load_file(&mut editor, &theme);
        finish(&rhai, &outcome, &mut out);
    }
    out
}

fn scratch_editor() -> Editor {
    let mut editor = Editor::new();
    editor.create_buffer("*scratch*");
    editor
        .state
        .active_buffer_mut()
        .expect("the scratch buffer")
        .set_major_mode(crate::rhai::INTERACTION_MODE);
    editor
}

fn finish(
    rhai: &RhaiConfig,
    outcome: &crate::plugin::host::Outcome,
    out: &mut CollectedDeclarations,
) {
    for error in &outcome.errors {
        out.errors
            .push(CollectionError::new(None, None, error.clone()));
    }
    out.report = rhai.use_reports();
    out.entries = entries(&rhai.plugin_uses(), &out.report);
}

fn entries(uses: &[crate::declare::PluginUse], reports: &[UseReport]) -> Vec<CollectedDeclaration> {
    let mut entries = Vec::new();
    for use_ in uses {
        let matching: Vec<&UseReport> = reports
            .iter()
            .filter(|report| {
                report.name == use_.name && !matches!(report.outcome, UseOutcome::Note)
            })
            .collect();
        let state = matching
            .last()
            .map(|report| match &report.outcome {
                UseOutcome::Collected { .. } => DeclarationState::Active,
                UseOutcome::Conditional { .. } => DeclarationState::Conditional,
                UseOutcome::Skipped { reason } => DeclarationState::Skipped(reason.clone()),
                UseOutcome::Error { .. } => DeclarationState::Invalid,
                UseOutcome::Note => DeclarationState::Active,
            })
            .unwrap_or(DeclarationState::Invalid);
        let mut deferred = Vec::new();
        let mut system_packages = Vec::new();
        for report in matching {
            for code in &report.deferred {
                if !deferred.contains(code) {
                    deferred.push(*code);
                }
            }
            system_packages.extend(report.system_packages.iter().cloned());
        }
        entries.push(CollectedDeclaration {
            name: use_.name.clone(),
            line: use_.line,
            declaration: Some(use_.clone()),
            state,
            deferred,
            system_packages,
        });
    }
    let declared: Vec<&str> = uses.iter().map(|use_| use_.name.as_str()).collect();
    for report in reports {
        if matches!(report.outcome, UseOutcome::Note) || declared.contains(&report.name.as_str()) {
            continue;
        }
        let state = match &report.outcome {
            UseOutcome::Skipped { reason } => DeclarationState::Skipped(reason.clone()),
            UseOutcome::Conditional { .. } => DeclarationState::Conditional,
            UseOutcome::Error { .. } => DeclarationState::Invalid,
            UseOutcome::Collected { .. } | UseOutcome::Note => continue,
        };
        entries.push(CollectedDeclaration {
            name: report.name.clone(),
            line: report.line,
            declaration: None,
            state,
            deferred: report.deferred.clone(),
            system_packages: report.system_packages.clone(),
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declare::{DeferredCode, PluginUse, SkipReason};

    #[test]
    fn active_declarations_and_order() {
        let out = collect_str(
            "init.rhai",
            r#"
            use_package("a", #{});
            use_package("b", #{ after: ["a"] });
            use_package("c", #{ disabled: true });
        "#,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.active_order(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(out.active_uses().len(), 2);
        assert_eq!(out.all_uses().len(), 2);
        assert_eq!(out.entries.len(), 3);
        assert_eq!(
            out.get("c").map(|entry| entry.state.clone()),
            Some(DeclarationState::Skipped(SkipReason::Disabled))
        );
    }

    #[test]
    fn closures_are_not_run() {
        let out = collect_str(
            "init.rhai",
            r#"
            use_package("a", #{
                preface: || { throw "preface ran" },
                init: || { throw "init ran" },
                config: || { throw "config ran" },
                when: || { throw "when ran" },
            });
        "#,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let entry = out.get("a").expect("the declaration");
        assert_eq!(entry.state, DeclarationState::Conditional);
        assert_eq!(
            entry.deferred,
            vec![
                DeferredCode::Condition,
                DeferredCode::Preface,
                DeferredCode::Init,
                DeferredCode::Config
            ]
        );
    }

    #[test]
    fn unless_closure_is_conditional() {
        let out = collect_str(
            "init.rhai",
            r#"use_package("a", #{ unless: || { true } });"#,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.get("a").map(|entry| entry.state.clone()),
            Some(DeclarationState::Conditional)
        );
    }

    #[test]
    fn static_conditions_still_skip() {
        let out = collect_str(
            "init.rhai",
            r#"
            use_package("a", #{ when: false });
            use_package("b", #{ "if": false });
            use_package("c", #{ unless: true });
        "#,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.get("a").map(|entry| entry.state.clone()),
            Some(DeclarationState::Skipped(SkipReason::WhenFalse))
        );
        assert_eq!(
            out.get("b").map(|entry| entry.state.clone()),
            Some(DeclarationState::Skipped(SkipReason::IfFalse))
        );
        assert_eq!(
            out.get("c").map(|entry| entry.state.clone()),
            Some(DeclarationState::Skipped(SkipReason::UnlessTrue))
        );
    }

    #[test]
    fn missing_requires_is_skipped() {
        let out = collect_str("init.rhai", r#"use_package("a", #{ requires: ["b"] });"#);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.get("a").map(|entry| entry.state.clone()),
            Some(DeclarationState::Skipped(SkipReason::MissingRequires(
                "b".to_string()
            )))
        );
    }

    #[test]
    fn system_packages_are_recorded_not_installed() {
        let dir = std::env::temp_dir().join(format!(
            "forge-collect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("installed");
        let exe = format!("forge-missing-{}", std::process::id());
        let source = format!(
            "use_package(\"a\", #{{ ensure_system_package: #{{ \"{exe}\": \"touch {}\" }} }});",
            marker.display()
        );
        let out = collect_str("init.rhai", &source);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let entry = out.get("a").expect("the declaration");
        assert_eq!(entry.system_packages.len(), 1);
        assert!(!entry.system_packages[0].present);
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_lines_match_use_package_report() {
        let out = collect_str(
            "init.rhai",
            r#"
            use_package("a", #{});
            use_package("b", #{ disabled: true });
        "#,
        );
        assert_eq!(
            out.report_lines(),
            vec![
                "a: loads at startup".to_string(),
                "b: skipped, disabled".to_string()
            ]
        );
    }

    #[test]
    fn errors_are_collected() {
        let out = collect_str("init.rhai", r#"use_package("a", #{ nope: 1 });"#);
        assert!(!out.errors.is_empty());
        assert_eq!(
            out.get("a").map(|entry| entry.state.clone()),
            Some(DeclarationState::Invalid)
        );
    }

    #[test]
    fn deferred_code_must_still_be_a_function() {
        let out = collect_str("init.rhai", r#"use_package("a", #{ preface: "nope" });"#);
        assert!(!out.errors.is_empty());
        assert_eq!(out.entries, Vec::new());
    }

    #[test]
    fn plugin_use_line_is_recorded() {
        let out = collect_str("init.rhai", "\nuse_package(\"a\", #{});");
        let use_: &PluginUse = out.get("a").and_then(|e| e.declaration.as_ref()).unwrap();
        assert_eq!(use_.line, Some(2));
    }
}
