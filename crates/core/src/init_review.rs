// SPDX-License-Identifier: MIT OR Apache-2.0
//! Review permission additions when `init.rhai` changes outside Forge.
//!
//! The source is compiled and walked, never evaluated. That lets startup
//! compare what the file asked for last time with what it asks for now
//! before deciding whether to run it.

use crate::config;
use crate::fsutil;
use rhai::{ASTNode, Dynamic, Engine, Expr, ImmutableString};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A changed init file that asks for permissions the last run did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReview {
    pub additions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Permissions {
    items: BTreeSet<String>,
}

#[derive(Debug, Clone)]
enum Literal {
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Literal>),
    Map(Vec<(String, Literal)>),
    Other,
}

impl Literal {
    fn from_expr(expr: &Expr) -> Self {
        match expr {
            Expr::BoolConstant(value, ..) => Self::Bool(*value),
            Expr::IntegerConstant(value, ..) => Self::Int(*value),
            Expr::StringConstant(value, ..) => Self::Str(value.to_string()),
            Expr::Array(items, ..) => Self::Array(items.iter().map(Self::from_expr).collect()),
            Expr::Map(entries, ..) => Self::Map(
                entries
                    .0
                    .iter()
                    .map(|(key, value)| (key.as_str().to_string(), Self::from_expr(value)))
                    .collect(),
            ),
            Expr::DynamicConstant(value, ..) => Self::from_dynamic(value),
            _ => Self::Other,
        }
    }

    fn from_dynamic(value: &Dynamic) -> Self {
        if let Some(value) = value.clone().try_cast::<bool>() {
            Self::Bool(value)
        } else if let Some(value) = value.clone().try_cast::<i64>() {
            Self::Int(value)
        } else if let Some(value) = value.clone().try_cast::<ImmutableString>() {
            Self::Str(value.to_string())
        } else if let Some(items) = value.clone().try_cast::<rhai::Array>() {
            Self::Array(items.iter().map(Self::from_dynamic).collect())
        } else if let Some(entries) = value.clone().try_cast::<rhai::Map>() {
            Self::Map(
                entries
                    .iter()
                    .map(|(key, value)| (key.to_string(), Self::from_dynamic(value)))
                    .collect(),
            )
        } else {
            Self::Other
        }
    }

