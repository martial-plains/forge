// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Mercurial backend: the core of the interface,
//! plus push, pull, merge, branches and tags.

use super::backend::*;
use std::path::Path;

pub struct Hg;

const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

impl Hg {
    fn hg(&self, root: &Path, args: &[&str]) -> Cmd {
        // Plain output whatever the user's hgrc says (HGPLAIN is in
        // runner::ENV, which every command gets).
        Cmd::new(&self.program(), root, &["--config", "ui.interactive=false"])
            .args(args.iter().map(|s| s.to_string()))
    }
}

fn state_of_code(c: char) -> Option<FileState> {
    Some(match c {
        'M' => FileState::Edited,
        'A' => FileState::Added,
        'R' => FileState::Removed,
        '!' => FileState::Missing,
        '?' => FileState::Unregistered,
        'I' => FileState::Ignored,
        'C' => FileState::UpToDate,
        _ => return None,
    })
}

/// Parse `hg status -0` (`X path\0` records).
pub fn parse_status(out: &str) -> Vec<StatusEntry> {
    out.split('\0')
        .filter(|r| r.len() > 2)
        .filter_map(|r| {
            let state = state_of_code(r.chars().next()?)?;
            Some(StatusEntry {
                path: r[2..].to_string(),
                state,
                extra: None,
            })
        })
        .collect()
}

fn log_template() -> String {
    format!(
        "{RS}{{node}}{FS}{{node|short}}{FS}{{author|person}}{FS}{{author|email}}{FS}{{date|isodate}}{FS}{{join(tags, ', ')}}{FS}{{join(parents, ' ')}}{FS}{{desc}}\u{1d}"
    )
}

impl Backend for Hg {
    fn kind(&self) -> Kind {
        Kind::Hg
    }

    fn caps(&self) -> Caps {
        Caps {
            push: true,
            pull: true,
            fetch: true,
            merge: true,
            branches: true,
            tags: true,
            stash: false,
            region_history: false,
            index: false,
            lock_based: false,
            ignore: true,
            amend: true,
        }
    }

    fn program(&self) -> String {
        super::config().hg_program
    }

    fn file_state(&self, root: &Path, file: &str) -> Result<FileState, String> {
        if let Ok(r) = self.hg(root, &["resolve", "-l", "--", file]).ok()
            && r.lines().any(|l| l.starts_with("U "))
        {
            return Ok(FileState::Conflict);
        }
        let out = self.hg(root, &["status", "-A", "-0", "--", file]).ok()?;
        Ok(parse_status(&out)
            .into_iter()
            .next()
            .map(|e| e.state)
            .unwrap_or(FileState::Unregistered))
    }

    fn dir_status_cmd(&self, root: &Path, dir: &str, ignored: bool) -> Cmd {
        let mut c = self.hg(root, &["status", "-0", "-mardu"]);
        if ignored {
            c = c.arg("-i");
        }
        c.arg("--").arg(if dir.is_empty() { "." } else { dir })
    }

    fn parse_dir_status(&self, root: &Path, out: &str) -> (Vec<StatusEntry>, RepoInfo) {
        let mut entries = parse_status(out);
        if let Ok(r) = self.hg(root, &["resolve", "-l"]).ok() {
            for l in r.lines() {
                if let Some(f) = l.strip_prefix("U ") {
                    match entries.iter_mut().find(|e| e.path == f) {
                        Some(e) => e.state = FileState::Conflict,
                        None => entries.push(StatusEntry {
                            path: f.to_string(),
                            state: FileState::Conflict,
                            extra: None,
                        }),
                    }
                }
            }
        }
        let info = RepoInfo {
            branch: self.current_branch(root),
            head: self
                .hg(root, &["log", "-r", ".", "-T", "{node|short}"])
                .ok()
                .ok(),
            upstream: self
                .hg(root, &["paths", "default"])
                .ok()
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            ..Default::default()
        };
        (entries, info)
    }

    fn dir_status(
        &self,
        root: &Path,
        dir: &str,
        ignored: bool,
    ) -> Result<(Vec<StatusEntry>, RepoInfo), String> {
        let out = self.dir_status_cmd(root, dir, ignored).ok()?;
        Ok(self.parse_dir_status(root, &out))
    }

    fn mode_line_rev(&self, root: &Path) -> Option<String> {
        self.current_branch(root)
    }

    fn working_revision(&self, root: &Path) -> Option<String> {
        self.hg(root, &["log", "-r", ".", "-T", "{node}"])
            .ok()
            .ok()
            .filter(|s| !s.is_empty() && !s.starts_with("0000000"))
    }

