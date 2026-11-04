// SPDX-License-Identifier: MIT OR Apache-2.0
//! Installing plugins from package indexes (forge_core::package):
//! M-x list-packages, package-install, package-delete,
//! package-upgrade-all, package-refresh-contents.
//!
//! Indexes come from `forge.opt.package_index` (one location or a list).
//! Local files are read at once; https:// ones, and the packages they
//! list, are downloaded with reqwest in the background. Downloads are checked
//! against the index's sha256, then the plugin loads as if installed by
//! hand (Forge asks for its permissions).

use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use forge_core::package::{self, Index, InstalledDb, Package};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const BUFFER: &str = "*Packages*";

/// What to do once the indexes are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Nothing,
    List,
    Install,
    UpgradeAll,
}

enum Fetch {
    Index { location: String, file: PathBuf },
    Package { package: Package, file: PathBuf },
}

#[derive(Default)]
pub struct Packages {
    pub indexes: Vec<String>,
    /// Where plugins are installed (None: the default plugins directory).
    pub plugin_dir: Option<PathBuf>,
    pub index: Option<Index>,
    /// Downloads under way: each on its own thread, answering here.
    downloads: Vec<(u64, Receiver<Result<(), String>>)>,
    next_download: u64,
    fetches: BTreeMap<u64, Fetch>,
    /// Indexes read so far in a refresh, and what to do after.
    refreshing: Option<(Index, Vec<String>, Then)>,
    /// Packages from version control (package_vc.rs).
    pub vc: crate::package_vc::Vc,
    /// Tree-sitter grammar installs (grammar_install.rs).
    pub grammar: crate::grammar_install::GrammarInstall,
}

impl Packages {
    pub fn busy(&self) -> bool {
        !self.downloads.is_empty() || self.vc.busy() || self.grammar.busy()
    }

    /// Something to poll for when idle (downloads, builds, a question).
    pub fn needs_poll(&self) -> bool {
        self.busy() || self.vc.has_question()
    }

    fn dir(&self) -> Option<PathBuf> {
        self.plugin_dir
            .clone()
            .or_else(forge_core::config::plugin_dir)
    }
}

fn db(app: &App) -> Option<InstalledDb> {
    app.packages.dir().map(|d| InstalledDb::load(&d))
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "list-packages" | "package-list-packages" => with_index(app, Then::List),
        "package-install" => with_index(app, Then::Install),
        "package-upgrade-all" => with_index(app, Then::UpgradeAll),
        "package-refresh-contents" => refresh(app, Then::Nothing),
        "package-delete" => {
            let names: Vec<String> = db(app)
                .map(|d| d.packages.keys().cloned().collect())
                .unwrap_or_default();
            if names.is_empty() {
                app.error("No packages installed from an index");
            } else {
                app.prompt(
                    "Delete package: ",
                    Purpose::Choose {
                        what: Choice::DeletePackage,
                        candidates: names,
                    },
                );
            }
        }
        "package-menu-install" => {
            if let Some(name) = name_on_line(app) {
                install_named(app, &name);
            }
        }
        "package-menu-delete" => {
            if let Some(name) = name_on_line(app) {
                delete(app, &name);
            }
        }
        "package-menu-refresh" => refresh(app, Then::List),
        _ if crate::grammar_install::execute(app, cmd) => {}
        _ => return crate::package_vc::execute(app, cmd),
    }
    true
}

fn with_index(app: &mut App, then: Then) {
    if app.packages.index.is_some() {
        after_refresh(app, then);
    } else {
        refresh(app, then);
    }
}

/// Read every index again, then do `then`.
pub fn refresh(app: &mut App, then: Then) {
    if app.packages.indexes.is_empty() {
        return app.error("No package index: set forge.opt.package_index in init.rhai");
    }
    if app.packages.refreshing.is_some() {
        return app.msg("Still reading the package indexes...");
    }
    let locations = app.packages.indexes.clone();
    app.packages.refreshing = Some((Index::default(), Vec::new(), then));
    let mut remote = 0;
    for loc in locations {
        match package::local_path(&loc) {
            Some(path) => {
                let r = std::fs::read_to_string(&path).map_err(|e| format!("{loc}: {e}"));
                index_read(app, &loc, r);
            }
            None => {
                let file = temp_file("index");
                match download(app, &loc, &file) {
                    Ok(id) => {
                        app.packages.fetches.insert(
                            id,
                            Fetch::Index {
                                location: loc,
                                file,
                            },
                        );
                        remote += 1;
                    }
                    Err(e) => index_read(app, &loc, Err(e)),
                }
            }
        }
    }
    if remote > 0 {
        app.msg("Reading the package indexes...");
    } else {
        finish_refresh(app);
    }
}

fn index_read(app: &mut App, location: &str, text: Result<String, String>) {
    let parsed = text.and_then(|t| Index::parse(&t, location));
    let Some((index, errors, _)) = app.packages.refreshing.as_mut() else {
        return;
    };
    match parsed {
        Ok(i) => index.merge(i),
        Err(e) => errors.push(e),
    }
}

