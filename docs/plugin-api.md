# The Forge plugin API

Plugins are WebAssembly components for the `forge:editor/plugin` world in
[`wit/forge-editor.wit`](../wit/forge-editor.wit). That file is the API;
this page explains how it's meant to be used and how it will change.

## How a plugin runs

Forge calls the plugin, waits for it to return, and carries on. The plugin
never runs on its own and never waits for the user:

| Forge calls            | When                                             |
|------------------------|--------------------------------------------------|
| `init`                 | once, right after loading                        |
| `handler.run-command`  | one of its registered commands runs (M-x, a key) |
| `handler.on-event`     | something it subscribed to happened              |
| `handler.on-answer`    | the user answered one of its questions           |

Inside a call, buffer and keymap changes happen immediately. Things that
need the user or the rest of the editor happen *after the call returns*:
questions (`minibuffer.read-string`, `completing-read`, `y-or-n`), echo-area
messages, and `commands.run`. A question returns a request id at once; the
answer arrives later through `on-answer` (none if the user pressed C-g).
The Rust SDK hides this: `read_string("Name: ", "", |name| ...)`.

Everything a plugin does in one call undoes as one step (C-/).

## Events

`events.subscribe(kind)` for: `buffer-opened`, `buffer-killed`,
`buffer-changed`, `buffer-saved`, `mode-changed` (each carries the buffer
id). Events arrive after the command that caused them. `buffer-changed` is
held back until the editor has been idle for 300 ms and then sent once per
buffer, so a plugin never runs on every keystroke. Changes a plugin makes
while handling an event aren't announced again (no ping-pong between
plugins). The `*Messages*` buffer never produces events.

## Decorations, options and syntax (0.1.1)