    fn string(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    fn bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    fn map(&self) -> Option<&[(String, Literal)]> {
        match self {
            Self::Map(entries) => Some(entries),
            _ => None,
        }
    }

    fn list(&self) -> Vec<&Literal> {
        match self {
            Self::Array(items) => items.iter().collect(),
            other => vec![other],
        }
    }
}

/// The last source Forge evaluated as the user's `init.rhai`.
pub fn snapshot_path() -> Option<PathBuf> {
    config::state_dir().map(|dir| dir.join("init-review").join("init.rhai"))
}

/// Record `source` as the last version Forge evaluated.
pub fn write_snapshot(source: &str) -> std::io::Result<()> {
    let Some(path) = snapshot_path() else {
        return Ok(());
    };
    if std::fs::read_to_string(&path).ok().as_deref() == Some(source) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    fsutil::write_atomic(&path, source)
}

/// Compare the last evaluated source with the file on disk.
pub fn review(previous: &str, current: &str) -> Option<InitReview> {
    if previous == current || !(opted_in(previous) || opted_in(current)) {
        return None;
    }
    let previous = permissions(previous).ok()?;
    let current = permissions(current).ok()?;
    let additions: Vec<String> = current.items.difference(&previous.items).cloned().collect();
    (!additions.is_empty()).then_some(InitReview { additions })
}

/// Does the source turn init review on? This is read from the AST, so the
/// setting itself cannot run code.
pub fn opted_in(source: &str) -> bool {
    let Ok(ast) = Engine::new_raw().compile(source) else {
        return false;
    };
    let mut enabled = false;
    ast.walk(&mut |path| {
        if let Some(ASTNode::Stmt(rhai::Stmt::Assignment(assignment))) = path.last() {
            let (_, expression) = &**assignment;
            if is_init_review(&expression.lhs)
                && matches!(Literal::from_expr(&expression.rhs), Literal::Bool(true))
            {
                enabled = true;
            }
        }
        true
    });
    enabled
}

fn is_init_review(expr: &Expr) -> bool {
    match expr {
        Expr::Index(binary, ..) => is_opt(&binary.lhs) && literal_is(&binary.rhs, "init_review"),
        Expr::Dot(binary, ..) => match &binary.rhs {
            Expr::Index(inner, ..) => is_opt(&inner.lhs) && literal_is(&inner.rhs, "init_review"),
            _ => is_opt(&binary.lhs) && property_is(&binary.rhs, "init_review"),
        },
        _ => false,
    }
}

fn is_opt(expr: &Expr) -> bool {
    property_is(expr, "opt")
}

fn property_is(expr: &Expr, wanted: &str) -> bool {
    matches!(
        expr,
        Expr::Property(parts, ..) if parts.2.as_str() == wanted
    )
}

fn literal_is(expr: &Expr, wanted: &str) -> bool {
    matches!(
        expr,
        Expr::StringConstant(value, ..) if value.as_str() == wanted
    )
}

fn permissions(source: &str) -> Result<Permissions, String> {
    let ast = Engine::new_raw()
        .compile(source)
        .map_err(|error| error.to_string())?;
    let mut out = Permissions::default();
    ast.walk(&mut |path| {
        if let Some(ASTNode::Expr(Expr::MethodCall(call, ..))) = path.last()
            && let Some(receiver) = method_receiver(path)
        {
            collect_call(&mut out, receiver, call.name.as_str(), &call.args);
        }
        true
    });
    Ok(out)
}

fn method_receiver<'a>(path: &'a [ASTNode<'a>]) -> Option<&'a str> {
    let parent = path.len().checked_sub(2)?;
    let ASTNode::Expr(Expr::Dot(binary, ..)) = &path[parent] else {
        return None;
    };
    if !matches!(binary.rhs, Expr::MethodCall(..)) {
        return None;
    }
    match &binary.lhs {
        Expr::Property(parts, ..) => Some(parts.2.as_str()),
        _ => None,
    }
}

fn collect_call(out: &mut Permissions, receiver: &str, name: &str, args: &[Expr]) {
    match (receiver, name) {
        ("plugin", "grant") => {
            let Some(spec) = args.get(1).map(Literal::from_expr) else {
                out.items
                    .insert("plugin grant with no permission map".into());
                return;
            };
            let plugin = args
                .first()
                .map(Literal::from_expr)
                .and_then(|value| value.string().map(str::to_string))
                .unwrap_or_else(|| "<dynamic>".into());
            match spec.map() {
                Some(entries) => collect_grant(out, &plugin, entries),
                None => {
                    out.items
                        .insert(format!("grant plugin {plugin}: <dynamic permissions>"));
                }
            }
        }
        ("process", "run") => {
            // Only the command is a permission: options and the callback
            // can change without asking for anything new. A command is a
            // shell line or the program and its arguments as an array.
            let mut parts = Vec::new();
            for value in args.iter().map(Literal::from_expr) {
                match value {
                    Literal::Str(value) => parts.push(value),
                    Literal::Array(items) => parts.extend(items.into_iter().map(|item| {
                        if let Literal::Str(value) = item {
                            value
                        } else {
                            "<dynamic>".to_string()
                        }
                    })),
                    _ => {}
                }
            }
            let command = if parts.is_empty() {
                "<dynamic>".to_string()
            } else {
                parts.join(" ")
            };
            out.items.insert(format!("run process {command}"));
        }
        ("http", "get") => {
            let url = arg(args, 0);
            out.items
                .insert(format!("network request {}", command_text(&url)));
        }
        ("http", "request") => {
            // request(method, url, opts?, callback): options and the
            // callback are not permissions.
            let method = arg(args, 0);
            let url = arg(args, 1);
            out.items.insert(format!(
                "network request {} {}",
                command_text(&method),
                command_text(&url)
            ));
        }
        ("fs", "write") => {
            let path = arg(args, 0)
                .string()
                .map(str::to_string)
                .unwrap_or_else(|| "<dynamic>".into());
            out.items.insert(format!("write file {path}"));
        }
        _ => {}
    }
}

fn arg(args: &[Expr], index: usize) -> Literal {
    args.get(index)
        .map(Literal::from_expr)
        .unwrap_or(Literal::Other)
}

fn collect_grant(out: &mut Permissions, plugin: &str, entries: &[(String, Literal)]) {
    for (key, value) in entries {
        match key.as_str() {
            "network" | "http" | "environment" => {
                if value.bool().unwrap_or(true) {
                    out.items.insert(format!("grant plugin {plugin}: {key}"));
                }
            }
            "memory_mb" => {
                if let Literal::Int(value) = value {
                    out.items
                        .insert(format!("grant plugin {plugin}: memory {value} MB"));
                }
            }
            "directories" => {
                let mut found = false;
                for item in value.list() {
                    match item {
                        Literal::Str(path) => {
                            found = true;
                            out.items
                                .insert(format!("grant plugin {plugin}: directory {path}"));
                        }
                        Literal::Map(fields) => {
                            let path = fields
                                .iter()
                                .find(|(key, _)| key == "path")
                                .and_then(|(_, value)| value.string())
                                .unwrap_or("<dynamic>");
                            let write = fields
                                .iter()
                                .find(|(key, _)| key == "write")
                                .and_then(|(_, value)| value.bool())
                                .unwrap_or(false);
                            found = true;
                            let access = if write { "write" } else { "read" };
                            out.items.insert(format!(
                                "grant plugin {plugin}: {access} directory {path}"
                            ));
                        }
                        _ => {}
                    }
                }
                if !found {
                    out.items
                        .insert(format!("grant plugin {plugin}: <dynamic directories>"));
                }
            }
            "processes" => {
                let mut found = false;
                for item in value.list() {
                    if let Literal::Str(program) = item {
                        found = true;
                        out.items
                            .insert(format!("grant plugin {plugin}: process {program}"));
                    }
                }
                if !found {
                    out.items
                        .insert(format!("grant plugin {plugin}: <dynamic processes>"));
                }
            }
            _ => {}
        }
    }
}

fn command_text(value: &Literal) -> String {
    match value {
        Literal::Str(value) => value.clone(),
        Literal::Array(items) => items
            .iter()
            .map(|item| match item {
                Literal::Str(value) => value.clone(),
                _ => "<dynamic>".into(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => "<dynamic>".into(),
    }
}

/// Record `path` when it is the user's `init.rhai`.
pub fn record(path: &Path) {
    if config::init_file().as_deref() != Some(path) {
        return;
    }
    if let Ok(source) = std::fs::read_to_string(path) {
        let _ = write_snapshot(&source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRANTS: &str = r#"
        forge.opt["init_review"] = true;
        forge.plugin.grant("notes", #{
            network: true,
            http: true,
            processes: ["git", "make"],
            directories: [#{ path: "~/Notes", write: true }],
        });
        forge.process.run("git", ["status"]);
        forge.http.get("https://example.test/feed");
        forge.http.request("POST", "https://example.test/items", #{}, |_r| ());
        forge.fs.write("/tmp/out", "text");
    "#;

    #[test]
    fn it_finds_permissions_without_running_the_file() {
        let found = permissions(GRANTS).unwrap();
        assert!(found.items.contains("grant plugin notes: network"));
        assert!(found.items.contains("grant plugin notes: http"));
        assert!(found.items.contains("grant plugin notes: process git"));
        assert!(found.items.contains("grant plugin notes: process make"));
        assert!(
            found
                .items
                .contains("grant plugin notes: write directory ~/Notes")
        );
        assert!(found.items.contains("run process git status"));
        assert!(
            found
                .items
                .contains("network request https://example.test/feed")
        );
        assert!(
            found
                .items
                .contains("network request POST https://example.test/items")
        );
        assert!(found.items.contains("write file /tmp/out"));
    }

    #[test]
    fn it_walks_closures_without_calling_them() {
        let source = r#"
            let later = || {
                forge.process.run("never-runs", ["--check"]);
            };
        "#;
        let found = permissions(source).unwrap();
        assert!(found.items.contains("run process never-runs --check"));
    }

    #[test]
    fn it_reviews_only_new_permissions_when_opted_in() {
        let old = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
        "#;
        let new = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
            forge.process.run("rm", ["-rf", "/tmp/x"]);
        "#;
        let review = review(old, new).expect("the new process is reviewed");
        assert_eq!(review.additions, vec!["run process rm -rf /tmp/x"]);
    }

    #[test]
    fn it_does_not_review_unchanged_files_or_without_opt_in() {
        let old = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
        "#;
        assert!(review(old, old).is_none());

        let new = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
            forge.process.run("rm", ["-rf", "/tmp/x"]);
        "#;
        let off = new.replace("forge.opt[\"init_review\"] = true;", "");
        assert!(review(&off, new).is_none());
    }

    #[test]
    fn it_reviews_new_grant_fields_and_processes() {
        let old = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
        "#;
        let http = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true, http: true });
        "#;
        assert_eq!(
            review(old, http).unwrap().additions,
            vec!["grant plugin notes: http"]
        );

        let processes = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true, processes: ["rm"] });
        "#;
        assert_eq!(
            review(old, processes).unwrap().additions,
            vec!["grant plugin notes: process rm"]
        );
    }

    #[test]
    fn it_ignores_edits_that_do_not_add_permissions() {
        let old = r#"
            forge.opt["init_review"] = true;
            forge.plugin.grant("notes", #{ network: true });
            forge.process.run("git", #{ timeout: 5 }, |r| print(r));
            forge.http.get("https://example.test/feed", |r| print(r));
            forge.http.request("POST", "https://example.test/items", #{}, |r| print(r));
        "#;
        let new = r#"
            forge.opt["init_review"] = true;
            forge.opt["line_numbers"] = "relative";
            forge.keymap.set("C-c n", "next-line");
            forge.plugin.grant("notes", #{ network: true });
            forge.process.run("git", #{ timeout: 60 }, |r| print("done"));
            forge.http.get("https://example.test/feed", #{ Accept: "text/plain" }, |r| print(r));
            forge.http.request("POST", "https://example.test/items", #{ body: "text" }, |r| print(r));
        "#;
        assert!(review(old, new).is_none());
    }
}
