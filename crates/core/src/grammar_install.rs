// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::syntax::{self, DynamicGrammar};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const OWNER: &str = "grammar-install";
const MANIFEST: &str = "installed.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipe {
    pub name: &'static str,
    pub mode: &'static str,
    pub repository: &'static str,
    pub revision: Option<&'static str>,
    pub source_dir: Option<&'static str>,
    pub extensions: &'static [&'static str],
    pub file_names: &'static [&'static str],
    pub interpreters: &'static [&'static str],
    pub line_comment: Option<&'static str>,
    pub indent_width: usize,
}

pub const RECIPES: &[Recipe] = &[
    Recipe {
        name: "typescript",
        mode: "typescript-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-typescript",
        revision: None,
        source_dir: Some("typescript"),
        extensions: &["ts", "mts", "cts"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 2,
    },
    Recipe {
        name: "tsx",
        mode: "tsx-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-typescript",
        revision: None,
        source_dir: Some("tsx"),
        extensions: &["tsx"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 2,
    },
    Recipe {
        name: "java",
        mode: "java-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-java",
        revision: None,
        source_dir: None,
        extensions: &["java"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "cpp",
        mode: "cpp-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-cpp",
        revision: None,
        source_dir: None,
        extensions: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "c-sharp",
        mode: "csharp-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-c-sharp",
        revision: None,
        source_dir: None,
        extensions: &["cs"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "ruby",
        mode: "ruby-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-ruby",
        revision: None,
        source_dir: None,
        extensions: &["rb"],
        file_names: &["Rakefile", "Gemfile"],
        interpreters: &["ruby"],
        line_comment: Some("#"),
        indent_width: 2,
    },
    Recipe {
        name: "php",
        mode: "php-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-php",
        revision: None,
        source_dir: Some("php"),
        extensions: &["php"],
        file_names: &[],
        interpreters: &["php"],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "swift",
        mode: "swift-mode",
        repository: "https://github.com/alex-pinkus/tree-sitter-swift",
        revision: Some("with-generated-files"),
        source_dir: None,
        extensions: &["swift"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "kotlin",
        mode: "kotlin-mode",
        repository: "https://github.com/fwcd/tree-sitter-kotlin",
        revision: None,
        source_dir: None,
        extensions: &["kt", "kts"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "scala",
        mode: "scala-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-scala",
        revision: None,
        source_dir: None,
        extensions: &["scala", "sc"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 2,
    },
    Recipe {
        name: "haskell",
        mode: "haskell-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-haskell",
        revision: None,
        source_dir: None,
        extensions: &["hs"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("--"),
        indent_width: 2,
    },
    Recipe {
        name: "ocaml",
        mode: "ocaml-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-ocaml",
        revision: None,
        source_dir: Some("grammars/ocaml"),
        extensions: &["ml"],
        file_names: &[],
        interpreters: &[],
        line_comment: None,
        indent_width: 2,
    },
    Recipe {
        name: "elixir",
        mode: "elixir-mode",
        repository: "https://github.com/elixir-lang/tree-sitter-elixir",
        revision: None,
        source_dir: None,
        extensions: &["ex", "exs"],
        file_names: &[],
        interpreters: &["elixir"],
        line_comment: Some("#"),
        indent_width: 2,
    },
    Recipe {
        name: "erlang",
        mode: "erlang-mode",
        repository: "https://github.com/WhatsApp/tree-sitter-erlang",
        revision: None,
        source_dir: None,
        extensions: &["erl", "hrl"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("%"),
        indent_width: 4,
    },
    Recipe {
        name: "zig",
        mode: "zig-mode",
        repository: "https://github.com/tree-sitter-grammars/tree-sitter-zig",
        revision: None,
        source_dir: None,
        extensions: &["zig"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 4,
    },
    Recipe {
        name: "dart",
        mode: "dart-mode",
        repository: "https://github.com/UserNobody14/tree-sitter-dart",
        revision: None,
        source_dir: None,
        extensions: &["dart"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("//"),
        indent_width: 2,
    },
    Recipe {
        name: "css",
        mode: "css-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-css",
        revision: None,
        source_dir: None,
        extensions: &["css"],
        file_names: &[],
        interpreters: &[],
        line_comment: None,
        indent_width: 2,
    },
    Recipe {
        name: "html",
        mode: "html-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-html",
        revision: None,
        source_dir: None,
        extensions: &["html", "htm"],
        file_names: &[],
        interpreters: &[],
        line_comment: None,
        indent_width: 2,
    },
    Recipe {
        name: "yaml",
        mode: "yaml-mode",
        repository: "https://github.com/tree-sitter-grammars/tree-sitter-yaml",
        revision: None,
        source_dir: None,
        extensions: &["yaml", "yml"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("#"),
        indent_width: 2,
    },
    Recipe {
        name: "dockerfile",
        mode: "dockerfile-mode",
        repository: "https://github.com/camdencheek/tree-sitter-dockerfile",
        revision: None,
        source_dir: None,
        extensions: &["dockerfile"],
        file_names: &["Dockerfile", "Containerfile"],
        interpreters: &[],
        line_comment: Some("#"),
        indent_width: 4,
    },
    Recipe {
        name: "nix",
        mode: "nix-mode",
        repository: "https://github.com/nix-community/tree-sitter-nix",
        revision: None,
        source_dir: None,
        extensions: &["nix"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("#"),
        indent_width: 2,
    },
    Recipe {
        name: "sql",
        mode: "sql-mode",
        repository: "https://github.com/m-novikov/tree-sitter-sql",
        revision: None,
        source_dir: None,
        extensions: &["sql"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some("--"),
        indent_width: 2,
    },
    Recipe {
        name: "julia",
        mode: "julia-mode",
        repository: "https://github.com/tree-sitter/tree-sitter-julia",
        revision: None,
        source_dir: None,
        extensions: &["jl"],
        file_names: &[],
        interpreters: &["julia"],
        line_comment: Some("#"),
        indent_width: 4,
    },
    Recipe {
        name: "r",
        mode: "r-mode",
        repository: "https://github.com/r-lib/tree-sitter-r",
        revision: None,
        source_dir: None,
        extensions: &["r", "R"],
        file_names: &[],
        interpreters: &["Rscript"],
        line_comment: Some("#"),
        indent_width: 2,
    },
    Recipe {
        name: "perl",
        mode: "perl-mode",
        repository: "https://github.com/tree-sitter-perl/tree-sitter-perl",
        revision: Some("release"),
        source_dir: None,
        extensions: &["pl", "pm"],
        file_names: &[],
        interpreters: &["perl"],
        line_comment: Some("#"),
        indent_width: 4,
    },
    Recipe {
        name: "clojure",
        mode: "clojure-mode",
        repository: "https://github.com/sogaiu/tree-sitter-clojure",
        revision: None,
        source_dir: None,
        extensions: &["clj", "cljs", "cljc", "edn"],
        file_names: &[],
        interpreters: &[],
        line_comment: Some(";"),
        indent_width: 2,
    },
    Recipe {
        name: "make",
        mode: "makefile-mode",
        repository: "https://github.com/alemuller/tree-sitter-make",
        revision: None,
        source_dir: None,
        extensions: &["mk", "mak"],
        file_names: &["Makefile", "makefile", "GNUmakefile"],
        interpreters: &[],
        line_comment: Some("#"),
        indent_width: 4,
    },
];

pub fn recipe(name: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| r.name == name)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledGrammar {
    pub name: String,
    pub mode: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub file_names: Vec<String>,
    #[serde(default)]
    pub interpreters: Vec<String>,
    pub library: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default)]
    pub highlights: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default = "default_indent_width")]
    pub indent_width: usize,
    #[serde(default)]
    pub indent_tabs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_comment: Option<String>,
    #[serde(default = "default_pairs")]
    pub pairs: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

fn default_indent_width() -> usize {
    4
}

fn default_pairs() -> String {
    syntax::DEFAULT_PAIRS.to_string()
}

impl InstalledGrammar {
    pub fn to_dynamic(&self) -> DynamicGrammar {
        DynamicGrammar {
            name: self.name.clone(),
            mode: self.mode.clone(),
            extensions: self.extensions.clone(),
            file_names: self.file_names.clone(),
            interpreters: self.interpreters.clone(),
            library: self.library.clone(),
            symbol: self.symbol.clone(),
            highlights: self.highlights.clone(),
            tags: self.tags.clone(),
            indent_width: self.indent_width,
            indent_tabs: self.indent_tabs,
            line_comment: self.line_comment.clone(),
            pairs: self.pairs.clone(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default, rename = "grammar")]
    grammar: Vec<InstalledGrammar>,
}

pub fn load_from(dir: &Path) -> Result<Vec<InstalledGrammar>, String> {
    let file = dir.join(MANIFEST);
    if !file.is_file() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(manifest.grammar)
}

pub fn save_to(dir: &Path, grammars: &[InstalledGrammar]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let text = toml::to_string_pretty(&Manifest {
        grammar: grammars.to_vec(),
    })
    .map_err(|e| format!("writing {MANIFEST}: {e}"))?;
    let file = dir.join(MANIFEST);
    let tmp = dir.join(format!(".{MANIFEST}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("writing {}: {e}", file.display()))?;
    Ok(())
}

pub fn load_installed() -> Result<Vec<InstalledGrammar>, String> {
    let Some(dir) = crate::config::grammar_dir() else {
        return Ok(Vec::new());
    };
    load_from(&dir)
}

pub fn save_installed(grammars: &[InstalledGrammar]) -> Result<(), String> {
    let dir = crate::config::grammar_dir().ok_or("no data directory")?;
    save_to(&dir, grammars)
}

pub fn register_into(editor: &mut crate::Editor, grammar: &InstalledGrammar) -> Result<(), String> {
    if syntax::language(&grammar.name).is_some() {
        return Ok(());
    }
    syntax::register_grammar(grammar.to_dynamic())?;
    if editor.modes.get(&grammar.mode).is_none() {
        editor
            .modes
            .define_major(
                &grammar.mode,
                grammar.parent.as_deref(),
                &grammar.extensions,
                Some(OWNER),
            )
            .map_err(|e| format!("{}: {e}", grammar.mode))?;
        if let Some(parent) = grammar.parent.as_deref() {
            editor.keymaps.set_mode_parent(&grammar.mode, parent);
        }
    }
    Ok(())
}

pub fn load_all(editor: &mut crate::Editor) -> Vec<String> {
    let installed = match load_installed() {
        Ok(v) => v,
        Err(e) => return vec![e],
    };
    let mut errors = Vec::new();
    for grammar in installed {
        if let Err(e) = register_into(editor, &grammar) {
            errors.push(format!("{}: {e}", grammar.name));
        }
    }
    errors
}

pub fn install<F>(recipe: &Recipe, progress: &mut F) -> Result<InstalledGrammar, String>
where
    F: FnMut(&str),
{
    // Google Play doesn't allow an app to download or build native code and
    // load it (Device and Network Abuse). The common grammars are compiled in.
    let dir = crate::config::grammar_dir().ok_or("no data directory")?;
    let src_root = dir.join("src");
    let src = src_root.join(recipe.name);
    if src.exists() {
        std::fs::remove_dir_all(&src).map_err(|e| format!("removing {}: {e}", src.display()))?;
    }
    std::fs::create_dir_all(&src_root)
        .map_err(|e| format!("creating {}: {e}", src_root.display()))?;

    progress(&format!("Cloning {}...", recipe.repository));
    let mut clone = Command::new("git");
    clone
        .args(["clone", "--quiet", "--depth", "1"])
        .arg(recipe.repository)
        .arg(&src);
    run(clone, "git clone", progress)?;

    if let Some(revision) = recipe.revision {
        progress(&format!("Checking out {revision}..."));
        let mut fetch = Command::new("git");
        fetch
            .arg("-C")
            .arg(&src)
            .args(["fetch", "--quiet", "--depth", "1", "origin", revision]);
        run(fetch, "git fetch", progress)?;
        let mut checkout = Command::new("git");
        checkout
            .arg("-C")
            .arg(&src)
            .args(["checkout", "--quiet", "FETCH_HEAD"]);
        run(checkout, "git checkout", progress)?;
    }

    let revision = run(
        {
            let mut cmd = Command::new("git");
            cmd.arg("-C").arg(&src).args(["rev-parse", "HEAD"]);
            cmd
        },
        "git rev-parse",
        progress,
    )?
    .trim()
    .to_string();

    let root = recipe
        .source_dir
        .map_or_else(|| src.clone(), |d| src.join(d));
    let parser = root.join("src").join("parser.c");
    if !parser.is_file() {
        return Err(format!("{} has no src/parser.c", root.display()));
    }

    let build = dir.join("build").join(recipe.name);
    if build.exists() {
        std::fs::remove_dir_all(&build)
            .map_err(|e| format!("removing {}: {e}", build.display()))?;
    }
    std::fs::create_dir_all(&build).map_err(|e| format!("creating {}: {e}", build.display()))?;

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_string());
    let include = root.join("src");
    let mut objects = Vec::new();

    let parser_obj = build.join("parser.o");
    compile(&cc, "-std=c11", &include, &parser, &parser_obj, progress)?;
    objects.push(parser_obj);

    let scanner_c = include.join("scanner.c");
    let scanner_cc = include.join("scanner.cc");
    let scanner_cpp = include.join("scanner.cpp");
    let mut cpp = false;
    if scanner_c.is_file() {
        let obj = build.join("scanner.o");
        compile(&cc, "-std=c11", &include, &scanner_c, &obj, progress)?;
        objects.push(obj);
    } else if scanner_cc.is_file() || scanner_cpp.is_file() {
        let source = if scanner_cc.is_file() {
            scanner_cc
        } else {
            scanner_cpp
        };
        let obj = build.join("scanner.o");
        compile(&cxx, "-std=c++17", &include, &source, &obj, progress)?;
        objects.push(obj);
        cpp = true;
    }

    let libdir = dir.join("lib");
    std::fs::create_dir_all(&libdir).map_err(|e| format!("creating {}: {e}", libdir.display()))?;
    let lib_name = library_name(recipe.name);
    let built = build.join(&lib_name);
    let linker = if cpp { &cxx } else { &cc };
    let mut link = Command::new(linker);
    if cfg!(target_os = "macos") {
        link.arg("-dynamiclib");
    } else {
        link.arg("-shared");
    }
    link.arg("-o").arg(&built).args(&objects);
    run(link, "linking grammar", progress)?;

    let library = libdir.join(&lib_name);
    std::fs::copy(&built, &library).map_err(|e| format!("copying {}: {e}", library.display()))?;

    let grammar = InstalledGrammar {
        name: recipe.name.to_string(),
        mode: recipe.mode.to_string(),
        extensions: recipe.extensions.iter().map(|s| s.to_string()).collect(),
        file_names: recipe.file_names.iter().map(|s| s.to_string()).collect(),
        interpreters: recipe.interpreters.iter().map(|s| s.to_string()).collect(),
        library,
        symbol: None,
        highlights: read_query(&src, "highlights"),
        tags: read_query(&src, "tags"),
        indent_width: recipe.indent_width,
        indent_tabs: false,
        line_comment: recipe.line_comment.map(str::to_string),
        pairs: syntax::DEFAULT_PAIRS.to_string(),
        parent: syntax::mode_parent(recipe.mode).map(str::to_string),
        revision: Some(revision),
    };

    let mut installed = load_from(&dir)?;
    installed.retain(|g| g.name != grammar.name);
    installed.push(grammar.clone());
    installed.sort_by(|a, b| a.name.cmp(&b.name));
    save_to(&dir, &installed)?;
    Ok(grammar)
}

pub fn library_name(name: &str) -> String {
    let base = format!("tree-sitter-{name}");
    if cfg!(target_os = "macos") {
        format!("lib{base}.dylib")
    } else if cfg!(target_os = "windows") {
        format!("{base}.dll")
    } else {
        format!("lib{base}.so")
    }
}

fn compile<F>(
    program: &str,
    standard: &str,
    include: &Path,
    source: &Path,
    output: &Path,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut cmd = Command::new(program);
    cmd.arg(standard)
        .arg("-O2")
        .arg("-fPIC")
        .arg("-I")
        .arg(include)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(output);
    run(cmd, &format!("compiling {}", source.display()), progress)?;
    Ok(())
}

fn run<F>(mut cmd: Command, label: &str, progress: &mut F) -> Result<String, String>
where
    F: FnMut(&str),
{
    progress(&format!("$ {cmd:?}"));
    let out = cmd.output().map_err(|e| format!("{label}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !stdout.trim().is_empty() {
        progress(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        progress(stderr.trim_end());
    }
    if !out.status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(format!("{label} failed ({}): {detail}", out.status));
    }
    Ok(stdout)
}

fn read_query(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join("queries").join(format!("{name}.scm"))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstalledGrammar {
        InstalledGrammar {
            name: "demo".into(),
            mode: "demo-mode".into(),
            extensions: vec!["demo".into()],
            file_names: vec!["DemoFile".into()],
            interpreters: vec!["demo".into()],
            library: PathBuf::from("/tmp/libtree-sitter-demo.so"),
            symbol: None,
            highlights: "(identifier) @variable".into(),
            tags: String::new(),
            indent_width: 2,
            indent_tabs: false,
            line_comment: Some("#".into()),
            pairs: syntax::DEFAULT_PAIRS.into(),
            parent: Some("prog-mode".into()),
            revision: Some("abc123".into()),
        }
    }

    #[test]
    fn recipes_have_unique_names_and_modes() {
        let mut names = std::collections::BTreeSet::new();
        let mut modes = std::collections::BTreeSet::new();
        for recipe in RECIPES {
            assert!(
                names.insert(recipe.name),
                "duplicate recipe {}",
                recipe.name
            );
            assert!(modes.insert(recipe.mode), "duplicate mode {}", recipe.mode);
            assert!(recipe.repository.starts_with("https://"));
            assert!(recipe.mode.ends_with("-mode"));
        }
    }

    #[test]
    fn manifest_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "forge-grammar-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let grammar = sample();
        save_to(&dir, std::slice::from_ref(&grammar)).unwrap();
        assert_eq!(load_from(&dir).unwrap(), vec![grammar]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_manifest_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "forge-grammar-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(load_from(&dir).unwrap().is_empty());
    }

    #[test]
    fn installed_grammar_becomes_a_dynamic_spec() {
        let grammar = sample();
        let dynamic = grammar.to_dynamic();
        assert_eq!(dynamic.name, grammar.name);
        assert_eq!(dynamic.mode, grammar.mode);
        assert_eq!(dynamic.extensions, grammar.extensions);
        assert_eq!(dynamic.library, grammar.library);
        assert_eq!(dynamic.indent_width, grammar.indent_width);
    }

    #[test]
    fn library_names_match_the_platform() {
        let name = library_name("demo");
        if cfg!(target_os = "macos") {
            assert_eq!(name, "libtree-sitter-demo.dylib");
        } else if cfg!(target_os = "windows") {
            assert_eq!(name, "tree-sitter-demo.dll");
        } else {
            assert_eq!(name, "libtree-sitter-demo.so");
        }
    }
}
