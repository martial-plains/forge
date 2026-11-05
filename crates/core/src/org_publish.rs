// SPDX-License-Identifier: MIT OR Apache-2.0
//! `org-publish`: projects (`org-publish-project-alist`) that turn a
//! directory of Org files into a site. A project is one string,
//! `NAME :key value ...`, with the keys Forge uses:
//!
//! ```text
//! notes :base-directory ~/org :publishing-directory ~/public_html
//!       :publishing-function html :recursive t :exclude "^private/"
//! static :base-directory ~/org :base-extension "png|jpg|css"
//!       :publishing-directory ~/public_html :publishing-function attachment
//! site :components "notes static"
//! ```
//!
//! Also: `:include`, `:auto-sitemap`, `:sitemap-filename`, `:sitemap-title`,
//! `:sitemap-sort-files`, `:sitemap-style`, `:html-head`, `:with-toc`,
//! `:section-numbers`. Only files that changed since the last publish are
//! written, unless forced.

use crate::export::{self, Output, Settings};
use crate::org_dblock::{get, parse_params, truthy};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub params: Vec<(String, String)>,
}

/// `NAME :key value ...`.
pub fn parse_project(entry: &str) -> Result<Project, String> {
    let t = entry.trim();
    let end = t.find(char::is_whitespace).unwrap_or(t.len());
    let name = &t[..end];
    if name.is_empty() || name.starts_with(':') {
        return Err(format!("Project needs a name first: {entry}"));
    }
    Ok(Project {
        name: name.to_string(),
        params: parse_params(&t[end..]),
    })
}

fn home(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => PathBuf::from(h).join(rest),
        _ if s == "~" => std::env::var("HOME").map(PathBuf::from).unwrap_or_default(),
        _ => PathBuf::from(s),
    }
}

/// One project that publishes files (a component, not a bundle).
#[derive(Debug, Clone)]
pub struct Job {
    pub name: String,
    pub base: PathBuf,
    pub dest: PathBuf,
    pub params: Vec<(String, String)>,
}

/// The jobs for `name`: itself, or its `:components` in order.
pub fn jobs(projects: &[Project], name: &str) -> Result<Vec<Job>, String> {
    fn walk(
        projects: &[Project],
        name: &str,
        seen: &mut Vec<String>,
        out: &mut Vec<Job>,
    ) -> Result<(), String> {
        if seen.iter().any(|s| s == name) {
            return Err(format!("Project {name} contains itself"));
        }
        let p = projects
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("No project called {name}"))?;
        if let Some(c) = get(&p.params, "components") {
            seen.push(name.to_string());
            for sub in c
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
            {
                walk(projects, sub, seen, out)?;
            }
            seen.pop();
            return Ok(());
        }
        let base = get(&p.params, "base-directory")
            .ok_or_else(|| format!("{name}: no :base-directory"))?;
        let dest = get(&p.params, "publishing-directory")
            .ok_or_else(|| format!("{name}: no :publishing-directory"))?;
        out.push(Job {
            name: name.to_string(),
            base: home(base),
            dest: home(dest),
            params: p.params.clone(),
        });
        Ok(())
    }
    let mut out = Vec::new();
    walk(projects, name, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// The job whose base directory holds `file`.
pub fn job_for_file(projects: &[Project], file: &Path) -> Option<Job> {
    projects
        .iter()
        .filter_map(|p| jobs(projects, &p.name).ok())
        .flatten()
        .find(|j| file.starts_with(&j.base) && j.matches(file))
}

/// The backend a `:publishing-function` names: `html`,
/// `org-html-publish-to-html`, ... or "attachment".
fn function(job: &Job) -> String {
    let f = get(&job.params, "publishing-function").unwrap_or("html");
    let f = f.trim_start_matches("org-");
    if f.starts_with("publish-attachment") || f == "attachment" {
        return "attachment".to_string();
    }
    f.split("-publish-to-")
        .next()
        .unwrap_or(f)
        .trim_end_matches("-publish")
        .to_string()
}

impl Job {
    fn extensions(&self) -> Option<Vec<String>> {
        let e = get(&self.params, "base-extension").unwrap_or("org");
        (e != "any").then(|| {
            e.split('|')
                .map(|s| s.trim_start_matches('.').to_string())
                .collect()
        })
    }

    fn excluded(&self, rel: &Path) -> bool {
        let rel = rel.to_string_lossy().replace('\\', "/");
        get(&self.params, "exclude")
            .and_then(|r| Regex::new(r).ok())
            .is_some_and(|re| re.is_match(&rel))
    }

    fn included(&self, rel: &Path) -> bool {
        get(&self.params, "include").is_some_and(|l| {
            l.split(['|', ',', ' '])
                .any(|f| !f.is_empty() && Path::new(f) == rel)
        })
    }

    fn matches(&self, file: &Path) -> bool {
        let Ok(rel) = file.strip_prefix(&self.base) else {
            return false;
        };
        if self.included(rel) {
            return true;
        }
        let ext_ok = self.extensions().is_none_or(|exts| {
            file.extension()
                .is_some_and(|e| exts.iter().any(|x| x.as_str() == e.to_string_lossy()))
        });
        let shallow = truthy(&self.params, "recursive", false) || rel.components().count() == 1;
        ext_ok && shallow && !self.excluded(rel)
    }

    /// The files to publish, relative to the base directory, sorted.
    pub fn files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, base: &Path, rec: bool, out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                if p.is_dir() {
                    if rec {
                        walk(&p, base, rec, out);
                    }
                } else if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.to_path_buf());
                }
            }
        }
        let mut all = Vec::new();
        walk(
            &self.base,
            &self.base,
            truthy(&self.params, "recursive", false),
            &mut all,
        );
        let mut out: Vec<PathBuf> = all
            .into_iter()
            .filter(|r| self.matches(&self.base.join(r)))
            .collect();
        out.sort();
        out
    }

    fn output(&self, rel: &Path) -> Option<PathBuf> {
        let f = function(self);
        if f == "attachment" {
            return Some(self.dest.join(rel));
        }
        let b = export::backend(&f)?;
        Some(self.dest.join(rel).with_extension(b.ext))
    }
}

