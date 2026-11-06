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

    std::fs::remove_dir_all(&dir).unwrap();
}
