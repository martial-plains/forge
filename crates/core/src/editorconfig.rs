// SPDX-License-Identifier: MIT OR Apache-2.0
//! .editorconfig (<https://editorconfig.org>), built into Forge 30: files in
//! a project say how they're indented, how long lines are, and what
//! happens on save. Forge reads every .editorconfig from the file's
//! directory up to the root (or one that says `root = true`); closer
//! files win.

use crate::buffer::Buffer;
use std::path::{Path, PathBuf};

/// The properties that apply to `file`, lower-cased, in order (later
/// ones win).
pub fn properties(file: &Path) -> Vec<(String, String)> {
    let mut configs: Vec<(PathBuf, String)> = Vec::new();
    let mut dir = file.parent();
    while let Some(d) = dir {
        let p = d.join(".editorconfig");
        if let Ok(text) = std::fs::read_to_string(&p) {
            let root = is_root(&text);
            configs.push((d.to_path_buf(), text));
            if root {
                break;
            }
        }
        dir = d.parent();
    }
    let mut out = Vec::new();
    for (dir, text) in configs.iter().rev() {
        let Ok(rel) = file.strip_prefix(dir) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let mut active = false;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                active = glob_matches(glob, &rel);
            } else if active && let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_lowercase(), v.trim().to_lowercase()));
            }
        }
    }
    out
}

fn is_root(text: &str) -> bool {
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            return false;
        }
        if let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case("root")
            && v.trim().eq_ignore_ascii_case("true")
        {
            return true;
        }
    }
    false
}

/// An EditorConfig glob as a regular expression. Without a `/` it matches
/// a file name in any directory.
fn glob_regex(glob: &str) -> String {
    let (anchored, glob) = match glob.strip_prefix('/') {
        Some(g) => (true, g),
        None => (glob.contains('/'), glob),
    };
    let mut re = String::from(if anchored { "^" } else { "^(?:.*/)?" });
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    let mut braces = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                re.push_str(".*");
                i += 1;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '[' => match chars[i..].iter().position(|&c| c == ']') {
                Some(end) => {
                    let class: String = chars[i + 1..i + end].iter().collect();
                    let class = class
                        .strip_prefix('!')
                        .map(|c| format!("^{c}"))
                        .unwrap_or(class);
                    re.push_str(&format!("[{}]", class.replace('\\', "\\\\")));
                    i += end;
                }
                None => re.push_str("\\["),
            },
            '{' => {
                // {1..3}: a number (the range itself isn't checked).
                let end = chars[i..].iter().position(|&c| c == '}').map(|e| i + e);
                let inner: Option<String> = end.map(|e| chars[i + 1..e].iter().collect());
                if let (Some(e), Some(inner)) = (end, &inner)
                    && inner.contains("..")
                    && !inner.contains(',')
                {
                    re.push_str("-?[0-9]+");
                    i = e + 1;
                    continue;
                }
                braces += 1;
                re.push_str("(?:");
            }
            ',' if braces > 0 => re.push('|'),
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            '\\' if i + 1 < chars.len() => {
                re.push_str(&regex::escape(&chars[i + 1].to_string()));
                i += 1;
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    re
}

fn glob_matches(glob: &str, path: &str) -> bool {
    regex::Regex::new(&glob_regex(glob)).is_ok_and(|re| re.is_match(path))
}

/// Set a buffer's options from the .editorconfig files that apply to its
/// file. Returns what was applied, for messages.
pub fn apply(buffer: &mut Buffer, file: &Path) -> Vec<String> {
    let props = properties(file);
    let get = |k: &str| {
        props
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    let mut applied = Vec::new();
    let mut set = |b: &mut Buffer, name: &str, value: &str| {
        if b.set_option(name, value).is_ok() {
            applied.push(format!("{name}={value}"));
        }
    };
    match get("indent_style") {
        Some("tab") => set(buffer, "indent-tabs", "true"),
        Some("space") => set(buffer, "indent-tabs", "false"),
        _ => {}
    }
    let size = match get("indent_size") {
        Some("tab") => get("tab_width"),
        s => s,
    };
    if let Some(n) = size.filter(|s| s.parse::<usize>().is_ok()) {
        set(buffer, "indent-width", n);
    }
    if let Some(n) = get("max_line_length").filter(|s| s.parse::<usize>().is_ok()) {
        set(buffer, "fill-column", n);
    }
    for (key, option) in [
        ("trim_trailing_whitespace", "trim-trailing-whitespace"),
        ("insert_final_newline", "insert-final-newline"),
    ] {
        if let Some(v @ ("true" | "false")) = get(key) {
            set(buffer, option, v);
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let m = glob_matches;
        assert!(m("*", "a.rs") && m("*", "src/a.rs"));
        assert!(m("*.rs", "src/deep/a.rs") && !m("*.rs", "a.rsx"));
        assert!(m("*.{js,ts}", "x/y.ts") && !m("*.{js,ts}", "y.py"));
        assert!(m("Makefile", "sub/Makefile"));
        assert!(m("lib/**.js", "lib/a/b.js") && !m("lib/**.js", "src/lib/a.js"));
        assert!(m("/top.txt", "top.txt") && !m("/top.txt", "a/top.txt"));
        assert!(m("file[0-9].txt", "file3.txt") && !m("file[!0-9].txt", "file3.txt"));
        assert!(m("v{1..3}.md", "v2.md"));
    }

    #[test]
    fn nearest_file_wins_and_root_stops() {
        let d = std::env::temp_dir().join(format!("forge-ec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj/src")).unwrap();
        std::fs::write(d.join(".editorconfig"), "[*]\nindent_size = 8\n").unwrap();
        std::fs::write(
            d.join("proj/.editorconfig"),
            "root = true\n[*]\nindent_style = space\nindent_size = 4\n[*.go]\nindent_style = tab\nindent_size = tab\ntab_width = 8\n",
        )
        .unwrap();
        std::fs::write(
            d.join("proj/src/.editorconfig"),
            "[*.py]\nindent_size = 2\nmax_line_length = 88\ntrim_trailing_whitespace = true\n",
        )
        .unwrap();

        let mut b = Buffer::new("a.py");
        let applied = apply(&mut b, &d.join("proj/src/a.py"));
        assert_eq!(
            applied,
            [
                "indent-tabs=false",
                "indent-width=2",
                "fill-column=88",
                "trim-trailing-whitespace=true"
            ]
        );
        let mut g = Buffer::new("m.go");
        apply(&mut g, &d.join("proj/m.go"));
        assert_eq!(
            (g.options.indent_tabs, g.options.indent_width),
            (Some(true), Some(8))
        );
        // root = true in proj/: the 8 from above it never applies.
        let mut r = Buffer::new("r.rs");
        apply(&mut r, &d.join("proj/r.rs"));
        assert_eq!(r.options.indent_width, Some(4));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