fn title_of(text: &str, rel: &Path) -> String {
    text.lines()
        .find_map(|l| {
            let t = l.trim();
            (t.len() > 8 && t[..8].eq_ignore_ascii_case("#+TITLE:"))
                .then(|| t[8..].trim().to_string())
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            rel.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

/// The sitemap's text for `files` (relative paths), per the job's options.
pub fn sitemap(job: &Job, files: &[PathBuf]) -> String {
    let mut entries: Vec<(PathBuf, String, std::time::SystemTime)> = files
        .iter()
        .filter(|r| r.extension().is_some_and(|e| e == "org"))
        .map(|r| {
            let p = job.base.join(r);
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let mtime = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (r.clone(), title_of(&text, r), mtime)
        })
        .collect();
    let ignore_case = truthy(&job.params, "sitemap-ignore-case", false);
    match get(&job.params, "sitemap-sort-files").unwrap_or("alphabetically") {
        "chronologically" => entries.sort_by_key(|e| e.2),
        "anti-chronologically" => entries.sort_by_key(|e| std::cmp::Reverse(e.2)),
        _ => entries.sort_by_key(|e| {
            let k = e.0.to_string_lossy().into_owned();
            if ignore_case { k.to_lowercase() } else { k }
        }),
    }
    let title = get(&job.params, "sitemap-title")
        .map(String::from)
        .unwrap_or_else(|| format!("Sitemap for project {}", job.name));
    let mut out = format!("#+TITLE: {title}\n\n");
    let tree = get(&job.params, "sitemap-style") == Some("tree");
    let mut open: Vec<String> = Vec::new();
    for (rel, t, _) in &entries {
        let dirs: Vec<String> = rel
            .parent()
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let depth = if tree {
            let common = open.iter().zip(&dirs).take_while(|(a, b)| a == b).count();
            open.truncate(common);
            for (i, d) in dirs.iter().enumerate().skip(common) {
                out.push_str(&format!("{}- {d}\n", "  ".repeat(i)));
                open.push(d.clone());
            }
            dirs.len()
        } else {
            0
        };
        out.push_str(&format!(
            "{}- [[file:{}][{t}]]\n",
            "  ".repeat(depth),
            rel.to_string_lossy().replace('\\', "/")
        ));
    }
    out
}

/// What a publish did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub published: Vec<PathBuf>,
    pub skipped: usize,
    pub errors: Vec<String>,
}

fn mtime_secs(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn cache_file(cache: &Path, project: &str) -> PathBuf {
    cache.join(format!("{}.timestamps", project.replace(['/', '\\'], "_")))
}

fn load_cache(cache: &Path, project: &str) -> BTreeMap<String, u64> {
    std::fs::read_to_string(cache_file(cache, project))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (t, p) = l.split_once('\t')?;
            Some((p.to_string(), t.parse().ok()?))
        })
        .collect()
}

fn save_cache(cache: &Path, project: &str, m: &BTreeMap<String, u64>) {
    let _ = std::fs::create_dir_all(cache);
    let text: String = m.iter().map(|(p, t)| format!("{t}\t{p}\n")).collect();
    let _ = std::fs::write(cache_file(cache, project), text);
}

/// Forget what was published, so the next publish writes everything.
pub fn remove_timestamps(cache: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(cache) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "timestamps")
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
    }
    n
}

