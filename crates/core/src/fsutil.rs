// SPDX-License-Identifier: MIT OR Apache-2.0
//! Small filesystem helpers shared across the crates.

#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically: write a hidden sibling temp file,
/// flush it to disk, then rename it over `path`. A crash or a full disk
/// leaves either the old file or the new one, never a half-written one.
///
/// The parent directory must already exist (like `std::fs::write`); the
/// callers that want a directory made call `create_dir_all` first. When
/// `path` already exists, its permissions are carried over to the new
/// file so an overwrite doesn't reset the mode. A symlink is followed, so
/// saving a buffer that visits one writes through it (a rename over the
/// link itself would replace the link with a regular file).
pub fn write_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let target = resolve_symlink(path);
    let tmp = temp_sibling(&target);
    let result = write_via_temp(&tmp, &target, bytes.as_ref());
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The file a symlink points at, or `path` itself when it isn't one.
fn resolve_symlink(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if is_link {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn write_via_temp(tmp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(tmp, meta.permissions());
    }
    std::fs::rename(tmp, path)
}

/// `.<name>.<pid>.tmp`, next to `path`: unique enough that two Forge
/// processes saving the same file don't write the same temp file.
fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "forge".to_string());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces() {
        let dir = std::env::temp_dir().join(format!("forge-fsutil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.toml");
        write_atomic(&path, "one").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        write_atomic(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != "f.toml")
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_parent_is_an_error() {
        let path =
            std::env::temp_dir().join(format!("forge-fsutil-{}/no/f.toml", std::process::id()));
        assert!(write_atomic(&path, "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("forge-fsutil-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.txt");
        let link = dir.join("link.txt");
        std::fs::write(&real, "one").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "two").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "two");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
