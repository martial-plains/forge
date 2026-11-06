# Forge

A keyboard-driven text editor in the classic tradition. Everything you can do
at the keyboard is a named command, so commands can be bound to keys, run by
name with `M-x`, documented, and remapped. Forge is configured in Rhai and
extended by WebAssembly plugins. It runs in a terminal and in native apps for
macOS, GNOME and Windows.

The [manual](docs/manual/forge.org) (also `C-h i` in the editor) explains the
editor in depth. The [plugin API](docs/plugin-api.md), the
[command line](docs/command-line.md), the [tutorial](docs/tutorial.org) and
the [example init.rhai](docs/init.example.rhai) have their own pages.

## Build and run

Forge needs Rust 1.85 or newer (the workspace uses Rust 2024), installed from
<https://rustup.rs>.

```sh
cargo run --release -p forge-terminal --bin forge      # terminal editor
cargo run --release -p forge-terminal --bin forge -- notes.txt
cargo test                                             # test suite
cargo build --workspace                                # every Rust crate
cargo run --release -p forge-terminal --bin forge --features gpu
sdks/test.sh                                           # build each SDK example
```

`forge-terminal` has two binaries, `forge` and `forgeclient`, so `--bin` picks
the editor. A bare `cargo build` or `cargo test` covers the terminal editor
and what it needs. The GUI cores (`forge-ffi`, `forge-capi`) and the wgpu
canvas renderer (`forge-canvas-gpu`) are built when asked for, with
`--workspace` or the `gpu` feature.

Building a Rust plugin needs the `wasm32-wasip2` target; see [Plugins](#plugins).

## Command line

```sh
forge notes.txt
forge +42:8 --file src/main.rs     # line 42, column 8 (zero-based)
forge -q                           # no init.rhai or saved session
forge --daemon=work                # a background edit server named "work"
forgeclient -s work -n +10 notes.org
```

`forge --help` lists every option, and [docs/command-line.md](docs/command-line.md)
explains the server and `forgeclient`. Options run in the order given, after
the named files are visited. Forge has one frame per process, so
`forgeclient` opens files in the running editor rather than starting a second
one.

The macOS app accepts the same file and location arguments when launched
directly, and opens files sent to it by macOS's Open command.

## Keys

`C-` is Ctrl, `M-` is Alt (or press ESC first), `S-`/`s-` is Shift, and named
keys are written `RET`, `SPC`, `TAB`, `DEL`, `<f10>`, `<mouse-1>` and so on.
Press `C-h b` inside Forge for every active binding. `C-h k` followed by a key
shows what it runs.

| Keys                 | Command                      |
|----------------------|------------------------------|
| C-x C-c              | Quit (asks if unsaved)       |
| C-x C-f / C-x C-s    | Find file / save             |
| C-x C-w              | Save as                      |
| C-x b / C-x k        | Switch / kill buffer         |
| C-x C-b              | List buffers                 |
| C-f C-b C-n C-p      | Char / line motion           |
| M-f M-b              | Word motion                  |
| C-a C-e              | Line start / end             |
| M-< M->              | Buffer start / end           |
| C-v M-v              | Page down / up               |
| C-l                  | Recenter (repeat to cycle)   |
| M-g g                | Go to line                   |
| C-d, DEL             | Delete forward / backward    |
| C-k                  | Kill line (repeat to append) |
| M-d, M-DEL           | Kill word forward / backward |
| C-SPC                | Set mark (start region)      |
| C-w / M-w            | Kill / copy region           |
| C-y / M-y            | Yank / cycle older kills     |
| C-x h, C-x C-x       | Select all, swap point/mark  |
| C-/ or C-_ or C-x u  | Undo                         |
| C-M-_                | Redo                         |
| C-s / C-r            | Incremental search           |
| M-u M-l M-c          | Upcase/downcase/capitalize   |
| C-t, C-o             | Transpose chars, open line   |
| M-x                  | Run a command by name (TAB completes) |
| C-g                  | Cancel                       |

Tab completion also works for file names and buffer names in prompts.

## More editing

| Keys                  | Command                                        |
|-----------------------|------------------------------------------------|
| C-u, M-0..M-9, M--    | Prefix argument: C-u 8 C-f, M-3 C-k, C-u C-y    |
| C-x ( C-x ) C-x e     | Record / end / run a keyboard macro (e repeats) |
| M-% / C-M-%           | Query replace (y n ! . , q) / regexp version   |
| C-x r k / y / t / o   | Kill / yank / string / open rectangle          |
| C-x r m / b           | Set / jump to a bookmark (kept between sessions) |
| C-x r s / i           | Copy region to / insert from a register        |
| C-x r SPC / j         | Save point to / jump to a register             |
| M-q, C-x f            | Fill paragraph (numeric arg justifies), set fill column |
| M-x fill-region, fill-region-as-paragraph | Fill the region's paragraphs / as one paragraph |
| M-x unfill-paragraph, center-line | Join a paragraph's lines, center a line |
| C-x C-t, M-^          | Transpose lines, join line                     |
| C-q                   | Insert the next key literally (C-q TAB)        |
| C-x 8 ' e, C-x 8 RET  | Type é; insert a character by Unicode name     |
| C-x 8 e s / l / r     | Insert an emoji by name / from a list / recent |
| C-x =, C-u C-x =      | Which character is at point; describe-char     |
| C-x \, C-\            | Input method for one character / on and off    |
| C-x RET C-\, C-h I    | Pick an input method / describe one            |
| C-x 2 / 3 / o / 0 / 1 | Split below / right, other, delete, only       |
| C-x ^ C-x { } C-x +   | Resize and balance windows                     |
| C-x 4 f, C-x 4 b      | File / buffer in the other window              |
| C-M-v                 | Scroll the other window                        |
| C-x 5 2 / 0 / 1 / o   | New / delete / delete-other / other frame      |
| C-x 5 f / b / d       | File / buffer / dired in another frame         |
| C-x 5 u               | Undelete the last deleted frame                |

`C-x 8` is iso-transl: about 300 key translations that type characters
wherever you are typing, including the buffer (with a prefix argument,
`C-u 3 C-x 8 o`), the minibuffer, isearch and keyboard macros.

- **Translations.** `C-x 8 ' e` types é, `C-x 8 " u` ü, `C-x 8 ~ n` ñ,
  `C-x 8 1 / 2` ½, `C-x 8 a >` →, `C-x 8 _ m` —, `C-x 8 * E` €, and
  `C-x 8 SPC` a no-break space. `C-x 8 C-h` lists them all.
  `M-x iso-transl-set-language` adds a language's shorter keys (German
  `C-x 8 a` for ä).
