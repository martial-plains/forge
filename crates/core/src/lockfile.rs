// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forge-lock.toml`: what the plugins `init.rhai` declared resolved to.
//!
//! `use_package` says *which* plugins you use; the lock file says the
//! exact bytes each one resolved to (a git commit and the component's
//! sha256), so a second machine that copies `init.rhai` and `forge-lock.toml`
//! gets the same plugins. It sits next to `init.rhai` (the config
//! directory) and is meant to be kept with it.
//!
//! Unlike `packages.toml` (a local record in the plugins directory of
//! everything installed), the lock file is only about *declared* plugins,
//! and it also remembers the permissions and settings their manifests
//! asked for, so `M-x package-vc-upgrade` can show what changed before
//! applying it.

use crate::permissions::{Grants, Setting};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE: &str = "forge-lock.toml";

/// One declared plugin, resolved.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LockEntry {
    /// The git repository it came from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The tag, branch or commit that was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The URL it was downloaded from, if it didn't come from git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The commit it was built from (git sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The sha256 of the installed component.
    pub sha256: String,
    /// What its manifest asked for when it was locked (for the upgrade
    /// review: a changed permission set always asks again).
    #[serde(default, skip_serializing_if = "Grants::is_empty")]
    pub permissions: Grants,
    /// What its manifest says it provides (for lazy loading and
    /// `M-x describe-plugin`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Setting>,
}

/// The on-disk shape of `forge-lock.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct LockData {
    #[serde(default)]
    packages: BTreeMap<String, LockEntry>,
}

/// `forge-lock.toml`, keyed by plugin name.
#[derive(Debug, Clone, Default)]
pub struct LockFile {
    pub packages: BTreeMap<String, LockEntry>,
    dir: Option<PathBuf>,
}

impl LockFile {
    /// An in-memory lock file with no place to save (tests).
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// Read the lock file from `dir`; a missing file means nothing is
    /// locked yet.
    pub fn load(dir: &Path) -> Self {
        let data: LockData = std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            packages: data.packages,
            dir: Some(dir.to_path_buf()),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn get(&self, name: &str) -> Option<&LockEntry> {
        self.packages.get(name)
    }

    pub fn set(&mut self, name: &str, entry: LockEntry) {
        self.packages.insert(name.to_string(), entry);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.packages.remove(name).is_some()
    }

    /// Write the lock file (atomic, like the other Forge books).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let data = LockData {
            packages: self.packages.clone(),
        };
        let text = toml::to_string_pretty(&data).map_err(std::io::Error::other)?;
        let body = format!(
            "# Plugins declared with use_package in init.rhai, and the\n\
             # commit and sha256 each resolved to. Keep this next to init.rhai.\n\n{text}"
        );
        let path = Self::path(dir);
        crate::fsutil::write_atomic(&path, body)
    }

    /// The entry for a plugin if the installed bytes match it (its sha256
    /// is what was locked): the fast path at startup, so a declared plugin
    /// that is already installed needn't be fetched again.
    pub fn satisfied(&self, name: &str, bytes: &[u8]) -> bool {
        self.packages
            .get(name)
            .is_some_and(|e| e.sha256 == crate::package::sha256_hex(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::SettingKind;

    #[test]
    fn round_trips_through_toml() {
        let dir = std::env::temp_dir().join(format!("forge-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut lock = LockFile::load(&dir);
        assert!(lock.packages.is_empty());
        lock.set(
            "org-modern",
            LockEntry {
                git: Some("https://github.com/you/forge-org-modern".into()),
                rev: Some("v0.2.0".into()),
                commit: Some("abc123".into()),
                sha256: "deadbeef".into(),
                permissions: Grants {
                    network: true,
                    ..Default::default()
                },
                commands: vec!["org-modern-mode".into()],
                settings: vec![Setting {
                    name: "progress".into(),
                    kind: SettingKind::Boolean,
                    default: Some(toml::Value::Boolean(false)),
                    doc: "show progress".into(),
                }],
                ..Default::default()
            },
        );
        lock.save().unwrap();
        let text = std::fs::read_to_string(LockFile::path(&dir)).unwrap();
        assert!(text.contains("[packages.org-modern]"), "{text}");
        assert!(text.contains("commit = \"abc123\""), "{text}");
        let again = LockFile::load(&dir);
        let e = again.get("org-modern").unwrap();
        assert_eq!(e.commit.as_deref(), Some("abc123"));
        assert_eq!(e.sha256, "deadbeef");
        assert_eq!(e.settings.len(), 1);
        // satisfied() checks the bytes.
        assert!(!again.satisfied("org-modern", b"x"));
        assert_eq!(again.dir(), Some(dir.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
