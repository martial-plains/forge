// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sharing plugins: package indexes and what's installed.
//!
//! An index is a TOML file anywhere Forge can read it: a local path, a
//! `file://` URL or an `https://` URL (fetched over HTTP). It lists
//! packages, each a plugin component:
//!
//! ```toml
//! [[package]]
//! name = "lsp"
//! version = "0.1.0"
//! description = "Language Server Protocol client"
//! url = "lsp-0.1.0.wasm"          # relative to the index, or absolute
//! sha256 = "9f2c..."              # of the .wasm; checked before installing
//! ```
//!
//! `forge --package-entry FILE.wasm [--url URL]` prints such an entry
//! (name, version and description come from the plugin's manifest).
//!
//! A package can also come from version control: instead of `url` and
//! `sha256`, `git = "https://github.com/you/plugin"` and optionally
//! `rev = "v1.2.0"` (a tag, branch or commit). Forge clones it and builds
//! it (see [`build_plan`]); `M-x package-vc-install` does the same for
//! any repository, without an index.
//!
//! Installing puts NAME.wasm in the plugins directory and records it in
//! `packages.toml` next to it, so upgrades know what's there. A plugin's
//! permissions aren't in the index: they are in its manifest, and Forge
//! asks for them when it loads the plugin, as for any other.

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    /// A Git repository to clone and build instead of downloading `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// For `git`: the tag, branch or commit (default: the default branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexFile {
    #[serde(default)]
    package: Vec<Package>,
}

/// The packages from one or more indexes, newest version of each name.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub packages: BTreeMap<String, Package>,
    versions: BTreeMap<String, Vec<Package>>,
}

impl Index {
    /// Parse an index read from `location` (relative package URLs are
    /// resolved against it).
    pub fn parse(src: &str, location: &str) -> Result<Self, String> {
        let file: IndexFile = toml::from_str(src).map_err(|e| format!("{location}: {e}"))?;
        let mut index = Self::default();
        for mut p in file.package {
            if p.name.is_empty() || p.name.contains(['/', '\\']) || p.name.starts_with('.') {
                return Err(format!("{location}: bad package name {:?}", p.name));
            }
            match (&p.git, p.url.is_empty(), p.sha256.is_empty()) {
                (Some(_), true, true) => {}
                (Some(_), _, _) => {
                    return Err(format!(
                        "{location}: {}: git, or url and sha256, not both",
                        p.name
                    ));
                }
                (None, false, false) => {}
                (None, _, _) => {
                    return Err(format!(
                        "{location}: {}: needs url and sha256 (or git)",
                        p.name
                    ));
                }
            }
            if !p.url.is_empty() {
                p.url = resolve(location, &p.url);
            }
            p.sha256 = p.sha256.to_ascii_lowercase();
            index.add(p);
        }
        Ok(index)
    }

    /// Add a package, keeping the newer version of a name.
    pub fn add(&mut self, p: Package) {
        let versions = self.versions.entry(p.name.clone()).or_default();
        if !versions
            .iter()
            .any(|old| old.version == p.version && old.url == p.url && old.git == p.git)
        {
            versions.push(p.clone());
            versions.sort_by(|a, b| compare_versions(&b.version, &a.version));
        }
        match self.packages.get(&p.name) {
            Some(old) if compare_versions(&old.version, &p.version).is_ge() => {}
            _ => {
                self.packages.insert(p.name.clone(), p);
            }
        }
    }

    pub fn merge(&mut self, other: Index) {
        for p in other.versions.into_values().flatten() {
            self.add(p);
        }
    }

    /// Resolve a package at its newest indexed version matching a semver
    /// requirement. Without a requirement, use the index's newest version.
    pub fn resolve(
        &self,
        name: &str,
        requirement: Option<&str>,
    ) -> Result<Option<&Package>, String> {
        let Some(requirement) = requirement else {
            return Ok(self.packages.get(name));
        };
        let requirement = semver::VersionReq::parse(requirement)
            .map_err(|e| format!("{name}: invalid version requirement {requirement:?}: {e}"))?;
        let Some(versions) = self.versions.get(name) else {
            return Ok(None);
        };
        for package in versions {
            let version = package
                .version
                .strip_prefix('v')
                .unwrap_or(&package.version);
            let Ok(version) = semver::Version::parse(version) else {
                continue;
            };
            if requirement.matches(&version) {
                return Ok(Some(package));
            }
        }
        Ok(None)
    }
}

/// Detached signatures live next to the index: `index.toml.minisig`.
pub const SIGNATURE_SUFFIX: &str = ".minisig";

/// Where an index's detached signature lives, keeping any query or fragment.
pub fn signature_location(location: &str) -> String {
    let end = location.find(['?', '#']).unwrap_or(location.len());
    format!("{}{SIGNATURE_SUFFIX}{}", &location[..end], &location[end..])
}