    fn list_tracked_files(&self, root: &Path) -> Result<Vec<String>, String> {
        let out = self.hg(root, &["files", "-0"]).ok()?;
        Ok(out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn register(&self, root: &Path, files: &[String]) -> Cmd {
        self.hg(root, &["add", "--"]).args(files.iter().cloned())
    }

    fn unregister(&self, root: &Path, files: &[String]) -> Cmd {
        self.hg(root, &["forget", "--"]).args(files.iter().cloned())
    }

    fn revert(&self, root: &Path, files: &[(String, FileState)]) -> Vec<Cmd> {
        vec![
            self.hg(root, &["revert", "--no-backup", "--"])
                .args(files.iter().map(|(f, _)| f.clone())),
        ]
    }

    fn checkin(&self, root: &Path, files: &[String], message: &str, opts: &CheckinOpts) -> Cmd {
        let mut c = self.hg(root, &["commit", "-l", "-"]);
        if opts.amend {
            c = c.arg("--amend");
        }
        if let Some(a) = &opts.author {
            c = c.arg("-u").arg(a.clone());
        }
        if let Some(d) = &opts.date {
            c = c.arg("-d").arg(d.clone());
        }
        if !files.is_empty() {
            c = c.arg("--").args(files.iter().cloned());
        }
        c.stdin(message.to_string())
    }

    fn last_message(&self, root: &Path) -> Option<String> {
        self.hg(root, &["log", "-r", ".", "-T", "{desc}"]).ok().ok()
    }

    fn find_revision(&self, root: &Path, file: &str, rev: &str) -> Result<String, String> {
        self.hg(root, &["cat", "-r", rev, "--", file]).ok()
    }

    fn revision_completions(&self, root: &Path, file: Option<&str>) -> Vec<String> {
        let mut c = self.hg(root, &["log", "-l", "100", "-T", "{node|short}\n"]);
        if let Some(f) = file {
            c = c.arg("--").arg(f);
        }
        let mut out = vec![".".to_string(), "tip".to_string()];
        out.extend(self.branches(root, false));
        out.extend(self.tags(root));
        if let Ok(s) = c.ok() {
            out.extend(s.lines().map(str::to_string));
        }
        out.dedup();
        out
    }

    fn delete_file(&self, root: &Path, file: &str) -> Cmd {
        self.hg(root, &["remove", "-f", "--", file])
    }

    fn rename_file(&self, root: &Path, from: &str, to: &str) -> Cmd {
        self.hg(root, &["rename", "--", from, to])
    }

    fn ignore(&self, root: &Path, pattern: &str) -> Result<std::path::PathBuf, String> {
        use std::io::Write;
        let path = root.join(".hgignore");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let line = format!("^{}$", regex::escape(pattern.trim_start_matches('/')));
        if existing.lines().any(|l| l.trim() == line) {
            return Ok(path);
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let sep = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        writeln!(f, "{sep}{line}").map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn diff(
        &self,
        root: &Path,
        files: &[String],
        rev1: Option<&str>,
        rev2: Option<&str>,
        ignore_ws: bool,
    ) -> Cmd {
        let mut c = self.hg(root, &["diff", "--git", "--nodates"]);
        if ignore_ws {
            c = c.arg("-w");
        }
        if let Some(r) = rev1 {
            c = c.arg("-r").arg(r);
        }
        if let Some(r) = rev2 {
            c = c.arg("-r").arg(r);
        }
        c.arg("--").args(files.iter().cloned())
    }

    fn log(&self, root: &Path, files: &[String], limit: usize, start: Option<&str>) -> Cmd {
        let mut c = self.hg(root, &["log", "-T"]).arg(log_template());
        if limit > 0 {
            c = c.arg("-l").arg(limit.to_string());
        }
        if files.len() == 1 {
            c = c.arg("-f");
        }
        if let Some(s) = start {
            c = c.arg("-r").arg(format!("reverse(::{s})"));
        }
        c.arg("--").args(files.iter().cloned())
    }

    fn parse_log(&self, out: &str) -> Vec<LogEntry> {
        super::git::parse_log_records(out)
            .into_iter()
            .map(|mut e| {
                // Mercurial's null parent means "none".
                e.parents.retain(|p| !p.chars().all(|c| c == '0'));
                e
            })
            .collect()
    }

    fn incoming(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        let mut c = self.hg(root, &["incoming", "-T"]).arg(log_template());
        if let Some(r) = remote {
            c = c.arg(r);
        }
        Ok(c)
    }

    fn outgoing(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        let mut c = self.hg(root, &["outgoing", "-T"]).arg(log_template());
        if let Some(r) = remote {
            c = c.arg(r);
        }
        Ok(c)
    }

    fn annotate(&self, root: &Path, file: &str, rev: Option<&str>) -> Cmd {
        let mut c = self
            .hg(root, &["annotate", "-T"])
            .arg(format!(
                "{{lines % \"{{node}}{FS}{{user}}{FS}{{date|hgdate}}{FS}{{lineno}}{FS}{{path}}{FS}{{line}}\"}}"
            ));
        if let Some(r) = rev {
            c = c.arg("-r").arg(r);
        }
        c.arg("--").arg(file)
    }

    fn parse_annotate(&self, out: &str) -> Vec<AnnotatedLine> {
        out.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.splitn(6, FS).collect();
                if f.len() < 6 {
                    return None;
                }
                let time: i64 = f[2].split_whitespace().next()?.parse().ok()?;
                Some(AnnotatedLine {
                    rev: f[0].to_string(),
                    author: f[1].to_string(),
                    time,
                    date: super::git::format_date(time, "+0000"),
                    file: f[4].to_string(),
                    previous: None,
                    summary: String::new(),
                    orig_line: f[3].parse().unwrap_or(0),
                    text: f[5].to_string(),
                })
            })
            .collect()
    }

    fn previous_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String> {
        let revset = match file {
            Some(f) => format!("last(::p1({rev}) and file('path:{f}'))"),
            None => format!("p1({rev})"),
        };
        self.hg(root, &["log", "-r", &revset, "-T", "{node}"])
            .ok()
            .ok()
            .filter(|s| !s.is_empty())
    }

    fn next_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String> {
        let revset = match file {
            Some(f) => format!("first(({rev}::.) - {rev} and file('path:{f}'))"),
            None => format!("first(children({rev}))"),
        };
        self.hg(root, &["log", "-r", &revset, "-T", "{node}"])
            .ok()
            .ok()
            .filter(|s| !s.is_empty())
    }

    fn mergebase(&self, root: &Path, a: &str, b: &str) -> Option<String> {
        self.hg(
            root,
            &["log", "-r", &format!("ancestor({a}, {b})"), "-T", "{node}"],
        )
        .ok()
        .ok()
    }

    fn pull(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.hg(root, &["pull", "-u"]))
    }

    fn push(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.hg(root, &["push"]))
    }

