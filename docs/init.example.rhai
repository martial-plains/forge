// Forge configuration. Copy to ~/.config/forge/init.rhai
// (or $XDG_CONFIG_HOME/forge/init.rhai, or $FORGE_CONFIG_DIR/init.rhai).
// Run it again without restarting: M-x reload-init-file
// Try a line without editing this file: M-: (eval-expression)
// Start without it:                forge -q
//
// This is Rhai. There is no file or OS access: files go through forge.fs,
// programs through forge.process, so nothing blocks the editor. print()
// goes to *Messages*. A call that runs too long is stopped.
// Keys use key notation: C- is Ctrl, M- is Alt/Meta (A- too),
// s-super- is Super (Cmd- too), sequences are
// separated by spaces. Named keys: RET SPC TAB DEL ESC <left> <f5> ...
//
// The API is namespaced like Neovim's: forge.opt, forge.keymap,
// forge.command, forge.ui, forge.minibuffer, forge.buffer, forge.mode, forge.plugin,
// forge.process, forge.fs, forge.http, forge.env, forge.system, forge.menu,
// forge.theme, forge.log. See docs/plugin-api.md for the whole list.
// forge.env is the process environment: what you set is what
// forge.process.run starts programs with.
// forge.system names the machine: forge.system.type(), .name(), .pid().

// ---- options

// Seconds to wait after a prefix key (C-x, M-g, ...) before listing the
// keys that can follow it. 0 turns this off; C-h after a prefix still works.
forge.opt["prefix_help_delay"] = 1.0;

// Let Forge handle the mouse (wheel scrolls, click moves point, drag
// selects). Most terminals still select text with Shift+drag.
forge.opt["mouse"] = true;

// Reload a plugin by itself when its .wasm file is rebuilt.
forge.opt["plugin_auto_reload"] = true;

// How many lines the *Messages* buffer keeps:
// true for no limit, false to stop recording, or a number of lines (the
// default is 100).
// forge.opt["message_log_max"] = 100;

// Keep messages out of the echo area; they still go to *Messages*.
// forge.opt["inhibit_message"] = false;

// Line numbers in every buffer that doesn't choose: "off", "absolute" or
// "relative" (the current line's number, distances for the others).
// forge.opt["line_numbers"] = "relative";

// RET indents the new line and a closing bracket re-indents its line.
forge.opt["electric_indent"] = true;