- **`C-x 8 RET`** (`insert-char`) reads a character by its Unicode name,
  completing over all of them, or by its code (`e9`, `#xe9`, `#o351`).
- **`C-x 8 e`** holds the emoji commands: `e` inserts one picked by group,
  `s` by name, `l` lists them all in `*Emoji*` (RET inserts), `r` offers
  recent ones, and `d` describes the one at point.
- **Checking a character.** `C-x =` names the character after point, and
  `C-u C-x =` (`describe-char`) tells everything about it, including how to
  type it.
- **Not here:** Alt-key and dead-key variants of the translations, because a
  terminal doesn't send those keys.

Input methods are Forge's own, generated by
`scripts/gen-input-methods.py`: the Latin prefix and postfix methods
(`latin-postfix`: `e'` → é), TeX (`\alpha` → α), Greek and Russian
transliteration, Korean 2-Bulsik (`korean-hangul`; F9 converts a syllable to
Hanja if you provide a `hanja.txt` table in your Forge configuration
directory), `ucs` (`U` and four hex digits) and Japanese.

- **`C-x \`** (`activate-transient-input-method`) turns one on for the next
  character only, then puts back whatever was on. It asks which the first
  time, or with `C-u`; `forge.opt["default_transient_input_method"]` sets it
  ahead.
- **`C-\`** toggles the buffer's input method
  (`forge.opt["default_input_method"]`), and `C-x RET C-\` picks one. `C-h I`
  describes a method and its keys, and `M-x list-input-methods` lists them
  all.
- **Typing** works as quail does. The keys so far show underlined in the
  buffer (not yet an edit). DEL takes one back, a digit picks among several
  candidates, and any other key ends the translation and does what it does.
- **TAB completion** lists every key sequence that can follow, with what each
  types, in `*Quail Completions*`. Click a candidate to choose it. As in
  Emacs, this works in the methods whose keymap has it (the Indic
  ITRANS/inscript, Tamil and IPA X-SAMPA/Kirshenbaum ones). In TeX and the
  other "simple" methods TAB stays TAB, unless you set
  `forge.opt["input_method_completion_everywhere"] = true`.
- **Japanese** (`japanese`, title `Aあ`) works as Emacs's does, with a
  dictionary (SKK-JISYO.L, built in). Romaji become hiragana underlined in a
  conversion region: `K` turns it to katakana and back, RET accepts it as it
  is, and SPC converts it to kanji. Then SPC / C-n and C-p go through the
  candidates (after four, the list shows in the echo area, and `l` / `L`
  page it while a digit picks), C-i / C-o make the phrase shorter or longer,
  C-f accepts the phrase and goes on to the next, K / H turn it to katakana /
  hiragana, DEL cancels back to kana, and RET accepts it all. Picks are
  learned for the session. `qq` switches to `japanese-ascii` and back, `qz` to
  `japanese-zenkaku`, `qh` back to `japanese`. `japanese-hiragana`,
  `japanese-katakana` and `japanese-hankaku-kana` transliterate only.
- **Same text:** tests type 2,881 key sequences across the methods, and 1,754
  in the Japanese ones (kana-kanji conversion included), and compare the
  results. The TAB listings are compared too.

Files are backed up on the first save: to `file~`, or numbered `file.~1~`,
`file.~2~`… (`forge.opt["version_control"] = "t"`; `kept_old_versions`,
`kept_new_versions`, `delete_old_versions` and `backup_directory` work). As in
Emacs, there are no backups for files under version control (the repository
already keeps the old text; `forge.opt["vc_make_backup_files"] = true` makes
them anyway) or for files in temporary directories (`/tmp`, `/dev/shm`,
`$TMPDIR`), and `forge.opt["make_backup_files"] = false` turns backups off
altogether. Modified buffers are auto-saved to `#file#` after 300 keystrokes
(`auto_save_interval`) and after 30 seconds idle (`auto_save_timeout`), and
when Forge is killed or crashes; saving deletes `#file#`, and
`M-x recover-this-file` (`=` shows the differences) brings it back after a
crash. A buffer that loses more than a quarter of a big text stops auto-saving
until you save. If another program changes a file, Forge reloads it when you
have no unsaved edits and warns you (`!!` in the mode line) when you do;
`M-x revert-buffer` reloads on demand (offering the auto-save file when it is
newer), and undo brings your text back.

