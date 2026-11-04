// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Git backend.

use super::backend::*;
use std::path::{Path, PathBuf};

pub struct Git;

/// Git's empty tree: what a repository with no commits diffs against.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Field and record separators for `git log --format`.
const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

fn cfg() -> Config {
    super::config()
}

impl Git {
    fn git(&self, root: &Path, args: &[&str]) -> Cmd {
        Cmd::new(&self.program(), root, args)
    }

    fn has_head(&self, root: &Path) -> bool {
        self.git(root, &["rev-parse", "--verify", "-q", "HEAD"])
            .ok()
            .is_ok()
    }

    /// The git directory (`.git`, or where a worktree's `.git` file points).
    fn git_dir(root: &Path) -> Option<PathBuf> {
        let dot = root.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        let p = PathBuf::from(target);
        Some(if p.is_absolute() { p } else { root.join(p) })
    }

    /// `HEAD`, read from the file (no process for the mode line): the
    /// branch, or None when detached.
    fn head_branch(root: &Path) -> Option<Result<String, String>> {
        let head = std::fs::read_to_string(Self::git_dir(root)?.join("HEAD")).ok()?;
        let head = head.trim();
        Some(match head.strip_prefix("ref: refs/heads/") {
            Some(b) => Ok(b.to_string()),
            None => Err(head.chars().take(7).collect()),
        })
    }
}

/// `XY` from porcelain v2 to a state, with what else to say.
fn state_of_xy(xy: &str) -> (FileState, Option<String>) {
    let mut c = xy.chars();
    let (x, y) = (c.next().unwrap_or('.'), c.next().unwrap_or('.'));
    let staged = || (x != '.' && y == '.').then(|| "staged".to_string());
    match (x, y) {
        ('A', 'D') => (FileState::Missing, None),
        ('A', _) => (FileState::Added, None),
        ('D', _) => (FileState::Removed, None),
        (_, 'D') => (FileState::Missing, None),
        _ => (FileState::Edited, staged()),
    }
}

/// Parse `git status --porcelain=v2 -z --branch --show-stash`.
pub fn parse_status_v2(out: &str) -> (Vec<StatusEntry>, RepoInfo) {
    let mut entries = Vec::new();
    let mut info = RepoInfo::default();
    let mut fields = out.split('\0').peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" if val != "(initial)" => {
                    info.head = Some(val.chars().take(7).collect())
                }
                "branch.head" if val != "(detached)" => info.branch = Some(val.to_string()),
                "branch.upstream" => info.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split_whitespace() {
                        if let Some(a) = part.strip_prefix('+') {
                            info.ahead = a.parse().ok();
                        } else if let Some(b) = part.strip_prefix('-') {
                            info.behind = b.parse().ok();
                        }
                    }
                }
                "stash" => info.stashes = val.trim().parse().unwrap_or(0),
                _ => {}
            }
            continue;
        }
        let kind = rec.chars().next().unwrap_or(' ');
        match kind {
            '1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = rec.splitn(9, ' ').collect();
                if parts.len() == 9 {
                    let (state, extra) = state_of_xy(parts[1]);
                    entries.push(StatusEntry {
                        path: parts[8].to_string(),
                        state,
                        extra,
                    });
                }
            }
            '2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let parts: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = fields.next().unwrap_or("");
                if parts.len() == 10 {
                    let what = if parts[8].starts_with('C') {
                        "copied"
                    } else {
                        "renamed"
                    };
                    entries.push(StatusEntry {
                        path: parts[9].to_string(),
                        state: FileState::Added,
                        extra: Some(format!("{what} from {orig}")),
                    });
                }
            }
            'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let parts: Vec<&str> = rec.splitn(11, ' ').collect();
                if parts.len() == 11 {
                    entries.push(StatusEntry {
                        path: parts[10].to_string(),
                        state: FileState::Conflict,
                        extra: Some(conflict_kind(parts[1]).into()),
                    });
                }
            }
            '?' => entries.push(StatusEntry {
                path: rec[2..].to_string(),
                state: FileState::Unregistered,
                extra: None,
            }),
            '!' => entries.push(StatusEntry {
                path: rec[2..].to_string(),
                state: FileState::Ignored,
                extra: None,
            }),
            _ => {}
        }
    }
    (entries, info)
}

