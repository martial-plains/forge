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
pub const LOCAL_FILE: &str = "forge-lock.local.toml";
const LOCK_VERSION: u32 = 1;
const SNAPSHOTS_DIR: &str = "package-snapshots";
const ARTIFACTS_DIR: &str = "package-artifacts";

/// One declared plugin, resolved.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
    /// SHA-256 of the installed component (explicit alias for new lockfiles).
    #[serde(
        rename = "artifact_sha256",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_sha256: Option<String>,
    /// SHA-256 of the declaration's static, non-closure fields.
    #[serde(rename = "spec_hash", default, skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<String>,
    /// `prebuilt` or `source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Hash of the source build command, toolchain, features, and target.
    #[serde(
        rename = "recipe_hash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recipe_hash: Option<String>,
    /// Package version when known; used to restore `packages.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Accepted source build command, when one was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
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
    /// Permissions and component imports captured at lock time.
    #[serde(default, skip_serializing_if = "LockCapabilities::is_empty")]
    pub capabilities: LockCapabilities,
    /// Fields introduced by later Forge versions survive a read/write cycle.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LockCapabilities {
    #[serde(default, skip_serializing_if = "Grants::is_empty")]
    pub permissions: Grants,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl LockCapabilities {
    fn is_empty(&self) -> bool {
        self.permissions.is_empty() && self.imports.is_empty() && self.extra.is_empty()
    }
}

/// The on-disk shape of `forge-lock.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LockData {
    #[serde(rename = "lock_version", default = "lock_version")]
    lock_version: u32,
    #[serde(default)]
    packages: BTreeMap<String, LockEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