/// Verify an index's exact bytes before parsing them.
///
/// With no trusted keys, signatures can't be checked: unsigned indexes are
/// accepted unless `strict` is set. With keys, a signature that is present
/// must verify; an unsigned index is accepted only when `strict` is false.
pub fn verify_index(
    location: &str,
    bytes: &[u8],
    signature: Option<&str>,
    keys: &[String],
    strict: bool,
) -> Result<(), String> {
    if keys.is_empty() {
        if strict {
            return Err(format!(
                "{location}: signed package indexes are required, but forge.opt.package_index_keys is empty"
            ));
        }
        return Ok(());
    }
    let Some(signature) = signature else {
        if strict {
            return Err(format!("{location}: unsigned package index (strict mode)"));
        }
        return Ok(());
    };
    let signature = Signature::decode(signature)
        .map_err(|e| format!("{location}: invalid package index signature: {e}"))?;
    let mut key_errors = Vec::new();
    for key in keys {
        match PublicKey::from_base64(key.trim()) {
            Ok(public_key) => {
                // Accept both current (prehashed) and legacy signatures.
                match public_key.verify(bytes, &signature, true) {
                    Ok(()) => return Ok(()),
                    Err(e) => key_errors.push(e.to_string()),
                }
            }
            Err(e) => key_errors.push(format!("invalid key: {e}")),
        }
    }
    let detail = key_errors
        .into_iter()
        .next()
        .unwrap_or_else(|| "no keys".to_string());
    Err(format!(
        "{location}: package index signature did not verify against forge.opt.package_index_keys ({detail})"
    ))
}

/// `url` relative to where the index came from.
pub fn resolve(index: &str, url: &str) -> String {
    if url.contains("://") || Path::new(url).is_absolute() {
        return url.to_string();
    }
    match index.rfind('/') {
        Some(i) => format!("{}/{url}", &index[..i]),
        None => url.to_string(),
    }
}

/// Is `location` read from disk (a path or file:// URL)? Returns the path.
pub fn local_path(location: &str) -> Option<PathBuf> {
    if let Some(p) = location.strip_prefix("file://") {
        return Some(PathBuf::from(p));
    }
    (!location.contains("://")).then(|| crate::permissions::expand_home(location))
}

/// Order versions like 1.2.10 > 1.2.9 (numeric parts compared as
/// numbers, anything else as text).
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Vec<String> { v.split(['.', '-', '+']).map(String::from).collect() };
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (
            pa.get(i).map_or("0", String::as_str),
            pb.get(i).map_or("0", String::as_str),
        );
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if o.is_ne() {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// -------------------------------------------------------------- installed

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub version: String,
    pub sha256: String,
    /// Where it was downloaded from (empty for version control).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The repository it was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// What was asked for (tag, branch, commit; none: the default branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The commit it was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The build command you agreed to run (asked again if it changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// The full recipe the command was run with, for an upgrade diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<BuildRecipe>,
}

/// `packages.toml` in the plugins directory: what was installed from an
/// index (plugins copied there by hand aren't in it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstalledDb {
    #[serde(default)]
    pub packages: BTreeMap<String, Installed>,
    #[serde(skip)]
    dir: PathBuf,
}

impl InstalledDb {
    pub fn load(plugins_dir: &Path) -> Self {
        let mut db: Self = std::fs::read_to_string(plugins_dir.join("packages.toml"))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        db.dir = plugins_dir.to_path_buf();
        db
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let path = self.dir.join("packages.toml");
        crate::fsutil::write_atomic(
            &path,
            format!("# Plugins installed with M-x package-install (Forge keeps this).\n{text}"),
        )
    }