// Built-in modes, like (each also has an M-x command):
// forge.opt["wrap"] = "word";                  // visual-line-mode everywhere
// forge.opt["hl_line"] = true;                 // highlight the current line
// forge.opt["electric_pair"] = true;           // ( inserts ()
// forge.opt["show_trailing_whitespace"] = true;
// forge.opt["fill_column_indicator"] = true;
// forge.opt["show_paren"] = false;             // on by default
// forge.opt["save_place"] = false;             // files open where you left them
// forge.opt["editorconfig"] = false;           // .editorconfig is read by default
// forge.opt["menu_bar"] = true;                // draw the menu bar (menu-bar-mode)
// forge.opt["context_menu"] = true;            // right-click / S-F10 menu (context-menu-mode)
// forge.opt["smooth_scroll"] = true;           // animate scrolling a line at a time
// forge.opt["pixel_scroll_precision"] = true;  // pixel-by-pixel smooth scrolling
// forge.opt["scroll_bar"] = true;              // vertical scroll bars (scroll-bar-mode)
// forge.opt["horizontal_scroll_bar"] = true;   // horizontal scroll bars (horizontal-scroll-bar-mode)
// Where web links in Org open: "auto" (the embedded webkit browser in the app,
// few in a terminal), "webkit", "few" or "external".
// forge.opt["browse_url_browser"] = "external";
// Org publishing (M-x org-publish, org-publish-all, org-publish-current-file):
// one string per project, NAME :key value ...  A project with :components
// publishes those in order.  Only changed files are written (C-u: all).
// forge.opt["org_publish_projects"] = [
//   "notes :base-directory ~/org :publishing-directory ~/public_html :recursive t :exclude \"^private/\" :auto-sitemap t",
//   "static :base-directory ~/org :base-extension \"png|jpg|css\" :recursive t :publishing-directory ~/public_html :publishing-function attachment",
//   "site :components \"notes static\"",
// ];
// Agenda files: a directory stands for every .org file under it, however
// deeply nested (like directory-files-recursively); files can be mixed in.
// forge.opt["org_agenda_files"] = ["~/org/projects", "~/org/areas", "~/org/inbox.org"];
// The span a new agenda shows ("day", "week", "month" or "year"; week by
// default), and how many days ahead deadlines start to show (default 14;
// 0: only on the day).
// forge.opt["org_agenda_span"] = "week";
// forge.opt["org_deadline_warning_days"] = 0;
// Stamp CLOSED: when a task is finished, in every file (org-log-done).
// State changes are always logged in the :LOGBOOK: drawer.
// forge.opt["org_log_done"] = true;
// Rewrite dynamic blocks (clocktable, columnview) when an Org buffer is
// saved, as a before-save-hook on org-update-all-dblocks does.
// forge.opt["org_update_dblocks_on_save"] = true;
// Files and directories searched for id: links that the agenda files don't
// hold (org-id-extra-files). Only the file holding the ID is opened.
// forge.opt["org_id_extra_files"] = ["~/org"];
// The search engine for webkit and few: duckduckgo (the default), google,
// bing, brave, startpage, mojeek, ecosia, kagi, wikipedia, github, or an
// address with %s where the words go. M-x set-search-engine sets it for
// the session.
// forge.opt["search_engine"] = "brave";
// forge.opt["webkit_search_engine"] = "https://kagi.com/search?q=%s";  // webkit only
// forge.opt["few_search_engine"] = "mojeek";                          // few only

// ---- key bindings

// C-c followed by a letter is reserved for you: modes and plugins never use it.
forge.keymap.set("C-c g", "goto-line");
forge.keymap.set("C-c b", "list-buffers");

// Bind to "undefined" to switch a key off, or remove a binding from a map:
// forge.keymap.set("C-t", "undefined");
// forge.keymap.del("C-x C-b");

// Remapping: every key that runs kill-line runs kill-region instead.
// forge.keymap.remap("kill-line", "kill-region");