/// The export keywords a project's options stand for, put before the
/// file's own (which win).
fn preamble(job: &Job) -> String {
    let mut s = String::new();
    if let Some(h) = get(&job.params, "html-head") {
        s.push_str(&format!("#+HTML_HEAD: {h}\n"));
    }
    if let Some(u) = get(&job.params, "html-link-up") {
        s.push_str(&format!("#+HTML_LINK_UP: {u}\n"));
    }
    if let Some(u) = get(&job.params, "html-link-home") {
        s.push_str(&format!("#+HTML_LINK_HOME: {u}\n"));
    }
    let mut opts = Vec::new();
    if !truthy(&job.params, "with-toc", true) {
        opts.push("toc:nil");
    }
    if !truthy(&job.params, "section-numbers", true) {
        opts.push("num:nil");
    }
    if !opts.is_empty() {
        s.push_str(&format!("#+OPTIONS: {}\n", opts.join(" ")));
    }
    s
}

/// Publish one job. `only` limits it to one file (relative); `force`
/// ignores the timestamps.
pub fn publish(job: &Job, cache: &Path, force: bool, only: Option<&Path>, today: &str) -> Report {
    let mut rep = Report::default();
    let f = function(job);
    if f != "attachment" && export::backend(&f).is_none() {
        rep.errors
            .push(format!("{}: unknown publishing function {f}", job.name));
        return rep;
    }
    let mut files = job.files();
    if truthy(&job.params, "auto-sitemap", false) && f != "attachment" {
        let name = get(&job.params, "sitemap-filename").unwrap_or("sitemap.org");
        let rel = PathBuf::from(name);
        files.retain(|r| *r != rel);
        let text = sitemap(job, &files);
        if std::fs::read_to_string(job.base.join(&rel)).ok().as_deref() != Some(text.as_str())
            && let Err(e) = std::fs::write(job.base.join(&rel), &text)
        {
            rep.errors.push(format!("Cannot write the sitemap: {e}"));
        }
        if !files.contains(&rel) {
            files.push(rel);
        }
    }
    let mut stamps = load_cache(cache, &job.name);
    let pre = preamble(job);
    let base = job.base.clone();
    for rel in files {
        if only.is_some_and(|o| o != rel) {
            continue;
        }
        let src = job.base.join(&rel);
        let Some(out) = job.output(&rel) else {
            continue;
        };
        let key = src.to_string_lossy().into_owned();
        let m = mtime_secs(&src).unwrap_or(0);
        if !force && out.exists() && stamps.get(&key).is_some_and(|&t| t >= m) {
            rep.skipped += 1;
            continue;
        }
        let result: Result<Vec<u8>, String> = if f == "attachment" {
            std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))
        } else {
            std::fs::read_to_string(&src)
                .map_err(|e| format!("{}: {e}", src.display()))
                .and_then(|text| {
                    let b2 = base.clone();
                    let b3 = base.clone();
                    let read = move |f: &str| std::fs::read_to_string(b2.join(f)).ok();
                    let st = Settings {
                        body_only: false,
                        source: src.to_string_lossy().into_owned(),
                        today: today.to_string(),
                        images: Some(Rc::new(move |f: &str| std::fs::read(b3.join(f)).ok())),
                    };
                    match export::export(&f, &format!("{pre}{text}"), &read, &st) {
                        Ok(Output::Text(t)) => Ok(t.into_bytes()),
                        Ok(Output::Bytes(b)) => Ok(b),
                        Err(e) => Err(format!("{}: {e}", src.display())),
                    }
                })
        };
        match result {
            Ok(bytes) => {
                if let Some(d) = out.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match std::fs::write(&out, bytes) {
                    Ok(()) => {
                        stamps.insert(key, m);
                        rep.published.push(out);
                    }
                    Err(e) => rep.errors.push(format!("{}: {e}", out.display())),
                }
            }
            Err(e) => rep.errors.push(e),
        }
    }
    save_cache(cache, &job.name, &stamps);
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-publish-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn site(tag: &str) -> (PathBuf, Vec<Project>) {
        let d = scratch(tag);
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::create_dir_all(d.join("src/private")).unwrap();
        fs::write(
            d.join("src/index.org"),
            "#+TITLE: Home\n\nSee [[file:sub/page.org][page]].\n",
        )
        .unwrap();
        fs::write(d.join("src/sub/page.org"), "#+TITLE: A page\n\nText.\n").unwrap();
        fs::write(d.join("src/private/x.org"), "secret\n").unwrap();
        fs::write(d.join("src/pic.png"), b"png").unwrap();
        let src = d.join("src").display().to_string();
        let out = d.join("out").display().to_string();
        let ps = vec![
            parse_project(&format!(
                "notes :base-directory {src} :publishing-directory {out} :recursive t :exclude \"^private/\" :auto-sitemap t :sitemap-title \"All\""
            ))
            .unwrap(),
            parse_project(&format!(
                "static :base-directory {src} :base-extension \"png|jpg\" :publishing-directory {out} :publishing-function attachment"
            ))
            .unwrap(),
            parse_project("site :components \"notes static\"").unwrap(),
        ];
        (d, ps)
    }

    #[test]
    fn components_and_file_lists() {
        let (d, ps) = site("lists");
        let js = jobs(&ps, "site").unwrap();
        assert_eq!(
            js.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(),
            ["notes", "static"]
        );
        let names: Vec<String> = js[0]
            .files()
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(names, ["index.org", "sub/page.org"]);
        assert_eq!(js[1].files(), vec![PathBuf::from("pic.png")]);
        assert!(jobs(&ps, "nope").is_err());
        let again = vec![
            parse_project("a :components b").unwrap(),
            parse_project("b :components a").unwrap(),
        ];
        assert!(jobs(&again, "a").is_err());
        let f = d.join("src/sub/page.org");
        assert_eq!(job_for_file(&ps, &f).unwrap().name, "notes");
        assert!(job_for_file(&ps, &d.join("src/private/x.org")).is_none());
    }

    #[test]
    fn publishing_writes_html_sitemap_and_attachments_once() {
        let (d, ps) = site("run");
        let cache = d.join("cache");
        let mut written = 0;
        for j in jobs(&ps, "site").unwrap() {
            written += publish(&j, &cache, false, None, "2026-11-02")
                .published
                .len();
        }
        // index, page, sitemap, and the picture.
        assert_eq!(written, 4);
        let html = fs::read_to_string(d.join("out/index.html")).unwrap();
        assert!(html.contains("sub/page.html"), "{html}");
        assert!(d.join("out/sub/page.html").is_file());
        assert!(d.join("out/pic.png").is_file());
        let map = fs::read_to_string(d.join("src/sitemap.org")).unwrap();
        assert!(
            map.contains("#+TITLE: All") && map.contains("[[file:sub/page.org][A page]]"),
            "{map}"
        );
        assert!(!d.join("out/private").exists());
        // Nothing changed: nothing is written. Forcing writes it all again.
        let j = &jobs(&ps, "notes").unwrap()[0];
        let again = publish(j, &cache, false, None, "2026-11-02");
        assert_eq!((again.published.len(), again.skipped), (0, 3));
        assert_eq!(
            publish(j, &cache, true, None, "2026-11-02").published.len(),
            3
        );
        assert_eq!(remove_timestamps(&cache), 2);
        assert_eq!(
            publish(j, &cache, false, Some(Path::new("index.org")), "x")
                .published
                .len(),
            1
        );
    }

    #[test]
    fn bad_functions_and_tree_sitemaps() {
        let (d, _) = site("tree");
        let p = parse_project(&format!(
            "t :base-directory {} :publishing-directory {} :recursive t :publishing-function nonsense :sitemap-style tree",
            d.join("src").display(),
            d.join("o2").display()
        ))
        .unwrap();
        let j = &jobs(&[p], "t").unwrap()[0];
        assert_eq!(publish(j, &d.join("c2"), false, None, "x").errors.len(), 1);
        let tree = sitemap(j, &j.files());
        assert!(
            tree.contains("- sub\n  - [[file:sub/page.org][A page]]"),
            "{tree}"
        );
        assert!(parse_project(":oops").is_err());
    }
}
