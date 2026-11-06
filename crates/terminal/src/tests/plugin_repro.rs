// SPDX-License-Identifier: MIT OR Apache-2.0
//! plugin-repro bundle tests.

use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).unwrap();
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

#[test]
fn writes_a_minimal_deterministic_bundle_without_secrets() {
    let (mut app, init_dir) = app_with_init(
        r#"
        use_package("alpha", #{
          git: "https://user:secret@example.com/alpha.git",
          pin: "main",
          config: #{
            style: "dark",
            api_token: "topsecret",
            nested: #{ password: "hunter2", safe: "kept" },
          },
        });
        use_package("beta", #{
          requires: "alpha",
          index: "https://idxuser:idxpass@example.com/index.toml?access_token=idxsecret",
          version: "^1.0",
        });
        use_package("gamma", #{ config: #{ token: "should-not-be-in-the-bundle" } });
        "#,
    );
    let plugins = init_dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    plugin_file_from_wat(
        &plugins,
        "alpha",
        include_str!("../../../../tests/fixtures/demo-plugin.wat"),
    );
    plugin_file_from_wat(
        &plugins,
        "beta",
        include_str!("../../../../tests/fixtures/demo-plugin.wat"),
    );
    std::fs::write(
        plugins.join("alpha.toml"),
        "[package]\nname = \"alpha\"\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    std::fs::write(
        plugins.join("beta.toml"),
        "[package]\nname = \"beta\"\nversion = \"2.0.0\"\n",
    )
    .unwrap();
    app.packages.plugin_dir = Some(plugins.clone());
    let alpha_hash =
        forge_core::package::sha256_hex(&std::fs::read(plugins.join("alpha.wasm")).unwrap());
    let beta_hash =
        forge_core::package::sha256_hex(&std::fs::read(plugins.join("beta.wasm")).unwrap());
    std::fs::write(
        init_dir.join("forge-lock.toml"),
        format!(
            "lock_version = 1\n\n\
             [packages.alpha]\n\
             git = \"https://user:secret@example.com/alpha.git\"\n\
             rev = \"main\"\n\
             commit = \"abc123def456\"\n\
             sha256 = \"{alpha_hash}\"\n\
             artifact_sha256 = \"{alpha_hash}\"\n\
             version = \"1.2.3\"\n\n\
             [packages.beta]\n\
             sha256 = \"{beta_hash}\"\n\
             artifact_sha256 = \"{beta_hash}\"\n\
             version = \"2.0.0\"\n",
        ),
    )
    .unwrap();

    let base = std::env::temp_dir().join(format!(
        "forge-plugin-repro-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let first = base.join("first");
    let second = base.join("second");
    crate::plugin_repro::write_to(&app, "beta", &first).unwrap();
    crate::plugin_repro::write_to(&app, "beta", &second).unwrap();

    let init = read(&first.join("init.rhai"));
    assert!(init.contains("use_package(\"alpha\""), "{init}");
    assert!(init.contains("use_package(\"beta\""), "{init}");
    assert!(!init.contains("gamma"), "{init}");
    assert!(init.contains("<redacted>"), "{init}");
    assert!(init.contains("kept"), "{init}");
    assert!(
        init.contains("https://<redacted>@example.com/alpha.git"),
        "{init}"
    );
    assert!(
        init.contains("https://<redacted>@example.com/index.toml?access_token=<redacted>"),
        "{init}"
    );
    assert!(!init.contains("user:secret"), "{init}");
    assert!(!init.contains("idxuser:idxpass"), "{init}");
    assert!(!init.contains("topsecret"), "{init}");
    assert!(!init.contains("idxsecret"), "{init}");
    assert!(!init.contains("hunter2"), "{init}");

    assert!(init.contains("pin: \"abc123def456\""), "{init}");
    let rhai = forge_core::RhaiConfig::new().unwrap();
    let mut editor = Editor::new();
    let loaded = rhai.load_file(&mut editor, &first.join("init.rhai"));
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let uses = rhai.plugin_uses();
    assert_eq!(
        uses.iter()
            .map(|use_| use_.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert_eq!(uses[0].rev.as_deref(), Some("abc123def456"));

    let lock = read(&first.join("forge-lock.toml"));
    assert!(lock.contains("commit = \"abc123def456\""), "{lock}");
    assert!(
        lock.contains("https://<redacted>@example.com/alpha.git"),
        "{lock}"
    );
    assert!(!lock.contains("user:secret"), "{lock}");

    for name in ["alpha", "beta"] {
        assert!(first.join("plugins").join(format!("{name}.wasm")).is_file());
        assert!(first.join("plugins").join(format!("{name}.toml")).is_file());
    }
    assert!(!first.join("plugins").join("gamma.wasm").exists());
    assert!(first.join("README.md").is_file());
    assert!(first.join("info.txt").is_file());

    let first_files = files(&first);
    let second_files = files(&second);
    assert_eq!(
        first_files.keys().collect::<Vec<_>>(),
        second_files.keys().collect::<Vec<_>>()
    );
    for (path, contents) in &first_files {
        assert!(
            !path.to_string_lossy().contains("plugin-grants.toml")
                && !path.to_string_lossy().contains("session.toml")
                && !path.to_string_lossy().contains("credential"),
            "unexpected sensitive file {}",
            path.display()
        );
        let text = String::from_utf8_lossy(contents);
        assert!(!text.contains("topsecret"), "{}", path.display());
        assert!(!text.contains("idxsecret"), "{}", path.display());
        assert!(!text.contains("idxpass"), "{}", path.display());
        assert!(!text.contains("hunter2"), "{}", path.display());
        assert!(
            !text.contains("should-not-be-in-the-bundle"),
            "{}",
            path.display()
        );
        if path != Path::new("info.txt") {
            assert_eq!(
                second_files.get(path),
                Some(contents),
                "{} is not deterministic",
                path.display()
            );
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&init_dir);
}