    pub fn plugin_file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.wasm"))
    }

    /// Check a download and put it in place (atomically), recording it.
    /// Returns the plugin's path.
    pub fn install(&mut self, p: &Package, bytes: &[u8]) -> Result<PathBuf, String> {
        let got = sha256_hex(bytes);
        if got != p.sha256 {
            return Err(format!(
                "{}: the download's sha256 is {got}, the index says {}",
                p.name, p.sha256
            ));
        }
        if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
            return Err(format!(
                "{}: the download isn't a WebAssembly component",
                p.name
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.plugin_file(&p.name);
        crate::fsutil::write_atomic(&path, bytes).map_err(|e| e.to_string())?;
        self.packages.insert(
            p.name.clone(),
            Installed {
                version: p.version.clone(),
                sha256: got,
                url: p.url.clone(),
                git: None,
                rev: None,
                commit: None,
                build: None,
                recipe: None,
            },
        );
        self.save().map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Install a plugin built from a repository (nothing to check it
    /// against: it was built here). Returns the plugin's path.
    pub fn install_built(
        &mut self,
        name: &str,
        version: &str,
        source: &VcSource,
        bytes: &[u8],
    ) -> Result<PathBuf, String> {
        if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
            return Err(format!(
                "{name}: the build didn't make a WebAssembly component"
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.plugin_file(name);
        crate::fsutil::write_atomic(&path, bytes).map_err(|e| e.to_string())?;
        self.packages.insert(
            name.to_string(),
            Installed {
                version: version.to_string(),
                sha256: sha256_hex(bytes),
                url: String::new(),
                git: Some(source.git.clone()),
                rev: source.rev.clone(),
                commit: Some(source.commit.clone()),
                build: source.build.clone(),
                recipe: source.recipe.clone(),
            },
        );
        self.save().map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Restore a previously locked artifact after checking its recorded hash.
    pub fn restore_locked(
        &mut self,
        name: &str,
        installed: Installed,
        bytes: &[u8],
    ) -> Result<PathBuf, String> {
        validate_package_name(name)?;
        let got = sha256_hex(bytes);
        if got != installed.sha256 {
            return Err(format!(
                "{name}: saved artifact hash is {got}, lockfile requires {}",
                installed.sha256
            ));
        }
        if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
            return Err(format!(
                "{name}: saved artifact is not a WebAssembly component"
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.plugin_file(name);
        crate::fsutil::write_atomic(&path, bytes).map_err(|e| e.to_string())?;
        self.packages.insert(name.to_string(), installed);
        self.save().map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Delete an installed package's file and record.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        if self.packages.remove(name).is_none() {
            return Err(format!("{name} wasn't installed from an index"));
        }

        let _ = std::fs::remove_file(self.plugin_file(name));
        let _ = std::fs::remove_file(self.dir.join(format!("{name}.toml")));
        self.save().map_err(|e| e.to_string())
    }

    /// Remove installed packages whose names are not in `keep`, along with
    /// their component and sidecar manifest. Returns the removed names.
    pub fn prune_except(&mut self, keep: &BTreeSet<String>) -> Result<Vec<String>, String> {
        let stale: Vec<String> = self
            .packages
            .keys()
            .filter(|name| !keep.contains(*name))
            .cloned()
            .collect();
        for name in &stale {
            validate_package_name(name)?;
        }
        for name in &stale {
            self.packages.remove(name);
            for path in [
                self.plugin_file(name),
                self.dir.join(format!("{name}.toml")),
            ] {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("{name}: removing {}: {error}", path.display()));
                    }
                }
            }
        }
        if !stale.is_empty() {
            self.save().map_err(|e| e.to_string())?;
        }
        Ok(stale)
    }

    /// Installed packages with a newer version in `index` (packages from
    /// version control are upgraded by fetching instead).
    pub fn upgradable<'a>(&self, index: &'a Index) -> Vec<&'a Package> {
        self.packages
            .iter()
            .filter(|(_, i)| i.git.is_none())
            .filter_map(|(name, i)| {
                index
                    .packages
                    .get(name)
                    .filter(|p| p.git.is_none() && compare_versions(&p.version, &i.version).is_gt())
            })
            .collect()
    }

    pub fn upgradable_for<'a>(
        &self,
        index: &'a Index,
        requirements: &BTreeMap<String, String>,
        frozen: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<&'a Package>, String> {
        let mut upgrades = Vec::new();
        for (name, installed) in &self.packages {
            if installed.git.is_some() || frozen.contains(name) {
                continue;
            }
            let available = index.resolve(name, requirements.get(name).map(String::as_str))?;
            if let Some(package) = available.filter(|p| {
                p.git.is_none() && compare_versions(&p.version, &installed.version).is_gt()
            }) {
                upgrades.push(package);
            }
        }
        Ok(upgrades)
    }

    /// Packages installed from version control: (name, repository, rev).
    pub fn from_vc(&self) -> Vec<(String, String, Option<String>)> {
        self.packages
            .iter()
            .filter_map(|(n, i)| i.git.clone().map(|g| (n.clone(), g, i.rev.clone())))
            .collect()
    }
}

fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\'])
        || name.bytes().any(|b| b.is_ascii_control())
    {
        return Err(format!("{name:?} is not a safe package name"));
    }
    Ok(())
}

