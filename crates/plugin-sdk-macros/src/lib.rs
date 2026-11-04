// SPDX-License-Identifier: MIT OR Apache-2.0
//! `manifest!()`: builds a plugin's manifest from its Cargo.toml and
//! embeds it in the `.wasm` as a `forge-manifest` custom section, so a
//! plugin is one file. `forge_plugin!` calls it; plugins don't need to.
//!
//! ```toml
//! [package]
//! name = "forge-spell"
//! version = "0.2.0"
//! description = "Spell checking"
//! homepage = "https://example.com/spell"
//! license = "MIT"
//!
//! [package.metadata.forge]
//! name = "spell"            # default: the package name
//! startup = false           # true: run init at startup, never lazily
//!
//! [package.metadata.forge.permissions]
//! network = false
//! environment = true        # read the process environment
//! memory-mb = 256
//! directories = [{ path = "~/dict", why = "word lists" }]
//!
//! [[package.metadata.forge.commands]]
//! name = "spell-check-buffer"
//! summary = "Check the whole buffer."
//! ```
//!
//! See `forge-plugin-sdk` for the user-facing re-export.

#![warn(missing_docs)]

use proc_macro::TokenStream;
use std::path::Path;

const KNOWN: &[&str] = &[
    "name",
    "startup",
    "permissions",
    "requires",
    "settings",
    "commands",
    "options",
    "modes",
    "extensions",
];

/// The manifest (TOML text) for a Cargo.toml, or an error to show. The
/// plugin's own facts go in `[package]`; everything else the manifest can
/// carry (`[[commands]]`, `[[options]]`, permissions, ...) is passed
/// through from `[package.metadata.forge]`.
fn manifest_from_cargo_toml(src: &str) -> Result<String, String> {
    let cargo: toml::Table = src.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    let package = cargo
        .get("package")
        .and_then(|p| p.as_table())
        .ok_or("Cargo.toml has no [package]")?;
    let forge = match package.get("metadata").and_then(|m| m.get("forge")) {
        Some(toml::Value::Table(t)) => t.clone(),
        Some(_) => return Err("[package.metadata.forge] must be a table".into()),
        None => toml::Table::new(),
    };
    if let Some(k) = forge.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!(
            "[package.metadata.forge]: unknown key {k:?} (known: {})",
            KNOWN.join(", ")
        ));
    }
    let mut pkg = toml::Table::new();
    let name = forge.get("name").or_else(|| package.get("name")).cloned();
    for (key, value) in [
        ("name", name),
        (
            "version",
            package.get("version").filter(|v| v.is_str()).cloned(),
        ),
        ("description", package.get("description").cloned()),
        ("homepage", package.get("homepage").cloned()),
        ("license", package.get("license").cloned()),
    ] {
        if let Some(v) = value {
            pkg.insert(key.into(), v);
        }
    }
    let mut out = toml::Table::new();
    out.insert("package".into(), toml::Value::Table(pkg));
    for (key, value) in [
        ("startup", forge.get("startup").cloned()),
        ("permissions", forge.get("permissions").cloned()),
        ("requires", forge.get("requires").cloned()),
        ("settings", forge.get("settings").cloned()),
        ("commands", forge.get("commands").cloned()),
        ("options", forge.get("options").cloned()),
        ("modes", forge.get("modes").cloned()),
        ("extensions", forge.get("extensions").cloned()),
    ] {
        if let Some(v) = value {
            out.insert(key.into(), v);
        }
    }
    toml::to_string(&out).map_err(|e| e.to_string())
}

/// Embed this plugin's manifest (built from its `Cargo.toml`) into the
/// `.wasm` as a `forge-manifest` custom section.
///
/// Called by `forge_plugin!`; plugins normally do not invoke it directly.
#[proc_macro]
pub fn manifest(_input: TokenStream) -> TokenStream {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let path = Path::new(&dir).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))
        .and_then(|src| manifest_from_cargo_toml(&src));
    let code = match text {
        Ok(text) => {
            let bytes: Vec<String> = text.bytes().map(|b| b.to_string()).collect();
            format!(
                "#[doc(hidden)]\n\
                 #[used]\n\
                 #[cfg_attr(target_family = \"wasm\", unsafe(link_section = \"forge-manifest\"))]\n\
                 static __FORGE_MANIFEST: [u8; {n}] = [{list}];\n\
                 // Rebuild when Cargo.toml changes.\n\
                 const _: &[u8] = include_bytes!({path:?});",
                n = bytes.len(),
                list = bytes.join(","),
                path = path.display().to_string(),
            )
        }
        Err(e) => format!(
            "compile_error!({:?});",
            format!("forge plugin manifest: {e}")
        ),
    };
    code.parse().expect("valid tokens")
}

#[cfg(test)]
mod tests {
    use super::manifest_from_cargo_toml as m;

    #[test]
    fn builds_the_manifest_from_cargo_toml() {
        let text = m(r#"
            [package]
            name = "forge-spell"
            version = "0.2.0"
            description = "Spell checking"
            [dependencies]
            x = "1"
            [package.metadata.forge]
            name = "spell"
            [package.metadata.forge.permissions]
            environment = true
            memory-mb = 256
            directories = [{ path = "~/dict", why = "word lists" }]
        "#)
        .unwrap();
        let t: toml::Table = text.parse().unwrap();
        assert_eq!(t["package"]["name"].as_str(), Some("spell"));
        assert_eq!(t["package"]["version"].as_str(), Some("0.2.0"));
        assert_eq!(t["package"]["description"].as_str(), Some("Spell checking"));
        assert_eq!(t["permissions"]["environment"].as_bool(), Some(true));
        assert_eq!(t["permissions"]["memory-mb"].as_integer(), Some(256));
        assert!(t.get("dependencies").is_none());
    }

    #[test]
    fn passes_command_docs_through() {
        let text = m(r#"
            [package]
            name = "forge-spell"
            version = "0.2.0"
            [package.metadata.forge]
            [[package.metadata.forge.commands]]
            name = "spell-check-buffer"
            summary = "Check the whole buffer."
        "#)
        .unwrap();
        let t: toml::Table = text.parse().unwrap();
        assert_eq!(
            t["commands"][0]["summary"].as_str(),
            Some("Check the whole buffer.")
        );
    }

    #[test]
    fn defaults_to_the_package_name_and_rejects_typos() {
        let t: toml::Table = m("[package]\nname = \"hello\"\nversion.workspace = true\n")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(t["package"]["name"].as_str(), Some("hello"));
        assert!(
            t["package"].get("version").is_none(),
            "inherited versions aren't known here"
        );
        let e =
            m("[package]\nname = \"x\"\n[package.metadata.forge]\npermision = {}\n").unwrap_err();
        assert!(e.contains("permision"), "{e}");
    }
}