fn finish_refresh(app: &mut App) {
    let Some((index, errors, then)) = app.packages.refreshing.take() else {
        return;
    };
    let n = index.packages.len();
    app.packages.index = Some(index);
    if !errors.is_empty() {
        app.error(errors.join("; "));
    } else if then == Then::Nothing {
        app.msg(format!(
            "{n} package{} in the index",
            if n == 1 { "" } else { "s" }
        ));
    }
    after_refresh(app, then);
}

fn after_refresh(app: &mut App, then: Then) {
    match then {
        Then::Nothing => {}
        Then::List => list(app),
        Then::Install => {
            let installed = db(app).map(|d| d.packages).unwrap_or_default();
            let names: Vec<String> = app
                .packages
                .index
                .as_ref()
                .map(|i| {
                    i.packages
                        .keys()
                        .filter(|n| !installed.contains_key(*n))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if names.is_empty() {
                app.msg("Everything in the index is installed");
            } else {
                app.prompt(
                    "Install package: ",
                    Purpose::Choose {
                        what: Choice::InstallPackage,
                        candidates: names,
                    },
                );
            }
        }
        Then::UpgradeAll => {
            let (Some(db), Some(index)) = (db(app), app.packages.index.clone()) else {
                return;
            };
            let todo: Vec<Package> = db.upgradable(&index).into_iter().cloned().collect();
            let vc = crate::package_vc::upgrade_all(app);
            if todo.is_empty() && vc == 0 {
                app.msg("All packages are up to date");
            }
            for p in todo {
                install(app, p);
            }
        }
    }
}

/// The *Packages* buffer.
pub fn list(app: &mut App) {
    let Some(index) = app.packages.index.clone() else {
        return;
    };
    let installed = db(app).map(|d| d.packages).unwrap_or_default();
    let mut text = String::from("Packages (RET install or upgrade, d delete, g refresh)\n\n");
    let width = index
        .packages
        .keys()
        .chain(installed.keys())
        .map(String::len)
        .max()
        .unwrap_or(4)
        .max(4);
    let mut names: Vec<&String> = index.packages.keys().chain(installed.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let avail = index.packages.get(name);
        let have = installed.get(name);
        let from_git = have.and_then(|h| {
            h.git.as_ref().map(|_| {
                h.commit
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(7)
                    .collect::<String>()
            })
        });
        let (version, status) = match (avail, have) {
            (_, Some(h)) if from_git.is_some() => (
                h.version.clone(),
                format!("git {}", from_git.unwrap_or_default()),
            ),
            (Some(a), None) if a.git.is_some() => (a.version.clone(), "available (git)".into()),
            (Some(a), Some(h)) if package::compare_versions(&a.version, &h.version).is_gt() => {
                (a.version.clone(), format!("upgrade from {}", h.version))
            }
            (Some(a), Some(_)) => (a.version.clone(), "installed".into()),
            (Some(a), None) => (a.version.clone(), "available".into()),
            (None, Some(h)) => (h.version.clone(), "installed (not in the index)".into()),
            (None, None) => continue,
        };
        let desc = avail.map(|a| a.description.as_str()).unwrap_or("");
        text.push_str(&format!(
            "  {name:width$}  {version:10}  {status:22}  {desc}\n"
        ));
    }
    app.show_special_buffer(BUFFER, "package-menu-mode", &text);
    app.buf().set_point(forge_core::Pos::new(2, 0));
}

fn name_on_line(app: &mut App) -> Option<String> {
    let b = app.buf_ref();
    let line = b.line(b.point().line);
    let name = line.split_whitespace().next().map(String::from);
    if name.is_none() || b.point().line < 2 {
        app.error("No package on this line");
        return None;
    }
    name
}

/// Install a plugin straight from a url (use_package's url + sha256).
pub fn install_url(app: &mut App, name: &str, url: &str, sha256: &str) {
    install(
        app,
        Package {
            name: name.to_string(),
            version: "0.0.0".to_string(),
            description: String::new(),
            url: url.to_string(),
            sha256: sha256.to_string(),
            git: None,
            rev: None,
        },
    );
}

/// RET in project-find-file style prompts: see minibuffer.rs.
pub fn install_named(app: &mut App, name: &str) {
    let Some(p) = app
        .packages
        .index
        .as_ref()
        .and_then(|i| i.packages.get(name))
        .cloned()
    else {
        return app.error(format!("No package {name} in the index"));
    };
    install(app, p);
}

fn install(app: &mut App, p: Package) {
    if let Some(git) = &p.git {
        return crate::package_vc::install(app, &p.name, git, p.rev.clone(), false);
    }
    match package::local_path(&p.url) {
        Some(path) => {
            let r = std::fs::read(&path).map_err(|e| format!("{}: {e}", p.url));
            installed(app, p, r);
        }
        None => {
            let file = temp_file(&p.name);
            match download(app, &p.url, &file) {
                Ok(id) => {
                    app.msg(format!("Downloading {} {}...", p.name, p.version));
                    app.packages
                        .fetches
                        .insert(id, Fetch::Package { package: p, file });
                }
                Err(e) => app.error(e),
            }
        }
    }
}

/// A download finished (or a local file was read): check, install, load.
fn installed(app: &mut App, p: Package, bytes: Result<Vec<u8>, String>) {
    let Some(mut db) = db(app) else {
        return app.error("No plugins directory (set HOME or FORGE_CONFIG_DIR)");
    };
    let r = bytes.and_then(|b| db.install(&p, &b));
    match r {
        Ok(path) => {
            app.msg(format!("Installed {} {}", p.name, p.version));
            crate::declare::record(app, &p.name, &path);
            crate::plugins::load(app, &path);
            if app
                .editor
                .find_buffer(BUFFER)
                .is_some_and(|id| id == app.active_id())
            {
                list(app);
            }
        }
        Err(e) => app.error(format!("Can't install {}: {e}", p.name)),
    }
}

pub fn delete(app: &mut App, name: &str) {
    let Some(mut db) = db(app) else { return };
    let from_vc = db.packages.get(name).is_some_and(|i| i.git.is_some());
    match db.delete(name) {
        Ok(()) => {
            if from_vc && let Some(src) = package::vc_dir(name) {
                let _ = std::fs::remove_dir_all(src);
            }
            if crate::plugins::loaded_names(app).iter().any(|n| n == name) {
                crate::plugins::unload(app, name);
            }
            app.msg(format!("Deleted {name}"));
            if app
                .editor
                .find_buffer(BUFFER)
                .is_some_and(|id| id == app.active_id())
                && app.packages.index.is_some()
            {
                list(app);
            }
        }
        Err(e) => app.error(e),
    }
}

fn temp_file(tag: &str) -> PathBuf {
    let dir = forge_core::config::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("downloads");
    let _ = std::fs::create_dir_all(&dir);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!("{tag}-{}-{n}", std::process::id()))
}

fn download(app: &mut App, url: &str, to: &std::path::Path) -> Result<u64, String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(format!(
            "{url}: only local files and http(s) URLs are supported"
        ));
    }
    let (tx, rx) = mpsc::channel();
    let (url_owned, to_owned) = (url.to_string(), to.to_path_buf());
    std::thread::Builder::new()
        .name("package-download".into())
        .spawn(move || {
            let _ = tx.send(http_get_to_file(&url_owned, &to_owned));
        })
        .map_err(|e| format!("can't start the download of {url}: {e}"))?;
    let id = app.packages.next_download;
    app.packages.next_download += 1;
    app.packages.downloads.push((id, rx));
    Ok(id)
}

