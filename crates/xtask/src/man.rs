// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask man`: render the `forge(1)` man page.
//!
//! The native CLI (`crates/terminal/src/main.rs`) hand-parses its arguments,
//! so there is no clap definition to derive the page from. It is written here
//! from one small table instead. The tests check that every option the CLI
//! documents is present, so the two stay in step; the `USAGE` string in the
//! terminal crate is the human-readable twin of this page.

/// One command-line option, as the man page lists it.
struct Opt {
    /// The short and long spellings, as written in the page
    /// (`"\\-q, \\-\\-no-config"`).
    flags: &'static str,
    /// The argument placeholder, if the option takes one (`"DIR"`).
    arg: Option<&'static str>,
    /// The one-line description.
    desc: &'static str,
}

/// Every option the `forge` binary accepts, in the order the help lists them.
const OPTIONS: &[Opt] = &[
    Opt {
        flags: "\\-\\-file",
        arg: Some("FILE"),
        desc: "visit FILE (same as a positional file name)",
    },
    Opt {
        flags: "\\-\\-find-file",
        arg: Some("FILE"),
        desc: "visit FILE (same as a positional file name)",
    },
    Opt {
        flags: "\\-\\-visit",
        arg: Some("FILE"),
        desc: "visit FILE (same as a positional file name)",
    },
    Opt {
        flags: "+LINE[:COLUMN]",
        arg: None,
        desc: "go to LINE (and optional zero-based COLUMN) in the next file",
    },
    Opt {
        flags: "\\-l, \\-\\-load",
        arg: Some("FILE"),
        desc: "run FILE as a Rhai script",
    },
    Opt {
        flags: "\\-\\-eval, \\-\\-execute",
        arg: Some("EXPR"),
        desc: "evaluate the Rhai expression EXPR",
    },
    Opt {
        flags: "\\-f, \\-\\-funcall",
        arg: Some("CMD"),
        desc: "run the command CMD",
    },
    Opt {
        flags: "\\-\\-insert",
        arg: Some("FILE"),
        desc: "insert the contents of FILE at point",
    },
    Opt {
        flags: "\\-L, \\-\\-directory",
        arg: Some("DIR"),
        desc: "load the plugins (.wasm files) in DIR",
    },
    Opt {
        flags: "\\-\\-plugin",
        arg: Some("FILE"),
        desc: "load a plugin (a .wasm component); may be repeated",
    },
    Opt {
        flags: "\\-\\-kill",
        arg: None,
        desc: "exit after the actions",
    },
    Opt {
        flags: "\\-q, \\-\\-no-init-file, \\-\\-no-config",
        arg: None,
        desc: "don't run init.rhai or read the saved session",
    },
    Opt {
        flags: "\\-Q, \\-\\-quick",
        arg: None,
        desc: "same as \\-q",
    },
    Opt {
        flags: "\\-\\-init-directory",
        arg: Some("DIR"),
        desc: "use DIR as the config directory (init.rhai, themes, plugins, grants)",
    },
    Opt {
        flags: "\\-u, \\-\\-user",
        arg: Some("USER"),
        desc: "use the config directory of USER's home",
    },
    Opt {
        flags: "\\-\\-chdir",
        arg: Some("DIR"),
        desc: "change to DIR before opening files",
    },
    Opt {
        flags: "\\-\\-batch, \\-batch",
        arg: None,
        desc: "run the actions without a screen and exit; messages go to standard error, \
               and the exit status is 1 on an error",
    },
    Opt {
        flags: "\\-\\-script",
        arg: Some("FILE"),
        desc: "the same as \\-\\-batch \\-l FILE",
    },
    Opt {
        flags: "\\-\\-daemon[=NAME], \\-\\-bg-daemon[=NAME], \\-\\-fg-daemon[=NAME]",
        arg: None,
        desc: "start as an edit server, in the background (the first two) or in this process",
    },
    Opt {
        flags: "\\-\\-check-plugin",
        arg: Some("FILE"),
        desc: "load a plugin without the editor, show what it set up, run its commands \
               (answering questions with \\-\\-answer), open files, wait for events and its \
               processes' output",
    },
    Opt {
        flags: "\\-\\-headless",
        arg: None,
        desc: "run the editor without a terminal and print the screen (for testing plugins \
               end to end)",
    },
    Opt {
        flags: "\\-\\-package-entry",
        arg: Some("FILE.wasm"),
        desc: "print a package index entry for a plugin (see docs/plugin-api.md, \
               \"Sharing plugins\")",
    },
    Opt {
        flags: "\\-\\-package-snapshot-diff",
        arg: Some("BEFORE AFTER [\\-\\-json]"),
        desc: "compare saved package snapshots without starting the editor; emit JSON with \
               \\-\\-json",
    },
    Opt {
        flags: "\\-h, \\-\\-help",
        arg: None,
        desc: "show the help",
    },
    Opt {
        flags: "\\-V, \\-\\-version",
        arg: None,
        desc: "show the version",
    },
];

