// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org publishing commands. The projects are in `forge.opt.org_publish_projects`
//! (see `forge_core::org_publish` for the format); the work is there too.

use crate::app::App;
use crate::org_cmds::{Ask, choose, now};
use forge_core::org_publish::{self as publish, Job, Project, Report};
use std::path::PathBuf;

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-publish",
        "Org: publish a project (a prefix argument publishes every file again).",
    )
    .body("Projects come from forge.opt.org_publish_projects."),
    forge_docs::CommandDoc::line(
        "org-publish-project",
        "Org: publish a project, asking which.",
    ),
    forge_docs::CommandDoc::line("org-publish-all", "Org: publish every project."),
    forge_docs::CommandDoc::line(
        "org-publish-current-file",
        "Org: publish the file in this buffer, with the project it belongs to.",
    ),
    forge_docs::CommandDoc::line(
        "org-publish-current-project",
        "Org: publish the project this buffer's file belongs to.",
    ),
    forge_docs::CommandDoc::line(
        "org-publish-remove-all-timestamps",
        "Org: forget what was published, so the next publish writes everything.",
    ),
];

fn projects(app: &mut App) -> Option<Vec<Project>> {
    let list = app
        .rhai
        .as_ref()
        .and_then(|l| l.settings().strings.get("org_publish_projects").cloned())
        .unwrap_or_default();
    if list.is_empty() {
        app.error("No projects: set forge.opt.org_publish_projects");
        return None;
    }
    let mut out = Vec::new();
    for entry in &list {
        match publish::parse_project(entry) {
            Ok(p) => out.push(p),
            Err(e) => {
                app.error(format!("forge.opt.org_publish_projects: {e}"));
                return None;
            }
        }
    }
    Some(out)
}

fn cache_dir() -> PathBuf {
    forge_core::config::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("org-publish-timestamps")
}

fn today() -> String {
    let (d, _) = now();
    format!("{:04}-{:02}-{:02}", d.y, d.m, d.d)
}

fn finish(app: &mut App, what: &str, rep: Report) {
    let n = rep.published.len();
    let mut msg = format!("Published {n} file{}", if n == 1 { "" } else { "s" });
    if rep.skipped > 0 {
        msg.push_str(&format!(", {} unchanged", rep.skipped));
    }
    msg.push_str(&format!(" ({what})"));
    if rep.errors.is_empty() {
        app.msg(msg);
    } else {
        let list = rep.errors.join("\n");
        app.show_special_buffer("*Org Publish*", "text-mode", &list);
        app.error(format!(
            "{msg}; {} failed (see *Org Publish*)",
            rep.errors.len()
        ));
    }
}

fn run(app: &mut App, jobs: Vec<Job>, force: bool, only: Option<PathBuf>, what: &str) {
    let mut total = Report::default();
    for j in &jobs {
        let rel = only.as_ref().and_then(|f| f.strip_prefix(&j.base).ok());
        if only.is_some() && rel.is_none() {
            continue;
        }
        let r = publish::publish(j, &cache_dir(), force, rel, &today());
        total.published.extend(r.published);
        total.skipped += r.skipped;
        total.errors.extend(r.errors);
    }
    finish(app, what, total);
}

fn publish_named(app: &mut App, name: &str, force: bool) {
    let Some(ps) = projects(app) else { return };
    match publish::jobs(&ps, name) {
        Ok(js) => run(app, js, force, None, name),
        Err(e) => app.error(e),
    }
}

fn current_file(app: &App) -> Option<PathBuf> {
    app.buf_ref().path().map(|p| p.to_path_buf())
}

/// Run an Org publish command. False if `cmd` isn't one.
pub fn command(app: &mut App, cmd: &str) -> bool {
    if !COMMANDS.iter().any(|cd| cd.name() == cmd) {
        return false;
    }
    let force = app.has_arg();
    match cmd {
        "org-publish" | "org-publish-project" => {
            let Some(ps) = projects(app) else { return true };
            let names = ps.iter().map(|p| p.name.clone()).collect();
            let prompt = if force {
                "Publish project (every file): "
            } else {
                "Publish project: "
            };
            app.org.publish_force = force;
            choose(app, prompt, Ask::PublishProject, names);
        }
        "org-publish-all" => {
            let Some(ps) = projects(app) else { return true };
            // Projects that are nobody's component, in order.
            let parts: Vec<String> = ps
                .iter()
                .filter_map(|p| {
                    forge_core::org_dblock::get(&p.params, "components").map(str::to_string)
                })
                .flat_map(|c| {
                    c.split(|ch: char| ch.is_whitespace() || ch == ',')
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut jobs = Vec::new();
            for p in ps.iter().filter(|p| !parts.contains(&p.name)) {
                match publish::jobs(&ps, &p.name) {
                    Ok(j) => jobs.extend(j),
                    Err(e) => {
                        app.error(e);
                        return true;
                    }
                }
            }
            run(app, jobs, force, None, "all projects");
        }
        "org-publish-current-file" | "org-publish-current-project" => {
            let Some(file) = current_file(app) else {
                app.error("This buffer has no file");
                return true;
            };
            let Some(ps) = projects(app) else { return true };
            let Some(job) = publish::job_for_file(&ps, &file) else {
                app.error("This file is in no publishing project");
                return true;
            };
            if cmd == "org-publish-current-file" {
                let name = job.name.clone();
                // A component of a bundle publishes with its own options.
                run(app, vec![job], true, Some(file), &name);
            } else {
                let name = job.name.clone();
                publish_named(app, &name, force);
            }
        }
        "org-publish-remove-all-timestamps" => {
            let n = publish::remove_timestamps(&cache_dir());
            app.msg(format!(
                "Forgot the publish times of {n} project{}",
                if n == 1 { "" } else { "s" }
            ));
        }
        _ => return false,
    }
    true
}

/// The prompt answered in `Ask::PublishProject`.
pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    if !matches!(a, Ask::PublishProject) {
        return false;
    }
    let force = std::mem::take(&mut app.org.publish_force);
    let name = input.trim();
    if !name.is_empty() {
        publish_named(app, name, force);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Buffer;

    #[test]
    fn publishing_without_projects_says_so() {
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(Buffer::new("t.org"));
        app.editor.show_buffer(id);
        app.run_command("org-publish-all");
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("org_publish_projects"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn a_configured_project_publishes() {
        let d = std::env::temp_dir().join(format!("forge-pub-ed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a.org"), "#+TITLE: A\n\nHi\n").unwrap();
        let init = d.join("init.rhai");
        let config_dir = d.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &init,
            format!(
                "forge.opt[\"org_publish_projects\"] = [\"n :base-directory {config_dir}/src :publishing-directory {config_dir}/out\"];\n",
            ),
        )
        .unwrap();
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(Buffer::new("t.org"));
        app.editor.show_buffer(id);
        app.init_path = Some(init);
        app.load_init_file();
        app.minibuffer = None;
        assert!(answer(&mut app, &Ask::PublishProject, "n"));
        assert!(d.join("out/a.html").is_file(), "{:?}", app.message);
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("Published 1 file"),
            "{:?}",
            app.message
        );
    }
}