/// GET `url` into `to` (an error for a failing status, as curl -f was).
fn http_get_to_file(url: &str, to: &std::path::Path) -> Result<(), String> {
    use std::error::Error;
    let describe = |e: &reqwest::Error| {
        let mut msg = e.to_string();
        let mut src = e.source();
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        msg
    };
    let local = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| matches!(h, "localhost" | "127.0.0.1" | "::1" | "[::1]"))
        })
        .unwrap_or(false);
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(300));
    if local {
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    if !resp.status().is_success() {
        return Err(format!(
            "download failed (the server answered {})",
            resp.status()
        ));
    }
    let mut file = std::fs::File::create(to).map_err(|e| format!("download failed ({e})"))?;
    resp.copy_to(&mut file)
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    Ok(())
}

/// Finished downloads (called when idle).
pub fn poll(app: &mut App) {
    if !app.packages.needs_poll() {
        return;
    }
    crate::package_vc::poll(app);
    crate::grammar_install::poll(app);
    let mut finished: Vec<(u64, Result<(), String>)> = Vec::new();
    app.packages
        .downloads
        .retain(|(id, rx)| match rx.try_recv() {
            Ok(r) => {
                finished.push((*id, r));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                finished.push((
                    *id,
                    Err("download failed (the download stopped unexpectedly)".to_string()),
                ));
                false
            }
        });
    {
        for (id, result) in finished {
            {
                let Some(fetch) = app.packages.fetches.remove(&id) else {
                    continue;
                };
                let failed = result.err();
                match fetch {
                    Fetch::Index { location, file } => {
                        let r = match failed {
                            Some(e) => Err(format!("{location}: {e}")),
                            None => std::fs::read_to_string(&file)
                                .map_err(|e| format!("{location}: {e}")),
                        };
                        let _ = std::fs::remove_file(&file);
                        index_read(app, &location, r);
                        if !app
                            .packages
                            .fetches
                            .values()
                            .any(|f| matches!(f, Fetch::Index { .. }))
                        {
                            finish_refresh(app);
                        }
                    }
                    Fetch::Package { package, file } => {
                        let r = match failed {
                            Some(e) => Err(e),
                            None => std::fs::read(&file).map_err(|e| e.to_string()),
                        };
                        let _ = std::fs::remove_file(&file);
                        installed(app, package, r);
                    }
                }
            }
        }
    }
}