// Mode keymaps take priority over the global one in buffers using that
// mode. help-mode and Buffer-menu-mode derive from special-mode.
forge.keymap.set("g", "beginning-of-buffer", #{ mode: "special-mode" });

// A prompt's own map, named by what is asking: find-file, goto-line, m-x,
// dired, and completion-list while the vertical list is up. With no map of
// its own a prompt uses the default one, so this only matters for the keys
// you're changing. A key the map doesn't bind is typed.
forge.keymap.set("C-s", "minibuffer-complete", #{ prompt: "goto-line" });

// A conditional layer: its keys apply only while the condition holds, so
// you can turn some keys on and off without a mode of your own. It sits
// above the modes and below the buffer-local map. A layer with no condition
// is never up, so set one before binding into it. The condition is the
// text Forge prints: region-active, read-only, in-comment, in-string,
// in-git-repo, major-derives-from(MODE), minor-mode-on(MODE), and the
// combinators not(...), all(...) and any(...).
forge.keymap.layer("read-only", "read-only");
forge.keymap.set("C-f", "forward-word", #{ layer: "read-only" });
// forge.keymap.remove_layer("read-only");

// A region map: the keymap of the text point is in, so text that means
// something can have keys of its own without a mode per kind of text. A
// Dired listing decorates each name with "dired-entry", its section
// headers with "dired-section" and its column headers with
// "dired-columns". A click is a key, so this is where you'd take
// <mouse-1> over from visiting the file (C-x 2 opens it in a split).
forge.keymap.set("<mouse-1>", "dired-find-file-other-window", #{ region: "dired-entry" });

// A project's keymap: bindings for every file under a directory, the way
// .dir-locals.el works. A project is the nearest directory above a file
// with a .forge/ or a .git in it, so you can also write `true` for "the
// project of the file being edited" -- which is what a project's own
// .forge/init.rhai wants, since it can't name itself.
forge.keymap.set("C-c b", "project-build", #{ project: "~/src/forge" });

// A key can run a Rhai function directly.
forge.keymap.set("C-c w", || forge.notify(forge.buffer.current().name()), #{ desc: "Show the buffer's name." });

// ---- commands

// Commands are Rhai functions. They get the prefix argument (C-u 3 -> 3,
// plain C-u -> 4, none -> ()). Questions take a function that runs when
// the answer arrives (() if you pressed C-g).
forge.command.create("insert-greeting", |n| {
  forge.ui.input(#{ prompt: "Greet whom? ", "default": "world" }, |name| {
    if name != () {
      let count = if n == () { 1 } else { n };
      for i in 0..count {
        forge.buffer.current().insert("Hello, " + name + "!\n");
      }
    }
  });
}, #{ desc: "Ask for a name and insert a greeting." });
forge.keymap.set("C-c H", "insert-greeting");

// Replace what is typed in the open prompt, leaving the cursor at the end.
// forge.command.create("try-dir", || forge.minibuffer.set_input("~/"));
// forge.minibuffer.suspend() sets the open prompt aside (no answer);
// forge.completion.sessions() lists what was set aside and
// forge.completion.resume(0) brings one back.
//
// The prompt list can be grouped, labelled and laid out:
// forge.completion.set_layout("grid", 8);       // vertical, flat, grid, reverse,
//                                               // unobtrusive or buffer
// forge.completion.set_group_format("-- %s --");
// forge.completion.group("candidate", |names| /* one name per candidate */ names);
// forge.completion.set_row_labels(["a", "s", "d"]);
// forge.completion.jump_to_row(0, true);         // select row 0 and accept it
// forge.minibuffer.on_open("fill-prompt");       // command must already exist

// Positions are (line, column) as the mode line shows them: lines from 1,
// columns from 0.
forge.command.create("duplicate-line", |n| {
  let b = forge.buffer.current();
  let point = b.point();
  let line = point[0];
  let col = point[1];
  let text = b.line(line);
  let count = if n == () { 1 } else { n };
  b.set_point(line, text.len());
  for i in 0..count {
    b.insert("\n" + text);
  }
  b.set_point(line + count, col);
}, #{ desc: "Copy the current line below itself." });
forge.keymap.set("C-c d", "duplicate-line");

forge.command.create("count-rhai-words", || {
  let text = forge.buffer.current().text();
  let words = if text.trim() == "" { 0 } else { text.split().len() };
  forge.notify(words.to_string() + " words");
}, #{ desc: "Show how many words the buffer has (M-= does more)." });
// ---- modes

// A major mode for files ending in .notes, with its own key.
forge.mode.define_major("notes-mode", #{ parent: "fundamental-mode", extensions: ["notes"] });
forge.keymap.set("C-c C-d", "duplicate-line", #{ mode: "notes-mode" });

// A mode derived from a built-in one gets its highlighting and
// indentation: here, Rust in .rsx files, with its own keymap.
// forge.mode.define_major("rsx-mode", #{ parent: "rust-mode", extensions: ["rsx"] });

// ---- grammars

// M-x treesit-install-language-grammar installs one from a built-in
// recipe (TypeScript, Java, Ruby, ...): it clones and builds the grammar,
// loads it, and records it in <data>/tree-sitter/installed.toml so it is
// there again next time. The manual route is forge.grammar.register:

// Load a tree-sitter grammar shared library and give it a major mode.
// This is trusted native code with the editor's full authority; only load
// libraries you trust. The symbol defaults to tree_sitter_<name>, with -
// and . turned into _. `mode` defaults to "<name>-mode"; `parent` makes
// the new mode derive from it, so its keys and hooks apply too. Queries
// can be inline or read from a file (`highlights_file`, `tags_file`).
// forge.grammar.register("zig", "~/grammars/zig.dylib", #{
//   extensions: ["zig"],
//   file_names: [],
//   interpreters: [],
//   mode: "zig-mode",
//   parent: "prog-mode",
//   symbol: "tree_sitter_zig",
//   highlights: "(fn_declaration name: (identifier) @function)",
//   highlights_file: "~/grammars/zig-highlights.scm",
//   tags: "(fn_declaration name: (identifier) @name @definition.function)",
//   tags_file: "~/grammars/zig-tags.scm",
//   indent_width: 4,
//   tabs: false,
//   line_comment: "//",
//   pairs: "()[]{}",
// });

// Mode hooks, as (add-hook 'prog-mode-hook ...): the command (or
// function) runs in each buffer whose mode is or derives from the mode,
// now and when files are opened. A minor mode is turned on, not toggled.
// Plugin commands are fine here: they run once the plugin has loaded.
// forge.mode.hook("prog-mode", "rainbow-delimiters-mode");
// forge.mode.hook("python-mode", || forge.notify("hello, Python"));
// forge.command.run("global-rainbow-delimiters-mode", 1);  // with C-u 1: on

// Events hooks: buffer-opened, buffer-changed (once typing
// pauses), buffer-saved, buffer-killed, mode-changed.
forge.on("buffer-opened", |buf| {
  if buf.major_mode() == "text-mode" {
    buf.set_option("wrap", "word");
  }
});

// Buffer options: line-numbers, wrap, indent-width, indent-tabs, fill-column,
// hl-line, whitespace, display-fill-column-indicator, ...
// (see docs/plugin-api.md).
forge.command.create("two-space-indent", || {
  forge.buffer.current().set_option("indent-width", 2);
}, #{ desc: "Indent this buffer with two spaces per level." });

// ---- plugins

// Plugins in ~/.config/forge/plugins/ load at startup. Permissions granted
// here aren't asked for; a plugin gets these plus any you grant when asked.
// M-x plugin-profile shows how long each plugin spent in the cache, compiling,
// initializing, starting lazily and in each call; forge --profile-startup
// prints the same report from the command line (--json for JSON). This warns
// in *Messages* when plugin startup takes longer than the budget:
// forge.opt["startup_budget_ms"] = 250;
// Review an init.rhai that changed outside Forge before evaluating it. If
// the new file adds a grant, process, network request or writable directory,
// Forge shows the diff and asks; declining behaves like starting with -q.
// forge.opt["init_review"] = true;
// forge.plugin.grant("spell", #{
//   directories: ["~/dict", #{ path: "~/.cache/spell", write: true }],
//   network: false,
//   environment: true,   // let it read the process environment
//   memory_mb: 256,
// });

// A plugin's own settings (its README lists them). Read when it loads; a
// setting it doesn't know stops it loading and names the setting.
// forge.mode.hook("org-mode", "org-modern-mode");
// forge.plugin.config("org-modern", #{
//   star: ["◉", "○", "✸"],
//   progress: false,
// });

// Declare a plugin instead of installing it by hand: one map says where
// it comes from, how it's configured and granted, and (with commands, modes or extensions) that it
// should load lazily. Anything missing is installed at startup, and
// forge-lock.toml (next to this file) records the commit and sha256 each
// one resolved to, so another machine rebuilds itself from these two files.
// Machine-only lock overrides go in the ignored forge-lock.local.toml.
// M-x package-snapshot-save/list/diff/restore manage named lock snapshots,
// including plugins loaded from a custom load_path; M-x package-restore
// reinstalls locked bytes from Forge's local artifact store and removes
// package-managed plugins that the lock does not list. M-x package-log shows
// package fetch/build/install/upgrade/rollback timings, and
// package-log-export writes them as JSON lines.
//
// use_package is forge.plugin.declare plus `bind` and `init`: `bind` binds
// keys (a command no one has defined yet makes its key load the plugin),
// and `init` runs now, before the plugin loads.
use_package("org-modern", #{
  git: "https://github.com/you/forge-org-modern", pin: "v0.2.0",
  config: #{ progress: false },
  hook: #{ "org-mode": "org-modern-mode" },
  bind: #{ "C-c o": "org-modern-mode" },
  grant: #{ network: false },
  commands: ["org-modern-mode"],           // load when first used:
  modes: ["org-mode"],                      // one of these modes is on,
  extensions: ["org"],                      // or such a file opens
  after: ["org"],                          // load after these
  init: || { forge.opt["wrap"] = "word"; },
});
// use_package has no `on` key: `commands`, `modes` and `extensions` are
// the lazy-loading triggers (forge.plugin.declare spells them `on`). The same with an index name (M-x list-packages
// installs it first) or a prebuilt component:
use_package("magit", #{ index: "magit", commands: ["magit-status"] });
// Index declarations can restrict versions; freeze prevents automatic
// upgrades. A `pin` also freezes the package and selects its Git revision.
use_package("theme", #{ index: "theme", version: "^1.2", freeze: true });
use_package("few", #{ url: "https://.../few.wasm", sha256: "..." });
// A plugin already in the plugins directory needs no source at all, only
// what to configure:
use_package("spell-check", #{
  grant: #{ directories: ["/usr/share/dict"] },
});
// forge.plugin.declare does the same without bind and init.

// consult, the way an Forge config would set it up. Every command in
// :bind is a lazy trigger, so the plugin loads on the first key. Forge's
// consult previews live on its own, so there is no :hook, advice or
// consult-customize to port; `consult-info`, `consult-complex-command`,
// `consult-buffer-other-frame` and `consult-register-store` don't exist
// (Forge has one frame per process).
use_package("consult", #{
  index: "consult",
  bind: #{
    "C-s": "consult-line",
    "C-c y": "consult-yank-pop",        "M-y": "consult-yank-pop",
    "C-c M-x": "consult-mode-command",
    "C-c h": "consult-history",
    "C-c k": "consult-kmacro",
    "C-c m": "consult-man",
    "C-c l": "consult-locate",          "M-s D": "consult-locate",
    "C-c b": "consult-buffer",          "C-x C-b": "consult-buffer",
    "C-x 4 b": "consult-buffer-other-window",
    "C-x r b": "consult-bookmark",
    "M-#": "consult-register-load",     "C-M-#": "consult-register",
    "M-g e": "consult-compile-error",   "M-g f": "consult-flymake",
    "M-g g": "consult-goto-line",       "M-g M-g": "consult-goto-line",
    "M-g o": "consult-outline",         "M-g m": "consult-mark",
    "M-g k": "consult-global-mark",     "M-g i": "consult-imenu",
    "M-g I": "consult-imenu-multi",
    "M-s d": "consult-find",            "M-s g": "consult-grep",
    "M-s G": "consult-git-grep",        "M-s r": "consult-ripgrep",
    "M-s l": "consult-line",            "M-s L": "consult-line-multi",
    "M-s k": "consult-keep-lines",      "M-s u": "consult-focus-lines",
    "M-s e": "consult-isearch-history",
  },
  // :map isearch-mode-map and :map minibuffer-local-map
  bind_prompt: #{
    "isearch": #{
      "M-e": "consult-isearch-history", "M-s e": "consult-isearch-history",
      "M-s l": "consult-line",          "M-s L": "consult-line-multi",
    },
    "prompt": #{ "M-s": "consult-history", "M-r": "consult-history" },
  },
});
//
// The rest of use-package, as keys of the same map. (`if` and
// `catch` are Rhai keywords, so quote them: "if", "catch".)
//
//   Whether to use it:
//     disabled: true             skip the declaration
//     when / "if" / unless       a boolean, or a function returning one
//     requires: ["base"]         skip unless these were declared before
//   Code and settings:
//     preface: || { ... }        runs first, before anything else
//     init:    || { ... }        runs before the plugin loads
//     config:  #{ ... }          the plugin's settings, or
//     config:  || { ... }        a function run once the rest is declared
//     custom:  #{ wrap: "word" } forge.opt values
//     "catch": true              log an error from the code above instead
//                                of failing (see use_package_report())
//   Keys, modes, mode line:
//     bind:      #{ "C-c o": "cmd" }
//     bind_mode: #{ "org-mode": #{ "C-c y": "cmd" } }   keys in one mode
//     mode:      #{ yes: "text-mode" }   files ending .yes open in a mode
//     delight:   #{ "tab-bar-mode": " T" }  or "mode" to hide its text
//   Loading:
//     defer is the default for anything with commands/modes/extensions/bind;
//     demand: true loads at startup whatever triggers it has
//   Installing:
//     vc: "https://..." (same as git; pin picks the revision), ensure: false (don't fetch it),
//     ensure_system_package: #{ rg: "ripgrep", tern: "npm i -g tern" }
//         (or "rg", or ["rg", "fd"]) installs what isn't on PATH, in the
//         background: a package name goes to whatever package manager the
//         machine has, a string with spaces runs as given.
// use_package_report() answers what each declaration did, one line each.
// Carried over from Forge under their Rhai names (underscores):
//     pin: "v1"                  the git tag, branch or commit (use_package
//                                has no `rev`; also holds automatic updates)
//     freeze: true               hold this package during automatic upgrades
//     version: "^1.2"            semver requirement for an index package
//     load_path: "~/dev/x"       look for x.wasm there first
//     no_require: true           install it but don't start it unless
//                                something lazy triggers it
//     autoload: ["cmd"]          same as commands
//     diminish: ...              same as delight
//     bind_star: #{ ... }        same as bind (there is no override map)
//     interpreter: #{ node: "js-mode" }   #! programs that pick a mode
//     magic / magic_fallback: #{ "^%PDF": "pdf-mode" }  a regexp on the
//         file's first lines; the fallback is tried after everything else
//     custom_face: #{ "comment": #{ italic: true } }   as forge.theme.set
//     vc: #{ url: "https://...", rev: "main",   as :vc
//            make: "cargo build --release --target wasm32-wasip2 && cp ... \"$OUT\"",
//            lisp_dir: "crates/plugin",           build inside this directory
//            features: ["fast"] }                 cargo features to enable
//         (the first build of a command still asks before running it)

// ---- files

// .gpg files are decrypted when visited and encrypted when saved, to the
// recipients the file already had. New ones use a passphrase unless:
// forge.opt["epa_file_encrypt_to"] = ["me@example.com"];

// forge.fs reads, writes, lists and checks without blocking: each takes a
// function that runs with the answer. forge.buffer opens files.
// forge.fs.read("~/org/agenda.txt", |text, err| {
//   if text != () { forge.notify(text.len().to_string() + " bytes of agenda"); }
// });
// forge.fs.list("~/org", |entries, err| {
//   if entries != () { for entry in entries { print(entry.name); } }
// });
// let notes = forge.buffer.open("~/org/notes.txt");

// forge.load("~/org/meta/other.rhai") runs another Rhai file in
// this session, like (load ...).
// forge.kill("text") puts text on the kill ring and clipboard.
// let b = forge.buffer.open_quiet(path) opens a file without showing it;
// b.set_text(text) replaces the buffer's contents and b.save() writes it.
//
// The `display` text property (Forge's, add_display_text_property(line1, col1,
// line2, col2, spec)) changes how text shows:
//   ["string", "→"]                  show other text instead
//   ["space", #{align_to: 20}]       a blank stretch (width: N, or align_to: COLUMN)
//   ["min-width", 8]                 pad short text to 8 cells
//   ["space-width", 2]               make the spaces twice as wide
//   ["height", 0.8] ["raise", 0.3]   smaller, raised text (macOS app)
//   ["margin", "left-margin", "text"] text in the left margin of the line
//   ["margin", "right-margin", "text"] the same, after the text
//   ["height", 2.0]                  taller text: the rows above stay blank
//   ["height", ["+", 1]]             one font step (1.2x) larger; ["-", 1] smaller
//   ["space", "align-to", [120]]     a width in pixels is written [N]
//   ["left-fringe", "right-arrow"]   a mark in the fringe beside the line
//                                    (also "right-fringe"; bitmap names
//                                    like left-arrow or filled-square, or
//                                    one character)
// Scale a whole buffer's text with C-x C-+ / C-x C-- / C-x C-0
// (text-scale-increase, text-scale-decrease, text-scale-reset).
//   ["slice", 0.0, 0.0, 0.5, 1.0]    part of a picture (pixels, or 0.0-1.0 fractions)

// ---- processes

// forge.process.run(cmd, opts, callback) runs a program without freezing
// the editor; the callback gets { code, stdout, stderr } when it exits.
// forge.process.run("exercism submit", #{ timeout: 60 }, |r| {
//   forge.notify(if r.code == 0 { "submitted" } else { "failed: " + r.stderr });
// });
// PATH and friends come from forge.env, per command from opts.env:
// forge.process.run("make", #{ env: #{ PATH: forge.env.get("HOME") + "/bin" } }, |r| {
// });

// ---- web requests

// forge.http fetches URLs without freezing the editor (Forge makes the
// request, never Rhai): the callback gets { status, headers, body } or ()
// with an error. get(url, headers?, cb) and request(method, url, opts,
// cb) where opts is { headers, body }; cancel(id) gives up on a request.
// forge.http.get("https://example.com/a", |r, err| {
//   if r != () { forge.notify(r.status.to_string() + " " + r.body); } else { forge.notify(err); }
// });
// forge.http.request("POST", "https://api.example.com/x",
//   #{ headers: #{ "Content-Type": "application/json" }, body: "{}" },
//   |r, err| { });

// ---- theme

// forge.theme.set(face, style) restyles a face; unset fields keep the
// frontend's style, so light and dark mode keep working.
// forge.theme.set("comment", #{ fg: "#6a9955", italic: true });
// forge.theme.faces() lists every face; forge.theme.reset() forgets all.

// ---- menus

// forge.menu.define(title, items) adds a menu after the built-ins;
// extend and extend_submenu add to one. Items are { label, command }
// (an empty command is a separator); commands can be Rhai ones.
// forge.menu.extend("Tools", [#{ label: "Greet", command: "insert-greeting" }]);
// forge.menu.extend_submenu("Tools", "Rhai", [#{ label: "Greet", command: "insert-greeting" }]);

// ---- save-time habits ----
// forge.opt["delete_trailing_whitespace_on_save"] = true;
// forge.opt["require_final_newline"] = true;
// forge.opt["make_scripts_executable_on_save"] = true;  // files starting with #!

// ---- Tether (Tramp): open /ssh:me@host:/path, /sudo::/etc/hosts or
// /ssh:jump|ssh:box:/path with C-x C-f; a directory opens as a Dired listing
// in the `ls -l` layout, and d/R/C/+/M work on it.

// ---- windows, buffers and reloading (all off until asked) ----
// forge.opt["winner_mode"] = true;                       // winner-undo / winner-redo
// forge.keymap.set("S-<left>", "windmove-left");         // also -right, -up, -down
// forge.opt["windmove_wrap_around"] = true;
// forge.opt["uniquify_buffer_name_style"] = "post-forward-angle-brackets";
//     // or "forward", "reverse", "post-forward"; uniquify_min_dir_components = 1
// forge.opt["global_auto_revert_mode"] = true;           // also buffers you can't see
// forge.opt["global_auto_revert_non_file_buffers"] = true; // Dired follows its directory
// forge.opt["auto_revert_interval"] = 5;                 // seconds between checks
// forge.opt["vc_follow_symlinks"] = true;                // open the real file of a VC link
// forge.opt["left_margin_width"] = 2;                    // blank columns beside every window's text
// forge.opt["right_margin_width"] = 2;

// --- term: the terminal ---------------------------------------------------
// M-x term runs your login shell ($SHELL) on a real pseudo-terminal. Pick
// another with forge.opt.term_shell (arguments allowed), or ask each time
// with C-u M-x term. C-c C-j is copy mode, C-c C-k goes back to the program.
// forge.opt.term_shell = "/bin/zsh";
// forge.opt.term_scrollback = 5000;
// M-x term-show-integration prints the lines that make your shell report its
// directory (OSC 7) and prompts (OSC 133) so the buffer follows `cd`.

// --- passwords for Tether: ~/.authinfo ---------------------------------------
// Tether reads ~/.authinfo.gpg, ~/.authinfo and ~/.netrc like TRAMP:
//   machine box.example login me port ssh password "secret"
//   machine box.example login root port sudo password "rootsecret"
// (for sudo the machine is the host it runs on). Name other files with:
// forge.opt.auth_source_files = ["~/.authinfo.gpg", "~/.secrets"];

// --- dashboard: the startup screen (the dashboard plugin) ---------------
// Install the dashboard plugin (its own repository), then M-x dashboard-open.
// It opens by itself on *scratch* unless startup is false.
// forge.plugin.config("dashboard", #{
//     title: "Welcome to Forge", banner: "forge", center: true,
//     items: [["recents", 5], ["bookmarks", 5], ["projects", 5], ["agenda", 5], ["registers", 5]],
// });

// ---------------------------------------------------------------- icons
// Icon types, tried in order: `system` is the
// native icon set of icon_platform, `image` a picture, `emoji` a colour
// emoji, `symbol` a one-cell symbol, `text` plain ASCII. The first one the
// display can show is used; the terminal falls through to symbol or text.
// forge.opt.icon_preference = "system emoji symbol text";
//
// Whose native icons `system` means: auto (this machine), macos (SF
// Symbols), windows (Segoe Fluent Icons), or
// linux (freedesktop icon names). The macOS app draws SF Symbols itself;
// the other sets are named by M-x describe-icon for their own frontends.
// forge.opt.icon_platform = "auto";
//
// Change single icons (what M-x customize-icon does for a session):
// forge.opt.icon_overrides = #{
//     "undo": "symbol:⟲",
//     "save": "system:square.and.arrow.down.fill",
//     "open": "image:~/icons/open.png",
// };

// ------------------------------------------------------------- mode line
// What the mode line shows. Each part is clickable: the
// modified flag toggles read-only, the buffer name goes to the previous
// (left) or next (right) buffer, the position opens a line/column menu,
// the major mode its menu, and a minor mode's name its own menu.
// forge.opt.line_number_mode = true;
// forge.opt.column_number_mode = true;
// forge.opt.size_indication_mode = false;

// -------------------------------------------------- canvas, bases, graph
// Plugins that draw on Forge's canvas (the canvas, bases and graph
// plugins, each its own repository). M-x canvas-demo shows what a canvas is.
//
// .canvas files open as boards (Obsidian's JSON Canvas format):
// forge.plugin.config("canvas", #{ auto_view: true, vault: "~/Notes" });
//
// Bases and graph read your notes, so say where they are and let them:
// forge.plugin.config("bases", #{ vault: "~/Notes", column_width: 32 });
// forge.plugin.grant("bases", #{ directories: ["~/Notes"] });
// forge.plugin.config("graph", #{ vault: "~/Notes", depth: 2, tags: false,
//                                 orphans: true, unresolved: false, follow: false });
// forge.plugin.grant("graph", #{ directories: ["~/Notes"] });