fn lock_version() -> u32 {
    LOCK_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Snapshot {
    #[serde(rename = "lock_version", default = "lock_version")]
    pub lock_version: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, LockEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotChange {
    pub name: String,
    pub change: String,
    pub before: Option<LockEntry>,
    pub after: Option<LockEntry>,
}

/// `forge-lock.toml`, keyed by plugin name.
#[derive(Debug, Clone, Default)]
pub struct LockFile {
    /// Effective entries (shared file overlaid by machine-local entries).
    pub packages: BTreeMap<String, LockEntry>,
    shared_packages: BTreeMap<String, LockEntry>,
    local_packages: BTreeMap<String, LockEntry>,
    lock_version: u32,
    extra: BTreeMap<String, toml::Value>,
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

    pub fn local_path(dir: &Path) -> PathBuf {
        dir.join(LOCAL_FILE)
    }

    /// Read the lock file from `dir`; a missing file means nothing is
    /// locked yet. Machine-local entries override shared entries by name.
    pub fn load(dir: &Path) -> Self {
        let data = std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|s| toml::from_str::<LockData>(&s).ok())
            .unwrap_or_default();
        let local = std::fs::read_to_string(Self::local_path(dir))
            .ok()
            .and_then(|s| toml::from_str::<LockData>(&s).ok())
            .unwrap_or_default();
        let mut packages = data.packages.clone();
        packages.extend(local.packages.clone());
        Self {
            packages,
            shared_packages: data.packages,
            local_packages: local.packages,
            lock_version: data.lock_version.max(LOCK_VERSION),
            extra: data.extra,
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
        self.shared_packages.insert(name.to_string(), entry.clone());
        self.packages.insert(
            name.to_string(),
            self.local_packages.get(name).cloned().unwrap_or(entry),
        );
    }

    pub fn remove(&mut self, name: &str) -> bool {
        if let Some(local) = self.local_packages.get(name) {
            self.packages.insert(name.to_string(), local.clone());
        } else {
            self.packages.remove(name);
        }
        self.shared_packages.remove(name).is_some()
    }

    pub fn replace_shared(&mut self, packages: BTreeMap<String, LockEntry>) {
        self.shared_packages = packages;
        self.packages = self.shared_packages.clone();
        self.packages.extend(self.local_packages.clone());
    }

    pub fn shared_packages(&self) -> &BTreeMap<String, LockEntry> {
        &self.shared_packages
    }

    /// Write the lock file (atomic, like the other Forge books).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let data = LockData {
            lock_version: self.lock_version.max(LOCK_VERSION),
            packages: self.shared_packages.clone(),
            extra: self.extra.clone(),
        };
        let text = toml::to_string_pretty(&data).map_err(std::io::Error::other)?;
        let body = format!(
            "# Plugins declared with use_package in init.rhai, and the\n\
             # commit and sha256 each resolved to. Keep this next to init.rhai.\n\n{text}"
        );
        let path = Self::path(dir);
        crate::fsutil::write_atomic(&path, body)
    }

    pub fn save_artifact(dir: &Path, hash: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        if crate::package::sha256_hex(bytes) != hash {
            return Err(std::io::Error::other(format!(
                "artifact hash mismatch: expected {hash}"
            )));
        }
        let path = Self::artifact_path(dir, hash)?;
        let Some(parent) = path.parent() else {
            return Err(std::io::Error::other("artifact path has no parent"));
        };
        std::fs::create_dir_all(parent)?;
        if path.exists() {
            let existing = std::fs::read(&path)?;
            if crate::package::sha256_hex(&existing) == hash {
                return Ok(path);
            }
        }
        crate::fsutil::write_atomic(&path, bytes)?;
        Ok(path)
    }

    pub fn artifact(dir: &Path, hash: &str) -> std::io::Result<Vec<u8>> {
        let path = Self::artifact_path(dir, hash)?;
        let bytes = std::fs::read(&path)?;
        if crate::package::sha256_hex(&bytes) != hash {
            return Err(std::io::Error::other(format!(
                "saved artifact {} failed its SHA-256 check",
                path.display()
            )));
        }
        Ok(bytes)
    }

    pub fn save_snapshot(&self, name: &str, plugins_dir: &Path) -> std::io::Result<()> {
        Self::validate_snapshot_name(name)?;
        let dir = self
            .dir
            .as_deref()
            .ok_or_else(|| std::io::Error::other("no config directory for snapshots"))?;
        for (plugin, entry) in &self.shared_packages {
            Self::validate_plugin_name(plugin)?;
            let path = plugins_dir.join(format!("{plugin}.wasm"));
            let bytes = std::fs::read(&path)?;
            let expected = Self::locked_hash(entry);
            Self::save_artifact(dir, expected, &bytes)?;
        }
        let snapshot = Snapshot {
            lock_version: self.lock_version.max(LOCK_VERSION),
            packages: self.shared_packages.clone(),
            extra: self.extra.clone(),
        };
        let text = toml::to_string_pretty(&snapshot).map_err(std::io::Error::other)?;
        let path = Self::snapshot_path(dir, name)?;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("snapshot path has no parent"))?;
        std::fs::create_dir_all(parent)?;
        crate::fsutil::write_atomic(&path, text)
    }

    pub fn snapshots(dir: &Path) -> std::io::Result<Vec<String>> {
        let snapshots = dir.join(SNAPSHOTS_DIR);
        let Ok(entries) = std::fs::read_dir(snapshots) else {
            return Ok(Vec::new());
        };
        let mut names = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .filter_map(|path| path.file_stem()?.to_str().map(str::to_string))
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    pub fn read_snapshot(dir: &Path, name: &str) -> std::io::Result<Snapshot> {
        let path = Self::snapshot_path(dir, name)?;
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(std::io::Error::other)
    }

    pub fn diff_snapshots(
        dir: &Path,
        before: &str,
        after: &str,
    ) -> std::io::Result<Vec<SnapshotChange>> {
        let before = Self::read_snapshot(dir, before)?;
        let after = Self::read_snapshot(dir, after)?;
        Ok(Self::diff(&before.packages, &after.packages))
    }

    /// Install artifacts in the plugins directory and atomically update the
    /// shared lockfile. Repeating after an interruption converges to the same
    /// bytes.
    pub fn restore_snapshot(
        dir: &Path,
        plugins_dir: &Path,
        name: &str,
    ) -> std::io::Result<Snapshot> {
        let snapshot = Self::read_snapshot(dir, name)?;
        let mut lock = Self::load(dir);
        lock.replace_shared(snapshot.packages.clone());
        let mut artifacts = Vec::with_capacity(lock.packages.len());
        for (plugin, entry) in &lock.packages {
            Self::validate_plugin_name(plugin)?;
            let bytes = Self::artifact(dir, Self::locked_hash(entry))?;
            artifacts.push((plugin, entry, bytes));
        }
        std::fs::create_dir_all(plugins_dir)?;
        for (plugin, _, bytes) in artifacts {
            crate::fsutil::write_atomic(&plugins_dir.join(format!("{plugin}.wasm")), bytes)?;
        }
        lock.lock_version = snapshot.lock_version.max(LOCK_VERSION);
        lock.extra = snapshot.extra.clone();
        lock.save()?;
        Ok(snapshot)
    }

    /// The entry for a plugin if the installed bytes match it (its sha256
    /// is what was locked): the fast path at startup, so a declared plugin
    /// that is already installed needn't be fetched again.
    pub fn satisfied(&self, name: &str, bytes: &[u8]) -> bool {
        self.packages
            .get(name)
            .is_some_and(|e| e.sha256 == crate::package::sha256_hex(bytes))
    }

    fn locked_hash(entry: &LockEntry) -> &str {
        entry
            .artifact_sha256
            .as_deref()
            .unwrap_or(entry.sha256.as_str())
    }

    fn artifact_path(dir: &Path, hash: &str) -> std::io::Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(std::io::Error::other("invalid artifact SHA-256"));
        }
        Ok(dir.join(ARTIFACTS_DIR).join(format!("{hash}.wasm")))
    }

    fn snapshot_path(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
        Self::validate_snapshot_name(name)?;
        Ok(dir.join(SNAPSHOTS_DIR).join(format!("{name}.toml")))
    }

    fn validate_snapshot_name(name: &str) -> std::io::Result<()> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(std::io::Error::other(
                "snapshot name must contain only letters, numbers, '.', '_' or '-'",
            ));
        }
        Ok(())
    }

    fn validate_plugin_name(name: &str) -> std::io::Result<()> {
        if name.is_empty()
            || name.starts_with('.')
            || name.contains(['/', '\\'])
            || name.bytes().any(|b| b.is_ascii_control())
        {
            return Err(std::io::Error::other(format!(
                "invalid plugin name in lockfile: {name:?}"
            )));
        }
        Ok(())
    }

    fn diff(
        before: &BTreeMap<String, LockEntry>,
        after: &BTreeMap<String, LockEntry>,
    ) -> Vec<SnapshotChange> {
        before
            .keys()
            .chain(after.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|name| {
                let old = before.get(name);
                let new = after.get(name);
                (old != new).then(|| SnapshotChange {
                    name: (*name).clone(),
                    change: match (old, new) {
                        (None, Some(_)) => "added".into(),
                        (Some(_), None) => "removed".into(),
                        _ => "changed".into(),
                    },
                    before: old.cloned(),
                    after: new.cloned(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::SettingKind;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("forge-lock-{tag}-{}", std::process::id()))
    }

    #[test]
    fn round_trips_through_toml() {
        let dir = temp_dir("round-trip");
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

    #[test]
    fn legacy_locks_load_and_unknown_fields_survive_rewrite() {
        let dir = temp_dir("unknown");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            LockFile::path(&dir),
            r#"
future-root = "keep"
[packages.demo]
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
future-entry = 42
"#,
        )
        .unwrap();
        let lock = LockFile::load(&dir);
        assert_eq!(lock.get("demo").unwrap().sha256.len(), 64);
        lock.save().unwrap();
        let written = std::fs::read_to_string(LockFile::path(&dir)).unwrap();
        assert!(written.contains("lock_version = 1"), "{written}");
        assert!(written.contains("future-root = \"keep\""), "{written}");
        assert!(written.contains("future-entry = 42"), "{written}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn local_lock_overrides_shared_without_being_written_into_it() {
        let dir = temp_dir("local");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            LockFile::path(&dir),
            "[packages.demo]\nsha256 = \"shared\"\n",
        )
        .unwrap();
        std::fs::write(
            LockFile::local_path(&dir),
            "[packages.demo]\nsha256 = \"local\"\n",
        )
        .unwrap();
        let mut lock = LockFile::load(&dir);
        assert_eq!(lock.get("demo").unwrap().sha256, "local");
        lock.set(
            "demo",
            LockEntry {
                sha256: "updated-shared".into(),
                ..Default::default()
            },
        );
        assert_eq!(lock.get("demo").unwrap().sha256, "local");
        lock.save().unwrap();
        let shared = std::fs::read_to_string(LockFile::path(&dir)).unwrap();
        assert!(shared.contains("updated-shared"), "{shared}");
        assert!(!shared.contains("local"), "{shared}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snapshot_diff_and_restore_round_trip_artifact_and_lock() {
        let config = temp_dir("snapshot-config");
        let plugins = temp_dir("snapshot-plugins");
        let _ = std::fs::remove_dir_all(&config);
        let _ = std::fs::remove_dir_all(&plugins);
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();
        let original = b"\0asm\x0d\0\x01\0original".to_vec();
        let changed = b"\0asm\x0d\0\x01\0changed".to_vec();
        let original_hash = crate::package::sha256_hex(&original);
        let changed_hash = crate::package::sha256_hex(&changed);
        std::fs::write(plugins.join("demo.wasm"), &original).unwrap();
        let mut lock = LockFile::load(&config);
        lock.set(
            "demo",
            LockEntry {
                sha256: original_hash.clone(),
                artifact_sha256: Some(original_hash.clone()),
                ..Default::default()
            },
        );
        lock.save().unwrap();
        lock.save_snapshot("before", &plugins).unwrap();

        std::fs::write(plugins.join("demo.wasm"), &changed).unwrap();
        lock.set(
            "demo",
            LockEntry {
                sha256: changed_hash,
                artifact_sha256: Some(crate::package::sha256_hex(&changed)),
                ..Default::default()
            },
        );
        lock.save().unwrap();
        lock.save_snapshot("after", &plugins).unwrap();

        let changes = LockFile::diff_snapshots(&config, "before", "after").unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change, "changed");
        let restored = LockFile::restore_snapshot(&config, &plugins, "before").unwrap();
        assert_eq!(restored.packages["demo"].sha256, original_hash);
        assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), original);
        LockFile::restore_snapshot(&config, &plugins, "before").unwrap();
        assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), original);
        assert_eq!(
            LockFile::load(&config)
                .get("demo")
                .unwrap()
                .artifact_sha256
                .as_deref(),
            Some(original_hash.as_str())
        );
        std::fs::remove_dir_all(&config).unwrap();
        std::fs::remove_dir_all(&plugins).unwrap();
    }
}