/// An index entry for a built plugin (`forge --package-entry`).
pub fn entry_for(wasm: &Path, url: Option<&str>) -> Result<String, String> {
    let bytes = std::fs::read(wasm).map_err(|e| format!("{}: {e}", wasm.display()))?;
    let manifest = crate::permissions::Manifest::for_component(wasm, &bytes)?.unwrap_or_default();
    let stem = wasm
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = manifest.name().map(str::to_string).unwrap_or(stem);
    let version = manifest
        .version()
        .map(str::to_string)
        .unwrap_or_else(|| "0.0.0".into());
    let file = wasm
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let p = Package {
        name,
        version,
        description: manifest.description().unwrap_or_default().to_string(),
        url: url.map_or(file, String::from),
        sha256: sha256_hex(&bytes),
        git: None,
        rev: None,
        release_notes: None,
    };
    let mut out = String::from("[[package]]\n");
    out.push_str(&toml::to_string(&p).map_err(|e| e.to_string())?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minisign_key_and_signature(bytes: &[u8], seed: u8, key_id: [u8; 8]) -> (String, String) {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;
        use ed25519_dalek::{Signer, SigningKey};

        let signing = SigningKey::from_bytes(&[seed; 32]);
        let mut public = vec![0x45, 0x64];
        public.extend_from_slice(&key_id);
        public.extend_from_slice(signing.verifying_key().as_bytes());

        let signature = signing.sign(bytes);
        let mut packet = vec![0x45, 0x64];
        packet.extend_from_slice(&key_id);
        packet.extend_from_slice(&signature.to_bytes());
        let trusted = "timestamp:0\tfile:index.toml";
        let mut global = signature.to_bytes().to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let global_signature = signing.sign(&global);
        let signature_file = format!(
            "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {trusted}\n{}",
            STANDARD.encode(packet),
            STANDARD.encode(global_signature.to_bytes())
        );
        (STANDARD.encode(public), signature_file)
    }

    #[test]
    fn signed_indexes_verify_before_parsing() {
        let src =
            "[[package]]\nname = \"x\"\nversion = \"1.0.0\"\nurl = \"x.wasm\"\nsha256 = \"abc\"\n";
        let (key, signature) =
            minisign_key_and_signature(src.as_bytes(), 7, [1, 2, 3, 4, 5, 6, 7, 8]);
        let (other_key, _) =
            minisign_key_and_signature(src.as_bytes(), 9, [9, 8, 7, 6, 5, 4, 3, 2]);

        assert_eq!(
            signature_location("https://e.org/index.toml?cache=1"),
            "https://e.org/index.toml.minisig?cache=1"
        );
        assert_eq!(
            signature_location("/srv/index.toml"),
            "/srv/index.toml.minisig"
        );

        verify_index(
            "index.toml",
            src.as_bytes(),
            Some(&signature),
            std::slice::from_ref(&key),
            true,
        )
        .unwrap();
        let index = Index::parse(src, "/srv/index.toml").unwrap();
        assert_eq!(index.packages["x"].version, "1.0.0");

        let tampered = src.replace("1.0.0", "9.9.9");
        let error = verify_index(
            "index.toml",
            tampered.as_bytes(),
            Some(&signature),
            std::slice::from_ref(&key),
            true,
        )
        .unwrap_err();
        assert!(error.contains("did not verify"), "{error}");

        // A second trusted key allows rotation without dropping the old one.
        verify_index(
            "index.toml",
            src.as_bytes(),
            Some(&signature),
            &[other_key, key.clone()],
            true,
        )
        .unwrap();

        // Unsigned indexes are allowed outside strict mode, and refused in it.
        verify_index(
            "index.toml",
            src.as_bytes(),
            None,
            std::slice::from_ref(&key),
            false,
        )
        .unwrap();
        let error = verify_index(
            "index.toml",
            src.as_bytes(),
            None,
            std::slice::from_ref(&key),
            true,
        )
        .unwrap_err();
        assert!(error.contains("unsigned"), "{error}");

        // Strict mode also needs somewhere to get the keys from.
        let error = verify_index("index.toml", src.as_bytes(), None, &[], true).unwrap_err();
        assert!(error.contains("package_index_keys is empty"), "{error}");
        verify_index("index.toml", src.as_bytes(), None, &[], false).unwrap();
    }

    #[test]
    fn indexes_versions_and_installing() {
        let dir = std::env::temp_dir().join(format!("forge-packages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = b"\0asm\x0d\0\x01\0fake".to_vec();
        let sum = sha256_hex(&wasm);
        let src = format!(
            "[[package]]\nname = \"x\"\nversion = \"0.9.0\"\nurl = \"x-0.9.wasm\"\nsha256 = \"{sum}\"\n\n\
             [[package]]\nname = \"x\"\nversion = \"0.10.0\"\ndescription = \"X\"\nurl = \"https://e.org/x.wasm\"\nsha256 = \"{}\"\n",
            sum.to_uppercase()
        );
        let index = Index::parse(&src, "/srv/index.toml").unwrap();
        let x = &index.packages["x"];
        assert_eq!(
            (x.version.as_str(), x.url.as_str(), x.sha256.as_str()),
            ("0.10.0", "https://e.org/x.wasm", sum.as_str())
        );
        assert_eq!(
            resolve("https://e.org/forge/index.toml", "a.wasm"),
            "https://e.org/forge/a.wasm"
        );
        assert_eq!(resolve("/srv/index.toml", "a.wasm"), "/srv/a.wasm");
        assert!(
            Index::parse(
                "[[package]]\nname = \"../evil\"\nversion = \"1\"\nurl = \"u\"\nsha256 = \"s\"",
                "i"
            )
            .is_err()
        );
        assert!(compare_versions("1.2.10", "1.2.9").is_gt());
        assert!(compare_versions("1.0", "1.0.0").is_eq());
        assert_eq!(local_path("file:///a/b"), Some(PathBuf::from("/a/b")));
        assert_eq!(local_path("https://x"), None);

        let mut db = InstalledDb::load(&dir);
        assert!(
            db.install(x, b"\0asm-tampered")
                .unwrap_err()
                .contains("sha256")
        );
        let old = Package {
            version: "0.9.0".into(),
            ..x.clone()
        };
        let path = db.install(&old, &wasm).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), wasm);
        let db2 = InstalledDb::load(&dir);
        assert_eq!(db2.packages["x"].version, "0.9.0");
        assert_eq!(db2.upgradable(&index).len(), 1);
        assert_eq!(
            index.resolve("x", Some("^0.9")).unwrap().unwrap().version,
            "0.9.0"
        );
        assert_eq!(
            index.resolve("x", Some("^0.10")).unwrap().unwrap().version,
            "0.10.0"
        );
        assert!(index.resolve("x", Some("^2.0")).unwrap().is_none());
        assert!(index.resolve("x", Some("not a range")).is_err());
        let requirements = BTreeMap::from([("x".into(), "^0.10".into())]);
        let frozen = std::collections::BTreeSet::new();
        assert_eq!(
            db2.upgradable_for(&index, &requirements, &frozen)
                .unwrap()
                .len(),
            1
        );
        assert!(
            db2.upgradable_for(
                &index,
                &requirements,
                &std::collections::BTreeSet::from(["x".into()])
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            db2.upgradable_for(
                &index,
                &BTreeMap::from([("x".into(), "invalid".into())]),
                &frozen
            )
            .is_err()
        );
        let mut db2 = db2;
        db2.delete("x").unwrap();
        assert!(!path.exists());
        assert!(InstalledDb::load(&dir).packages.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pruning_removes_packages_not_kept() {
        let dir = std::env::temp_dir().join(format!("forge-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = b"\0asm\x0d\0\x01\0fake".to_vec();
        let sha = sha256_hex(&wasm);
        let package = |name: &str| Package {
            name: name.into(),
            version: "1.0.0".into(),
            description: String::new(),
            url: format!("{name}.wasm"),
            sha256: sha.clone(),
            git: None,
            rev: None,
            release_notes: None,
        };
        let mut db = InstalledDb::load(&dir);
        db.install(&package("keep"), &wasm).unwrap();
        db.install(&package("stale"), &wasm).unwrap();
        std::fs::write(dir.join("stale.toml"), "name = \"stale\"\n").unwrap();
        std::fs::write(dir.join("manual.wasm"), &wasm).unwrap();

        let removed = db
            .prune_except(&BTreeSet::from(["keep".to_string()]))
            .unwrap();
        assert_eq!(removed, vec!["stale"]);
        assert!(dir.join("keep.wasm").exists());
        assert!(dir.join("manual.wasm").exists());
        assert!(!dir.join("stale.wasm").exists());
        assert!(!dir.join("stale.toml").exists());
        let db = InstalledDb::load(&dir);
        assert!(db.packages.contains_key("keep"));
        assert!(!db.packages.contains_key("stale"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// ------------------------------------------------------ version control

/// Where a package built from version control came from.
#[derive(Debug, Clone, PartialEq)]
pub struct VcSource {
    pub git: String,
    pub rev: Option<String>,
    pub commit: String,
    pub build: Option<String>,
    /// The full recipe used for the build, for a later upgrade diff.
    pub recipe: Option<BuildRecipe>,
}

/// `URL[@REV]` as typed for package-vc-install: the repository and the
/// tag, branch or commit. An `@` in `git@host:path` isn't a rev.
pub fn parse_vc_spec(spec: &str) -> (String, Option<String>) {
    let spec = spec.trim();
    let path_start = spec
        .find("://")
        .map_or_else(|| spec.find(':').map_or(0, |i| i + 1), |i| i + 3);
    match spec[path_start..].rfind('@') {
        Some(i) if !spec[path_start + i + 1..].is_empty() => (
            spec[..path_start + i].to_string(),
            Some(spec[path_start + i + 1..].to_string()),
        ),
        _ => (spec.to_string(), None),
    }
}

/// The package name a repository gives by default: its last path part
/// without `.git` (and without a `forge-` prefix).
pub fn vc_name(repo: &str) -> String {
    #[cfg(windows)]
    let last = repo
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or(repo);
    #[cfg(not(windows))]
    let last = repo
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(repo);
    let name = last.trim_end_matches(".git");
    name.strip_prefix("forge-")
        .filter(|n| !n.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// Where a package's repository is kept: `$FORGE_DATA_DIR/vc/NAME`, else
/// `$XDG_DATA_HOME/forge/vc/NAME`, else `~/.local/share/forge/vc/NAME`.
pub fn vc_dir(name: &str) -> Option<PathBuf> {
    Some(crate::config::data_dir()?.join("vc").join(name))
}

/// How to get a plugin out of a checked-out repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    /// The package name.
    pub name: String,
    /// A shell command to run in the repository (None: the component is
    /// already there). It gets the output path in `$OUT`.
    pub command: Option<String>,
    /// Where the component is afterwards.
    pub output: PathBuf,
    /// How this was decided, for the confirmation prompt.
    pub how: String,
}

/// Everything that decides how a source plugin is built: the command, the
/// toolchain file, cargo features and the target. Stored with an install so
/// an upgrade can show a full diff, not just whether the command changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BuildRecipe {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub toolchain: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// Fields introduced by later Forge versions survive a read/write cycle.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

pub const BUILD_TARGET: &str = "wasm32-wasip2";

impl BuildRecipe {
    /// Read the recipe from a checked-out source tree.
    pub fn for_checkout(command: &str, features: &[String], root: &Path) -> Self {
        let toolchain = ["rust-toolchain.toml", "rust-toolchain"]
            .iter()
            .find_map(|file| std::fs::read_to_string(root.join(file)).ok())
            .unwrap_or_default();
        Self {
            command: command.to_string(),
            toolchain,
            features: features.to_vec(),
            target: BUILD_TARGET.to_string(),
            extra: BTreeMap::new(),
        }
    }

    /// The same hash older lockfiles recorded in `recipe_hash`.
    pub fn hash(&self) -> String {
        let recipe = serde_json::json!({
            "command": self.command,
            "toolchain": self.toolchain,
            "features": self.features,
            "target": self.target,
        });
        serde_json::to_vec(&recipe)
            .map(|bytes| sha256_hex(&bytes))
            .unwrap_or_default()
    }

    /// Lines describing what changed between this recipe and `new`.
    pub fn diff(&self, new: &Self) -> Vec<String> {
        let mut out = Vec::new();
        let show = |s: &str| {
            if s.is_empty() {
                "(none)".to_string()
            } else {
                s.to_string()
            }
        };
        if self.command != new.command {
            out.push(format!(
                "build command: {} -> {}",
                show(&self.command),
                show(&new.command)
            ));
        }
        if self.toolchain != new.toolchain {
            out.push(format!(
                "toolchain: {} -> {}",
                show(&self.toolchain),
                show(&new.toolchain)
            ));
        }
        if self.features != new.features {
            let list = |features: &[String]| {
                if features.is_empty() {
                    "(none)".to_string()
                } else {
                    features.join(", ")
                }
            };
            out.push(format!(
                "features: {} -> {}",
                list(&self.features),
                list(&new.features)
            ));
        }
        if self.target != new.target {
            out.push(format!(
                "target: {} -> {}",
                show(&self.target),
                show(&new.target)
            ));
        }
        out
    }
}

/// Default time a source build may run before it is killed.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Environment variables a build may inherit by default. A trailing `*`
/// is a prefix match; every other name is an exact match.
pub const DEFAULT_BUILD_ENV: &[&str] = &[
    // Core.
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LC_*",
    // Toolchains.
    "CARGO_HOME",
    "RUSTUP_HOME",
    // Windows.
    "SystemRoot",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SystemDrive",
    // Proxies and certificate bundles.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// What a source build may inherit from the parent process and how long
/// it may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPolicy {
    /// Names, or `PREFIX_*` patterns, copied from the parent environment.
    /// Everything else is dropped.
    pub env_allow: Vec<String>,
    pub timeout: Duration,
}

impl Default for BuildPolicy {
    fn default() -> Self {
        Self {
            env_allow: DEFAULT_BUILD_ENV
                .iter()
                .map(|name| name.to_string())
                .collect(),
            timeout: DEFAULT_BUILD_TIMEOUT,
        }
    }
}

impl BuildPolicy {
    /// Add names or patterns to the allowlist.
    pub fn allow<I, S>(&mut self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.env_allow.extend(names.into_iter().map(Into::into));
    }
}

/// Copy the variables `policy` allows out of `parent`.
///
/// Pure: the caller reads the process environment (see [`process_env`]),
/// so the filter is testable without touching the real environment.
pub fn build_env(policy: &BuildPolicy, parent: &[(String, String)]) -> Vec<(String, String)> {
    parent
        .iter()
        .filter(|(name, _)| {
            policy
                .env_allow
                .iter()
                .any(|pattern| env_name_matches(pattern, name))
        })
        .cloned()
        .collect()
}

/// The real process environment, with non-Unicode values made lossy.
pub fn process_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

fn env_name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name_has_prefix(name, prefix),
        None => name_eq(name, pattern),
    }
}

#[cfg(windows)]
fn name_eq(name: &str, expected: &str) -> bool {
    name.eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
fn name_eq(name: &str, expected: &str) -> bool {
    name == expected
}

#[cfg(windows)]
fn name_has_prefix(name: &str, prefix: &str) -> bool {
    let (name, prefix) = (name.as_bytes(), prefix.as_bytes());
    name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix)
}

#[cfg(not(windows))]
fn name_has_prefix(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgePackageToml {
    name: Option<String>,
    /// Shell command; $OUT is where to put the component (unless `wasm`).
    build: Option<String>,
    /// The component, relative to the repository (after `build`, if any).
    wasm: Option<String>,
}

/// Decide how to build the plugin in `src`, in this order:
///
/// 1. `forge-package.toml` at the top: `name`, `build` (a shell command,
///    `$OUT` is where the component goes) and/or `wasm` (where it is,
///    for a prebuilt component or a build that picks its own path).
/// 2. An executable `build.sh`: run as `./build.sh -o "$OUT"` (what the
///    Lua and C SDKs' scripts and plugins/lsp take).
/// 3. `Cargo.toml`: `cargo build --release --target wasm32-wasip2`.
/// 4. `package.json` with a "forge" field: `npm ci --ignore-scripts` and
///    the JS SDK's `forge-plugin build` on its "main" (or plugin.js /
///    plugin.ts). The repository must commit `package-lock.json` or
///    `npm-shrinkwrap.json`.
/// 5. Exactly one `.wasm` at the top or in `dist/`: used as is.
pub fn build_plan(src: &Path, default_name: &str) -> Result<BuildPlan, String> {
    let out = src
        .join(".forge-build")
        .join(format!("{default_name}.wasm"));
    let manifest = src.join("forge-package.toml");
    if manifest.exists() {
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        let m: ForgePackageToml =
            toml::from_str(&text).map_err(|e| format!("forge-package.toml: {e}"))?;
        let name = m.name.unwrap_or_else(|| default_name.to_string());
        let output = match &m.wasm {
            Some(w) => src.join(w),
            None => src.join(".forge-build").join(format!("{name}.wasm")),
        };
        if m.build.is_none() && m.wasm.is_none() {
            return Err("forge-package.toml needs build or wasm".into());
        }
        return Ok(BuildPlan {
            name,
            command: m.build,
            output,
            how: "forge-package.toml".into(),
        });
    }
    let name = default_name.to_string();
    if is_executable(&src.join("build.sh")) {
        return Ok(BuildPlan {
            name,
            command: Some("./build.sh -o \"$OUT\"".into()),
            output: out,
            how: "build.sh".into(),
        });
    }
    if let Ok(text) = std::fs::read_to_string(src.join("Cargo.toml")) {
        let table: toml::Table = toml::from_str(&text).map_err(|e| format!("Cargo.toml: {e}"))?;
        let krate = table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .ok_or("Cargo.toml has no [package] name (a workspace? add forge-package.toml)")?
            .replace('-', "_");
        return Ok(BuildPlan {
            name,
            command: Some("cargo build --release --target wasm32-wasip2".into()),
            output: src
                .join("target/wasm32-wasip2/release")
                .join(format!("{krate}.wasm")),
            how: "Cargo.toml".into(),
        });
    }
    if let Ok(text) = std::fs::read_to_string(src.join("package.json")) {
        let has_forge = text.contains("\"forge\"");
        if has_forge {
            let main = ["plugin.ts", "plugin.js", "index.ts", "index.js"]
                .iter()
                .find(|f| src.join(f).exists())
                .copied();
            let main = extract_json_string(&text, "main")
                .or(main.map(String::from))
                .ok_or("package.json: no \"main\" or plugin.js")?;
            if !src.join("package-lock.json").exists() && !src.join("npm-shrinkwrap.json").exists()
            {
                return Err(format!(
                    "{default_name}: package.json has no package-lock.json or npm-shrinkwrap.json, so npm ci can't run. Ask the author to commit one, or declare a reviewed build = \"...\" for this plugin."
                ));
            }
            return Ok(BuildPlan {
                name,
                command: Some(format!(
                    "npm ci --ignore-scripts --no-audit --no-fund && npx --no-install forge-plugin build {main} -o \"$OUT\""
                )),
                output: out,
                how: "package.json".into(),
            });
        }
    }
    let mut wasm: Vec<PathBuf> = Vec::new();
    for dir in [src.to_path_buf(), src.join("dist")] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            wasm.extend(
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "wasm")),
            );
        }
    }
    match wasm.len() {
        1 => Ok(BuildPlan {
            name,
            command: None,
            output: wasm.remove(0),
            how: "a prebuilt .wasm".into(),
        }),
        0 => Err(
            "don't know how to build it: add forge-package.toml (build = \"...\", wasm = \"...\")"
                .into(),
        ),
        _ => Err("several .wasm files: say which in forge-package.toml (wasm = \"...\")".into()),
    }
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#""{key}"\s*:\s*"([^"]+)""#)).ok()?;
    re.captures(json).map(|c| c[1].to_string())
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(test)]
mod vc_tests {
    use super::*;

    #[test]
    fn specs_and_names() {
        assert_eq!(
            parse_vc_spec("https://github.com/a/forge-lsp@v1.0"),
            ("https://github.com/a/forge-lsp".into(), Some("v1.0".into()))
        );
        assert_eq!(
            parse_vc_spec("git@github.com:a/b.git"),
            ("git@github.com:a/b.git".into(), None)
        );
        assert_eq!(
            parse_vc_spec("git@github.com:a/b.git@main"),
            ("git@github.com:a/b.git".into(), Some("main".into()))
        );
        assert_eq!(parse_vc_spec("/src/plugin"), ("/src/plugin".into(), None));
        assert_eq!(vc_name("https://github.com/a/forge-lsp.git"), "lsp");
        assert_eq!(vc_name("git@github.com:a/spell"), "spell");
        assert_eq!(vc_name("/src/my-plugin/"), "my-plugin");
        #[cfg(windows)]
        {
            assert_eq!(
                parse_vc_spec(r"C:\src\forge-demo@v1"),
                (r"C:\src\forge-demo".into(), Some("v1".into()))
            );
            assert_eq!(vc_name(r"C:\src\forge-demo\\"), "demo");
        }
    }

    #[test]
    fn build_recipes_diff_every_part() {
        let dir = std::env::temp_dir().join(format!("forge-recipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"stable\"\n",
        )
        .unwrap();
        let old = BuildRecipe::for_checkout("cargo build", &["a".into()], &dir);
        assert!(old.toolchain.contains("stable"));
        let same = BuildRecipe::for_checkout("cargo build", &["a".into()], &dir);
        assert_eq!(old.hash(), same.hash());
        assert!(old.diff(&same).is_empty());

        std::fs::write(
            dir.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"nightly\"\n",
        )
        .unwrap();
        let new =
            BuildRecipe::for_checkout("cargo build --release", &["a".into(), "b".into()], &dir);
        let diff = old.diff(&new);
        assert!(
            diff.iter().any(|line| line.starts_with("build command:")),
            "{diff:?}"
        );
        assert!(
            diff.iter().any(|line| line.starts_with("toolchain:")),
            "{diff:?}"
        );
        assert!(
            diff.iter().any(|line| line == "features: a -> a, b"),
            "{diff:?}"
        );
        assert_ne!(old.hash(), new.hash());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_plans() {
        let dir = std::env::temp_dir().join(format!("forge-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        assert!(
            build_plan(&dir, "p")
                .unwrap_err()
                .contains("forge-package.toml")
        );
        std::fs::write(dir.join("dist/p.wasm"), "").unwrap();
        let plan = build_plan(&dir, "p").unwrap();
        assert_eq!((plan.command, plan.output), (None, dir.join("dist/p.wasm")));
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"forge-p\"\n").unwrap();
        let plan = build_plan(&dir, "p").unwrap();
        assert_eq!(
            plan.output,
            dir.join("target/wasm32-wasip2/release/forge_p.wasm")
        );
        std::fs::write(
            dir.join("forge-package.toml"),
            "name = \"q\"\nbuild = \"make\"\n",
        )
        .unwrap();
        let plan = build_plan(&dir, "p").unwrap();
        assert_eq!(
            (plan.name.as_str(), plan.command.as_deref()),
            ("q", Some("make"))
        );
        assert_eq!(plan.output, dir.join(".forge-build/q.wasm"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_entries_in_indexes() {
        let i = Index::parse(
            "[[package]]\nname = \"x\"\nversion = \"1\"\ngit = \"https://g/x\"\nrev = \"v1\"\n",
            "i.toml",
        )
        .unwrap();
        assert_eq!(i.packages["x"].git.as_deref(), Some("https://g/x"));
        assert!(Index::parse("[[package]]\nname = \"x\"\nversion = \"1\"\n", "i").is_err());
        assert!(Index::parse("[[package]]\nname = \"x\"\nversion = \"1\"\ngit = \"g\"\nurl = \"u\"\nsha256 = \"s\"\n", "i").is_err());
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_build_policy_allows_toolchains_and_proxies() {
        let policy = BuildPolicy::default();
        let parent = vec![
            ("PATH".into(), "/bin".into()),
            ("CARGO_HOME".into(), "/c".into()),
            ("LC_ALL".into(), "C".into()),
            ("HTTPS_PROXY".into(), "http://p".into()),
            ("NPM_TOKEN".into(), "secret".into()),
        ];
        let env = build_env(&policy, &parent);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"PATH"));
        assert!(names.contains(&"CARGO_HOME"));
        assert!(names.contains(&"LC_ALL"));
        assert!(names.contains(&"HTTPS_PROXY"));
        assert!(!names.contains(&"NPM_TOKEN"), "{names:?}");
    }

    #[test]
    fn build_env_exact_and_prefix_patterns() {
        let policy = BuildPolicy {
            env_allow: vec!["MY_*".into(), "TOKEN".into()],
            ..Default::default()
        };
        let parent = vec![
            ("MY_TOKEN".into(), "a".into()),
            ("MY_".into(), "b".into()),
            ("TOKEN".into(), "c".into()),
            ("TOKEN_EXTRA".into(), "d".into()),
            ("OTHER".into(), "e".into()),
        ];
        let env = build_env(&policy, &parent);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["MY_TOKEN", "MY_", "TOKEN"]);
    }

    #[cfg(windows)]
    #[test]
    fn build_env_names_ignore_case_on_windows() {
        let policy = BuildPolicy {
            env_allow: vec!["Path".into(), "lc_*".into()],
            ..Default::default()
        };
        let parent = vec![
            ("PATH".into(), "a".into()),
            ("LC_ALL".into(), "b".into()),
            ("other".into(), "c".into()),
        ];
        let env = build_env(&policy, &parent);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["PATH", "LC_ALL"]);
    }

    #[cfg(unix)]
    #[test]
    fn build_env_keeps_allowlisted_values_and_drops_secrets_from_spawn() {
        use crate::process::{ProcessEvent, Processes, Program};

        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let secret = format!("FORGE_TEST_SECRET_{}", std::process::id());
        let allowed = format!("FORGE_TEST_ALLOWED_{}", std::process::id());
        // SAFETY: this test holds ENV_LOCK, so it is the only one in this
        // module changing the environment while it runs.
        unsafe {
            std::env::set_var(&secret, "top-secret");
            std::env::set_var(&allowed, "visible");
        }
        let mut policy = BuildPolicy::default();
        policy.allow([allowed.clone()]);
        let env = build_env(&policy, &process_env());
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert!(!names.contains(&secret.as_str()), "{names:?}");
        assert!(names.contains(&allowed.as_str()), "{names:?}");

        let mut processes = Processes::new();
        let dir = std::env::temp_dir();
        let script = format!("printf '%s|%s' \"${{{allowed}:-unset}}\" \"${{{secret}:-unset}}\"");
        let id = processes
            .spawn_env(Program::Shell(&script), &dir, false, &env)
            .unwrap();
        let events = processes.wait(id, Duration::from_secs(10));
        let output: String = events
            .iter()
            .filter_map(|event| match event {
                ProcessEvent::Output(_, text) | ProcessEvent::Stderr(_, text) => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(output, "visible|unset");
        // SAFETY: still holding ENV_LOCK.
        unsafe {
            std::env::remove_var(&secret);
            std::env::remove_var(&allowed);
        }
    }

    #[test]
    fn npm_plan_requires_a_lockfile_and_disables_scripts() {
        let dir = std::env::temp_dir().join(format!("forge-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.js"), "").unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"main":"plugin.js","forge":{}}"#,
        )
        .unwrap();
        let error = build_plan(&dir, "p").unwrap_err();
        assert!(error.contains("package-lock.json"), "{error}");
        assert!(error.contains("npm-shrinkwrap.json"), "{error}");

        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        let plan = build_plan(&dir, "p").unwrap();
        let command = plan.command.unwrap();
        assert!(command.starts_with("npm ci --ignore-scripts"), "{command}");
        assert!(
            command.contains("npx --no-install forge-plugin build plugin.js"),
            "{command}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
