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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        match self.packages.get(&p.name) {
            Some(old) if compare_versions(&old.version, &p.version).is_ge() => {}
            _ => {
                self.packages.insert(p.name.clone(), p);
            }
        }
    }

    pub fn merge(&mut self, other: Index) {
        for p in other.packages.into_values() {
            self.add(p);
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            },
        );
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

    /// Packages installed from version control: (name, repository, rev).
    pub fn from_vc(&self) -> Vec<(String, String, Option<String>)> {
        self.packages
            .iter()
            .filter_map(|(n, i)| i.git.clone().map(|g| (n.clone(), g, i.rev.clone())))
            .collect()
    }
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
    };
    let mut out = String::from("[[package]]\n");
    out.push_str(&toml::to_string(&p).map_err(|e| e.to_string())?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut db2 = db2;
        db2.delete("x").unwrap();
        assert!(!path.exists());
        assert!(InstalledDb::load(&dir).packages.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// ------------------------------------------------------ version control

/// Where a package built from version control came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcSource {
    pub git: String,
    pub rev: Option<String>,
    pub commit: String,
    pub build: Option<String>,
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
/// 4. `package.json` with a "forge" field: `npm install` and the JS SDK's
///    `forge-plugin build` on its "main" (or plugin.js / plugin.ts).
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
            return Ok(BuildPlan {
                name,
                command: Some(format!(
                    "npm install --no-audit --no-fund && npx forge-plugin build {main} -o \"$OUT\""
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
}
