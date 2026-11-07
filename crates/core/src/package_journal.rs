// SPDX-License-Identifier: MIT OR Apache-2.0
//! A durable journal for package updates.
//!
//! Before Forge overwrites an installed plugin, it records the old artifact
//! and the old `packages.toml`/`forge-lock.toml` records here. A killed
//! update leaves this journal behind; the next startup either finishes a
//! committed transaction or restores everything from before the update.
//!
//! The journal lives under the data directory (it is machine-local, not a
//! shared file). It is deliberately timestamp-free, sorted by package name,
//! versioned, and preserves unknown fields so it stays hand-diffable.

use crate::lockfile::{LockEntry, LockFile};
use crate::package::{Installed, InstalledDb};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const JOURNAL_FILE: &str = "package-journal.toml";
pub const ARTIFACTS_DIR: &str = "package-journal-artifacts";
const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JournalState {
    #[default]
    Applying,
    Committed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JournalLocations {
    #[serde(default, skip_serializing_if = "path_is_empty")]
    config: PathBuf,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    plugins: PathBuf,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JournalEntry {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_installed: Option<Installed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_lock: Option<LockEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JournalData {
    #[serde(rename = "journal_version", default = "journal_version")]
    journal_version: u32,
    #[serde(default)]
    state: JournalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(default)]
    locations: JournalLocations,
    #[serde(default)]
    packages: Vec<JournalEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

fn journal_version() -> u32 {
    JOURNAL_VERSION
}

fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// What startup did with a journal it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// A committed transaction only needed its journal removed.
    Finished,
    /// An interrupted transaction was rolled back to its old records.
    RolledBack { packages: usize },
}

/// One package update transaction.
#[derive(Debug, Clone)]
pub struct Journal {
    data_dir: PathBuf,
    state: JournalState,
    operation: Option<String>,
    locations: JournalLocations,
    packages: BTreeMap<String, JournalEntry>,
    extra: BTreeMap<String, toml::Value>,
}

impl Journal {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(JOURNAL_FILE)
    }

    pub fn artifacts_dir(data_dir: &Path) -> PathBuf {
        data_dir.join(ARTIFACTS_DIR)
    }

    /// A new transaction. Nothing is written until the first package is
    /// added, so read-only previews and declines leave no journal behind.
    pub fn new(data_dir: &Path, operation: &str, config: &Path, plugins: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            state: JournalState::Applying,
            operation: Some(operation.to_string()),
            locations: JournalLocations {
                config: config.to_path_buf(),
                plugins: plugins.to_path_buf(),
                extra: BTreeMap::new(),
            },
            packages: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn load(data_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::path(data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!("could not read {}: {error}", path.display()));
            }
        };
        let data: JournalData = toml::from_str(&text)
            .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
        if data.journal_version != JOURNAL_VERSION {
            return Err(format!(
                "{}: unsupported package journal version {}",
                path.display(),
                data.journal_version
            ));
        }
        let packages = data
            .packages
            .into_iter()
            .map(|entry| (entry.name.clone(), entry))
            .collect();
        Ok(Some(Self {
            data_dir: data_dir.to_path_buf(),
            state: data.state,
            operation: data.operation,
            locations: data.locations,
            packages,
            extra: data.extra,
        }))
    }

    pub fn state(&self) -> JournalState {
        self.state
    }

    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Package names recorded in this transaction, sorted.
    pub fn names(&self) -> Vec<String> {
        self.packages.keys().cloned().collect()
    }

    /// Record a package's state *before* the caller changes any file.
    /// Returns `false` when this transaction already recorded it.
    pub fn add(&mut self, name: &str) -> Result<bool, String> {
        if self.packages.contains_key(name) {
            return Ok(false);
        }
        validate_name(name)?;
        let (config, plugins) = self.locations()?;
        let installed = InstalledDb::load(&plugins);
        let artifact = match std::fs::read(installed.plugin_file(name)) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!("{name}: reading current artifact: {error}"));
            }
        };
        let before_artifact = match artifact {
            Some(bytes) => {
                let hash = crate::package::sha256_hex(&bytes);
                self.save_backup(&hash, &bytes)?;
                Some(hash)
            }
            None => None,
        };
        let before_lock = LockFile::load(&config)
            .map_err(|error| format!("{name}: reading the package lock: {error}"))?
            .get(name)
            .cloned();
        self.packages.insert(
            name.to_string(),
            JournalEntry {
                name: name.to_string(),
                before_artifact,
                before_installed: installed.packages.get(name).cloned(),
                before_lock,
                extra: BTreeMap::new(),
            },
        );
        self.save()
            .map_err(|error| format!("{name}: writing package update journal: {error}"))?;
        Ok(true)
    }

    /// Mark every change durable, then remove the journal. If the process
    /// dies after this write, startup only has to finish the cleanup.
    pub fn commit(&mut self) -> Result<(), String> {
        self.state = JournalState::Committed;
        self.save()
            .map_err(|error| format!("writing package update journal: {error}"))?;
        self.cleanup()
            .map_err(|error| format!("removing package update journal: {error}"))
    }

    /// Restore every package recorded in this transaction and remove the
    /// journal. Repeating after an interruption converges to the same state.
    pub fn rollback(&mut self) -> Result<(), String> {
        let (config, plugins) = self.locations()?;
        struct Restore {
            name: String,
            bytes: Option<Vec<u8>>,
            installed: Option<Installed>,
            lock: Option<LockEntry>,
        }
        let mut restores = Vec::with_capacity(self.packages.len());
        for entry in self.packages.values() {
            validate_name(&entry.name)?;
            let bytes = match &entry.before_artifact {
                Some(hash) => Some(self.read_backup(hash)?),
                None => None,
            };
            restores.push(Restore {
                name: entry.name.clone(),
                bytes,
                installed: entry.before_installed.clone(),
                lock: entry.before_lock.clone(),
            });
        }
        std::fs::create_dir_all(&plugins).map_err(|error| {
            format!("creating plugins directory {}: {error}", plugins.display())
        })?;
        for restore in &restores {
            let path = plugins.join(format!("{}.wasm", restore.name));
            match &restore.bytes {
                Some(bytes) => crate::fsutil::write_atomic(&path, bytes).map_err(|error| {
                    format!(
                        "{}: restoring artifact {}: {error}",
                        restore.name,
                        path.display()
                    )
                })?,
                None => match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!(
                            "{}: removing failed artifact {}: {error}",
                            restore.name,
                            path.display()
                        ));
                    }
                },
            }
        }
        let mut installed = InstalledDb::load(&plugins);
        for restore in &restores {
            if let Some(record) = &restore.installed {
                installed
                    .packages
                    .insert(restore.name.clone(), record.clone());
            } else {
                installed.packages.remove(&restore.name);
            }
        }
        installed
            .save()
            .map_err(|error| format!("restoring packages.toml: {error}"))?;
        let mut lock = LockFile::load(&config)
            .map_err(|error| format!("reading the package lock: {error}"))?;
        for restore in &restores {
            if let Some(entry) = &restore.lock {
                lock.set(&restore.name, entry.clone());
            } else {
                lock.remove(&restore.name);
            }
        }
        lock.save()
            .map_err(|error| format!("restoring forge-lock.toml: {error}"))?;
        self.cleanup()
            .map_err(|error| format!("removing package update journal: {error}"))
    }

    fn locations(&self) -> Result<(PathBuf, PathBuf), String> {
        if self.locations.config.as_os_str().is_empty()
            || self.locations.plugins.as_os_str().is_empty()
        {
            return Err("package update journal has no config or plugins directory".into());
        }
        Ok((
            self.locations.config.clone(),
            self.locations.plugins.clone(),
        ))
    }

    fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let data = JournalData {
            journal_version: JOURNAL_VERSION,
            state: self.state,
            operation: self.operation.clone(),
            locations: self.locations.clone(),
            packages: self.packages.values().cloned().collect(),
            extra: self.extra.clone(),
        };
        let text = toml::to_string_pretty(&data).map_err(std::io::Error::other)?;
        let body = format!(
            "# Package update journal (Forge removes this when the update is safe).\n\n{text}"
        );
        crate::fsutil::write_atomic(&Self::path(&self.data_dir), body)
    }

    fn cleanup(&self) -> std::io::Result<()> {
        match std::fs::remove_file(Self::path(&self.data_dir)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match std::fs::remove_dir_all(Self::artifacts_dir(&self.data_dir)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn save_backup(&self, hash: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.backup_path(hash)?;
        let Some(parent) = path.parent() else {
            return Err("package journal artifact path has no parent".into());
        };
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating package journal artifacts: {error}"))?;
        if path.exists()
            && let Ok(existing) = std::fs::read(&path)
            && crate::package::sha256_hex(&existing) == hash
        {
            return Ok(());
        }
        crate::fsutil::write_atomic(&path, bytes)
            .map_err(|error| format!("saving package journal artifact: {error}"))
    }

    fn read_backup(&self, hash: &str) -> Result<Vec<u8>, String> {
        let path = self.backup_path(hash)?;
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "reading package journal artifact {}: {error}",
                path.display()
            )
        })?;
        if crate::package::sha256_hex(&bytes) != hash {
            return Err(format!(
                "package journal artifact {} failed its SHA-256 check",
                path.display()
            ));
        }
        Ok(bytes)
    }

    fn backup_path(&self, hash: &str) -> Result<PathBuf, String> {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid package journal artifact SHA-256".into());
        }
        Ok(Self::artifacts_dir(&self.data_dir).join(format!("{hash}.wasm")))
    }
}

