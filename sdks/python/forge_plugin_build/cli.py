# SPDX-License-Identifier: MIT OR Apache-2.0
"""python -m forge_plugin_build build APP.py [-o OUT.wasm]
python -m forge_plugin_build bindings DIR

(Also installed as the ``forge-plugin-py`` command.)

``build`` makes a component with componentize-py and embeds the manifest
from the nearest pyproject.toml ([project] name, version, description and
[tool.forge]):

    [tool.forge]
    name = "spell"            # default: the project name
    startup = false           # true: never start lazily
    [tool.forge.permissions]
    network = false
    memory-mb = 256
    directories = [{ path = "~/dict", why = "word lists" }]

``bindings`` writes the generated ``wit_world`` package into DIR, so
editors can complete the raw API.
"""

import os
import subprocess
import sys
import tomllib
from pathlib import Path

# The forge_plugin package (and the WIT inside it).
SDK = Path(__file__).resolve().parent.parent / "forge_plugin"
WIT = SDK / "wit"
KNOWN = {"name", "startup", "permissions"}


def usage(msg=""):
    if msg:
        print(f"forge-plugin-py: {msg}", file=sys.stderr)
    print(__doc__.split("\n\n")[0], file=sys.stderr)
    sys.exit(2)


def nearest_pyproject(start: Path) -> dict:
    for d in [start, *start.parents]:
        p = d / "pyproject.toml"
        if p.exists():
            return tomllib.loads(p.read_text())
    return {}


def toml_value(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, str):
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    raise SystemExit(f"forge-plugin-py: can't put {v!r} in a manifest")


def manifest(pyproject: dict) -> str:
    project = pyproject.get("project", {})
    forge = pyproject.get("tool", {}).get("forge", {})
    unknown = set(forge) - KNOWN
    if unknown:
        raise SystemExit(f"forge-plugin-py: [tool.forge]: unknown key {sorted(unknown)[0]!r} (known: {', '.join(sorted(KNOWN))})")
    # Root keys first, then tables (TOML attaches bare keys to the last
    # table header). The plugin's own facts go in [package].
    lines = []
    if "startup" in forge:
        lines.append(f"startup = {toml_value(forge['startup'])}")
    lines.append("[package]")
    name = forge.get("name", project.get("name"))
    if name:
        lines.append(f"name = {toml_value(name)}")
    for key in ("version", "description"):
        if key in project:
            lines.append(f"{key} = {toml_value(project[key])}")
    perms = dict(forge.get("permissions", {}))
    dirs = perms.pop("directories", [])
    if perms or dirs:
        lines += ["", "[permissions]"] + [f"{k} = {toml_value(v)}" for k, v in perms.items()]
        for d in dirs:
            d = {"path": d} if isinstance(d, str) else d
            lines += ["", "[[permissions.directories]]"] + [f"{k} = {toml_value(v)}" for k, v in d.items()]
    return "\n".join(lines) + "\n"


def leb(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def with_custom_section(wasm: bytes, name: str, data: str) -> bytes:
    body = leb(len(name.encode())) + name.encode() + data.encode()
    return wasm + b"\x00" + leb(len(body)) + body


def build(app: str, out: str) -> None:
    app_path = Path(app).resolve()
    if app_path.suffix != ".py":
        usage(f"{app} isn't a .py file")
    module = app_path.stem
    subprocess.run(
        [
            "componentize-py", "-d", str(WIT), "-w", "plugin", "componentize", module,
            "-p", str(app_path.parent), "-p", str(SDK.parent), "-o", out,
        ],
        check=True,
    )
    data = Path(out).read_bytes()
    Path(out).write_bytes(with_custom_section(data, "forge-manifest", manifest(nearest_pyproject(app_path.parent))))
    print(f"{out}: {os.path.getsize(out) / 1e6:.1f} MB")


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) >= 2 and argv[0] == "build":
        out = Path(argv[1]).stem + ".wasm"
        rest = argv[2:]
        if rest[:1] == ["-o"] and len(rest) == 2:
            out = rest[1]
        elif rest:
            usage(f"unknown arguments {rest}")
        build(argv[1], out)
    elif len(argv) == 2 and argv[0] == "bindings":
        subprocess.run(["componentize-py", "-d", str(WIT), "-w", "plugin", "bindings", argv[1]], check=True)
    else:
        usage()


if __name__ == "__main__":
    main()