fn conflict_kind(xy: &str) -> &'static str {
    match xy {
        "DD" => "both deleted",
        "AU" => "added by us",
        "UD" => "deleted by them",
        "UA" => "added by them",
        "DU" => "deleted by us",
        "AA" => "both added",
        _ => "both modified",
    }
}

/// Parse the log format [`Backend::log`] asks for. A record that carries
/// a patch (`git log -L`) keeps it in `diff`.
pub fn parse_log_records(out: &str) -> Vec<LogEntry> {
    out.split(RS)
        .filter(|r| !r.trim().is_empty())
        .filter_map(|rec| {
            let f: Vec<&str> = rec.splitn(8, FS).collect();
            if f.len() < 8 {
                return None;
            }
            // The message ends at the end marker; a patch may follow it.
            let (message, diff) = match f[7].split_once("\u{1d}") {
                Some((m, d)) => (m, Some(d)),
                None => (f[7], None),
            };
            let diff = diff
                .map(|d| d.trim_start_matches('\n').to_string())
                .filter(|d| !d.trim().is_empty());
            Some(LogEntry {
                rev: f[0].trim().to_string(),
                short: f[1].to_string(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                date: f[4].to_string(),
                refs: f[5]
                    .split(", ")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                parents: f[6].split_whitespace().map(str::to_string).collect(),
                message: message.trim_end().to_string(),
                diff,
            })
        })
        .collect()
}

/// The `--format` that [`parse_log_records`] reads.
pub fn log_format() -> String {
    format!("--format={RS}%H{FS}%h{FS}%an{FS}%ae{FS}%ad{FS}%D{FS}%P{FS}%B%x1d")
}

/// Parse `git blame --porcelain`.
pub fn parse_blame_porcelain(out: &str) -> Vec<AnnotatedLine> {
    use std::collections::HashMap;
    #[derive(Default, Clone)]
    struct Commit {
        author: String,
        time: i64,
        tz: String,
        summary: String,
        file: String,
        previous: Option<(String, String)>,
    }
    let mut commits: HashMap<String, Commit> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<(String, usize)> = None;
    for line in out.lines() {
        if let Some(text) = line.strip_prefix('\t') {
            let Some((rev, orig)) = current.take() else {
                continue;
            };
            let c = commits.get(&rev).cloned().unwrap_or_default();
            lines.push(AnnotatedLine {
                date: format_date(c.time, &c.tz),
                rev,
                author: c.author,
                time: c.time,
                file: c.file,
                previous: c.previous,
                summary: c.summary,
                orig_line: orig,
                text: text.to_string(),
            });
            continue;
        }
        let mut words = line.splitn(2, ' ');
        let first = words.next().unwrap_or("");
        let rest = words.next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let orig = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            commits.entry(first.to_string()).or_default();
            current = Some((first.to_string(), orig));
            continue;
        }
        let Some((rev, _)) = &current else { continue };
        let c = commits.entry(rev.clone()).or_default();
        match first {
            "author" => c.author = rest.to_string(),
            "author-time" => c.time = rest.parse().unwrap_or(0),
            "author-tz" => c.tz = rest.to_string(),
            "summary" => c.summary = rest.to_string(),
            "filename" => c.file = rest.to_string(),
            "previous" => {
                if let Some((r, f)) = rest.split_once(' ') {
                    c.previous = Some((r.to_string(), f.to_string()));
                }
            }
            _ => {}
        }
    }
    lines
}

