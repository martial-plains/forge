# Command line and forgeclient

`forge --help` lists every option. They follow Forge's, run in the order
given. options that would do nothing here (the X display, fonts,
colours, `-nw`, the splash screen) are not accepted.

```
forge -Q --batch -l build.rhai --eval 'publish()'     # a script, no screen
forge +120:4 src/main.rs -f split-window-right        # open at a place, then a command
forge --daemon=work                                   # a background server named "work"
forgeclient -s work -n +10 notes.org                  # open a file in it, don't wait
forgeclient -s work -e '1 + 2'                        # evaluate Rhai there, print 3
```

| Option | Meaning |
| --- | --- |
| `FILE`, `--file`, `--find-file`, `--visit`, `+LINE[:COL]` | visit files |
| `-l`, `--load FILE` | run a Rhai script |
| `--eval`, `--execute EXPR` | evaluate a Rhai expression |
| `-f`, `--funcall CMD` | run a command |
| `--insert FILE` | insert a file at point |
| `-L`, `--directory DIR` | load the `.wasm` plugins in DIR |
| `--kill` | exit after the actions |
| `--batch`, `--script FILE` | no screen; messages to standard error; exit status 1 on an error |
| `--daemon[=NAME]`, `--bg-daemon`, `--fg-daemon` | run as a server |
| `-q`, `-Q`, `--no-init-file` | no init.rhai, no saved session |
| `--init-directory DIR`, `-u USER` | another config directory |
| `--chdir DIR` | change directory first |
| `package declarations [--json]` | collect `use_package` declarations without starting the editor; closures are reported, not run |
| `package doctor [--json]` | check declared and installed plugins without starting the editor; exit status is 1 on an error finding |
| `package snapshot diff BEFORE AFTER [--json]` | compare saved package snapshots without starting the editor |
| `package lockfile diff BEFORE AFTER [--json]` | compare two lockfiles without starting the editor |
| `--headless ... [--test CMD]` | run the editor without a terminal; `--test` runs the plugin bisect with CMD (a nonzero exit status means the problem happens; CMD gets `FORGE_BISECT_ENABLED`) |

`--json` emits the result as JSON and is only accepted with a `package`
command or `--profile-startup`.

One difference: visited files open before the other actions, whatever their
place on the command line.

## The server

`M-x server-start` (or a daemon) listens on 127.0.0.1 on a random port and
writes `<state dir>/server/NAME` with the port and a random key. The file is
readable by its owner only and every request carries the key. `M-x
server-shutdown` stops it. `C-x #` (`server-edit`) saves the file and tells
a client that was waiting for it that it is done; killing the buffer does too.

`forgeclient`: `-n` don't wait, `-e` evaluate the arguments (`-u` don't print
the values), `-s NAME` or `-f FILE` pick the server (`$FORGE_SERVER_NAME`),
`-a EDITOR` run that if there is no server (an empty value starts
`forge --daemon` and tries again), `-q` no messages. Forge has one frame per
process, so there is no `-c`, `-t` or `-r`: files open in the running editor.
