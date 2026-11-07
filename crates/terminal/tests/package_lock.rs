// SPDX-License-Identifier: MIT OR Apache-2.0

use forge_core::package::{InstalledDb, Package};
use forge_core::{LockEntry, LockFile};
use std::path::PathBuf;
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("forge-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn package_declarations_collect_without_running_code() {
    let dir = temp_dir("package-declarations");
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("installed");
    let exe = format!("forge-missing-{}", std::process::id());
    let init = dir.join("init.rhai");
    std::fs::write(
        &init,
        format!(
            r#"use_package("demo", #{{
                when: || {{ throw "condition ran" }},
                preface: || {{ throw "preface ran" }},
                ensure_system_package: #{{ "{exe}": "touch {}" }},
            }});
"#,
            marker.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "declarations", "--json", "--init-directory"])
        .arg(&dir)
        .env("FORGE_DATA_DIR", dir.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let declared: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = &declared["entries"][0];
    assert_eq!(entry["name"], "demo");
    assert_eq!(entry["state"]["kind"], "conditional");
    assert!(
        entry["deferred"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("condition"))
    );
    assert!(!marker.exists(), "collection installed a package");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_lock_is_reported_and_not_rewritten() {
    let dir = temp_dir("malformed-lock");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("forge-lock.toml");
    let bad = "[packages.demo\nsha256 = \"0123\"\n";
    std::fs::write(&lock, bad).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--batch")
        .arg("--init-directory")
        .arg(&dir)
        .args(["-f", "package-restore"])
        .env("FORGE_DATA_DIR", dir.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Can't read the package lock"), "{stderr}");
    assert!(stderr.contains(&lock.display().to_string()), "{stderr}");
    assert!(stderr.contains("could not parse"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), bad);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn snapshot_save_reads_components_from_load_path() {
    let config = temp_dir("snapshot-load-path");
    let custom = temp_dir("snapshot-load-dir");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&custom).unwrap();
    let bytes = b"\0asm\x0d\0\x01\0custom".to_vec();
    let hash = forge_core::package::sha256_hex(&bytes);
    std::fs::write(custom.join("demo.wasm"), &bytes).unwrap();
    let init = config.join("init.rhai");
    std::fs::write(
        &init,
        format!(
            "use_package(\"demo\", #{{ load_path: {:?} }});\n",
            custom.display().to_string()
        ),
    )
    .unwrap();
    let mut lock = LockFile::load(&config).unwrap();
    lock.set(
        "demo",
        LockEntry {
            sha256: hash.clone(),
            artifact_sha256: Some(hash.clone()),
            ..Default::default()
        },
    );
    lock.save().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--headless")
        .arg("--init")
        .arg(&init)
        .args([
            "--keys",
            "M-x",
            "--type",
            "package-snapshot-save",
            "--keys",
            "RET",
            "--type",
            "custom",
            "--keys",
            "RET",
        ])
        .env("FORGE_CONFIG_DIR", &config)
        .env("FORGE_CACHE_DIR", config.join("cache"))
        .env("FORGE_STATE_DIR", config.join("state"))
        .env("FORGE_DATA_DIR", config.join("data"))
        .output()
        .unwrap();
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{output_text}");
    assert!(
        config.join("package-snapshots/custom.toml").exists(),
        "{output_text}"
    );
    assert_eq!(LockFile::artifact(&config, &hash).unwrap(), bytes);

    std::fs::remove_dir_all(&config).unwrap();
    std::fs::remove_dir_all(&custom).unwrap();
}

#[test]
fn lockfile_diff_reports_changes_and_json() {
    let before_dir = temp_dir("lockfile-diff-before");
    let after_dir = temp_dir("lockfile-diff-after");
    std::fs::create_dir_all(&before_dir).unwrap();
    std::fs::create_dir_all(&after_dir).unwrap();
    let mut before = LockFile::load(&before_dir).unwrap();
    before.set(
        "demo",
        LockEntry {
            commit: Some("aaaaaaaaaaaaaaaa".into()),
            sha256: "before".into(),
            ..Default::default()
        },
    );
    before.set(
        "old",
        LockEntry {
            commit: Some("dddddddddddddddd".into()),
            sha256: "old".into(),
            ..Default::default()
        },
    );
    before.save().unwrap();
    let mut after = LockFile::load(&after_dir).unwrap();
    after.set(
        "demo",
        LockEntry {
            commit: Some("bbbbbbbbbbbbbbbb".into()),
            sha256: "after".into(),
            ..Default::default()
        },
    );
    after.set(
        "new",
        LockEntry {
            commit: Some("cccccccccccccccc".into()),
            sha256: "new".into(),
            ..Default::default()
        },
    );
    after.save().unwrap();
    let before_path = LockFile::path(&before_dir);
    let after_path = LockFile::path(&after_dir);

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "lockfile", "diff"])
        .arg(&before_path)
        .arg(&after_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("Package lockfile diff:"), "{stdout}");
    assert!(
        stdout.contains("changed demo: aaaaaaaaaaaa -> bbbbbbbbbbbb"),
        "{stdout}"
    );
    assert!(stdout.contains("added new: - -> cccccccccccc"), "{stdout}");
    assert!(
        stdout.contains("removed old: dddddddddddd -> -"),
        "{stdout}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "lockfile", "diff"])
        .arg(&before_path)
        .arg(&after_path)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let changes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let changes = changes.as_array().unwrap();
    assert_eq!(changes.len(), 3);
    let demo = changes
        .iter()
        .find(|change| change["name"] == "demo")
        .unwrap();
    assert_eq!(demo["change"], "changed");
    assert_eq!(demo["after"]["commit"], "bbbbbbbbbbbbbbbb");

    std::fs::remove_dir_all(&before_dir).unwrap();
    std::fs::remove_dir_all(&after_dir).unwrap();
}

#[test]
fn package_restore_removes_installed_packages_absent_from_lock() {
    let dir = temp_dir("restore-prune");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let kept = b"\0asm\x0d\0\x01\0kept".to_vec();
    let stale = b"\0asm\x0d\0\x01\0stale".to_vec();
    let kept_hash = forge_core::package::sha256_hex(&kept);
    let stale_hash = forge_core::package::sha256_hex(&stale);
    let package = |name: &str, sha: &str| Package {
        name: name.into(),
        version: "1.0.0".into(),
        description: String::new(),
        url: format!("{name}.wasm"),
        sha256: sha.into(),
        git: None,
        rev: None,
        release_notes: None,
    };
    let mut installed = InstalledDb::load(&plugins);
    installed
        .install(&package("keep", &kept_hash), &kept)
        .unwrap();
    installed
        .install(&package("stale", &stale_hash), &stale)
        .unwrap();
    std::fs::write(plugins.join("stale.toml"), "name = \"stale\"\n").unwrap();

    let mut lock = LockFile::load(&dir).unwrap();
    lock.set(
        "keep",
        LockEntry {
            url: Some("keep.wasm".into()),
            sha256: kept_hash.clone(),
            artifact_sha256: Some(kept_hash.clone()),
            version: Some("1.0.0".into()),
            ..Default::default()
        },
    );
    lock.save().unwrap();
    LockFile::save_artifact(&dir, &kept_hash, &kept).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--batch")
        .arg("-q")
        .arg("--init-directory")
        .arg(&dir)
        .args(["-f", "package-restore"])
        .env("FORGE_DATA_DIR", dir.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Restored 1 locked plugin(s)"), "{stderr}");
    assert!(stderr.contains("stale: installed -> removed"), "{stderr}");
    assert_eq!(std::fs::read(plugins.join("keep.wasm")).unwrap(), kept);
    assert!(!plugins.join("stale.wasm").exists());
    assert!(!plugins.join("stale.toml").exists());
    let installed = InstalledDb::load(&plugins);
    assert!(installed.packages.contains_key("keep"));
    assert!(!installed.packages.contains_key("stale"));

    // A second run, as after an interrupted restore, changes nothing.
    let packages = std::fs::read(plugins.join("packages.toml")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--batch")
        .arg("-q")
        .arg("--init-directory")
        .arg(&dir)
        .args(["-f", "package-restore"])
        .env("FORGE_DATA_DIR", dir.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("all resolved commits and artifact hashes already matched"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(plugins.join("keep.wasm")).unwrap(), kept);
    assert_eq!(
        std::fs::read(plugins.join("packages.toml")).unwrap(),
        packages
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

fn plugin_file_from_wat(dir: &std::path::Path, name: &str, source: &str) -> PathBuf {
    use wit_component::{ComponentEncoder, StringEncoding};
    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/wit-0.1.0/forge-editor.wit"
        ))
        .unwrap();
    let world = resolve.select_world(&[pkg], Some("plugin")).unwrap();
    let mut module = wat::parse_str(source).unwrap();
    wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{name}.wasm"));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "tag.gpgSign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn package_sync_honors_the_locked_artifact() {
    let dir = temp_dir("package-sync-lock");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let locked = b"\0asm\x0d\0\x01\0locked".to_vec();
    let locked_hash = forge_core::package::sha256_hex(&locked);
    let declared = b"\0asm\x0d\0\x01\0declared".to_vec();
    let declared_hash = forge_core::package::sha256_hex(&declared);
    std::fs::write(dir.join("declared.wasm"), &declared).unwrap();
    std::fs::write(
        dir.join("init.rhai"),
        format!(
            "use_package(\"demo\", #{{ url: {:?}, sha256: {:?} }});\n",
            "declared.wasm", declared_hash
        ),
    )
    .unwrap();
    let mut lock = LockFile::load(&dir).unwrap();
    lock.set(
        "demo",
        LockEntry {
            url: Some("locked.wasm".into()),
            rev: Some("main".into()),
            sha256: locked_hash.clone(),
            artifact_sha256: Some(locked_hash.clone()),
            version: Some("1.0.0".into()),
            ..Default::default()
        },
    );
    lock.save().unwrap();
    LockFile::save_artifact(&dir, &locked_hash, &locked).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "sync", "--json", "--init-directory"])
        .arg(&dir)
        .env("FORGE_DATA_DIR", dir.join("data"))
        .env("FORGE_CACHE_DIR", dir.join("cache"))
        .env("FORGE_STATE_DIR", dir.join("state"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "ok", "{report}");
    assert_eq!(report["packages"][0]["action"], "installed");
    assert_eq!(std::fs::read(plugins.join("demo.wasm")).unwrap(), locked);
    let lock = LockFile::load(&dir).unwrap();
    let entry = lock.get("demo").unwrap();
    assert_eq!(entry.url.as_deref(), Some("locked.wasm"));
    assert_eq!(entry.rev.as_deref(), Some("main"));
    assert_eq!(entry.sha256, locked_hash);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_sync_blocks_a_build_unless_allowed() {
    let dir = temp_dir("package-sync-build");
    let repo = dir.join("repo");
    let wasm = plugin_file_from_wat(
        &repo,
        "demo",
        include_str!("../../../tests/fixtures/demo-plugin.wat"),
    );
    let hash = forge_core::package::sha256_hex(&std::fs::read(&wasm).unwrap());
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "demo"]);
    let build = if cfg!(windows) {
        "copy /Y demo.wasm \"%OUT%\" >nul"
    } else {
        "cp demo.wasm \"$OUT\""
    };
    std::fs::write(
        repo.join("forge-package.toml"),
        format!("build = {build:?}\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "build recipe"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(
        dir.join("init.rhai"),
        format!(
            "use_package(\"demo\", #{{ git: {:?} }});\n",
            repo.display().to_string()
        ),
    )
    .unwrap();
    let mut lock = LockFile::load(&dir).unwrap();
    lock.set(
        "demo",
        LockEntry {
            git: Some(repo.display().to_string()),
            rev: Some("main".into()),
            commit: Some(commit.clone()),
            sha256: hash.clone(),
            artifact_sha256: Some(hash.clone()),
            ..Default::default()
        },
    );
    lock.save().unwrap();

    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_forge"))
            .args(["package", "sync"])
            .args(extra)
            .arg("--init-directory")
            .arg(&dir)
            .env("FORGE_DATA_DIR", dir.join("data"))
            .env("FORGE_CACHE_DIR", dir.join("cache"))
            .env("FORGE_STATE_DIR", dir.join("state"))
            .output()
            .unwrap()
    };
    let output = run(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("--allow-build"), "{stderr}");
    assert!(stderr.contains("package_allow_build"), "{stderr}");
    assert!(!dir.join("plugins/demo.wasm").exists());

    let output = run(&["--json", "--allow-build"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "ok", "{report}");
    assert_eq!(
        std::fs::read(dir.join("plugins/demo.wasm")).unwrap(),
        std::fs::read(&wasm).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_sync_bad_usage_exits_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "sync", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown package option"), "{stderr}");
}

#[test]
fn package_sync_installs_a_fresh_machine_from_init_and_lock() {
    let dir = temp_dir("package-sync-fresh");
    let source = plugin_file_from_wat(
        &dir.join("source"),
        "demo",
        include_str!("../../../tests/fixtures/demo-plugin.wat"),
    );
    let hash = forge_core::package::sha256_hex(&std::fs::read(&source).unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("init.rhai"),
        format!(
            "use_package(\"demo\", #{{ url: {:?}, sha256: {:?} }});\n",
            source.display().to_string(),
            hash
        ),
    )
    .unwrap();
    let mut lock = LockFile::load(&dir).unwrap();
    lock.set(
        "demo",
        LockEntry {
            url: Some(source.display().to_string()),
            sha256: hash.clone(),
            artifact_sha256: Some(hash.clone()),
            ..Default::default()
        },
    );
    lock.save().unwrap();
    assert!(!dir.join("plugins/demo.wasm").exists());
    assert!(!dir.join("package-artifacts").exists());

    let output = Command::new(env!("CARGO_BIN_EXE_forge"))
        .args(["package", "sync", "--json", "--init-directory"])
        .arg(&dir)
        .env("FORGE_DATA_DIR", dir.join("data"))
        .env("FORGE_CACHE_DIR", dir.join("cache"))
        .env("FORGE_STATE_DIR", dir.join("state"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert_eq!(
        std::fs::read(dir.join("plugins/demo.wasm")).unwrap(),
        std::fs::read(&source).unwrap()
    );
    assert!(LockFile::artifact(&dir, &hash).is_ok());

    std::fs::remove_dir_all(&dir).unwrap();
}