/// `YYYY-MM-DD` for seconds since the epoch, in the given `+hhmm` zone.
pub fn format_date(secs: i64, tz: &str) -> String {
    let offset = parse_tz(tz);
    let days = (secs + offset).div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_tz(tz: &str) -> i64 {
    let (sign, digits) = match tz.as_bytes().first() {
        Some(b'-') => (-1, &tz[1..]),
        Some(b'+') => (1, &tz[1..]),
        _ => return 0,
    };
    let n: i64 = digits.parse().unwrap_or(0);
    sign * ((n / 100) * 3600 + (n % 100) * 60)
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl Backend for Git {
    fn kind(&self) -> Kind {
        Kind::Git
    }

    fn caps(&self) -> Caps {
        Caps {
            push: true,
            pull: true,
            fetch: true,
            merge: true,
            branches: true,
            tags: true,
            stash: true,
            region_history: true,
            index: true,
            lock_based: false,
            ignore: true,
            amend: true,
        }
    }

    fn program(&self) -> String {
        cfg().git_program
    }

    fn file_state(&self, root: &Path, file: &str) -> Result<FileState, String> {
        let out = self
            .git(
                root,
                &[
                    "status",
                    "--porcelain=v2",
                    "-z",
                    "--ignored=matching",
                    "--untracked-files=all",
                    "--",
                    file,
                ],
            )
            .ok()?;
        let (entries, _) = parse_status_v2(&out);
        if let Some(e) = entries.into_iter().find(|e| e.path == file) {
            return Ok(e.state);
        }
        // Nothing to report: tracked and unchanged, or not there at all.
        let tracked = self.git(root, &["ls-files", "-z", "--", file]).ok()?;
        Ok(if tracked.is_empty() {
            FileState::Unregistered
        } else {
            FileState::UpToDate
        })
    }

    fn dir_status_cmd(&self, root: &Path, dir: &str, ignored: bool) -> Cmd {
        let mut c = self.git(
            root,
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--show-stash",
                "--untracked-files=all",
            ],
        );
        if ignored {
            c = c.arg("--ignored=matching");
        }
        c = c.arg("--");
        if dir.is_empty() {
            c.arg(".")
        } else {
            c.arg(dir)
        }
    }

    fn parse_dir_status(&self, root: &Path, out: &str) -> (Vec<StatusEntry>, RepoInfo) {
        let (entries, mut info) = parse_status_v2(out);
        info.operation = Self::git_dir(root).and_then(|g| {
            [
                ("MERGE_HEAD", "merge"),
                ("REBASE_HEAD", "rebase"),
                ("rebase-merge", "rebase"),
                ("rebase-apply", "rebase"),
                ("CHERRY_PICK_HEAD", "cherry-pick"),
                ("REVERT_HEAD", "revert"),
                ("BISECT_LOG", "bisect"),
            ]
            .iter()
            .find(|(f, _)| g.join(f).exists())
            .map(|(_, what)| what.to_string())
        });
        info.remote_url = self
            .git(root, &["config", "--get", "remote.origin.url"])
            .ok()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
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
        match Self::head_branch(root)? {
            Ok(b) => Some(b),
            Err(hash) if hash.len() == 7 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
                Some(hash)
            }
            Err(_) => self
                .git(root, &["rev-parse", "--short", "HEAD"])
                .ok()
                .ok()
                .map(|s| s.trim().to_string()),
        }
    }

    fn working_revision(&self, root: &Path) -> Option<String> {
        self.git(root, &["rev-parse", "HEAD"])
            .ok()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn list_tracked_files(&self, root: &Path) -> Result<Vec<String>, String> {
        let out = self.git(root, &["ls-files", "-z"]).ok()?;
        Ok(out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn register(&self, root: &Path, files: &[String]) -> Cmd {
        self.git(root, &["add", "--"]).args(files.iter().cloned())
    }

    fn unregister(&self, root: &Path, files: &[String]) -> Cmd {
        self.git(root, &["rm", "-q", "--cached", "--"])
            .args(files.iter().cloned())
    }

    fn revert(&self, root: &Path, files: &[(String, FileState)]) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        let (added, rest): (Vec<_>, Vec<_>) =
            files.iter().partition(|(_, s)| *s == FileState::Added);
        if !added.is_empty() {
            // Reverting an addition unregisters it; the file stays.
            cmds.push(
                self.git(root, &["rm", "-q", "--cached", "--"])
                    .args(added.iter().map(|(f, _)| f.clone())),
            );
        }
        if !rest.is_empty() {
            let names: Vec<String> = rest.iter().map(|(f, _)| f.clone()).collect();
            if self.has_head(root) {
                cmds.push(
                    self.git(root, &["checkout", "-q", "HEAD", "--"])
                        .args(names),
                );
            } else {
                cmds.push(self.git(root, &["checkout", "-q", "--"]).args(names));
            }
        }
        cmds
    }

    fn checkin(&self, root: &Path, files: &[String], message: &str, opts: &CheckinOpts) -> Cmd {
        let mut c = self.git(root, &["commit", "-F", "-", "--cleanup=strip"]);
        if opts.amend {
            c = c.arg("--amend");
        }
        if let Some(a) = &opts.author {
            c = c.arg(format!("--author={a}"));
        }
        if let Some(d) = &opts.date {
            c = c.arg(format!("--date={d}"));
        }
        if opts.signoff {
            c = c.arg("--signoff");
        }
        if opts.no_verify {
            c = c.arg("--no-verify");
        }
        // Only these files, whatever else is staged. Amending with no files changes only
        // the message.
        if !files.is_empty() || opts.amend {
            c = c.arg("--only");
        }
        if !files.is_empty() {
            c = c.arg("--").args(files.iter().cloned());
        }
        c.stdin(message.to_string())
    }

    fn last_message(&self, root: &Path) -> Option<String> {
        self.git(root, &["log", "-1", "--format=%B"])
            .ok()
            .ok()
            .map(|s| s.trim_end().to_string())
    }

    fn find_revision(&self, root: &Path, file: &str, rev: &str) -> Result<String, String> {
        self.git(root, &["show", &format!("{rev}:{file}")]).ok()
    }

    fn revision_completions(&self, root: &Path, file: Option<&str>) -> Vec<String> {
        let mut c = self.git(root, &["log", "-n", "100", "--format=%h %s"]);
        if let Some(f) = file {
            c = c.arg("--").arg(f);
        }
        let mut out: Vec<String> = vec!["HEAD".into()];
        out.extend(self.branches(root, true));
        out.extend(self.tags(root));
        if let Ok(log) = c.ok() {
            out.extend(log.lines().map(|l| {
                l.split_once(' ')
                    .map(|(h, _)| h.to_string())
                    .unwrap_or_else(|| l.to_string())
            }));
        }
        out.dedup();
        out
    }

    fn delete_file(&self, root: &Path, file: &str) -> Cmd {
        self.git(root, &["rm", "-q", "-f", "--", file])
    }

    fn rename_file(&self, root: &Path, from: &str, to: &str) -> Cmd {
        self.git(root, &["mv", "--", from, to])
    }

    fn ignore(&self, root: &Path, pattern: &str) -> Result<PathBuf, String> {
        use std::io::Write;
        let path = root.join(".gitignore");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing.lines().any(|l| l.trim() == pattern) {
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
        writeln!(f, "{sep}{pattern}").map_err(|e| e.to_string())?;
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
        let mut c = self.git(
            root,
            &["diff", "--no-color", "--no-ext-diff", "--no-prefix", "-M"],
        );
        c = c.args(cfg().git_diff_switches);
        if ignore_ws {
            c = c.arg("-w");
        }
        let head = if self.has_head(root) {
            "HEAD"
        } else {
            EMPTY_TREE
        };
        c = c.arg(rev1.unwrap_or(head));
        if let Some(r2) = rev2 {
            c = c.arg(r2);
        }
        c.arg("--").args(files.iter().cloned())
    }

    fn log(&self, root: &Path, files: &[String], limit: usize, start: Option<&str>) -> Cmd {
        let mut c = self.git(
            root,
            &[
                "log",
                "--no-color",
                "--date=format:%Y-%m-%d %H:%M",
                "--decorate=short",
            ],
        );
        c = c.arg(log_format());
        c = c.args(cfg().git_log_switches);
        if limit > 0 {
            c = c.arg(format!("-n{limit}"));
        }
        if files.len() == 1 && !root.join(&files[0]).is_dir() {
            c = c.arg("--follow");
        }
        if let Some(s) = start {
            c = c.arg(s);
        }
        c.arg("--").args(files.iter().cloned())
    }

    fn parse_log(&self, out: &str) -> Vec<LogEntry> {
        parse_log_records(out)
    }

    fn region_history(
        &self,
        root: &Path,
        file: &str,
        from: usize,
        to: usize,
    ) -> Result<Cmd, String> {
        Ok(self
            .git(
                root,
                &[
                    "log",
                    "--no-color",
                    "--date=format:%Y-%m-%d %H:%M",
                    "--decorate=short",
                ],
            )
            .arg(log_format())
            .arg(format!("-L{from},{to}:{file}")))
    }

    fn incoming(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        let up = remote
            .map(str::to_string)
            .unwrap_or_else(|| "@{upstream}".into());
        Ok(self
            .git(
                root,
                &[
                    "log",
                    "--no-color",
                    "--date=format:%Y-%m-%d %H:%M",
                    "--decorate=short",
                ],
            )
            .arg(log_format())
            .arg(format!("HEAD..{up}")))
    }

    fn outgoing(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        let up = remote
            .map(str::to_string)
            .unwrap_or_else(|| "@{upstream}".into());
        Ok(self
            .git(
                root,
                &[
                    "log",
                    "--no-color",
                    "--date=format:%Y-%m-%d %H:%M",
                    "--decorate=short",
                ],
            )
            .arg(log_format())
            .arg(format!("{up}..HEAD")))
    }

    fn annotate(&self, root: &Path, file: &str, rev: Option<&str>) -> Cmd {
        let mut c = self.git(root, &["blame", "--porcelain"]);
        c = c.args(cfg().git_annotate_switches);
        if let Some(r) = rev {
            c = c.arg(r);
        }
        c.arg("--").arg(file)
    }

    fn parse_annotate(&self, out: &str) -> Vec<AnnotatedLine> {
        parse_blame_porcelain(out)
    }

    fn previous_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String> {
        let mut c = self.git(root, &["rev-list", "-n", "2", rev]);
        if let Some(f) = file {
            c = c.arg("--").arg(f);
        }
        let out = c.ok().ok()?;
        out.lines().nth(1).map(str::to_string)
    }

    fn next_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String> {
        // The commit whose parent is `rev`, among those that touch the
        // file, on the way from rev to HEAD.
        let mut c = self.git(root, &["rev-list", "--reverse", &format!("{rev}..HEAD")]);
        if let Some(f) = file {
            c = c.arg("--").arg(f);
        }
        let out = c.ok().ok()?;
        out.lines().next().map(str::to_string)
    }

    fn mergebase(&self, root: &Path, a: &str, b: &str) -> Option<String> {
        self.git(root, &["merge-base", a, b])
            .ok()
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn pull(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.git(root, &["pull", "--stat"]))
    }

    fn push(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.git(root, &["push"]))
    }

    fn fetch(&self, root: &Path) -> Result<Cmd, String> {
        Ok(self.git(root, &["fetch", "--prune"]))
    }

    fn has_upstream(&self, root: &Path) -> bool {
        self.git(
            root,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .ok()
        .is_ok()
    }

    fn remotes(&self, root: &Path) -> Vec<String> {
        self.git(root, &["remote"])
            .ok()
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn merge(&self, root: &Path, rev: &str) -> Result<Cmd, String> {
        Ok(self.git(root, &["merge", "--no-edit", rev]))
    }

    fn branches(&self, root: &Path, remote: bool) -> Vec<String> {
        let mut c = self.git(root, &["branch", "--format=%(refname:short)"]);
        if remote {
            c = c.arg("--all");
        }
        c.ok()
            .map(|s| {
                s.lines()
                    .map(str::to_string)
                    .filter(|b| !b.ends_with("/HEAD") && !b.starts_with('('))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn current_branch(&self, root: &Path) -> Option<String> {
        Self::head_branch(root)?.ok()
    }

    fn create_branch(&self, root: &Path, name: &str, start: Option<&str>) -> Result<Cmd, String> {
        let mut c = self.git(root, &["checkout", "-b", name]);
        if let Some(s) = start {
            c = c.arg(s);
        }
        Ok(c)
    }

    fn switch_branch(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.git(root, &["checkout", name]))
    }

    fn rename_branch(&self, root: &Path, from: &str, to: &str) -> Result<Cmd, String> {
        Ok(self.git(root, &["branch", "-m", from, to]))
    }

    fn delete_branch(&self, root: &Path, name: &str, force: bool) -> Result<Cmd, String> {
        Ok(self.git(root, &["branch", if force { "-D" } else { "-d" }, name]))
    }

    fn tags(&self, root: &Path) -> Vec<String> {
        self.git(root, &["tag", "--sort=-creatordate"])
            .ok()
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn create_tag(&self, root: &Path, name: &str, rev: Option<&str>) -> Result<Cmd, String> {
        let mut c = self.git(root, &["tag", name]);
        if let Some(r) = rev {
            c = c.arg(r);
        }
        Ok(c)
    }

    fn retrieve_tag(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.git(root, &["checkout", name]))
    }

    fn stashes(&self, root: &Path) -> Vec<(String, String)> {
        self.git(root, &["stash", "list", "--format=%gd%x1f%cr: %gs"])
            .ok()
            .map(|s| {
                s.lines()
                    .filter_map(|l| l.split_once(FS))
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn stash(&self, root: &Path, message: &str) -> Result<Cmd, String> {
        let mut c = self.git(root, &["stash", "push"]);
        if !message.is_empty() {
            c = c.arg("-m").arg(message);
        }
        Ok(c)
    }

    fn stash_apply(&self, root: &Path, name: &str, pop: bool) -> Result<Cmd, String> {
        Ok(self.git(
            root,
            &["stash", if pop { "pop" } else { "apply" }, "-q", name],
        ))
    }

    fn stash_drop(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.git(root, &["stash", "drop", "-q", name]))
    }

    fn stash_show(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Ok(self.git(
            root,
            &["stash", "show", "-p", "--no-color", "--no-prefix", name],
        ))
    }

    fn stage(&self, root: &Path, files: &[String]) -> Result<Cmd, String> {
        Ok(self.git(root, &["add", "--"]).args(files.iter().cloned()))
    }

    fn unstage(&self, root: &Path, files: &[String]) -> Result<Cmd, String> {
        if self.has_head(root) {
            Ok(self
                .git(root, &["reset", "-q", "HEAD", "--"])
                .args(files.iter().cloned()))
        } else {
            Ok(self
                .git(root, &["rm", "-q", "--cached", "--"])
                .args(files.iter().cloned()))
        }
    }

    fn mark_resolved(&self, root: &Path, files: &[String]) -> Cmd {
        self.git(root, &["add", "--"]).args(files.iter().cloned())
    }

    fn conflict_versions(&self, root: &Path, file: &str) -> Result<ConflictVersions, String> {
        let stage = |n: u8| self.git(root, &["show", &format!(":{n}:{file}")]).ok();
        Ok(ConflictVersions {
            base: stage(1).ok(),
            mine: stage(2)?,
            theirs: stage(3)?,
        })
    }

    fn grep_command(&self, root: &Path, regexp: &str, files: &str) -> Option<String> {
        let q = forge_core::process::shell_quote;
        let mut line = format!(
            "{} --no-pager grep -n -I --no-color -E -e {}",
            q(&self.program()),
            q(regexp)
        );
        let files = files.trim();
        if !files.is_empty() && files != "*" {
            line.push_str(" --");
            for f in files.split_whitespace() {
                line.push(' ');
                line.push_str(&q(f));
            }
        }
        let _ = root;
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_status() {
        let out = "# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0# stash 3\0\
1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 M. N... 100644 100644 100644 aaa bbb staged.rs\0\
1 A. N... 000000 100644 100644 000 bbb new.rs\0\
1 D. N... 100644 000000 000000 aaa 000 gone.rs\0\
1 .D N... 100644 100644 000000 aaa aaa missing.rs\0\
2 R. N... 100644 100644 100644 aaa aaa R100 to name.rs\0from.rs\0\
u UU N... 100644 100644 100644 100644 a b c both.rs\0\
? notes with space.txt\0\
! target/\0";
        let (e, info) = parse_status_v2(out);
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.head.as_deref(), Some("1234567"));
        assert_eq!(info.upstream.as_deref(), Some("origin/main"));
        assert_eq!((info.ahead, info.behind), (Some(2), Some(1)));
        assert_eq!(info.stashes, 3);
        let s: Vec<(&str, FileState)> = e
            .iter()
            .map(|e| (e.path.as_str(), e.state.clone()))
            .collect();
        assert_eq!(
            s,
            vec![
                ("src/a.rs", FileState::Edited),
                ("staged.rs", FileState::Edited),
                ("new.rs", FileState::Added),
                ("gone.rs", FileState::Removed),
                ("missing.rs", FileState::Missing),
                ("to name.rs", FileState::Added),
                ("both.rs", FileState::Conflict),
                ("notes with space.txt", FileState::Unregistered),
                ("target/", FileState::Ignored),
            ]
        );
        assert_eq!(e[1].extra.as_deref(), Some("staged"));
        assert_eq!(e[5].extra.as_deref(), Some("renamed from from.rs"));
        assert_eq!(e[6].extra.as_deref(), Some("both modified"));
    }

    #[test]
    fn log_records() {
        let out = format!(
            "{RS}abc123{FS}abc{FS}Ann{FS}ann@x{FS}2024-01-02 03:04{FS}HEAD -> main, tag: v1{FS}p1 p2{FS}Subject line\n\nBody text\n\u{1d}\n{RS}def{FS}de{FS}Bob{FS}b@x{FS}2023-12-31 00:00{FS}{FS}{FS}Only subject\n\u{1d}\ndiff --git a b\n@@ -1 +1 @@\n-x\n+y\n"
        );
        let e = parse_log_records(&out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].subject(), "Subject line");
        assert_eq!(e[0].message, "Subject line\n\nBody text");
        assert_eq!(e[0].refs, vec!["HEAD -> main", "tag: v1"]);
        assert_eq!(e[0].parents, vec!["p1", "p2"]);
        assert_eq!(e[0].diff, None);
        assert!(e[1].parents.is_empty());
        assert!(e[1].diff.as_deref().unwrap().starts_with("diff --git"));
    }

    #[test]
    fn blame_porcelain() {
        let h1 = "a".repeat(40);
        let h2 = "b".repeat(40);
        let out = format!(
            "{h1} 1 1 2\nauthor Ann\nauthor-mail <a@x>\nauthor-time 86400\nauthor-tz +0000\nsummary First\nfilename f.txt\n\tline one\n{h1} 2 2\n\tline two\n{h2} 3 3 1\nauthor Bob\nauthor-time 0\nauthor-tz -0100\nsummary Second\nprevious {h1} old.txt\nfilename f.txt\n\tline three\n"
        );
        let l = parse_blame_porcelain(&out);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].author, "Ann");
        assert_eq!(l[0].date, "1970-01-02");
        assert_eq!(l[1].rev, h1);
        assert_eq!(l[1].text, "line two");
        assert_eq!(l[1].orig_line, 2);
        assert_eq!(l[2].author, "Bob");
        assert_eq!(l[2].date, "1969-12-31", "the author's time zone");
        assert_eq!(l[2].previous, Some((h1.clone(), "old.txt".into())));
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(format_date(1_700_000_000, "+0000"), "2023-11-14");
    }
}