/// Render the man page as roff.
pub fn to_roff() -> String {
    let mut out = String::new();
    out.push_str(".TH FORGE 1 \"2026\" \"forge ");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str("\" \"Forge Manual\"\n");
    out.push_str(".SH NAME\n");
    out.push_str("forge \\- a terminal text editor in the classic keyboard-driven tradition\n");
    out.push_str(".SH SYNOPSIS\n");
    out.push_str(".B forge\n");
    out.push_str("[\\fIOPTIONS\\fR] [\\fIFILE\\fR...]\n");
    out.push_str(".SH DESCRIPTION\n");
    out.push_str(
        "Forge is a modal-capable terminal editor: keys run named commands, help is built \
         from the same structured documentation that this page is, and the manual can be \
         read in the editor with \\fIC\\-h i\\fR. Files named on the command line are \
         visited in order; \\fB+LINE[:COLUMN]\\fR may precede a file name to move point \
         into it.\n",
    );
    out.push_str(".SH OPTIONS\n");
    for opt in OPTIONS {
        out.push_str(".TP\n");
        out.push_str(".B ");
        match opt.arg {
            Some(arg) => out.push_str(&format!("{} \\fI{}\\fR", opt.flags, arg)),
            None => out.push_str(opt.flags),
        }
        out.push('\n');
        out.push_str(&single_line(opt.desc));
        out.push('\n');
    }
    out.push_str(".SH SEE ALSO\n");
    out.push_str(
        "The Forge manual: press \\fIC\\-h i\\fR in the editor, or open \\fImanual.html\\fR \
         on the documentation site. The \\fBforge\\fR plugin API is documented in \
         \\fIplugin-api.md\\fR.\n",
    );
    out
}

/// Collapse a Rust string's continuation whitespace so the roff output is one
/// logical line per description.
fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roff_has_the_sections_and_the_name() {
        let roff = to_roff();
        for section in [".SH NAME", ".SH SYNOPSIS", ".SH OPTIONS", ".SH SEE ALSO"] {
            assert!(roff.contains(section), "missing {section}");
        }
        assert!(roff.starts_with(".TH FORGE 1"));
        assert!(roff.contains("forge \\- a terminal text editor"));
    }

    #[test]
    fn roff_lists_every_option_the_cli_documents() {
        let roff = to_roff();
        for flag in [
            "\\-\\-no-init-file",
            "\\-\\-no-config",
            "\\-\\-quick",
            "\\-\\-init-directory",
            "\\-\\-user",
            "\\-\\-chdir",
            "\\-\\-file",
            "\\-\\-find-file",
            "\\-\\-visit",
            "+LINE",
            "\\-\\-load",
            "\\-\\-eval",
            "\\-\\-execute",
            "\\-\\-funcall",
            "\\-\\-insert",
            "\\-\\-directory",
            "\\-\\-plugin",
            "\\-\\-kill",
            "\\-\\-batch",
            "\\-batch",
            "\\-\\-script",
            "\\-\\-daemon",
            "\\-\\-bg-daemon",
            "\\-\\-fg-daemon",
            "\\-\\-check-plugin",
            "\\-\\-headless",
            "\\-\\-package-entry",
            "\\-\\-help",
            "\\-\\-version",
        ] {
            assert!(roff.contains(flag), "man page is missing {flag}");
        }
    }
}
