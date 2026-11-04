# @forge-editor/plugin: Forge plugins in JavaScript and TypeScript

```js
import { plugin, command, readString, Buffer } from '@forge-editor/plugin';

plugin(() => {
  command('greet', 'Ask for a name and insert a greeting.', (prefix) => {
    readString('Name: ', '', (name) => {
      if (name) Buffer.current().insert(`Hello, ${name}!`);
    });
  });
});
```

    npm install @forge-editor/plugin
    npx forge-plugin build plugin.js -o greet.wasm      # .ts works too
    forge --check-plugin greet.wasm --run greet --answer Ada

`forge-plugin build` bundles your code with esbuild (so npm packages
work), turns it into a component with ComponentizeJS, and embeds the
manifest from the `"forge"` field of your package.json:

```json
"forge": {
  "name": "greet",
  "startup": false,
  "permissions": { "network": false, "directories": [{ "path": "~/notes", "write": true, "why": "saves notes" }] }
}
```

(`name` defaults to the package name; `version` and `description` come
from package.json.)

## API

| Function | |
|----------|---|
| `plugin(setup)` | Your setup, run once when Forge loads the plugin |
| `command(name, doc, run)` | `run(prefix)`: prefix argument or undefined |
| `readString(prompt, initial, then)`, `completingRead(prompt, candidates, requireMatch, then)`, `yOrN(prompt, then)` | `then` gets the answer later (undefined for C-g) |
| `on(kind, f)` | `f(bufferId)` on `'buffer-opened'`, `'buffer-changed'`, `'buffer-saved'`, `'buffer-killed'`, `'mode-changed'` |
| `echo(text)`, `log.info(...)`, `console.log(...)` | Echo area; `*Messages*` |
| `runCommand(name)` | After this call returns |
| `Buffer`, `keymap`, `modes`, `decorations`, `commands`, `events`, `minibuffer` | The raw API ([types/](types)) |

Forge's errors are thrown as `Error`s; throwing from a command shows the
error. Keys: `keymap.bind({ tag: 'global' }, 'C-c C-j', 'greet')` (or
`{ tag: 'mode', val: 'text-mode' }`).

TypeScript: `types/` (generated from the WIT with `npm run types`)
describes the raw API; see [examples/word-count](examples/word-count) for
a `tsconfig.json` that type-checks a plugin with `tsc --noEmit`.

`fetch()` isn't available (Forge doesn't provide `wasi:http`). The engine
(StarlingMonkey) makes each plugin about 12 MB; Forge compiles it once and
caches it.
