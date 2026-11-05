// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backup files makes them (files.el, "Backup Files").
//!
//! The first save of a file in a session first copies the old file to a
//! backup. Which name it gets depends on `version_control`:
//!
//! - `"nil"` (the default): numbered backups (`file.~1~`, `file.~2~`, ...)
//!   if the file has some already, otherwise a single `file~`;
//! - `"t"`: always numbered;
//! - `"never"`: always a single `file~`.
//!
//! Of the numbered backups, the `kept_old_versions` oldest (2) and the
//! `kept_new_versions` newest (2) are kept; the ones in between are
//! "excess" and are deleted at once when `delete_old_versions` is true, or
//! after asking when it isn't.
//!
//! `backup_directory` puts every backup in one directory, named after the
//! whole path with `/` turned into `!` (`backup-directory-alist`
//! with a single `("." . DIR)` entry). With no-littering on
//! (`forge.opt.no_littering`), backups go to `var/backup/` in the state
//! directory instead, except for files in temporary directories.
//!
//! Which files get one follows Forge too: none at all with
//! `make_backup_files` false; none for files under version control (the
//! repository is their backup: `vc-make-backup-files` is nil, and
//! the buffer is marked `backup-inhibited`); and none for files in the
//! temporary directory.
//!
//! Backups are made by copying, so the original file keeps its inode: a
//! symbolic link stays a link to the same target, hard links stay linked,
//! and the file keeps its owner and permissions (the backup gets the same
//! permissions as the original).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VersionControl {
    /// nil: numbered if numbered backups exist already.
    #[default]
    Existing,
    /// t.
    Always,
    /// `never`.
    Never,
}

impl VersionControl {
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s {
            "nil" | "existing" => Ok(Self::Existing),
            "t" | "numbered" | "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "version_control is \"t\", \"nil\" or \"never\", not {s:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub version_control: VersionControl,
    pub kept_old_versions: usize,
    pub kept_new_versions: usize,
    /// Some(true): delete excess backups silently; Some(false) never;
    /// None asks.
    pub delete_old_versions: Option<bool>,
    /// All backups go here, if set.
    pub directory: Option<PathBuf>,
    /// Put backups in no-littering's `var/backup/` unless `directory`
    /// says otherwise.
    pub no_littering: bool,
    /// make-backup-files.
    pub make_backup_files: bool,
    /// Files under these directories get no backups.
    pub temporary_dirs: Vec<PathBuf>,
}

/// The directories whose files aren't backed up by default: $TMPDIR (or
/// /tmp) and, on Unix, /dev/shm.
pub fn default_temporary_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    let v = vec![std::env::temp_dir(), PathBuf::from("/dev/shm")];
    #[cfg(not(unix))]
    let v = vec![std::env::temp_dir()];
    v.into_iter()
        .map(|d| d.canonicalize().unwrap_or(d))
        .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version_control: VersionControl::Existing,
            kept_old_versions: 2,
            kept_new_versions: 2,
            delete_old_versions: None,
            directory: None,
            no_littering: false,
            make_backup_files: true,
            temporary_dirs: default_temporary_dirs(),
        }
    }
}

fn config_cell() -> &'static Mutex<Config> {
    static C: OnceLock<Mutex<Config>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn config() -> Config {
    config_cell().lock().map(|c| c.clone()).unwrap_or_default()
}

pub fn set_config(c: Config) {
    if let Ok(mut cur) = config_cell().lock() {
        *cur = c;
    }
}

/// Where `file`'s backups live, and the name they start with: next to it
/// as `name`, or in the backup directory as `!dir!name`.
fn backup_base(file: &Path, cfg: &Config) -> Option<PathBuf> {
    let name = file.file_name()?;
    match &cfg.directory {
        None => {
            if cfg.no_littering
                && let Some(base) = crate::no_littering::backup_path(file, &cfg.temporary_dirs)
            {
                return Some(base);
            }
            Some(file.with_file_name(name))
        }
        Some(dir) => Some(crate::no_littering::mangled_path(dir, file)),
    }
}

fn with_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The numbered backups of `file` there are, oldest first: (number, path).
pub fn numbered_backups(file: &Path) -> Vec<(u32, PathBuf)> {
    numbered_backups_in(file, &config())
}

