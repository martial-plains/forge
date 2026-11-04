// SPDX-License-Identifier: MIT OR Apache-2.0
//! Projects: the directories bindings follow, the way `.dir-locals.el`
//! does.
//!
//! A project is the nearest directory above a file with a `.forge/`
//! directory in it. A `.git` is deliberately not a marker: a repository is
//! the version-control layer's business, not a project. Its keymap
//! ([`Scope::Project`](crate::keymap::Scope::Project),
//! `forge_core::keymap`) applies to every file inside it, so a key can be
//! bound for one checkout without touching the rest.
//!
//! A project can also carry its own `init.rhai` (`.forge/init.rhai`), and
//! that is arbitrary code from a directory the user may have just cloned.
//! So it only runs once the user has granted it, in its own section of the
//! grant file plugins use ([`crate::permissions::ProjectGrants`]).
//! Everything else here is reading: which directory a file is in, and
//! whether the marker is there.

use std::path::{Path, PathBuf};

/// Forge's marker directory, and where a project's own `init.rhai` is.
pub const DIR: &str = ".forge";

/// The project a file (or a directory) is in: the nearest directory above
/// it holding `.forge/`. None for a file with no `.forge/`
/// anywhere above it — an ordinary file in a home directory has no
/// project, and shouldn't pick up bindings from one.
pub fn root(start: &Path) -> Option<PathBuf> {
    let dir = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    // Canonicalized, so the root a project is registered under and the one
    // a file's path walks up to are the same directory: /tmp and
    // /private/tmp are one place on macOS, and a symlinked checkout
    // shouldn't look like two projects.
    let dir = dir.canonicalize().ok()?;
    dir.ancestors().find(|d| is_root(d)).map(Path::to_path_buf)
}

/// Is `dir` the top of a project?
pub fn is_root(dir: &Path) -> bool {
    dir.join(DIR).is_dir()
}

/// The project's own `init.rhai`, if it has one.
pub fn init_file(root: &Path) -> Option<PathBuf> {
    let path = root.join(DIR).join("init.rhai");
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-project-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_marker_above_a_file_makes_a_project() {
        let d = scratch("root");
        let deep = d.join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("a.rs"), "").unwrap();
        assert_eq!(root(&deep.join("a.rs")), None, "no marker yet");
        fs::create_dir_all(d.join(".git")).unwrap();
        assert_eq!(root(&deep.join("a.rs")), None, ".git is not a marker");
        fs::remove_dir_all(d.join(".git")).unwrap();
        fs::create_dir_all(d.join(DIR)).unwrap();
        assert_eq!(root(&deep.join("a.rs")), Some(d.canonicalize().unwrap()));
        assert_eq!(
            root(&deep),
            Some(d.canonicalize().unwrap()),
            "a directory too"
        );
        assert_eq!(init_file(&d), None, "no init.rhai yet");
        fs::write(d.join(DIR).join("init.rhai"), "").unwrap();
        assert_eq!(init_file(&d), Some(d.join(DIR).join("init.rhai")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_nearest_marker_wins() {
        let outer = scratch("outer");
        let inner = outer.join("sub/project");
        fs::create_dir_all(inner.join(DIR)).unwrap();
        fs::create_dir_all(outer.join(DIR)).unwrap();
        let file = inner.join("a.rs");
        fs::write(&file, "").unwrap();
        assert_eq!(
            root(&file),
            Some(inner.canonicalize().unwrap()),
            "the innermost project, not the one outside it"
        );
        let _ = fs::remove_dir_all(&outer);
    }
}