**Decorations** put a face on buffer text without changing it: a
misspelled word, a compiler error. `decorations.set(buffer, list)` replaces
all of *this plugin's* decorations in a buffer; `clear` removes them.
Faces are `error`, `warning`, `info`, `spell-error`, `highlight`, or a
syntax face (`keyword`, `string`, `escape`, `comment`, `function`, `type`,
`constant`, `builtin`, `property`, `attribute`) for semantic highlighting.
Nested brackets have faces of their own, `rainbow-depth-1` to
`rainbow-depth-9` (1 is the outermost), `rainbow-unmatched` and
`rainbow-mismatched`, which the rainbow-delimiters plugin sets and a theme
can restyle. A Forge without them rejects them as unknown faces, so a plugin
that wants to run there should fall back to the faces above.
Frontends pick the colors (the terminal underlines errors in red, or turns
the text red where it can't color an underline separately). A
decoration can carry a message, shown in the echo area while point is on
it. Decorations move with edits, vanish with their text, and go away when
the plugin is unloaded.

`decorations.set-regions(buffer, list)` attaches keymaps to stretches of
text: each is a span and the name of a region map, and that map's bindings
apply while point is inside the span. It's separate from `set` because a
keymap isn't a face — the same text can be coloured by one call and made
clickable by another — and a region doesn't need a face of its own. See
"Keys and modes" for what a region map is and how a click becomes a key.

**Syntax.** Forge parses buffers with tree-sitter itself (see Phase 7 in
the manual for why it's native). `decorations.syntax(buffer, span)` returns
the highlighting of a span as runs of syntax faces, empty if the major
mode has no grammar. The spell checker uses it to check only comments and
strings.

**Buffer options** are per-buffer settings, like buffer-local
variables: `buffer.set-option(name, value)` and `get-option(name)`.
`line-numbers` (`off`, `absolute`, `relative`), `wrap` (`off`, `char`,
`word`), `indent-width` (1-16), `indent-tabs`, `fill-column`, and the
on/off ones: `hl-line`, `display-fill-column-indicator`,
`show-trailing-whitespace`, `whitespace`, `trim-trailing-whitespace` and
`insert-final-newline` (both applied on save), and `cursor` (`box`,
`bar`, `hbar`: how the cursor looks, for modal editing). The fill column
line also takes `display-fill-column-indicator-column` (`t` for
fill-column, a number, or `nil` for no line) and
`display-fill-column-indicator-character` (one character, or `nil` for
no line). An empty value goes back to the default.
`buffer.indent-line(n)` re-indents a line the way TAB does.

## Inline images (0.1.25)

A plugin can pin a picture under a buffer line: `images.set(buffer, list)`
where each entry is `{ line, path }` (0-based line, a file the frontend
reads), and `images.clear(buffer)` removes this plugin's pictures. The
macOS app draws the file under the line; the terminal shows Org image
links as badges. Lines a picture sits under get blank rows reserved (never
point's own line). A path the frontend can't load is ignored, so this is
safe to call everywhere. Several plugins can pin the same line; each
sets and clears only its own.

A line that also has an Org image link keeps the link's own picture, and
the text browser's image cache wins too — `images.set` is for pictures
those don't cover. The `org-remoteimg` plugin shows how it's used: it finds
`[[https://.../pic.png]]` links, fetches each once with `http.get-async` (the
`http` permission), writes the bytes with `files.write` (a writable
directory grant), and pins the cached file to the link's line.

## Canvases (0.1.29)

A plugin draws by describing a *scene*, not by writing pixels. `canvas.show(name,
scene-json)` makes (or replaces) a canvas and its `canvas-mode` buffer
`*Canvas: name*` and returns the buffer; `canvas.display(name)` selects it.
The scene is `{ "width", "height", "background", "ops": [...] }` where an op
is a `rect`, `ellipse`, `line`, `path` (SVG path data), `text` (wrapped to
`max_width`, cut at `max_lines`), `image` or `group` (a `transform`, a
`clip`, an `opacity` and child `ops`); fills are colours or linear
gradients, strokes dash and end in arrowheads. Any op may carry an `id`. A
Rust plugin builds scenes with the `forge-canvas` crate (`default-features =
false` keeps the renderer out of the wasm), which also does the measuring,
hit testing, bounds and SVG export natively.

**Colours.** A colour in a scene may be a role instead of a hex code: `@bg`,
`@sunken`, `@card`, `@chip`, `@line`, `@faint`, `@fainter`, `@fg`, `@strong`,
`@soft`, `@dim`, `@tag` and `@accent0` to `@accent7`. Forge picks the colour
when it draws the scene, from the user's theme (or the system's light or dark
mode), and draws it again when that changes, so a plugin needs no theme code
and nothing to redraw. In Rust, `Palette::current()` (from `forge-canvas`)
gives these roles by default: `pal.card`, `pal.accent(2)`. A plugin that wants
the real colours (to mix its own) can read `theme.palette()`. Scenes are drawn
on a separate thread, and a canvas asked for again while it is waiting is
drawn once, in its newest state.

`canvas.on-event(name, command)` runs `command` when the pointer is used on
the canvas; the command reads the event with `canvas.last-event(name)`, JSON
with `kind` (`press`, `release`, `drag`, `double`, `scroll`), `button` (1
left, 2 middle, 3 right), `x`/`y` in scene coordinates, `frame-x`/`frame-y`,
`id` (the topmost shape with an id under the pointer, or null), `dy` for the
wheel and `shift`/`ctrl`/`alt`. Forge does the hit testing, so a plugin needs no pixel
maths. `canvas.set-view` / `get-view` pan and zoom (the scene point at the
frame's top-left, and the zoom); the wheel and middle-drag do that
by themselves unless `canvas.handle-view-events(name, true)`. A plugin
only drives canvases it made. `canvas.render-png(scene, path, scale)` draws
a scene to a file (it needs write access to the path), `canvas.names()`
lists the plugin's canvases and `canvas.close(name)` kills one.

`files.list-dir(dir, recursive)` (sorted paths, directories ending in `/`,
hidden entries skipped, a million at most) and `files.stat(path)` (size,
modified, created, directory) were added with it, for plugins that read a
folder of notes. Both need the directory to be granted, like `files.read`.

Also since 0.1.29, for plugins that index a lot of files:
`files.list-dir-info(dir, recursive)` is `list-dir` with each entry's size
and times (so a few hundred thousand files can be checked for changes in one
call instead of a `stat` apiece), and `files.cache-dir()` is a folder of the
plugin's own, `~/.cache/forge/plugin-data/NAME`, for files it can rebuild (an
index, a saved layout). The plugin reads and writes there with no grant and
touches nothing of the user's; Forge or the user may delete it, so a plugin
must work with it empty.
`forge-plugin-canvas` (JSON Canvas boards), `forge-plugin-bases` (Obsidian Bases) and
`forge-plugin-graph` (local and global link graphs) are the examples; their
logic lives in `crates/notes`, which is tested natively.

The directory functions are here too, in the same permission model
(reads need a granted directory, changes need `write`): `files.make-directory`,
`copy-directory`, `directory-files` (with `matching`, a regular expression
on each name, and `count`), `directory-files-and-attributes`,
`directory-empty`, `directory-files-recursively`, `locate-dominating-file`,
`in-directory` (`file-in-directory-p`) and `expand-wildcards`
(`file-expand-wildcards`). The file-name functions need no permission:
`name-directory`, `name-nondirectory`, `name-as-directory`, `directory-name`,
`name-extension`, `name-sans-extension`, `name-base`, `name-with-extension`,
`name-concat`, `name-absolute`, `name-split`, `name-parent-directory`,
`expand-name`, `relative-name` and `abbreviate-name`. `files.create` is
`make-empty-file`.


### Widgets

`canvas.show-widgets(name, tree-json, page-width)` shows an interface instead
of a drawing. The tree is columns and rows of text, cards, lists, kanban
boards, buttons, pictures and separators:

```json
{"w":"column","gap":12,"children":[
  {"w":"text","style":"title","text":"Reading list"},
  {"w":"board","lanes":[
    {"id":"todo","title":"To read","cards":[
      {"w":"card","id":"n1","title":"Dune","subtitle":"Frank Herbert",
       "badges":["sci-fi"],"image":"/path/cover.jpg"}]}]}]}
```

Forge lays the tree out (text is measured and wrapped, cards never overlap,
lanes are as tall as the tallest) and draws it as a scrolling canvas, so it
looks right in the terminal, in a window and on a phone, follows the theme,
and costs the plugin no layout code. A widget with an `id` (a card, list
item, lane or button) is what `canvas.last-event` reports as `id` when the
pointer is used on it, as for a shape in a scene. In Rust, `forge_canvas::Widget`
parses and builds the same tree; `forge_canvas::layout_widgets` is the layout
(useful in tests). Fields: `column` and `row` (`children`, `gap`; a row
shares its width equally), `text` (`text`, `style` of `title`, `heading`,
`body`, `caption` or `code`), `card` (`id`, `title`, `subtitle`, `body`,
`badges`, `image`, `children`), `list` (`items` of `{id, title, subtitle,
trailing}`), `board` (`lanes` of `{id, title, cards}`, 260 wide, scrolling
sideways when wider than the page), `button` (`id`, `label`), `image`
(`path`, `height`) and `separator`.

## Web requests (0.1.26, asynchronous from 0.1.27, synchronous form removed in 0.1.28)

`http` is its own permission (`http = true` in the manifest, asked for on
its own), separate from `network` and `processes`. It's the way for a plugin
to reach the network without spawning a program: Forge makes the request
with its own HTTP client. The body comes back as bytes — to keep them, write
them with `files.write` into a writable directory grant.

Requests are asynchronous: a request goes out over the network, so it can
take seconds (Forge gives up after 30), and waiting would hold up the whole
editor. So a request returns at once and its answer arrives later:

- `get-async(url, headers)` and `http-request-async(method, url, headers,
  body)` start a request and return at once with an id
  (`result<http-request, string>`).
- The answer arrives later through `http-handler.on-response(id, answer)`,
  during the editor's idle time — never in the middle of another call, just
  like a process's output. `answer` is `result<response, string>`.
- A plugin that uses them exports the `http-handler` interface (world
  `plugin-with-http`); the Rust SDK's `forge_plugin!` does this for you.
- `cancel(id)` gives up on a request: its `on-response` never comes.

A `response` is `{ status: u16, headers: list<tuple<string, string>>, body:
list<u8> }`, and `headers` is a list of `(name, value)` pairs. A 4xx or 5xx
status comes back as an `Err`, as do connection
failures and a request the server doesn't answer in time. Forge sends its
own `User-Agent` unless you pass one.

The Rust SDK wraps the asynchronous calls: `get_async(url, headers,
on_response)` and `request_async(method, url, headers, body, on_response)`
take a closure that runs when the answer arrives; `cancel(id)` takes the id.
That's what `org-remoteimg` does to fetch pictures.

## Completion (0.1.3, sorted and inspected from 0.1.4)

Forge does the work of completing, whatever the prompt or buffer:
filtering as you type, drawing the list, handling the keys. Plugins
choose how it looks and matches, and provide candidates and annotations.
Forge asks them once per list, never on every key. The `completion`
interface covers both kinds.

**In the minibuffer** (vertico):

- `set-minibuffer-ui(vertical, lines)` lists candidates under every prompt
  that has them (M-x, C-x b, C-x C-f, plugin questions). C-n / C-p move,
  RET picks, TAB inserts, and M-RET takes the input as typed.
- `set-layout(layout, lines)` (0.1.31) says how to draw that list:
  `vertical` (the default, top-down), `flat` (every candidate on one row),
  `grid` (fill up to `lines` rows across), `reverse` (the selection sits
  near the bottom, the way vertico-reverse reads), `unobtrusive` (show only
  the selected candidate) or `buffer` (list in the `*Completions*` buffer
  in another window, as TAB does). `lines` is clamped to 1..50 and also
  updates the count `set-minibuffer-ui` chose.
- `group(category)` (0.1.31) has Forge call this plugin's
  `completion-handler.group(category, candidates)` once per list and show
  the matching headings; an empty name leaves a candidate ungrouped.
  `set-group-format("-- %s --")` changes the heading, `%s` being the group
  name, and `""` hides headings. Grouping applies to the vertical layouts
  (`reverse` included); `flat`, `grid`, `unobtrusive` and `buffer` draw no
  headings. Row labels follow the rows whichever layout draws.
- `set-row-labels(["a", "s", "d"])` (0.1.31) shows a short label before
  each visible row, in order, for commands like vertico-quick; an empty
  list clears them. `jump-to-row(row, accept)` selects the candidate on
  visible row `row` (0 is the top), taking it as RET would when `accept`
  is true. Labels are a display aid: a list longer than the labels simply
  leaves the rest unlabeled.
- `minibuffer.suspend()` (0.1.31) hides the open prompt without answering
  it, `sessions()` lists the prompts this plugin suspended (newest first,
  as `{category, prompt, input, total}`), and `resume(index)` brings one
  back, suspending the open prompt first. A plugin uses this to offer a
  picker from inside a picker without nesting prompts.
- `minibuffer.set-input(text)` (0.1.31) replaces what is typed in the open
  prompt, leaving the cursor at the end; it does nothing when no prompt is
  open. `minibuffer.on-open(command)` (0.1.31) runs one of the plugin's own
  commands after a prompt opens, so a command can read `input`/`selected`
  and adjust it.
- `set-minibuffer-cycle(true)` makes C-n at the last candidate wrap around
  to the first (and C-p at the first to the last), and similarly for page
  moves. Off by default (vertico-cycle); a plugin that isn't loaded leaves
  it off.
- Moving the list is on the same controls as the popup: `first()` and
  `last()` jump to an end, `next(n)` / `previous(n)` step, and `scroll(down)`
  pages. With no popup open these drive the minibuffer's list, so a plugin
  like vertico can define `vertico-first` / `vertico-last` and bind
  M-< / M-> in the `completion-list` prompt map.
- `set-matching(["orderless", "flex"])` chooses how typed text matches:
  `orderless` (space-separated parts in any order; `!part` excludes),
  `prefix` (the default), `substring` or `flex`. The first style that
  matches anything wins.
- `annotate-category(category)` gives this plugin the category's
  annotations. Forge calls `completion-handler.annotate(category,
  candidates)` once per list and shows the results next to the
  candidates (marginalia). The categories are `command`, `buffer`,
  `file`, `symbol` (imenu), `project-file`, `package`, `plugin`, and
  `candidate` for plugin questions.

**At point** (corfu):

- `set-at-point-ui(popup, auto, delay-ms, min-prefix, lines)` shows
  candidates in a popup at point. With `auto`, it opens by itself once
  typing pauses after `min-prefix` word characters. Without the popup,
  `M-x completion-at-point` (C-M-i, M-TAB) inserts the common part and
  lists the rest in the echo area.
- `register-source(name, modes)` adds a source for buffers whose major
  mode is, or derives from, one of `modes`. Forge calls
  `completion-handler.complete(request, source, buffer, point)`. The
  plugin answers with `deliver(request, span, candidates)`, then or later:
  the lsp plugin answers when the language server does. `deliver-decorated`
  (0.1.15) is the same but each candidate may also carry a `margin` (an
  icon or kind, shown before it) and say it is `deprecated`.
  `deliver-detailed` (0.1.16) adds `documentation` (shown for the selection,
  corfu's `M-h`) and a `location` (corfu's `M-g`); a completion-handler's
  `commit(candidate, status)` (0.1.16) is called when a candidate is
  inserted, with the CAPF exit status `"sole"`, `"exact"` or `"finished"`.
- Words from the open buffers are always a source.

**Both** (0.1.4):

- `set-sorting(["history-length", "length"])` orders the candidates that
  match, after filtering. The names are `length` (shortest first, then
  alphabetical), `alpha`, `history` (most recently used first in that
  prompt's category, then length), `history-length` and `position` (the
  order the source produced). The first name applies; unknown ones are
  ignored, so a plugin asking for more than the running Forge knows still
  loads. Without it, matches come back exact first, then prefix, then the
  rest. The `history` sorts only have something to work from in the
  minibuffer.
- `selected()` is the candidate under the cursor in an open list —
  vertical or popup — as `{text, annotation, category, index, total}`
  (since 0.1.16 also `documentation` and `location`), or
  none when no list is open. A plugin uses it to describe the selection
  (corfu's `M-h`) without owning the list. Since 0.1.17 `location` is a
  `{path, line, column}` record, so it can name another file.
- `locate()` asks the source of the selection where it is defined (corfu's
  `M-g`); the source answers in its own time (a language server round trip,
  for one) by exporting `completion-handler.resolve-location(selection)` and
  saying the place itself (usually with `message`). It returns whether a
  source was asked, so a caller can fall back.
- `set-popup-frame(min-width, max-width, border)` widens the popup at
  point to fit its candidates, within those columns (0 leaves that end to
  Forge; the window's width is the hard limit either way). A terminal
  draws no borders, so `border` is accepted and ignored.
- `set-popup-info(show-documentation, show-location, delay-ms, max-width,
  max-height)` (0.1.18) asks for an auxiliary popup beside the popup at
  point holding the selection's documentation and/or location, wrapped to
  `max-width` columns and at most `max-height` rows (0 leaves either to
  Forge). It appears once the selection has settled for `delay-ms`.
  `toggle-popup-info()` hides or shows it for the open session (corfu's
  `corfu-popupinfo-toggle`), `scroll-popup-info(down)` pages it,
  `popup-info-beginning()`/`popup-info-end()` jump to either end. The
  editor draws it, so it looks the same in the terminal and the app.

**Driving the popup at point** (0.1.14):

A plugin can drive the popup at point itself, rather than only choosing
how Forge shows it. The host owns no corfu command; the corfu plugin owns
them all and uses this API.

- `current()` is the open session at point as `{open, popup, index, total,
  deselected, lines, input}`, or none when none is open.
- `set-options(options)` sets the popup's policy: `cycle` (`C-n` on the
  last candidate wraps to the first), `preselect-prompt` (nothing is
  selected until you move), `separator` (the component character), the
  `quit-at-boundary` and `quit-no-match` rules (`"never"`, `"always"` or
  `"separator"`), `continue-commands` (names, or `name*` prefixes, that
  keep the popup open at the boundary), `indexed` (number the rows),
  `quick1`/`quick2` (the quickselect key sets), `echo` and `echo-delay-ms`
  (show a note after a pause), `mouse` (a click picks a candidate) and
  `history` (rank by and remember what was completed).
- `next(offset)`/`previous(offset)` move the selection (cycling when it
  is set), `first()`/`last()` jump to an end, `scroll(down)` pages by
  whole screens, `accept-row(index)` picks a row by its 0-based index, and
  `arm-quick(action)` labels the rows with their quickselect keys for the
  next key (`"insert"`, `"complete"` or `"jump"`; an unknown action does
  nothing).
- `accept()` takes the selection, `expand()` inserts the common part of
  the matches (`M-TAB`), `insert-separator()` splits the input at the
  separator, `reset()` deselects, and `quit()` closes the session.

A plugin with a `completion-handler` targets the `plugin-with-completion`
world, or `plugin-full` (processes too). Settings go away when the plugin
is unloaded, and lazily started plugins keep theirs, because they're
saved in the plugin's summary.

## The menu bar (0.1.5)

Forge has menus (File, Edit, Options, Buffers, Tools, Help).
In the terminal `menu-bar-mode` draws them across the top of the frame and
`F10` or a click opens one; the macOS app shows the same menus in its own
native menu bar. A plugin adds to that list, so its commands appear in
both.

```rust
use forge_plugin_sdk::{command, define_menu, define_menu_for_mode, menu_item, menu_separator, open_menu, menu_selection};

fn init() -> Result<(), String> {
    // A menu of its own, after the built-in ones.
    define_menu("Meow", &[menu_item("Motion", "meow-insert"), menu_separator()])?;
    // ...or one that only shows while a major mode (or one derived from
    // it) is on.
    define_menu_for_mode("Meow", &[menu_item("Motion", "meow-insert")], "meow-mode")?;
    // ...or items in one of Forge's.
    forge_plugin_sdk::extend_menu("Tools", &[menu_item("Motion", "meow-insert")])?;
    // A plugin that wants a menu can turn the bar on as it loads.
    forge_plugin_sdk::set_menu_bar(true);
    Ok(())
}
```

- `define-menu(title, items)` adds a menu. Fails if this plugin already
  has one, or if the title is taken.
- `define-menu-for-mode(title, items, mode)` adds a menu that only shows
  while `mode` is the buffer's major mode or one it derives from, the way
  a mode's own menu does. It fails under the same conditions as
  `define-menu`.
- `extend-menu(title, items)` adds items to a menu someone else defined,
  after the ones already there.
- `extend-submenu(menu, label, items)` adds items to the submenu called
  `label` in the menu called `menu`, after the ones already there. A
  missing label becomes a submenu header; a label on a plain item fails,
  since opening it would hide the command it runs.
- `remove-menu(title)` takes back what this plugin added to it.
- An item is `{label, command}`; an empty command is a separator. An
  item whose command isn't defined — the plugin that provides it isn't
  loaded — is shown greyed out rather than silently missing, so the menus
  don't change shape as plugins come and go.
- Forge's own items also carry a condition, which is why `Save` and
  `Revert` are greyed out in a buffer with nothing in it, the region
  commands are greyed with no region selected, and the Options > Show/Hide menu shows a
  tick on the display options that are on. Those conditions are built in:
  a menu item is answered from one snapshot of the editor taken when the
  menu is drawn, so an item can ask about the editor but can't run code
  each time it's opened. A plugin's items are always enabled unless their
  command is undefined.
- `set-menu-bar(enabled)` / `menu-bar-enabled()` turn the bar on or off in
  the current buffer, like any other minor mode. From init.rhai,
  `forge.opt["menu_bar"] = true` turns it on everywhere, since the bar frames
  the window rather than belonging to a buffer.
- `open-menu(title)` / `close-menu()` / `open-menu-title()` drive it from
  the keyboard.
- `choose-menu-item()` runs the item under the cursor.
- `menu-selection()` is that item, with its index in the menu and whether
  it can run — for showing what it does.

Each item shows the key that runs it, resolved through the buffer's
keymap, so a shadowed or remapped binding is what you see. Unloading the
plugin takes its menu and its items away again.

## Keys and modes (0.1.3)

- `keymap.execute-keys("C-x C-f")` handles keys as if typed, after the
  call. A keypad works this way.
- `keymap.is-prefix(keys)` and `keymap.where-is(command)` answer "is this
  a prefix?" and "which keys run this?".
- `minibuffer.read-key(prompt)` reads one key; the answer, like "a" or
  "C-x" (none for C-g), arrives through `on-answer`.
- `commands.doc(name)` returns a command's documentation.
- `modes.set-lighter(mode, text)` sets what the mode line shows for one
  of the plugin's minor modes ("[N]"; "" hides it).
- `modes.major-mode-parent(mode)` returns the major mode a mode derives
  from, if any, so a plugin can walk the chain and inherit per-mode rules.
- A binding for `ESC` on its own (in a minor mode's map, say) makes ESC
  a key rather than the Meta prefix while that map is active.
- `keymap.push-transient(name, opts)` puts a map on top of every other
  layer for as long as it is up, the way transient maps work. Bind
  into it first with `keymap.bind-transient(name, keys, command)`, then
  push it; `pop-transient(name)` takes it down and `transients()` lists
  what is up. While a map is up, `where-is`, `describe-key`, prefix hints
  and the menu shortcuts see it, because it goes through the same lookup.
- `opts` is a list: `one-shot` ends the map after the first key it
  resolves, `timeout(seconds)` ends it after a while, and `on-unbound`
  says what a key the map doesn't bind should do — `stay` (pass it down),
  `swallow` (drop it, and keep the map up: right for a keypad that owns
  the keyboard), or `exit` (take the map down, then let the key resolve
  again in what's left).
- `keymap.bind-layer(name, when, keys, command)` binds into a conditional
  layer and sets the condition it applies while. A layer sits above the
  modes and below the buffer-local map, so a plugin can put keys in place
  without taking the user's own away, and without inventing a minor mode
  just to turn some keys on and off. `when` is written the way Forge prints
  a condition: `region-active`, `read-only`, `in-comment`, `in-string`,
  `in-git-repo`, `major-derives-from(MODE)`, `minor-mode-on(MODE)`, and the
  combinators `not(CONDITION)`, `all(CONDITION ...)` and
  `any(CONDITION ...)`. Meow's rule that special buffers keep their own keys
  is one of these: `not(major-derives-from(special-mode))`.
  `remove-layer(name)` drops one. A layer with no condition is never up, so
  binding into a name that was never set changes nothing.
- Every place that reads a key — the key itself, `where-is`, `describe-key`,
  the menu's shortcuts, prefix hints — sees the same layers, because they
  all go through the one lookup.
- Only built-in conditions: each is answered from a snapshot Forge takes
  once per keypress, since a syntax check and a git check would otherwise
  run per key. A predicate a plugin supplied would run on every keypress,
  so it isn't offered yet.
- `keymap.bind-prompt(name, keys, command)` binds into the prompt map
  `name`, which is named by what is asking: `find-file`, `goto-line`,
  `dired`, `m-x`, `completion-popup`, `completion-list`. A prompt with no
  map of its own uses the default one, so a binding here only shadows the
  key it names; anything the map doesn't bind is typed, which is what lets
  a plugin add a key of its own to the completion popup without taking the
  editing and history keys away. corfu's `M-g` and `M-h` work this way.
- `keymap.bind-region(name, keys, command)` binds into the region map
  `name`, the keymap a stretch of text brings with it: it applies while
  point is inside a decoration that names it, which
  `decorations.set-regions` attaches. A region map sits above the
  buffer-local map and below the conditional layers, so text that means
  something — a name in a listing, a magit section, a column header — can
  have keys of its own without a minor mode per kind of text. Regions move
  with edits and go with their text, like any other decoration.
  `remove-region(name)` drops one.
- A click is a key like any other, so `<mouse-1>` and `<mouse-2>` are
  keys a region map can bind (with the terminal's mouse support on). The
  click moves point first, then dispatches, so the map that answers is the
  one for the text under the pointer: this is how a name in a Dired listing
  opens on a click, and how a plugin's own clickable text can do the same.
- Keys are key notation: `C-`, `M-`, and — where the terminal can
  report them, which with the kitty keyboard protocol it can — `S-` for
  Shift, `H-` for Hyper, `s-super-` and `s-m-` for Super and Meta. Shift
  on a letter is written on the lower-case letter whichever terminal you
  are on (`S-a`), and Shift on a symbol is the character it makes (`S-2`
  is `@`), so a binding means the same key everywhere. `TAB` and `C-i` are
  one key, as are `RET` and `C-m`: a binding in either name answers for
  both, and one in both means itself.
- `keymap.bind-project(root, keys, command)` binds for every file under
  `root`, the way `.dir-locals.el` bindings work. The innermost project
  wins, so a repository inside a directory that has bindings of its own
  uses its own. A project map sits above the major mode and below the minor
  modes. `remove-project(root)` drops it.
- A *project* is the nearest directory above a file with a `.forge/` or a
  `.git` in it, so this is bindings that follow a checkout without the
  user having to name a path. Naming a directory is enough either way: a
  map for a directory with no marker in it still applies to the files
  under it.
- A project can carry its own `.forge/init.rhai`, which runs in the same
  Rhai state as your init.rhai. **It only runs once the user has granted
  it.** That is the `.dir-locals` problem, answered: code from a directory
  you may have just cloned is the user's decision, not Forge's.
  It is asked for and remembered in `~/.local/state/forge/plugin-grants.toml`,
  but in a section of its own rather than as one of a plugin's
  capabilities — a manifest can't ask for it, and it isn't one:

  ```toml
  [projects."/home/me/src/forge"]
  code = true
  ```

  `M-x revoke-plugin-permissions` forgets it (by `project:<directory>`),
  `M-x list-projects` shows it, and saying no isn't remembered across
  sessions while saying yes is.
- `keymap.set-priority(scope, n)` puts a layer at priority `n`, higher
  wins, and returns what it had been. A modal plugin uses this to sit its
  maps above the major mode without rebinding the keys it wants:
  `keymap.set-priority(keymap.scope.mode("my-normal-mode"), 425)`.
  `keymap.reset-priority(scope)` goes back to the default for that scope.
  The defaults, highest first: transient 1000, prompt 900, region 600,
  buffer-local 500, conditional 450, minor modes 400, project 350, major
  mode and its parents 300, global 100. Maps that share a priority keep
  the order they always had, so a number between two defaults is enough.

`forge --headless --plugin P.wasm --keys "M-x" --type "buf" --screen`
runs the whole editor without a terminal and prints the screen, for
testing plugins like these end to end. `--open`, `--wait` and `--size`
are there too.

## Background calls (0.1.2)

A slow command (a formatter, an indexer, a whole-buffer check) doesn't
have to freeze the editor. A command registered with
`commands.register-background` runs on another thread with a snapshot of
the editor, and Forge doesn't wait for it. The same goes for event
handlers subscribed with `events.subscribe-background`. The mode line
shows `NAME...` while it works.

When it returns, what it did is merged in:

- **Its buffer edits.** If the buffer didn't change meanwhile, they're
  replayed exactly, as one undo step. If you edited the buffer too, the
  plugin's change and yours are each reduced to the span they touched.
  The plugin's change is applied if the two spans don't overlap, and
  dropped with a message if they do. Your edits always stay.
- **Decorations** it set or cleared, moved along with those edits.
- **Messages, questions, logs and commands to run**, as from any call.

Calls to a plugin while it's busy wait their turn, in order.

In a background call a plugin can read everything and edit text, set
decorations, ask questions and run processes. It can't register commands,
bind keys, define modes, subscribe, set options, or create, show, visit or
kill buffers. Those that return a `result` fail with an error; the others
trap. Point and mark moves aren't kept. A background call may run for 60
seconds, not 5.

In the SDKs this is `command_background` / `on_background` (Rust),
`{ background = true }` (Lua), `{ background: true }` (JavaScript) or
`forge_command_background` (C).

## Processes (0.1.2)

A plugin can run programs: a language server, a formatter, a linter.
`processes.spawn(program, args, dir)` starts one (in `dir`, or the project
of the current buffer) and returns its id; `write` sends bytes to its
stdin, `close-input` ends it, `kill` stops it. Output arrives through the
`process-handler` export (`on-output`, then `on-exit` with the exit code),
delivered when the editor is idle, never in the middle of another call.
Stderr goes to the plugin's log (`*Messages*`, debug level). A plugin's
processes are killed when it's unloaded. `project-root(path)` finds the
project a file is in (the nearest directory with `.git`, else with a build
file).

A plugin that runs processes targets the `plugin-with-processes` world
(`plugin` plus `export process-handler`). Starting a program needs the
`processes` permission naming it (below): that trusts the plugin a lot, as
the program runs outside the sandbox with your rights.

`buffer.visit(path)` opens a file in a buffer without showing it (for
go-to-definition). It needs a directory grant covering the file, or the
processes permission.

The LSP client (`forge-plugin-lsp`) uses all of
this.

## Files, idle work and the mode line (0.1.10)

- **The `files` interface works on paths, not buffers.**
  - `files.exists(path)` and `files.read(path)` read; `~` is your home.
  - `files.create(path, overwrite)` makes an empty file and its missing
    directories.
  - `files.write(path, data)` writes bytes (a downloaded picture, say) to a
    file, making it and its missing directories. Since 0.1.26.
  - `files.rename(source, target, overwrite)` moves a file. Buffers
    visiting it, or files under it, follow along.
  - `files.delete(path, recursive)` deletes.
  - Reading needs a directory grant covering the path, or the `processes`
    permission. Writing needs a writable grant, or the `processes`
    permission.
- **`commands.run-when-idle(name, seconds)`** runs one of the plugin's own
  commands once each time the editor has been idle for that long. The
  idle time counts keys, clicks and the wheel as input, not pointer
  motion, and the hook doesn't run while the minibuffer is open.
  - Calling it again with the same command replaces the delay.
  - A delay of 0 or less removes the hook.
  - The Rust SDK's `when_idle(name, doc, seconds, run)` registers the
    command and the hook together.
- **`minibuffer.current-message()`** is what the echo area shows now, so
  idle documentation (eldoc) can leave someone else's message alone.
- **Mode-line info.** `buffer.set-mode-line-info(text)` adds the plugin's
  text to a buffer's mode line, after the VC state (Cairn's
  `[cairn:project] 1E 0W`). `none` removes it, and `buffer.mode-line-info()`
  reads it back. Each plugin has its own slot, cleared when the plugin is
  unloaded.
- **The `symbol-highlight` face** marks the other occurrences of the
  symbol at point.

## Permissions

A plugin can always use the editor API. Anything beyond that needs the
user's permission, asked for in its manifest. The manifest is TOML, kept
in the component itself (a `forge-manifest` custom section; the Rust SDK
writes it from Cargo.toml, see Building) or in a file next to it
(`spell.wasm` -> `spell.toml`). If both exist the file wins, so a user can
override a plugin without rebuilding it.

```toml
# Root keys come before the first [table].
startup = false                 # true: never start lazily (see Loading)
requires = ["org"]              # other plugins this one needs (loaded first)
modes = ["org-modern-mode"]     # what it provides, so Forge can load it
extensions = ["org"]            # lazily without running it (see Loading)

[package]
name = "spell"                  # optional; defaults to the file name
version = "0.3.0"
description = "Spell checking"  # one line, shown by describe-plugin
homepage = "https://example.com/spell"
license = "MIT"
doc = "README.org"              # long description, a file next to the plugin

# What the plugin provides, documented so the host can show it before the
# plugin runs (and set up lazy loading). The plugin still registers these
# itself when it runs. The older bare form, commands = ["spell-check-buffer"],
# is still accepted.
[[commands]]
name = "spell-check-buffer"
summary = "Check the whole buffer."
doc = "..."                     # optional long text (see Doc markup)

[[options]]
name = "spell-language"
type = "string"
default = "en_US"
summary = "The dictionary to check against."

[permissions]
network = true                  # wasi:sockets: connect, look up names
http = true                     # make web requests (see Web requests)
environment = true              # read the process environment (see below)
memory-mb = 256                 # default 128, at most 4096
processes = ["aspell"]          # programs it may run ("*": any)
[[permissions.directories]]
path = "~/.local/share/dict"
why = "word lists"              # shown to the user
# write = true                  # read-only unless asked

# What the plugin reads through forge.plugin.config. The host checks the
# values against these before the plugin loads, so a typo or the wrong
# type is refused (naming the setting) instead of reaching the plugin.
# The values show in M-x describe-plugin, and an editor can complete them.
[[settings]]
name = "progress"               # the key under forge.plugin.config("name", ...)
kind = "boolean"                # boolean, number, string, list, table, any
default = false
doc = "Show progress in the mode line"
```

A manifest with no `settings` accepts any config (the plugin validates its
own keys, as before); a manifest with settings refuses unknown names and
wrong types before loading.

When a plugin asks for something it hasn't been granted, Forge shows the
request and every interface the component imports, and asks: `y` grants
and loads, `n` loads it without those permissions, `C-g` doesn't load it.
Answers are kept in `~/.local/state/forge/plugin-grants.toml`, bound to the
plugin's binary; Forge asks again when a plugin wants something new or the
same name points at a different binary. Users can also grant in init.rhai,
and then aren't asked at all:

```rhai
forge.plugin.grant("spell", #{ directories: ["~/.local/share/dict"], memory_mb: 256 });
```

A plugin gets the union of both. `M-x list-plugins` shows what each plugin
has and where each grant came from, and `M-x revoke-plugin-permissions`
forgets the interactive ones.

Without permission, a plugin sees no directories, every network call is
refused, and reading the environment fails (see below). It gets the
errors WASI defines for that (no preopened directories, `access-denied`),
not a crash, so it can carry on without the feature. Granted directories
appear under their real paths. Stdout and stderr always work; they go to
`*Messages*`. Clocks and random numbers are always available.

## Sharing your configuration

When syncing a Forge configuration, share `init.rhai` and `forge-lock.toml`.
Keep per-machine approvals, installed plugin binaries, saved sessions and
credential files private. Interactive plugin approvals are stored in
`~/.local/state/forge/plugin-grants.toml`; an older config-directory file is
migrated on first use.

| Share | Keep local |
|---|---|
| `init.rhai`, `forge-lock.toml` | `plugin-grants.toml`, authinfo/netrc, `plugins/`, `session.toml` |

Add these rules to the `.gitignore` for a configuration or plugin workspace:

```gitignore
# Forge machine-local state and credentials
forge-lock.local.toml
plugin-grants.toml
plugins/
session.toml
authinfo
.authinfo
.authinfo.gpg
netrc
.netrc
```

Treat an unfamiliar `init.rhai` like executable code, not passive settings.
Read it before running or copying it: it can run programs, load plugins and
request access to files, processes or the network. Review the referenced
plugin sources and lock entries as well.

## The environment (0.1.13)

The `environment` interface reads the environment Forge itself runs with
(the same one `forge.env` shows in init.rhai, and the one a
process from `processes.spawn` inherits):

- `environment.get(name)` is the variable's value, or none when it is
  unset.
- `environment.variables()` is every variable as a `(name, value)` pair,
  in order.

Variables often hold secrets (tokens, credentials), so reading them needs
the `environment` permission: `environment = true` in the manifest (or
`forge.plugin.grant("name", #{ environment: true })` in init.rhai). Without
it, both calls return an error naming the permission; the plugin keeps
running. A plugin can't set a variable: change the environment Forge runs
in, or set variables per process with `processes.spawn-with` (0.1.9).

The rest of the interface is where Forge runs, which a plugin needs to
adapt to and which needs no permission. These are the same values as the
`forge.system` namespace in init.rhai:

- `system-type()` is the kind of system: `"gnu/linux"`, `"darwin"`,
  `"windows-nt"`, `"gnu"`.
- `system-configuration()` is the name Forge was built for, e.g.
  `"x86_64-unknown-linux-gnu"`.
- `system-name()` is this machine's hostname.
- `pid()` is this process's id.
- `null-device()` is where output is discarded: `"/dev/null"` or `"NUL"`.
- `path-separator()` is `":"` or `";"`.
- `invocation-name()` is the name Forge was invoked as.
- `parse-path(path)` splits a path list on `path-separator()` into its
  directories (an empty component, meaning the current directory, comes
  back as `""`).

Interfaces Forge doesn't provide at all (`wasi:http`, anything unknown)
don't stop a plugin loading, but calling one fails with an error that
names it and the plugin is unloaded. A missing `forge:editor` function is
different: see Versions below.

## Sharing plugins

A package index is a TOML file (a local path, `file://` or `https://`)
listing plugin components with their sha256:

```toml
[[package]]
name = "lsp"
version = "0.1.0"
description = "Language Server Protocol client"
url = "lsp-0.1.0.wasm"      # relative to the index, or a full URL
sha256 = "9f2c..."
release_notes = "Fixes reconnect handling."
```

`forge --package-entry lsp.wasm [--url URL]` prints the entry for a
built plugin: the name, version and description come from its manifest.
Point Forge at one or more indexes in init.rhai:

```rhai
forge.opt["package_index"] = "https://example.org/forge/index.toml";   // or a list
```

An index can have a detached [Minisign](https://jedisct1.github.io/minisign/)
signature beside it, named `index.toml.minisig` (Forge adds `.minisig`
to the index location, before any query or fragment). Put the public
keys in `package_index_keys`; Forge verifies the exact downloaded bytes
before parsing the TOML and accepts a signature from any listed key, so
you can add a new key before removing the old one:

```rhai
forge.opt["package_index_keys"] = ["RWQ...", "RWQ..."];   // or one key
forge.opt["package_index_strict"] = true;                 // optional
```

When keys are configured, a signature that is present must verify; an
unsigned index is still accepted unless strict mode is on. Strict mode
refuses unsigned indexes and needs at least one key. Sign an index with
`minisign -S -s secret.key -m index.toml`; the signature is written as
`index.toml.minisig`.

To restrict where indexes and packages may come from, set
`package_allowed_sources`. An unset option leaves sources unrestricted; an
empty list denies all. Entries are exact hosts (`github.com`, any scheme or
port), subdomain suffixes (`*.example.org`, not the apex), exact sources
(`https://example.org/forge/index.toml`), or directory prefixes ending in
`/` (`https://example.org/forge/`, `/srv/forge/pkgs/`, `file:///srv/forge/`).
A URL matches the same scheme, host and port; an exact index URL also covers
its `.minisig` sibling, and HTTP redirect targets must match the same list.
Allowing an index does not implicitly allow the packages it lists, so use a
host or directory-prefix rule for those.

```rhai
forge.opt["package_allowed_sources"] = [
  "github.com",
  "https://example.org/forge/",
];
```

For Git packages, `package_repository_checks` (off by default) asks GitHub
whether the repository is archived or has moved to another owner/name. The
checks are advisory: failures are ignored and warnings never block an
install or upgrade.

The commands:

- `M-x list-packages` shows what's available and installed. In that
  buffer, RET installs or upgrades, `d` deletes, `g` refreshes and `U`
  upgrades everything.
- `M-x package-install`, `package-delete`, `package-upgrade-all` and
  `package-refresh-contents` do the same from anywhere.
- `M-x package-upgrade NAME` upgrades one installed package after showing
  index release notes and asking for confirmation. If the new manifest asks
  for capabilities the lockfile did not record, the review lists the
  additions and asks again; removals are only listed.
- `M-x package-upgrade-all-dry-run` lists changed versions or commits and
  the build commands at the new revision without changing plugin or data
  files. `C-u M-x package-upgrade-all` shows the same plan.
- Automatic upgrades skip declarations with `pin` or `freeze: true`.
  Index declarations may select a semver range, for example
  `version: "^1.2"`.

Downloads happen in the background. A download whose sha256 doesn't
match the index isn't installed. Installed plugins go in the plugins
directory, recorded in `packages.toml` there, and load like any other:
Forge asks for their permissions. An unattended upgrade (`--batch` or
`--headless`) stops when it would add capabilities: it reports what changed
and leaves the installed plugin and lockfile as they were.

There is no central index yet: any static file host (a Git repository's
raw files, GitHub Pages, S3) can serve one.

### From version control

`M-x package-vc-install` takes a Git repository, `URL` or
`URL@REV` (a tag, branch or commit; default: the default branch). A
local path works too. Forge clones it into `~/.local/share/forge/vc/NAME`
(`$FORGE_DATA_DIR/vc/NAME`), builds it and installs the component. NAME
is the repository's name without `.git` or a `forge-` prefix. An index
entry can point at a repository instead of a download:

```toml
[[package]]
name = "lsp"
version = "0.1.0"
git = "https://github.com/you/forge-lsp"
rev = "v0.1.0"                  # optional
```

How it's built, in order:

1. `forge-package.toml` in the repository: `build = "shell command"`
   (it gets the output path in `$OUT`) and/or `wasm = "path"` (a
   prebuilt component, or where the build leaves it), and optionally
   `name`.
2. An executable `build.sh`, run as `./build.sh -o "$OUT"`. That's what
   the Lua and C SDKs' scripts take.
3. `Cargo.toml`: `cargo build --release --target wasm32-wasip2`.
4. `package.json` with a `"forge"` field: `npm ci --ignore-scripts`
   (the repository must commit `package-lock.json` or
   `npm-shrinkwrap.json`), then `npx --no-install forge-plugin build`.
5. Exactly one `.wasm` at the top or in `dist/`, used as is.

A build runs code from the repository with your rights, so Forge shows
the command and asks first. It asks again only if the command changes.
Git's and the build's output go to `*Package Build*`.

Builds get a scrubbed environment. By default they inherit only `PATH`,
`HOME`, `USER`, `LOGNAME`, `LANG`, `LC_*`, the temporary-directory
variables, the Rust toolchain variables (`CARGO_HOME`, `RUSTUP_HOME`),
the Windows system variables, and proxy/certificate variables such as
`HTTP(S)_PROXY` and `NODE_EXTRA_CA_CERTS`. Tokens such as `NPM_TOKEN` and
`SSH_AUTH_SOCK` are not passed. A declaration can add names or `PREFIX_*`
patterns with `build_env` and change the limit with
`build_timeout_secs` (default 30 minutes); a build that runs longer is
killed and reported as timed out. `*Package Build*` lists the variable
names passed through, never their values. Files under `HOME` are still
readable. Because `npm ci` runs with `--ignore-scripts`, a package whose
dependencies need a `postinstall` step (native modules) will not build;
the author can commit a reviewed `build = "..."` instead.

`M-x package-vc-upgrade` and `package-vc-upgrade-all` fetch, check out
the same rev again (a branch moves on, a tag stays unless it was moved)
and rebuild only if the commit changed. Forge shows commit subjects before
applying and flags `BREAKING` markers. It also warns when the new commit is
not a descendant of the locked commit (a force-push or rewritten history)
and when a pinned tag now points to a different commit. These warnings do
not block an interactive upgrade. Unattended runs stop on a rewritten
history before installing anything unless the declaration sets
`allow_history_rewrite: true`; moved-tag warnings are informational.
Ancestry needs commit objects, so Forge fetches old and new commits into a
scratch repository rather than changing the real checkout. Before
installing the built component it applies the same capability diff as an
index upgrade. Unattended runs also stop on capability additions; a
checkout and build may already exist, but the installed component and lock
entry are not replaced. `C-u` on either Git upgrade command shows a plan
using `git ls-remote` for revisions and tags and a scratch repository for
ancestry, without fetching into or changing the real checkout, and detects
the build command in the remote revision rather than reusing the installed
one. Failed builds, smoke tests or lock recording restore the previous
artifact, installed record and lock entry. A smoke test loads the candidate
in an isolated editor, runs its `init`, and checks that manifest commands
registered. `package-upgrade-all` includes Git packages and rolls back the
whole run if any package fails; `package-delete` removes the clone too.
`packages.toml` records the repository, rev, commit and build command of
each.

Updates keep a journal beside the Forge data directory. If Forge exits
before an update finishes, the next start restores the previous artifact,
installed record and lock entry. A failed recovery keeps the journal and
blocks further package updates until it is repaired.

An index entry can include a `release_notes` string; Forge displays it in
the review buffer before asking to apply an update. Semver requirements use
standard Rust semver syntax (`^1.2`, `~1.2.3`, or an exact comparator).

### Package operation log

`M-x package-log` shows one line per completed package operation: fetch,
build, install, upgrade, and rollback, with its duration and any recorded
source, revision, commit, version, command or error. `M-x package-log-export`
shows the same records as JSON lines and asks where to write them.

Records use the additive schema `forge.package.operation`, version `1`.
Readers should ignore unknown fields. Environment values are never recorded.
URL credentials, sensitive query values, authentication headers, and known
token shapes are redacted before a line is written.

The log lives in the state directory as `package-operations.jsonl`
(`$XDG_STATE_HOME/forge` or `~/.local/state/forge` by default). When it
reaches 1 MiB, it rotates to `package-operations.jsonl.1`; both files are
read for the buffer and export. Each record is appended as one JSON line.
A partial final line left by a killed process is dropped before the next
append.

### Lockfiles and snapshots

`forge-lock.toml` next to `init.rhai` records each declared plugin's
resolved source, commit and artifact hash, plus its declaration hash,
source-build recipe hash, tier and capabilities. Older lockfiles without
`lock_version` continue to load. Unknown fields are retained when Forge
rewrites a lockfile. A malformed shared or local lock file is reported with
the file name and parse problem, and Forge leaves the file untouched.

An optional ignored `forge-lock.local.toml` in the same directory overlays
shared entries by plugin name. Keep machine-specific changes there rather
than changing the shared lockfile.

- `M-x package-snapshot-save` saves the shared lock and its verified
  installed artifacts under a name, including components loaded from a
  custom `load_path`.
- `M-x package-snapshot-list`, `package-snapshot-diff` and
  `package-snapshot-restore` list, compare and restore saved snapshots.
  Restoring a snapshot also removes package-managed plugins installed
  after it.
- `M-x package-restore` puts locked artifacts back from Forge's local
  content-addressed artifact store. It reports when a resolved commit or
  artifact hash differs. If an artifact is not available locally, restore
  stops with an error rather than resolving a different version. It also
  removes package-managed plugins that `forge-lock.toml` does not list;
  hand-copied plugins are left alone. A rerun after an interrupted restore
  converges to the same files and lockfile.
- `forge package declarations [--json]` collects `use_package`
  declarations without starting the editor; closures are reported, not
  run.
- `forge package sync [--json]` installs what `init.rhai` declares,
  honoring the commits and artifact hashes in `forge-lock.toml`, without
  prompting. A locked package is fetched at its locked commit or URL when
  it is not in the artifact cache, and the result must match the locked
  sha256. Operations that need consent stop with exit status 3 unless
  `--allow-build`, `--allow-capabilities` or `--allow-grants` is given.
  The same approvals can be set with `forge.opt["package_allow_build"]`,
  `forge.opt["package_allow_capabilities"]` and
  `forge.opt["package_allow_grants"]`.
- `forge package snapshot diff BEFORE AFTER --json` compares snapshots
  without starting the editor and emits JSON.
- `forge package lockfile diff BEFORE AFTER --json` compares two
  lockfiles by path, without local overlays, and emits JSON.

## Limits

- **Time:** a single call (init, a command, an event, an answer) may run
  for 5 seconds (60 in the background). After 150 ms the echo area shows "NAME is busy... (C-g to
  stop)"; C-g stops it at once. Keys typed meanwhile are kept and handled
  afterwards. A stopped plugin is unloaded.
- **Memory:** 128 MB unless granted more (`memory-mb`). Growing past the
  limit fails the way running out of memory does in WebAssembly
  (`memory.grow` returns -1).

## What plugins may not do

- Bind `C-c` followed by a plain letter, or bare `C-c`: those keys belong
  to the user. `C-c C-x`, `C-c .` and everything else is fine.
- Take a name that's already used: a built-in command, another plugin's
  command or mode, or one defined in init.rhai. A plugin may re-register
  its own (when reloaded).

## Loading

Plugins in `~/.config/forge/plugins/` load at startup, as `NAME.wasm` or
`NAME/NAME.wasm` (a directory for plugins that come with other files).
`M-x load-plugin` and `forge --plugin FILE.wasm` load one from anywhere.

**Compiled code is cached** in `~/.cache/forge/wasmtime` (or
`$XDG_CACHE_HOME/forge`, or `$FORGE_CACHE_DIR`), so a plugin is only
compiled again when it or Forge changes.

**Plugins start lazily.** After a plugin's init runs, Forge writes down
what it set up: its commands (and their docs), key bindings and
remappings in global and mode keymaps, modes, and the events it subscribed
to. At the next start, if the file and its grants are exactly the same,
those come from that summary and the plugin isn't compiled or run at all.
It starts the first time one of its commands runs, an event it
subscribed to happens, one of its modes becomes a buffer's major (or
minor) mode, or a file with one of its extensions is opened, then handles
that command or event as usual. So an installed plugin costs almost
nothing until it's used.

`use_package` takes the rest of use-package keywords too: `disabled`,
`when`/`"if"`/`unless` (a boolean or a function), `requires`, `preface`,
`init`, `config` (a map of settings or a function), `custom` (`forge.opt`
values), `"catch"`, `bind`, `bind_mode`, `bind_prompt` (keys in the `prompt`, `isearch` or `completion-list` map, for `:map minibuffer-local-map`), `remap`, `mode`, `delight`, `demand`, `vc`,
`ensure`, `ensure_system_package`. `use_package_report()` returns one line
per declaration saying whether it was skipped, deferred or loads at startup.
`docs/init.example.rhai` describes each. The remaining use-package keywords
work under their underscore names: `pin` (the git revision; `use_package` has no `rev` or `on` key, those are `forge.plugin.declare`'s), `load_path`, `no_require`,
`autoload`, `diminish`, `bind_star`, `interpreter`, `magic`, `magic_fallback`,
`custom_face`. `vc` is a git url or a map like `:vc`: `#{ url, rev, make, lisp_dir, features }`, where `make` is the build command (given `$OUT`), `lisp_dir` the subdirectory to build in, and `features` cargo features to turn on. `build_env` (names or `PREFIX_*` patterns) and `build_timeout_secs` are top-level keys on the same declaration. The build still asks before it first runs.

A plugin declared with `use_package` or `forge.plugin.declare` can say the
same up front, with `on` (see the plugin API), so it starts lazily without
needing a summary from a previous run: the declaration's `on.commands`, `on.modes` and
`on.extensions` (together with what the manifest lists under `commands`,
`modes` and `extensions`) are what wakes it.

Init therefore must not depend on *when* it runs. Forge only starts a
plugin lazily if its init did nothing but register things. One that shows
a message, asks a question, runs a command, creates a buffer or binds keys
in a single buffer always starts at startup. So does one whose manifest
says `startup = true`. Deleting `~/.cache/forge` is always safe.

### Profiling plugin startup

`M-x plugin-profile` shows where the time went: for each plugin, the cache
lookup, a compile on a cache miss, `init`, lazy start, and every command,
event, answer, completion and process/HTTP callback it handled (count, total
and longest). The same report is printed by `forge --profile-startup`, which
exits without opening a terminal; add `--json` for the machine-readable
report. Profiling starts with the plugin-loading phase, after init.rhai.

Set `forge.opt["startup_budget_ms"] = 250` in init.rhai to warn in
`*Messages*` when plugin startup exceeds that many milliseconds. Batch and
headless runs exit nonzero when the budget is exceeded, and
`forge --profile-startup` exits nonzero for a report over budget.

Profiling is off by default, including when a budget is not set. While it is
off the hot paths only read one atomic boolean: no clock is read, no lock is
taken and no label is allocated. Tests use a manual clock, so a fixture's
report is deterministic.

**Reloading.** `M-x reload-plugin` loads a plugin again from its file.
Forge also watches the files of loaded plugins and reloads one when it's
rebuilt (once the file has stopped changing for a second), so
`cargo build` is enough to try a change. `forge.opt["plugin_auto_reload"] =
false` in init.rhai turns this off. Reloading is unloading and loading:
the plugin starts fresh, and a plugin whose new init fails stays unloaded
until the next rebuild.

## Unloading and crashes

`M-x unload-plugin` removes everything the plugin added: its commands, its
modes, and the key bindings and remappings it made (unless something else
replaced them since). A plugin that traps (a Rust panic, for example) is
unloaded the same way, and the error goes to `*Messages*`. Anything it
printed to stdout or stderr is in `*Messages*` too.

## Settings from init.rhai (0.1.12)

A plugin's settings are the user's to give, in init.rhai, and the plugin's to
read:

```rhai
forge.plugin.config("org-modern", #{
  star: ["◉", "○"],
  progress: false,
});
```

Any nesting of tables, lists, text, numbers and booleans works. They are
kept by plugin name, not in `forge.opt` (that is for the editor's own
options), and a plugin sees only its own. `config.get()` returns them as a
JSON object, `{}` if there are none; the Rust SDK has `config_json()`.
init.rhai runs before plugins load, so read them in `init`: a change takes
effect the next time the plugin loads (`M-x reload-plugin`). Forge doesn't
know what a plugin accepts, so the plugin should refuse a key it doesn't
know, naming it, by returning an error from `init`: the plugin doesn't
load and the message says why, so a typo isn't silent. Later
`forge.plugin.config` calls for a plugin replace earlier ones, and
`M-x reload-init-file` forgets them before init.rhai runs again.

## Versions and compatibility

The package is `forge:editor@0.1.31`. 0.1.1 added buffer options,
`indent-line` and the `decorations` interface; 0.1.2 added `processes`,
the `process-handler` export (world `plugin-with-processes`) and
`buffer.visit`, and background calls (`commands.register-background`,
`events.subscribe-background`). 0.1.3 added completion, `read-key`,
`execute-keys`, `is-prefix`, `where-is`, `commands.doc`, `set-lighter`
and the `cursor` option. 0.1.4 added `completion.set-sorting`,
`completion.selected` and `completion.set-popup-frame`. 0.1.5 added the `menus` interface, so a
plugin can add its own menus to the menu bar. 0.1.6 added
`keymap.set-priority` and `keymap.reset-priority`, so a plugin can place
its own map among the layers instead of only binding keys, plus the
transient maps (`bind-transient`, `push-transient`, `pop-transient`,
`transients`), prompt maps (`bind-prompt`) and conditional layers
(`bind-layer`, `remove-layer`). 0.1.7 added region maps
(`keymap.bind-region`, `keymap.remove-region`),
`decorations.set-regions`, so a stretch of text can bring keys with it and
a click can be bound like any other key (`<mouse-1>`, `<mouse-2>`), and
project maps (`keymap.bind-project`, `keymap.remove-project`), for
bindings that follow a checkout. 0.1.8 added `menus.extend-submenu`, so
a plugin can hang its own submenu off a menu (a missing label becomes a
submenu header). 0.1.9 added `processes.spawn-with`, to set a program's
environment (`GIT_EDITOR`, `LC_ALL`, ...), and `processes.error-output`,
which hands `on-exit` what the program wrote to stderr, so a plugin can
say why a command failed; and the `read-only` buffer option, which
refuses commands' edits while the plugin's own writes still go through;
and a transient map pushed with `on-unbound: swallow` (a keypad, a popup
of commands) has its keys listed in a panel while it is up, each by the
first sentence of its command's doc. 0.1.10 added the `files` interface,
`commands.run-when-idle`, `minibuffer.current-message`,
`buffer.set-mode-line-info` and `buffer.mode-line-info`, and the
`symbol-highlight` face. 0.1.11 added `decorations.set-display`, which
shows a stretch of text as something else or hides it (the buffer's text
is unchanged, and point's line is always shown as it is), and the
`org-modern-*` faces. 0.1.12 added the `config` interface: `config.get`
returns the settings the user gave the plugin in init.rhai with
`forge.plugin.config(name, { ... })`, as a JSON object (`{}` if none).
0.1.13 added the `environment` interface (see The environment above):
reading the process environment needs the `environment` permission, and
the system facts (`system-type`, `system-configuration`, `system-name`,
`pid`, `null-device`, `path-separator`, `invocation-name`, `parse-path`)
need none.
0.1.14 lets a plugin drive the completion popup at point (see Completion
above): `completion.current` reads the session,
`completion.set-options` sets its policy (cycling, the separator, the
`quit-at-boundary` and `quit-no-match` rules, `continue-commands`,
indexed rows, the quickselect keys, echo, mouse and history), and
`completion.next`, `previous`, `first`, `last`, `scroll`, `accept`,
`expand`, `quit`, `reset`, `accept-row`, `arm-quick` and
`insert-separator` move and act on it. The host owns no corfu command;
the corfu plugin drives this API.
0.1.15 lets a source decorate its candidates (see Completion above):
`completion.deliver-decorated` answers a request like `deliver` but each
candidate may carry a `margin` (an icon or a short kind, shown before it)
and say it is `deprecated` (drawn dimmed / struck through). The popup also
highlights the part of each candidate the typed input matches. A plugin
that doesn't decorate keeps using `deliver`.
0.1.16 lets a source give everything a completion-at-point source can:
`completion.deliver-detailed` adds `documentation` and a `location` to the
decorated candidate, `completion.selection` carries them for the candidate
under the cursor, and a `completion-handler` may export
`commit(candidate, status)` to hear when a candidate is inserted (the CAPF
exit status). The new fields come after the old ones, so a plugin built
against an earlier 0.1.x keeps working.
0.1.17 lets a completion plugin ask where the selection is defined:
`completion.locate` asks the source, which exports
`completion-handler.resolve-location(selection)` and answers later (the
corfu plugin binds it to `M-g`). The selection's `location` becomes a
`{path, line, column}` record, so a place in another file can be named.
This is the one place a 0.1.x change alters a type older plugins already
import: a source built against 0.1.16 that fills in a `location` must be
rebuilt (Forge rebuilds its own; the shape of the record changed). Plugins
that don't fill in a location, and those built against 0.1.0 through
0.1.15, load unchanged (Forge's tests load a 0.1.0 plugin next to newer
ones).
0.1.18 lets a completion plugin add an auxiliary popup beside the popup
at point holding the selection's documentation and/or location (see
Completion above): `completion.set-popup-info` configures it,
`completion.toggle-popup-info` shows or hides it,
`completion.scroll-popup-info`, `popup-info-beginning` and
`popup-info-end` move through it. The editor draws it, so it looks the
same in the terminal and the app; the corfu plugin binds it to
`corfu-popupinfo-mode`.
0.1.19 adds `completion.set-minibuffer-cycle`, so a plugin (vertico) can
say whether the minibuffer list wraps around at the ends. Everything else
needed to move and take the list was already there, so a plugin built
against an earlier 0.1.x loads unchanged.
0.1.20 adds the selection-changed hook: `completion.on-selection-changed`
has the host run one of the plugin's own commands whenever the selection
in an open list moves (C-n, C-p, typing that filters, the list opening or
closing), so a command can preview the candidate under the cursor
(`completion.selected` reads it). consult's live preview is built on it.
0.1.21 adds read-only access to what Forge keeps around, for jumping
commands: the `kill-ring` interface (`length`, `entries`, `current`,
`get`, `rotate`, `select`) reads and moves through the clipboard history;
the `session` interface (`history`, `bookmark-names`, `bookmark`,
`recent-files`) reads the minibuffer history, bookmarks and recent files
Forge remembers between sessions; `buffers.buffer.definitions` gives the
buffer's imenu definitions (name, kind and position) from its grammar's
tags query; and `commands.names` and `keymap.bindings(scope)` list every
command and the keys bound directly in a keymap. consult's
`consult-yank-from-kill-ring`, `consult-bookmark`, `consult-recent-file`,
`consult-imenu` and `consult-mode-command` use them. All are additions,
so a plugin built against an earlier 0.1.x loads unchanged.
0.1.22 adds more read-only state, for the rest of consult's jumping
commands: the `registers` interface (`length`, `names`, `describe`,
`text`) reads the C-x r registers; the `kmacro` interface (`length`,
`last`, `entries`, `select`) reads the keyboard macros and makes one the
last, so `call-last-kbd-macro` runs it; the `marks` interface
(`buffer-marks`, `global-marks`, `pop-mark`) reads the mark rings the
editor keeps as you set marks (C-SPC, C-u C-SPC); `decorations.get`
returns every decoration on a buffer, so a plugin can read back the
editor's own error/warning/info diagnostics; and the `xref` interface
(`publish`, `all`) is a small store where a language plugin publishes the
definitions and references it found, for a jumping command to list. The
LSP plugin publishes its `M-.`/`M-?` results; consult's `consult-register`,
`consult-kmacro`, `consult-mark`, `consult-global-mark`, `consult-flymake`
and `consult-xref` use them.
0.1.23 adds `registers.position`, which returns where a position register
(C-x r SPC) points as the same `position-mark` the `marks` interface uses,
so a plugin can jump to it rather than only read its description. consult's
`consult-register` uses it. Like the rest, it is an addition, so a plugin
built against an earlier 0.1.x loads unchanged.
0.1.24 adds three things. The `theme` interface lets a plugin define named
themes (`theme.define`, `undefine`, `names`, `describe`), apply one
(`theme.load`), and drop the applied one (`theme.disable`); init.rhai and
the `~/.config/forge/themes/*.rhai` files use the same registry through Rhai
(`forge.theme`), and the editor commands `load-theme`, `disable-theme` and
`list-themes` drive it. The `org` interface reads the Org agenda engine:
`org.files` lists the configured agenda files, and `org.span`, `org.todos`,
`org.matching`, `org.search` and `org.stuck` each return a list of
`agenda-entry` (the buffer, line and display fields of a selected heading),
so a plugin can list and jump to them. And the `minibuffer` additions —
`active`, `history-kind`, `input` and `completing-read-over` — let a plugin
read the prompt that is open and offer its own history, taking the prompt
over while it runs and handing the pick back to it. consult's
`consult-theme`, `consult-org-agenda` and `consult-history` use them, and
`consult-isearch-history` uses the same calls during Isearch: the search
records each string in the `search` history, `minibuffer.history-kind`
reads as `search` while searching and `minibuffer.input` is the search
string, and a command bound in the `isearch` prompt map runs without
ending the search, with the pick resuming it. All are additions, so a
plugin built against an earlier 0.1.x loads unchanged.
0.1.29 adds `menus.define-for-mode`, so a menu can be tied to a major mode
and hidden while that mode is off, and `kill-ring.push`, which puts text
on the kill ring and the clipboard as M-w does (Rhai's `forge.kill` is the
same call). It is an addition, so a plugin built against an earlier 0.1.x
loads unchanged.
0.1.30 adds `minibuffer.on-input-changed` and
`completion.on-selection-changed`, so a plugin's command runs whenever the
text or the selected candidate in an open prompt changes.
0.1.31 lets a plugin shape the prompt list itself:
`completion.set-layout` (vertical, flat, grid, reverse, unobtrusive or
buffer), `completion.group` with `completion.set-group-format`,
`completion.set-row-labels` and `completion.jump-to-row`, and
`completion.sessions` with `completion.resume` so prompts can be suspended
and brought back. `minibuffer.set-input` replaces the open prompt's text,
`minibuffer.suspend` hides it, and `minibuffer.on-open` runs a command
after a prompt opens. All are additions, so a plugin built against an
earlier 0.1.x loads unchanged.

Completing in the minibuffer and moving a popup completion there need no
WIT either: both build on the prompt list (which
`completion.set-minibuffer-ui` has always chosen) plus one host command,
`completion-popup-move-to-minibuffer` (M-m in the popup), so they look the
same in the terminal and the app. The corfu plugin exposes them as the
off-by-default `corfu-minibuffer-mode` and `corfu-move-to-minibuffer`.

Moving the minibuffer's list needs no WIT: `completion.first` /
`.last` / `.next` / `.previous` / `.scroll` (0.1.14, written for the popup)
drive whichever list is active, so with no popup open they move the prompt's
list. The vertico plugin wraps them as `vertico-first` / `-last` / `-next` /
`-previous` / `-scroll-up` / `-scroll-down` and binds M-< / M-> in the
`completion-list` map; `completion.set-minibuffer-cycle` (0.1.19) makes
`vertico-next` / `-previous` wrap at the ends.

Note that 0.1.6 and 0.1.7 add functions to `keymap` but no case to the
`scope` variant: adding a variant case changes a type that older plugins
already import, which would stop them loading. Bind into a transient map
with `bind-transient(name, ...)`, into a prompt map with
`bind-prompt(name, ...)`, into a conditional layer with
`bind-layer(name, ...)`, into a region map with `bind-region(name, ...)`
and into a project's with `bind-project(root, ...)` rather than
`bind(scope.transient(name), ...)`, `bind(scope.prompt(name), ...)`,
`bind(scope.layer(name), ...)`, `bind(scope.region(name), ...)` and
`bind(scope.project(root), ...)`. A `scope` case would also have to carry
a directory, which a variant of strings can't say.

A layer's condition is a string rather than a WIT type, for the same reason
keys are: a type that contains itself needs a `box<>`, which the plugin
toolchain here can't read yet.

- Within 0.1.x, interfaces and functions may be **added**; nothing that
  exists changes its name, parameters, results or meaning, and nothing is
  removed.
- A plugin built against any 0.1.x loads as long as every function it
  actually uses exists in the running Forge. (Wasmtime matches imports
  semver-compatibly.) If it uses one the running Forge doesn't have,
  loading fails with an error naming it.
- The exports plugins provide (`init`, `handler`) don't change within 0.1.x.
  New optional exports (like `process-handler`) are looked up when a
  plugin loads; plugins without them work as before.
- Anything else is 0.2.0, a breaking change. Forge may then load both
  0.1 and 0.2 plugins for a while.
- No function in the world is `async`, so WASI 0.2 toolchains (most
  non-Rust languages today) can target it; plugins may import WASI 0.2 or
  WASI 0.3 interfaces.

## Building

Rust: see [`plugins/hello`](../plugins/hello) and the `forge-plugin-sdk`
crate (`cargo build --release --target wasm32-wasip2`). `forge_plugin!`
embeds the manifest, built from Cargo.toml into the same `[package]` shape
a hand-written manifest uses: the package's name (or
`[package.metadata.forge] name`), version, description, homepage and
license, plus anything else from `[package.metadata.forge]` (`startup`,
`permissions`, `requires`, `[[commands]]`, `[[options]]`, `modes`,
`extensions`):

```toml
[package]
name = "forge-spell"
version = "0.2.0"
description = "Spell checking"
homepage = "https://example.com/spell"
license = "MIT"

[package.metadata.forge]
startup = false

[package.metadata.forge.permissions]
memory-mb = 256
directories = [{ path = "~/.local/share/dict", why = "word lists" }]

[[package.metadata.forge.commands]]
name = "spell-check-buffer"
summary = "Check the whole buffer."
```

A typo in `[package.metadata.forge]` is a compile error. A plugin's own
logic can be unit-tested natively with `cargo test` (the SDK only exports
in wasm builds). `cargo check --features forge-plugin-sdk/check-exports`
type-checks the component's exports natively too, without the
wasm32-wasip2 target. The SDK targets the `plugin-full` world, so every
API is there: `spawn` (processes), `command_background`, `read_key`,
`execute_keys`, `annotate` and `completion_source`. See forge-plugin-vertico,
corfu, meow and lsp.

Other languages: [sdks/](../sdks) has SDKs for JavaScript/TypeScript,
Python, C and Lua, each with the same callback helpers and a build command
that embeds the manifest (from package.json, pyproject.toml or
forge.toml). Any other toolchain that makes components works too:
generate bindings from `wit/forge-editor.wit` for the `plugin` world.

`forge --check-plugin FILE --run COMMAND --answer TEXT` loads a plugin
without the editor, prints what it registered and runs commands; handy
for testing plugins in CI. `--open FILE`, `--point LINE:COL` and
`--wait SECONDS` (let events and process output arrive) test plugins
like language servers:

    forge --check-plugin lsp.wasm --open app.py --wait 20 --point 5:7 --run lsp-hover --wait 5

Install by copying the `.wasm` to `~/.config/forge/plugins/`, or load it
with `M-x load-plugin` or `forge --plugin FILE.wasm`. Plugins from other
toolchains ship `NAME.toml` next to the component, or add a
`forge-manifest` custom section holding the same TOML text.