    fn fetch(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.hg(root, &["pull"]))
    }

    fn remotes(&self, root: &Path) -> Vec<String> {
        self.hg(root, &["paths", "-q"])
            .ok()
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn has_upstream(&self, root: &Path) -> bool {
        !self.remotes(root).is_empty()
    }

    fn merge(&self, root: &Path, rev: &str) -> Result<Cmd, String> {
        Ok(self.hg(root, &["merge", "--tool", "internal:merge", rev]))
    }

    fn branches(&self, root: &Path, _remote: bool) -> Vec<String> {
        self.hg(root, &["branches", "-T", "{branch}\n"])
            .ok()
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn current_branch(&self, root: &Path) -> Option<String> {
        // .hg/branch says, without a process; no file means "default".
        Some(
            std::fs::read_to_string(root.join(".hg/branch"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "default".into()),
        )
    }

    fn create_branch(&self, root: &Path, name: &str, _start: Option<&str>) -> Result<Cmd, String> {
        Ok(self.hg(root, &["branch", name]))
    }

    fn switch_branch(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.hg(root, &["update", name]))
    }

    fn tags(&self, root: &Path) -> Vec<String> {
        self.hg(root, &["tags", "-q"])
            .ok()
            .map(|s| {
                s.lines()
                    .map(str::to_string)
                    .filter(|t| t != "tip")
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_tag(&self, root: &Path, name: &str, rev: Option<&str>) -> Result<Cmd, String> {
        let mut c = self.hg(root, &["tag"]);
        if let Some(r) = rev {
            c = c.arg("-r").arg(r);
        }
        Ok(c.arg(name))
    }

    fn retrieve_tag(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.hg(root, &["update", name]))
    }

    fn mark_resolved(&self, root: &Path, files: &[String]) -> Cmd {
        self.hg(root, &["resolve", "-m", "--"])
            .args(files.iter().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_records() {
        let e = parse_status("M a\0A new file\0? unk\0! gone\0R old\0");
        let s: Vec<(&str, FileState)> = e
            .iter()
            .map(|e| (e.path.as_str(), e.state.clone()))
            .collect();
        assert_eq!(
            s,
            vec![
                ("a", FileState::Edited),
                ("new file", FileState::Added),
                ("unk", FileState::Unregistered),
                ("gone", FileState::Missing),
                ("old", FileState::Removed),
            ]
        );
    }
}
