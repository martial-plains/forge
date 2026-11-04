# forge-plugin: Forge plugins in Python

```python
from forge_plugin import plugin, command, read_string, Buffer
from forge_plugin import WitWorld, Handler  # componentize-py looks for these

@plugin
def setup():
    command("greet", "Ask for a name and insert a greeting.", greet)

def greet(prefix):
    def insert(name):
        if name:
            Buffer.current().insert(f"Hello, {name}!")
    read_string("Name: ", "", insert)
```

    pip install forge-plugin          # brings componentize-py
    forge-plugin-py build app.py -o greet.wasm
    forge --check-plugin greet.wasm --run greet --answer Ada

The manifest comes from the nearest pyproject.toml: `[project]` name,
version and description, plus `[tool.forge]`:

```toml
[tool.forge]
name = "greet"
[tool.forge.permissions]
directories = [{ path = "~/notes", write = true, why = "saves notes" }]
```

## API

| Function | |
|----------|---|
| `@plugin` | Your setup, run once when Forge loads the plugin |
| `command(name, doc, run)` | `run(prefix)`: prefix argument or None |
| `read_string`, `completing_read`, `y_or_n` | Take a function that gets the answer later (None for C-g) |
| `on(kind, f)` | `f(buffer_id)` on "buffer-opened", "buffer-changed", "buffer-saved", "buffer-killed", "mode-changed" |
| `bind(keys, command, mode=None)`, `define_minor_mode`, `define_major_mode` | |
| `echo(text)`, `info(...)`, `print(...)` | Echo area; `*Messages*` |
| `Buffer`, `decorations`, `keymap`, `modes` | The raw API (`wit_world.imports`) |

Forge's errors raise `ForgeError`; raising from a command shows the
error. `forge-plugin-py bindings DIR` writes the generated `wit_world`
package into DIR for your editor's completion. Pure-Python packages on the
path are bundled; ones with native extensions need WASI builds (see
componentize-py). Plugins are about 18 MB (they
carry CPython); Forge compiles each once and caches it.