fn numbered_backups_in(file: &Path, cfg: &Config) -> Vec<(u32, PathBuf)> {
    let Some(base) = backup_base(file, cfg) else {
        return Vec::new();
    };
    let (Some(dir), Some(stem)) = (base.parent(), base.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}.~", stem.to_string_lossy());
    let mut out: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let num = n.strip_prefix(&prefix)?.strip_suffix('~')?.parse().ok()?;
            Some((num, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// What making a backup did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Made {
    pub backup: Option<PathBuf>,
    /// Numbered backups beyond what's kept, when they weren't deleted
    /// (the caller asks; see [`delete_excess`]).
    pub excess: Vec<PathBuf>,
}

/// The name the next backup of `file` gets.
pub fn next_backup_name(file: &Path) -> Option<PathBuf> {
    let cfg = config();
    next_backup_name_in(file, &cfg)
}

fn next_backup_name_in(file: &Path, cfg: &Config) -> Option<PathBuf> {
    let base = backup_base(file, cfg)?;
    let numbered = numbered_backups_in(file, cfg);
    let use_numbers = match cfg.version_control {
        VersionControl::Always => true,
        VersionControl::Never => false,
        VersionControl::Existing => !numbered.is_empty(),
    };
    Some(if use_numbers {
        let next = numbered.last().map_or(1, |(n, _)| n + 1);
        with_suffix(&base, &format!(".~{next}~"))
    } else {
        with_suffix(&base, "~")
    })
}

/// Should `file` be backed up at all?
pub fn backup_enabled(file: &Path) -> bool {
    backup_enabled_in(file, &config())
}

fn backup_enabled_in(file: &Path, cfg: &Config) -> bool {
    if !cfg.make_backup_files {
        return false;
    }
    let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    !cfg.temporary_dirs.iter().any(|d| file.starts_with(d))
}

/// Back `file` up before it is overwritten (if backups are enabled for it).
pub fn make_backup(file: &Path) -> io::Result<Made> {
    let cfg = config();
    if !file.is_file() || !backup_enabled_in(file, &cfg) {
        return Ok(Made::default());
    }
    let Some(target) = next_backup_name_in(file, &cfg) else {
        return Ok(Made::default());
    };
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Copying keeps the original where it is: links stay links, and the
    // file keeps its owner. fs::copy gives the copy the same permissions.
    std::fs::copy(file, &target)?;
    let mut made = Made {
        backup: Some(target),
        excess: Vec::new(),
    };
    let numbered = numbered_backups_in(file, &cfg);
    let keep = cfg.kept_old_versions + cfg.kept_new_versions;
    if numbered.len() > keep {
        let excess: Vec<PathBuf> = numbered
            [cfg.kept_old_versions..numbered.len() - cfg.kept_new_versions]
            .iter()
            .map(|(_, p)| p.clone())
            .collect();
        match cfg.delete_old_versions {
            Some(true) => delete_excess(&excess),
            Some(false) => {}
            None => made.excess = excess,
        }
    }
    Ok(made)
}

pub fn delete_excess(paths: &[PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

/// The newest backup of `file` there is: the highest numbered one, else
/// `file~` (for diff-backup).
pub fn latest_backup(file: &Path) -> Option<PathBuf> {
    let cfg = config();
    if let Some((_, p)) = numbered_backups_in(file, &cfg).pop() {
        return Some(p);
    }
    let plain = with_suffix(&backup_base(file, &cfg)?, "~");
    plain.exists().then_some(plain)
}

/// Is `path` a backup file name (`x~` or `x.~N~`)? The file it backs up.
pub fn file_of_backup(path: &Path) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    let stripped = s.strip_suffix('~')?;
    if let Some(i) = stripped.rfind(".~")
        && stripped[i + 2..].bytes().all(|b| b.is_ascii_digit())
        && i + 2 < stripped.len()
    {
        return Some(PathBuf::from(&stripped[..i]));
    }
    Some(PathBuf::from(stripped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn numbered_backups_keep_the_oldest_and_newest() {
        let d = dir("numbered");
        let f = d.join("f.txt");
        let cfg = Config {
            version_control: VersionControl::Always,
            ..Default::default()
        };
        for i in 1..=4 {
            std::fs::write(&f, format!("v{i}")).unwrap();
            let t = next_backup_name_in(&f, &cfg).unwrap();
            assert_eq!(t, d.join(format!("f.txt.~{i}~")));
            std::fs::copy(&f, &t).unwrap();
        }
        let n = numbered_backups_in(&f, &cfg);
        assert_eq!(n.len(), 4);
        assert_eq!(n[0].0, 1);
        // Existing: numbered because there are some.
        let existing = Config::default();
        assert_eq!(
            next_backup_name_in(&f, &existing).unwrap(),
            d.join("f.txt.~5~")
        );
        let never = Config {
            version_control: VersionControl::Never,
            ..Default::default()
        };
        assert_eq!(next_backup_name_in(&f, &never).unwrap(), d.join("f.txt~"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_central_directory_mangles_the_path() {
        let d = dir("central");
        let cfg = Config {
            directory: Some(d.join("backups")),
            ..Default::default()
        };
        let t = next_backup_name_in(Path::new("/home/me/a.txt"), &cfg).unwrap();
        assert_eq!(t, d.join("backups").join("!home!me!a.txt~"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn no_littering_leaves_temporary_files_and_explicit_directories_alone() {
        let d = dir("nolitter");
        let temp_file =
            std::env::temp_dir().join(format!("forge-nolitter-backup-{}", std::process::id()));
        let cfg = Config {
            no_littering: true,
            ..Default::default()
        };
        assert_eq!(
            next_backup_name_in(&temp_file, &cfg).unwrap(),
            with_suffix(&temp_file, "~")
        );
        let central = Config {
            no_littering: true,
            directory: Some(d.join("backups")),
            ..Default::default()
        };
        assert_eq!(
            next_backup_name_in(Path::new("/home/me/a.txt"), &central).unwrap(),
            d.join("backups").join("!home!me!a.txt~")
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn temporary_files_and_make_backup_files() {
        let cfg = Config {
            temporary_dirs: vec![PathBuf::from("/scratch")],
            ..Default::default()
        };
        assert!(!backup_enabled_in(Path::new("/scratch/x.txt"), &cfg));
        assert!(backup_enabled_in(Path::new("/home/me/x.txt"), &cfg));
        let off = Config {
            make_backup_files: false,
            ..Default::default()
        };
        assert!(!backup_enabled_in(Path::new("/home/me/x.txt"), &off));
        assert!(
            default_temporary_dirs().contains(&std::env::temp_dir().canonicalize().unwrap()),
            "$TMPDIR is temporary"
        );
    }

    #[test]
    fn backup_names_lead_back_to_their_file() {
        assert_eq!(
            file_of_backup(Path::new("/a/b.txt~")),
            Some("/a/b.txt".into())
        );
        assert_eq!(
            file_of_backup(Path::new("/a/b.txt.~12~")),
            Some("/a/b.txt".into())
        );
        assert_eq!(file_of_backup(Path::new("/a/b.txt")), None);
    }
}
