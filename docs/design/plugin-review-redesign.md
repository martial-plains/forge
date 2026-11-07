# Plugin review redesign

Status: implemented on `package-manager`.

The permission prompt, import mismatch, upgrade capability review, plugin
list and `describe-plugin` now use the package manager's title, rule,
section, badge and band system. Permission behavior and keys do not change:
`y` grants and loads, `n` loads without permissions, `C-g` does not load,
and `revoke-plugin-permissions` forgets grants. `RET` and `TAB` only fold
the review's import list.

## Permission prompt

Before:

```text
 Plugin Permissions

demo asks for permission
  + network   connect to the internet
  + directory read /tmp/templates (greeting templates)

What its code can call
  forge:editor/log@0.1.0
  ...
```

After:

```text
 Plugin Permissions                                            spellcheck 0.3.0
--------------------------------------------------------------------------------

spellcheck asks for permission

 ! same name, different binary
The saved approval was for a different build, so it was forgotten.

Asks for (2 new) ---------------------------------------------------------------
  + network     connect to the internet                             higher risk
  + directory   write ~/.cache/spellcheck (word lists)              higher risk

What its code can call (2 imports)
  forge:editor/log@0.1.0
  forge:editor/buffers@0.1.0

y grant and load   n load without permissions   C-g don't load
Grants are kept in the state directory; M-x revoke-plugin-permissions forgets them.
```

Risk is written out, not carried by color alone. The old `Settings` section
moved to `describe-plugin`.

## Import mismatch

Before: a short error message in the minibuffer.

After:

```text
 Import Mismatch                                                        sockets
--------------------------------------------------------------------------------

sockets can't load: a declared permission is missing

 x the manifest doesn't declare every permission its imports need

Mismatched imports
  x wasi:sockets/instance-network@0.2.12  needs network, not declared

What its code can call (2)
  wasi:sockets/instance-network@0.2.12  (needs network, not declared)
  forge:editor/log@0.1.0

Add the missing permissions to the plugin's manifest, then load it again.
```

There is no `y`/`n` prompt: this is a broken manifest, not a grant question.

## Upgrade capability review

Before: a confirmation prompt with a flat list of new capabilities.

After:

```text
 Upgrade Review                                       org-modern 0.2.0 -> 0.3.1
--------------------------------------------------------------------------------

org-modern

 ! unattended upgrades stop and change nothing

Capabilities (+1 -1 =2) --------------------------------------------------------
  + network: open connections and look up host names                higher risk
  - programs: run git (outside the sandbox, with your rights)
  = files: read ~/.config/forge
  = memory: up to 256 MB

Build --------------------------------------------------------------------------
  command: cargo build --release --target wasm32-wasip2

y apply upgrade   n skip   C-g cancel
```

`+` means added, `-` removed and `=` unchanged. `BREAKING` commit subjects
use the error face and `WARNING` subjects use the warning face. Index
upgrades show release notes; Git upgrades show commit subjects and the
build recipe diff.

## Plugin list

Before: one undifferentiated list of plugins.

After:

```text
 Plugins                                                       1 plugin
----------------------------------------------------------------------------------------

Not started (1) ------------------------------------------------------------------------
| demo                  no permissions  not started
    file: <plugins>/demo.wasm
    commands: demo-greet
    permissions: none (no files, network or environment)
    memory limit: 128 MB

RET expand  j/k select  r revoke  d describe  g refresh  q quit
```

Rows are grouped into `Loaded`, `Not started` and `Needs attention`.
Expanded rows show the file, commands, grant source (`[init.rhai]` or
`[granted when asked, last changed ...]`), memory limit and unsupported
imports. The first list view expands the first row once; expansion survives
refresh.

## Contrast

The default 16-colour badge and band pairs measure:

| Pair | Ratio |
| --- | ---: |
| black on yellow | 19.56:1 |
| black on cyan | 16.75:1 |
| black on red | 5.25:1 |
| black on dark gray | 5.32:1 |

`docs/init.example.rhai` has dark and light badge examples whose pairs are
all at least 4.5:1. On 16-colour terminals the faces can stay unset because
the label, mark and wording carry the meaning.

## Tests

Six text snapshots cover the fresh prompt, different-binary prompt, import
mismatch, an upgrade with an addition, an upgrade with only removals, and a
list containing a not-started plugin. The permission toggle and old `y`,
`n`, `C-g` behavior are covered by the existing plugin tests.