File locks work like Emacs's and interoperate with them: the first change to
a file's buffer makes a `.#file` symlink (`user@host.pid:boottime`), and
saving, reverting, undoing back to the saved text, killing the buffer or
quitting removes it. If someone else holds the lock, Forge asks
`file locked by user@host (pid N): (s, q, p, ?)?` — steal it, give up the
change, or proceed without it. Stale locks (a dead process, or one from
before a reboot) are replaced quietly. `forge.opt["create_lockfiles"] = false`
turns locks off.

## Version control (C-x v)

VC, for Git and Mercurial: `C-x v v` does the next sensible thing (register,
commit, resolve), `C-x v =`/`D` diff, `C-x v l`/`L` log, `C-x v g` annotate,
`C-x v u` revert, `C-x v d` the status buffer, `C-x v +`/`P` pull/push,
`C-x v m` merge, `C-x v b` branches, `C-x v I`/`O` incoming/outgoing,
`C-x v h` region history. The mode line shows `Git-main` (`Git:main` when
edited). Commits are written in `*vc-log*` (`C-c C-c`), with the diff
alongside. Diffs open in diff-mode (`n`/`p` hunks, RET to the source,
`C-c C-a` apply a hunk, `C-c C-v` side by side). Conflict markers turn on
smerge-mode (`C-c ^ u`/`l` keep upper/lower, `C-c ^ E` three-way view). See
the comments at the top of `crates/editor/src/vc/*.rs` for every key.

## Code: highlighting and indentation