/// Inspect the data directory at startup: finish a committed transaction or
/// roll an interrupted one back. A missing journal is not an error.
pub fn recover(data_dir: &Path) -> Result<Option<Recovery>, String> {
    let Some(mut journal) = Journal::load(data_dir)? else {
        return Ok(None);
    };
    match journal.state {
        JournalState::Committed => {
            journal
                .cleanup()
                .map_err(|error| format!("finishing package update journal: {error}"))?;
            Ok(Some(Recovery::Finished))
        }
        JournalState::Applying => {
            let packages = journal.package_count();
            journal.rollback()?;
            Ok(Some(Recovery::RolledBack { packages }))
        }
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) || name.contains("..")
    {
        return Err(format!("invalid package name in journal: {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "forge-journal-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    fn package(name: &str, sha: &str) -> Package {
        Package {
            name: name.into(),
            version: "1.0.0".into(),
            description: String::new(),
            url: format!("{name}.wasm"),
            sha256: sha.into(),
            git: None,
            rev: None,
            release_notes: None,
        }
    }

    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = temp_dir(name);
        let config = root.join("config");
        let plugins = root.join("plugins");
        let data = root.join("data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();
        (config, plugins, data)
    }

    #[test]
    fn interrupted_update_restores_artifact_records_and_lock() {
        let (config, plugins, data) = setup("rollback");
        let old = b"\0asm\x0d\0\x01\0old".to_vec();
        let new = b"\0asm\x0d\0\x01\0new".to_vec();
        let old_hash = crate::package::sha256_hex(&old);
        let new_hash = crate::package::sha256_hex(&new);
        let mut installed = InstalledDb::load(&plugins);
        installed
            .install(&package("demo", &old_hash), &old)
            .unwrap();
        let mut lock = LockFile::load(&config).unwrap();
        lock.set(
            "demo",
            LockEntry {
                sha256: old_hash.clone(),
                artifact_sha256: Some(old_hash.clone()),
                version: Some("1.0.0".into()),
                ..Default::default()
            },
        );
        lock.save().unwrap();

        let mut journal = Journal::new(&data, "upgrade", &config, &plugins);
        journal.add("demo").unwrap();
        assert!(Journal::path(&data).exists());
        let backup = Journal::artifacts_dir(&data).join(format!("{old_hash}.wasm"));
        assert_eq!(std::fs::read(backup).unwrap(), old);

        let mut new_installed = InstalledDb::load(&plugins);
        new_installed
            .install(&package("demo", &new_hash), &new)
            .unwrap();
        lock.set(
            "demo",
            LockEntry {
                sha256: new_hash.clone(),
                artifact_sha256: Some(new_hash.clone()),
                ..Default::default()
            },
        );
        lock.save().unwrap();
        drop(journal);

        assert_eq!(
            recover(&data).unwrap(),
            Some(Recovery::RolledBack { packages: 1 })
        );
        assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), old);
        assert_eq!(
            InstalledDb::load(&plugins).packages["demo"].sha256,
            old_hash
        );
        assert_eq!(
            LockFile::load(&config)
                .unwrap()
                .get("demo")
                .and_then(|entry| entry.artifact_sha256.clone()),
            Some(old_hash)
        );
        assert!(!Journal::path(&data).exists());
        assert!(!Journal::artifacts_dir(&data).exists());
        std::fs::remove_dir_all(root_for(&config)).unwrap();
    }

    #[test]
    fn committed_journal_is_only_cleaned_up() {
        let (config, plugins, data) = setup("committed");
        let old = b"\0asm\x0d\0\x01\0old".to_vec();
        let old_hash = crate::package::sha256_hex(&old);
        let mut installed = InstalledDb::load(&plugins);
        installed
            .install(&package("demo", &old_hash), &old)
            .unwrap();
        let mut journal = Journal::new(&data, "upgrade", &config, &plugins);
        journal.add("demo").unwrap();
        journal.state = JournalState::Committed;
        journal.save().unwrap();
        std::fs::write(plugins.join("demo.wasm"), b"\0asm\x0d\0\x01\0new").unwrap();

        assert_eq!(recover(&data).unwrap(), Some(Recovery::Finished));
        assert_eq!(
            std::fs::read(plugins.join("demo.wasm")).unwrap(),
            b"\0asm\x0d\0\x01\0new"
        );
        assert!(!Journal::path(&data).exists());
        std::fs::remove_dir_all(root_for(&config)).unwrap();
    }

    #[test]
    fn missing_backup_keeps_the_journal_for_a_retry() {
        let (config, plugins, data) = setup("missing-backup");
        let old = b"\0asm\x0d\0\x01\0old".to_vec();
        let hash = crate::package::sha256_hex(&old);
        let mut installed = InstalledDb::load(&plugins);
        installed.install(&package("demo", &hash), &old).unwrap();
        let mut journal = Journal::new(&data, "upgrade", &config, &plugins);
        journal.add("demo").unwrap();
        drop(journal);
        std::fs::remove_dir_all(Journal::artifacts_dir(&data)).unwrap();

        assert!(recover(&data).is_err());
        assert!(Journal::path(&data).exists());
        std::fs::remove_dir_all(root_for(&config)).unwrap();
    }

    #[test]
    fn journal_preserves_unknown_fields_and_has_no_timestamps() {
        let (config, plugins, data) = setup("additive");
        std::fs::create_dir_all(&data).unwrap();
        let text = format!(
            "journal_version = 1\nstate = \"applying\"\nfuture-root = \"keep\"\n\n\
             [locations]\nconfig = {:?}\nplugins = {:?}\nfuture-location = 7\n\n\
             [[package]]\nname = \"demo\"\nbefore-artifact = \"{}\"\nfuture-entry = true\n",
            config.display().to_string(),
            plugins.display().to_string(),
            "a".repeat(64)
        );
        std::fs::write(Journal::path(&data), text).unwrap();
        let journal = Journal::load(&data).unwrap().unwrap();
        journal.save().unwrap();
        let saved = std::fs::read_to_string(Journal::path(&data)).unwrap();
        assert!(saved.contains("future-root"));
        assert!(saved.contains("future-location"));
        assert!(saved.contains("future-entry"));
        assert!(!saved.to_ascii_lowercase().contains("timestamp"));
        std::fs::remove_dir_all(root_for(&config)).unwrap();
    }

    fn root_for(config: &Path) -> PathBuf {
        config.parent().unwrap_or(config).to_path_buf()
    }
}
