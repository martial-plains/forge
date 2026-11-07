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
| `package sync [--json] [--allow-build] [--allow-capabilities] [--allow-grants]` | install exactly what `init.rhai` declares and `forge-lock.toml` pins, without prompting; exit status is 0 on success, 1 on failure, 2 on bad usage and 3 when an operation needs an approval flag or setting |
| `package snapshot diff BEFORE AFTER [--json]` | compare saved package snapshots without starting the editor |
| `package lockfile diff BEFORE AFTER [--json]` | compare two lockfiles without starting the editor |
| `--headless ... [--test CMD]` | run the editor without a terminal; `--test` runs the plugin bisect with CMD (a nonzero exit status means the problem happens; CMD gets `FORGE_BISECT_ENABLED`) |

`--json` emits the result as JSON and is only accepted with a `package`
command or `--profile-startup`.

`package sync` never prompts. A locked git package is fetched at its locked
commit, and a locked artifact must match its recorded sha256. If a build,
new capability or plugin permission would normally ask, sync exits 3 unless
`--allow-build`, `--allow-capabilities` or `--allow-grants` is given (or the
matching `forge.opt["package_allow_build"]`, `package_allow_capabilities` or
`package_allow_grants` is true). Without a lock, sync installs declarations
and records the result, like startup.

One difference: visited files open before the other actions, whatever their
place on the command line.

## Package checks in CI

`package doctor` and `--profile-startup` use exit statuses, so a workflow can
gate on them. Keep the job offline by checking in a fixture configuration with
`init.rhai`, `forge-lock.toml`, and a prebuilt component. The repository's
fixture is in `tests/fixtures/ci-packages/`; copy it outside the checkout so
the doctor does not warn about a plugins directory inside the Git work tree.
No package index, Git remote, or network access is needed for the package
commands. Only the first Cargo build uses the network.

Pin every action to a full commit SHA. The cache key combines the job's
`shared-key`, `Cargo.lock`, the Rust toolchain, and the workspace manifests, so
the package job does not evict the caches used by the test and documentation
jobs.

```yaml
name: Package checks
on:
  push:
    branches: [main]
  pull_request:
permissions:
  contents: read

jobs:
  packages:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
      - uses: dtolnay/rust-toolchain@683b588c487a69d01f6c6ddf40f036ff682cc2c8 # master
        with:
          toolchain: stable
      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2.9.3
        with:
          shared-key: package-ci
          key: ${{ runner.os }}-package-${{ hashFiles('**/Cargo.lock') }}
          cache-on-failure: true
      - name: Build Forge
        run: cargo build --locked --profile fast -p forge-terminal --bin forge
      - name: Stage the offline package fixture
        run: |
          mkdir -p "$RUNNER_TEMP/forge-ci/config"
          cp -R tests/fixtures/ci-packages/. "$RUNNER_TEMP/forge-ci/config/"
          echo "FORGE_DATA_DIR=$RUNNER_TEMP/forge-ci/data" >> "$GITHUB_ENV"
          echo "FORGE_CACHE_DIR=$RUNNER_TEMP/forge-ci/cache" >> "$GITHUB_ENV"
          echo "FORGE_STATE_DIR=$RUNNER_TEMP/forge-ci/state" >> "$GITHUB_ENV"
      - name: Package doctor
        run: target/fast/forge package doctor --json --init-directory "$RUNNER_TEMP/forge-ci/config"
      - name: Startup budget
        run: target/fast/forge --profile-startup --json --init-directory "$RUNNER_TEMP/forge-ci/config"
```

The fixture's `init.rhai` sets `startup_budget_ms = 5000`, so the profile step
fails if the fixture plugin takes longer than that to start. The doctor exits
`1` only for error findings; its size finding is informational. Use `--json`
when a later step needs to inspect the findings.

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