Rust, Python, JavaScript, C, Go, JSON, TOML, shell and Lua files open in their
major mode (by extension, or a script's `#!` line) and are highlighted from a
[tree-sitter](https://tree-sitter.github.io) syntax tree that's updated
incrementally as you type. Markdown files open in markdown-mode, highlighted
the same way (the editing commands come from the markdown-mode plugin).
TypeScript, Java, Ruby and other grammars can be installed with
`M-x treesit-install-language-grammar`.

| Keys      | Command                                                   |
|-----------|-----------------------------------------------------------|
| TAB       | Indent this line the way the mode says (region: every line) |
| RET       | New line, indented (a closing `}` re-indents as you type) |
| C-j       | New line without indenting                                |
| M-m       | Back to the indentation                                   |
| C-M-\\     | Re-indent the region                                      |

`forge.opt["electric_indent"] = false` stops RET and `}` indenting.

## Built in

| Keys / command                         | What                                                  |
|----------------------------------------|-------------------------------------------------------|
| M-; / C-x C-;                          | comment-dwim / comment-line, in the language's syntax |
| M-/                                    | dabbrev-expand: complete the word from the buffers    |
| M-=                                    | count-words (region, or buffer)                       |
| M-g i                                  | imenu: jump to a function or type                     |
| M-s o                                  | occur: every line matching a regexp (RET goes there)  |
| M-p / M-n in prompts                   | earlier and later input (kept between sessions)       |
| F10                                   | the menu bar, if menu-bar-mode is on                  |
| M-x menu-bar-mode                     | toggle it (C-u: on)                                    |
| M-x display-line-numbers-mode          | line numbers here (C-u: relative); global-… everywhere |
| M-x visual-line-mode                   | wrap at words, C-n/C-p by screen line                 |
| M-x toggle-truncate-lines              | wrap anywhere, or cut long lines off                  |
| M-x hl-line-mode, whitespace-mode      | highlight the current line; show spaces and tabs      |
| M-x display-fill-column-indicator-mode | a line at the fill column (global-… everywhere)       |
| M-x auto-fill-mode                     | break lines at the fill column as you type            |
| M-x electric-pair-mode                 | ( inserts (), DEL between them deletes both           |
| M-x delete-trailing-whitespace         |                                                       |
| M-x recentf-open                       | a recently opened file                                |
| C-x d, C-x C-j                         | dired: edit a directory (C-x C-f on one does too); C-x C-j starts on this file |
| M-x compile, M-g n / M-g p             | build (in the project root), visit the errors         |
| M-! / M-&                              | shell command (C-u M-!: insert the output), async      |
| C-x p f / C-x p g                      | a file of the project; grep the project (rg/git grep) |
| M-x grep                               | grep, matches in *grep* (M-g n visits them)           |
| M-x list-packages                      | install plugins from a package index                  |
| M-x package-vc-install                 | install a plugin from a Git repository (URL@tag)      |

In Dired, `RET`/`f` visits, `o` visits in the other window, `^` goes up,
`m`/`u` mark and unmark, `d` flags for deletion and `x` deletes the flagged
files, `C`, `R` and `D` copy, rename and delete the marked files (or the one
under point), `+` makes a directory and `g` reads the directory again.
Clicking a name visits it too, as `RET` does, while a drag out of one is still
a selection. `i` on a directory lists just that one inline as a tree, below
its parent in its own section (like Dired's `i`, but drawn with `├── ` cells);
`C-M-n`/`C-M-p` move between sections and `C-c C-k` removes the section point
is in. `C-c C-t` draws everything as a tree instead.

The listing is `ls -al`-style by default; `C-c C-e` switches to detailed
columns (or set `forge.opt["dired_listing"] = "columns"`). Both hide dotfiles
and show a Nerd Font icon before each name (`FORGE_DIRED_ICONS=0` turns them
off). In the column layout, permissions, size, owner and date are drawn in
Dired's colors, names are colored by what they are — by kind (directories
bold blue, executables bold green, links cyan), and otherwise by name and
extension into file types (images and videos purple, music cyan,
cryptography and documents green, archives red, temporary files dimmed,
compiled files yellow, build files bold underlined yellow like `Makefile` and
`Cargo.toml`, source files bold yellow). Sizes are shaded by magnitude, so
the columns read green below a megabyte and then yellow, red and purple.
Symlinks show `name -> target` with the target in its own color. The listing
has its own defaults too: dotfiles are hidden and the columns have no inode
numbers.

Most layout toggles moved under `C-c C-`; `s`, `i`, `I`, `r`, `/` and `B`
keep keys of their own:

| Key       | Command                     | Option          |
|-----------|-----------------------------|-----------------|
| `s`       | `dired-toggle-sort`         | `--sort=name,date,size,extension` (cycles) |
| `i`       | `dired-maybe-insert-subdir` | one `--tree` inline |
| `I`       | `dired-toggle-inodes`       | `--inode`       |
| `r`       | `dired-toggle-reverse`      | `--reverse`     |
| `/`       | `dired-toggle-only-dirs`    | `-D`            |
| `B`       | `dired-toggle-icons`        | `--icons`       |
| `C-c C-e` | `dired-toggle-ls-style`     | `ls -l` lines or columns |
| `C-c C-a` | `dired-toggle-hidden`       | `--all`         |
| `C-c C-d` | `dired-toggle-dirs-first`   | `--group-directories-first` |
| `C-c C-t` | `dired-toggle-tree`         | `--tree` (everything) |
| `C-c C-c` | `dired-toggle-color-scale`  | `--color-scale` |
| `C-c C-g` | `dired-toggle-git-ignore`   | `--git-ignore`  |
| `C-c C-f` | `dired-toggle-only-files`   | `-f`            |
| `C-c C-l` | `dired-tree-set-level`      | `--level`       |
| `C-c C-k` | `dired-kill-subdir`         | remove an inline listing |

`C-c C-t` draws everything as a tree, recursing into every subdirectory with
`├── ` cells (down to eight levels, `C-c C-l` sets how many); `i` draws just
the directory on that line that way, inline below its parent. While the tree
is on there are no inline subdirectories to add, and those commands say so.
`C-c C-c` shades sizes and dates against each other across the listing, the
way `--color-scale` does: the size and date columns are read back out of the
text and each value's lightness is moved along an Oklab ramp, so the largest
and newest stand out without the hue shifting.

A click on a name opens the file, as `RET` does, and middle-click opens it in
a split. That isn't special-cased to Dired: each name is a region with keys
of its own (`dired-entry`), so `#{ region: "dired-entry" }` in `init.rhai`
rebinds what a click does there.

Inside a git repository a `Git` column shows two letters, staged then
unstaged (`N` new, `M` modified, `D` deleted, `R` renamed, `T` type change,
`U` conflicted, `I` ignored), and a directory shows the worst status inside
it (`FORGE_DIRED_GIT=0` turns the column off). `forge .` or `forge DIR` starts
in Dired.

Org files (`.org`) get a minimal org-mode: TAB and S-TAB fold headings,
`C-c C-t` cycles TODO keywords (and `#+TODO:` sequences), `C-c C-c` toggles
checkboxes and updates `[1/3]` cookies, M-RET adds a heading or item,
M-arrows promote, demote and move subtrees. Entries tagged `:crypt:` encrypt
on save (`M-x org-encrypt-entry`, `org-decrypt-entry`, and the `-entries`
variants for the whole buffer; symmetric OpenPGP only so far).

The C-h keys are full: `C-h f` and `C-h k` show a command's keys (and the
keymap each is in, including modes that aren't on), its menu entry,
documentation and related commands; `C-h m` explains the buffer's modes and
lists each key with what it does; `C-h a`/`C-h d` search commands by name or
documentation; `C-h l` shows the last keys typed and `C-h ?` lists every help
key. Help buffers are coloured. In Dired, `?` summarises the keys and `C-h m`
explains all of them.

`M-x menu-bar-mode` puts a menu bar across the top — its own File, Edit,
Options, Buffers, Tools and Help menus, with what Forge adds to them — with
F10, a click, or the pointer to work it, C-n/C-p and the arrows to move, RET
to run, ESC to close. Each item shows the key that runs it. The macOS app
shows the same menus in its native menu bar. `M-x context-menu-mode` (off by
default; `forge.opt["context_menu"] = true` turns it on) makes a right-click,
or S-F10, pop up a menu of what makes sense at the pointer: Undo,
Cut/Copy/Paste, Paste from Kill Menu, Select. A plugin can add its own menus
with `forge.menu.define`/`forge.menu.extend` (see `docs/plugin-api.md`).
Matching brackets are highlighted (`show-paren-mode`), files open where you
left them (`save-place`), and `.editorconfig` files set indentation, fill
column and what happens on save. Each has a `forge.opt` setting in
`init.rhai` (`line_numbers`, `wrap`, `hl_line`, `show_paren`, `electric_pair`,
`show_trailing_whitespace`, `fill_column_indicator`, `save_place`, `recentf`,
`savehist`, `editorconfig`); what Forge remembers between sessions is in
`~/.local/state/forge/session.toml`.

## Help

| Keys       | Command                                           |
|------------|---------------------------------------------------|
| C-h k      | describe-key: press keys, see what they run       |
| C-h c      | describe-key-briefly (answer in the echo area)    |
| C-h f      | describe-function: docs and keys for a command    |
| C-h o      | describe-symbol: everything known about a command, option, setting, mode or face |
| C-h w      | where-is: which keys run a command                |
| C-h b      | describe-bindings: every active key, by keymap    |
| C-h m      | describe-mode: the buffer's modes and their keys  |

Stop after a prefix key like `C-x` and Forge lists the keys that can follow it
after a second. `C-h` after a prefix (`C-x C-h`) shows the list right away.

## Configuration: init.rhai

Forge runs `~/.config/forge/init.rhai` at startup (or
`$XDG_CONFIG_HOME/forge/init.rhai`, or `$FORGE_CONFIG_DIR/init.rhai`; see
[docs/init.example.rhai](docs/init.example.rhai)). It is Rhai:

```rhai
forge.opt["prefix_help_delay"] = 0.5;              // 0 turns prefix hints off
forge.keymap.set("C-c g", "goto-line");            // C-c <letter> is reserved for you
forge.keymap.set("C-t", "undefined");              // switch a key off
forge.keymap.remap("kill-line", "kill-region");
forge.keymap.set("x", "quit-window", #{ mode: "help-mode" });
forge.keymap.set("C-c w", || forge.notify("hi"), #{ desc: "Say hi." });

forge.command.create("insert-greeting", |n| {
  forge.ui.input(#{ prompt: "Greet whom? ", "default": "world" }, |name| {
    if name != () {
      forge.buffer.current().insert("Hello, " + name + "!\n");
    }
  });
}, #{ desc: "Ask for a name and insert a greeting." });
forge.keymap.set("C-c H", "insert-greeting");

forge.plugin.grant("spell", #{ directories: ["~/dict"] });  // don't ask

// As (add-hook 'prog-mode-hook #'rainbow-delimiters-mode), but in Rhai:
forge.mode.hook("prog-mode", "rainbow-delimiters-mode");
forge.mode.hook("rust-mode", || { forge.opt["wrap"] = "off"; });
forge.command.run("global-rainbow-delimiters-mode", 1);  // 1: turn on, not toggle
forge.mode.hook("org-mode", "org-modern-mode");
forge.plugin.config("org-modern", #{ progress: false });
```

Plugins load after `init.rhai`, so a hook or `forge.command.run` that names a
plugin's command waits until the plugin has loaded (and says so in
`*Messages*` if no plugin ever defines it). As in Emacs, a minor mode named
in a hook is turned on, not toggled, and a parent mode's hooks run before its
children's.

The API is namespaced like Neovim's: `forge.opt`, `forge.keymap`,
`forge.command`, `forge.ui`, `forge.buffer`, `forge.mode`, `forge.plugin`,
`forge.process`, `forge.fs`, `forge.http`, `forge.env`, `forge.system`,
`forge.menu`, `forge.theme`, `forge.log`, `forge.notify`, `forge.on` and
`forge.grammar`. `M-:` evaluates a Rhai expression and shows the result.
`M-x reload-init-file` runs `init.rhai` again without restarting, and
`forge -q` starts without it. Errors (with the line number) show in the echo
area and `*Messages*`; `print` writes there too. For the current session only,
use `M-x global-set-key` or `M-x local-set-key` (this buffer only).

Rhai code has no direct file, OS or network access: files go through
`forge.fs`, programs through `forge.process`, and web requests through
`forge.http`, so nothing blocks the editor. A single call that runs longer
than 2 seconds is stopped, and Rhai is limited to 64 MB of memory.
Positions are (line, column) as the mode line shows them, lines from 1 and
columns from 0. A project's own `.forge/init.rhai` cannot use
`forge.fs.write`, `forge.process.run`, `forge.http.get` or
`forge.http.request`.

Keymaps are searched in this order, and the first match wins: buffer-local,
minor modes, major mode (then the mode it derives from), global.
`forge.keymap.remap` then swaps the command once.

## Projects

A project is the nearest directory above a file with a `.forge/` or a `.git`
in it. Bindings made for one apply to every file inside it, the way
`.dir-locals.el` bindings do, and sit between the modes and the global map:

```rhai
forge.keymap.set("C-c b", "project-build", #{ project: "~/src/forge" });
forge.keymap.set("C-c b", "make", #{ project: true });   // the buffer's project
```

A project can carry its own `.forge/init.rhai`, which binds with
`#{ project: true }`, since it can't name itself. That file is code from a
directory you may have just cloned, so Forge asks before running it. The
answer is kept in `~/.local/state/forge/plugin-grants.toml` in a section
of its own (an older file next to `init.rhai` is migrated on first use):

```toml
[projects."/home/me/src/forge"]
code = true
```

`M-x revoke-plugin-permissions` forgets it (by `project:<directory>`) and
`M-x list-projects` shows it. Saying no isn't remembered across sessions;
saying yes is.

## Plugins

Plugins are WebAssembly components written against
[wit/forge-editor.wit](wit/forge-editor.wit): they can register commands, bind
keys, read and edit buffers, ask questions in the minibuffer, define modes,
react to events, and draw canvases. They run sandboxed in Wasmtime with no
access to files, network or environment unless granted. See
[docs/plugin-api.md](docs/plugin-api.md).

```sh
rustup target add wasm32-wasip2
cd plugins/hello && cargo build --release --target wasm32-wasip2 && cd ../..
cargo run --release -p forge-terminal --bin forge -- \
  --plugin plugins/hello/target/wasm32-wasip2/release/forge_hello.wasm
```

Then `C-c C-h` runs the plugin's `hello-greet`. `M-x load-plugin`,
`M-x reload-plugin` and `M-x unload-plugin` work at any time; plugin messages
go to `*Messages*`.

To install a plugin, put it in `~/.config/forge/plugins/` (as `NAME.wasm`, or
`NAME/NAME.wasm` with any files it comes with). Plugins there load at startup.
A plugin you rebuild reloads by itself
(`forge.opt["plugin_auto_reload"] = false` turns that off). Compiled plugins
are cached in `~/.cache/forge`, and an unchanged plugin doesn't even start
until you use one of its commands or something it listens for happens, so
having many installed costs almost nothing at startup.

Plugins get no files and no network unless you allow it. A plugin asks in its
manifest, which the Rust SDK builds into the `.wasm` from
`[package.metadata.forge]` in `Cargo.toml` (a `spell.toml` next to
`spell.wasm` overrides it); Forge shows what it wants and asks before loading,
unless `init.rhai` already granted it with `forge.plugin.grant`. Answers are
kept in `~/.local/state/forge/plugin-grants.toml`, bound to the plugin's
binary (an older config-directory file is migrated on first use).
`M-x list-plugins` shows what
each plugin has; `M-x revoke-plugin-permissions` takes it back. A plugin call
that takes long shows "NAME is busy... (C-g to stop)", and C-g stops it.

Only the example plugin `plugins/hello` lives in this repository. The others
(vertico, corfu, meow, lsp, magit, consult, markdown-mode, spell-check,
environment, line-numbers, dashboard, org-modern, rainbow-delimiters,
tip-of-the-day, canvas, bases and graph) are separate repositories named
`forge-plugin-NAME`. Build one with
`cargo build --release --target wasm32-wasip2` and copy the `.wasm` to
`~/.config/forge/plugins/`, or list it in `use_package` with a `vc` source.

`forge --headless --plugin P.wasm --keys "M-x" --screen` runs the editor
without a terminal and prints the screen: that's how these are tested.

Plugins can be written in Rust, JavaScript/TypeScript, Python, C or Lua: see
[sdks/](sdks) (each with an example). They can run programs (with permission)
and run slow commands in the background without freezing the editor.
`forge --check-plugin FILE --run COMMAND` tries one without the editor.
Components built for WASI 0.2 or 0.3 both load, so any toolchain that makes
components works.

## Frontends

Forge runs in a terminal, and has graphical apps for macOS (SwiftUI and
Metal), GNOME (GTK 4) and Windows (WinUI 3). The GNOME and Windows apps are
thin: the editor draws its own screen, menu bar and tabs as a grid of styled
text, and the app shows that grid, sends keys, text and the mouse back, and
provides what only the system can (the clipboard and the file picker). A
canvas arrives in the grid as a picture, drawn by the shared renderer.

**macOS.** `frontends/apple/Forge.xcodeproj` is a native SwiftUI app around
the same editor: open it in Xcode and run. Its build runs
`scripts/build-apple.sh`, which builds the Rust core (`crates/ffi`) and its
Swift bindings; you need Rust installed. Option is Meta (Settings can turn
that off), Command is for the menus (Cmd-O opens, Cmd-C copies, Cmd-V pastes),
and it shares `~/.config/forge` with the terminal version. The app keeps one
window per frame: `C-x 5 2` opens a window, closing one deletes its frame, and
the File menu's New/Delete/Undo Frame do the same. Frames share buffers but
each has its own windows and point, and a window becomes the current frame
when you give it focus.

**GNOME.** Build the GTK 4 app with `cargo build --release` in
`frontends/gnome`; it needs the GTK 4 development files.

**Windows.** Build the WinUI 3 app with `scripts/build-windows.ps1`; it needs
Rust with the MSVC toolchain, the .NET 10 SDK and the Windows App SDK
workload. The script builds `forge-capi` (the C ABI the app calls) and
publishes the app. The WinUI app is not verified to compile yet.

Canvases are drawn on the GPU (wgpu) when one is available, and on the CPU
otherwise; both draw the same scene. `M-x canvas-renderer` says which is in
use.

## Mouse

The wheel scrolls the window under the pointer, a click moves point, and a
drag selects. Most terminals still select text with Shift+drag. Turn it off
with `M-x xterm-mouse-mode` or `forge.opt["mouse"] = false` in `init.rhai`.

A click is dispatched as a key (`<mouse-1>`, `<mouse-2>`, `<mouse-3>`) after
point moves, so whatever is bound for the text under the pointer runs:
clicking a name in a Dired listing opens it. Text can bring its own keys with
it this way, through a region map:

```rhai
// middle-click a name in a Dired listing to open it in a split
forge.keymap.set("<mouse-2>", "dired-find-file-other-window", #{ region: "dired-entry" });
```

## Terminal support

**Keys a terminal can't tell apart.** A plain terminal sends one byte for
Ctrl+I and for TAB, and one for Ctrl+M and for RET, so those keys were
indistinguishable, and Shift came through folded into the character. Forge
asks for the kitty keyboard protocol at startup (kitty, ghostty, WezTerm, foot
and others answer it) and pops it again on the way out. Where the terminal
answers it, `C-i` is not `TAB`, `C-m` is not `RET`, and `S-a`, `C-S-a`,
`H-a`, `s-super-a` and `s-m-a` are keys you can bind. Where it doesn't,
nothing changes: `TAB` and `C-i` are the same key either way, and a binding
in one name answers for the other. (Inside tmux, turn on
`set -g extended-keys on`, or tmux swallows the sequences.)

The notation takes `S-`/`s-` for Shift, `H-` for Hyper, `s-super-` and `s-m-`
for the Super and Meta keys, alongside `C-` and `M-`. Shift on a letter is
written on the lower-case letter whichever terminal you are on, so a binding
means the same key everywhere; Shift on a symbol is the character it makes
(`S-2` is `@`), because that character is what gets typed.

Errors, warnings and misspelled words are underlined in a color of their own
where the terminal can do that (kitty, WezTerm, Ghostty, foot, Alacritty,
iTerm2 3.4+, and VTE terminals such as GNOME Terminal). Anywhere else,
including macOS Terminal.app, they are underlined and the text itself takes
the color, because a terminal that doesn't know about underline colors shows
the escape sequence for one as stray blinking, bold or italic text.

Forge picks by looking at `TERM`, `TERM_PROGRAM` and friends, and treats a
terminal it doesn't recognize as one that can't. Inside tmux or screen it
can't tell either. If it guesses wrong, say what your terminal can do:

```sh
export FORGE_UNDERLINE_COLOR=1    # or 0
```

(In tmux, tmux itself also has to be set up to pass underline colors on to your
terminal.)

## Layout

    crates/core        editor engine: buffers, modes, keymaps, plugin runtime
    crates/docs        documentation data used by the editor
    crates/editor      the editor application: keys to commands, minibuffer,
                       plugins, VC, Org and init.rhai — every frontend drives this
    crates/terminal    terminal frontend (ratatui) — builds `forge` and `forgeclient`
    crates/plugin-sdk  write plugins in Rust (wit-bindgen)
    crates/plugin-sdk-macros  embeds a plugin's manifest from its Cargo.toml
    crates/ffi         the editor for the Apple app (Swift bindings via UniFFI)
    crates/capi        the C ABI for the Windows app
    crates/canvas      canvas scenes, layout and SVG export
    crates/notes       notes backends for the canvas, bases and graph plugins
    crates/canvas-gpu  the wgpu canvas renderer
    crates/xtask       build tasks (man page, docs)
    wit/               the plugin API, in WIT
    plugins/hello      example plugin
    sdks/              plugin SDKs for JavaScript/TypeScript, Python, C and Lua
    frontends/apple/   the macOS app (SwiftUI, Xcode project)
    frontends/gnome/   the GNOME app (GTK 4)
    frontends/winui/   the Windows app (WinUI 3)
    docs/              manual, tutorial, plugin API guide, example init.rhai
    scripts/           build and code-generation helpers
    tests/fixtures/    test plugins written in WAT

## Org: rich content and export

Footnotes (`C-c C-x f`), citations (`C-c C-x @`, from `#+BIBLIOGRAPHY:`
files), source blocks and export are built in.

- **Source blocks** are highlighted in their own language; `C-c '` edits a
  block in a buffer of its own. `C-c C-c` (or `C-c C-v e`) runs a block
  (sh, python, js, ruby, perl, lua, awk) after asking, and puts the result
  under `#+RESULTS:`. `:var`, `:results`, `:dir`, `:noweb`, `:tangle` and
  `#+CALL:` work; `C-c C-v t` tangles. Sessions and other languages don't.
  `M-x org-babel-toggle-confirm-evaluate` stops the question.
- **Export**: `C-c C-e` for ASCII, UTF-8, HTML, Markdown, LaTeX (and PDF when
  latexmk or pdflatex is installed), Beamer, ODT, Org, Texinfo (and Info with
  makeinfo) and iCalendar. `C-u` exports just the subtree.
- Footnotes, `[cite:...]`, LaTeX fragments and `\entities` are highlighted;
  links preview as badges (`C-c C-x C-v`, `org-link-preview`), and links show
  just their description (`M-x org-toggle-link-display`). Local image links
  are drawn inline where the frontend can, and remote `http(s)` image links
  are fetched and cached in the background
  (`forge.opt["org_display_remote_inline_images"]`); what counts as an image
  follows `forge.opt["image_file_name_extensions"]` and
  `image_file_name_regexps`.

## License

Forge is licensed under either of MIT or Apache-2.0, at your option
(`LICENSE-MIT`, `LICENSE-APACHE`). See `LICENSING.md` for the per-directory
table (some plugins are GPL ports and keep the GPL) and
`THIRD_PARTY_NOTICES.md` for included third-party material.
