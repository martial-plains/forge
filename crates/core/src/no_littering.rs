// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keep auto-save and backup files out of the directories you visit,
//! like the `no-littering` package.
//!
//! When `forge.opt.no_littering = true`, auto-saves go to `auto-save/`
//! and backups to `backup/` inside `var/` in Forge's state directory
//! (`$FORGE_STATE_DIR`, else `$XDG_STATE_HOME/forge`, else
//! `~/.local/state/forge`). Each name carries the visited file's whole
//! absolute path, with `/` turned into `!`, so files with the same name
//! in different directories don't collide.
//!
//! As in `no-littering-theme-backups`, files in temporary directories are
//! left alone, so their auto-saves stay next to them (backups still follow
//! Forge's usual temporary-file rule). Those directories are usually on a
//! `tmpfs`, and the files vanish at reboot.
//!
//! Lock files are not themed: they are transient and format is
//! shared with other editors.

use std::path::{Path, PathBuf};

/// `var/` in the state directory: persistent data from visited files.
pub fn var_dir() -> Option<PathBuf> {
    crate::config::state_dir()
        .or_else(crate::config::config_dir)
        .map(|d| d.join("var"))
}

pub fn auto_save_dir() -> Option<PathBuf> {
    var_dir().map(|d| d.join("auto-save"))
}

pub fn backup_dir() -> Option<PathBuf> {
    var_dir().map(|d| d.join("backup"))
}

fn absolute(file: &Path) -> PathBuf {
    if file.is_absolute() {
        file.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(file)
    }
}

/// The visited file's whole path with `/` turned into `!` (and `!`
/// doubled), the name backups in a central directory already use.
pub(crate) fn mangled_path(dir: &Path, file: &Path) -> PathBuf {
    let abs = absolute(file);
    let mangled = abs.to_string_lossy().replace('!', "!!").replace('/', "!");
    dir.join(mangled)
}

pub fn is_temporary(file: &Path, temporary_dirs: &[PathBuf]) -> bool {
    let abs = absolute(file);
    let canonical = file.canonicalize().ok().or_else(|| {
        let parent = file.parent()?.canonicalize().ok()?;
        Some(parent.join(file.file_name()?))
    });
    temporary_dirs.iter().any(|d| {
        let real = d.canonicalize().unwrap_or_else(|_| d.clone());
        abs.starts_with(d)
            || abs.starts_with(&real)
            || canonical
                .as_ref()
                .is_some_and(|f| f.starts_with(d) || f.starts_with(&real))
    })
}

fn auto_save_path_in(file: &Path, temporary_dirs: &[PathBuf], dir: &Path) -> Option<PathBuf> {
    if is_temporary(file, temporary_dirs) {
        return None;
    }
    let base = mangled_path(dir, file);
    let name = base.file_name()?.to_string_lossy();
    Some(dir.join(format!("#{name}#")))
}

fn backup_path_in(file: &Path, temporary_dirs: &[PathBuf], dir: &Path) -> Option<PathBuf> {
    if is_temporary(file, temporary_dirs) {
        return None;
    }
    Some(mangled_path(dir, file))
}

/// Where `file`'s auto-save goes when no-littering covers it; `None`
/// means next to the file (temporary file, or no state directory).
pub fn auto_save_path(file: &Path, temporary_dirs: &[PathBuf]) -> Option<PathBuf> {
    auto_save_path_in(file, temporary_dirs, &auto_save_dir()?)
}

/// The base name of `file`'s backups when no-littering covers it, in
/// `var/backup/`; `None` means next to the file.
pub fn backup_path(file: &Path, temporary_dirs: &[PathBuf]) -> Option<PathBuf> {
    backup_path_in(file, temporary_dirs, &backup_dir()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-nolitter-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn central_names_keep_the_whole_path() {
        let central = dir("names");
        let temps = vec![std::env::temp_dir().canonicalize().unwrap()];
        let a = auto_save_path_in(Path::new("/home/ada/a.txt"), &temps, &central).unwrap();
        let b = auto_save_path_in(Path::new("/home/bob/a.txt"), &temps, &central).unwrap();
        assert_eq!(a, central.join("#!home!ada!a.txt#"));
        assert_eq!(b, central.join("#!home!bob!a.txt#"));
        assert_ne!(a, b);
        assert_eq!(
            backup_path_in(Path::new("/home/ada/a.txt"), &temps, &central).unwrap(),
            central.join("!home!ada!a.txt")
        );
        std::fs::remove_dir_all(central).unwrap();
    }

    #[test]
    fn temporary_files_stay_where_they_are() {
        let central = dir("temp");
        let temps = vec![std::env::temp_dir().canonicalize().unwrap()];
        let file = std::env::temp_dir().join(format!("forge-nolitter-file-{}", std::process::id()));
        assert_eq!(auto_save_path_in(&file, &temps, &central), None);
        assert_eq!(backup_path_in(&file, &temps, &central), None);
        let _ = std::fs::remove_file(file);
        std::fs::remove_dir_all(central).unwrap();
    }

    #[test]
    fn exclamation_marks_are_escaped() {
        let central = dir("bang");
        let temps = Vec::new();
        assert_eq!(
            backup_path_in(Path::new("/home/a!b/c.txt"), &temps, &central).unwrap(),
            central.join("!home!a!!b!c.txt")
        );
        std::fs::remove_dir_all(central).unwrap();
    }
}
