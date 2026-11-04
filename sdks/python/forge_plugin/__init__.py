# SPDX-License-Identifier: MIT OR Apache-2.0
"""Write Forge plugins in Python.

A plugin is a WebAssembly component for the ``forge:editor/plugin`` world
(``wit/forge-editor.wit``), built with componentize-py. The raw API is in
``wit_world.imports`` (generated when you build); this module adds the
plumbing that makes the callback style pleasant, like the Rust SDK::

    from forge_plugin import plugin, command, read_string, Buffer

    @plugin
    def setup():
        command("greet", "Ask for a name and insert a greeting.", greet)

    def greet(prefix):
        read_string("Name: ", "", lambda name: name and Buffer.current().insert(f"Hello, {name}!"))

    # componentize-py looks for these two names in your module:
    from forge_plugin import WitWorld, Handler

Build with ``python -m forge_plugin_build build app.py -o greet.wasm``.

Forge calls a plugin and waits for it to return; a plugin never waits for
the user. So questions take a function that runs when the answer arrives,
and prompts, messages and ``run_command`` take effect after the current
call returns. Functions that can fail raise ``ForgeError``.
"""

from typing import Callable, Dict, List, Optional, Tuple

import wit_world
from componentize_py_types import Err
from wit_world import exports
from wit_world.imports import buffers, commands, decorations, events, keymap, log as _log, minibuffer, modes, types

Buffer = buffers.Buffer
Position = types.Position
Span = types.Span
Decoration = decorations.Decoration

__all__ = [
    "plugin", "command", "run_command", "read_string", "completing_read", "y_or_n", "on", "echo",
    "bind", "define_minor_mode", "define_major_mode", "debug", "info", "warn", "error", "ForgeError",
    "Buffer", "Position", "Span", "Decoration", "decorations", "keymap", "modes", "WitWorld", "Handler",
]


class ForgeError(Exception):
    """Forge refused something (a taken command name, an unknown mode...)."""


def _call(f, *args):
    """Call a Forge function, turning its errors into ForgeError."""
    try:
        return f(*args)
    except Err as e:
        raise ForgeError(e.value) from None


_commands: Dict[str, Callable[[Optional[int]], None]] = {}
_answers: Dict[int, Callable[[Optional[str]], None]] = {}
_events: List[Tuple[str, Callable[[int], None]]] = []
_setup: Optional[Callable[[], None]] = None


def plugin(setup: Callable[[], None]) -> Callable[[], None]:
    """Decorator: the plugin's setup, run once when Forge loads it."""
    global _setup
    _setup = setup
    return setup


def command(name: str, doc: str, run: Callable[[Optional[int]], None]) -> None:
    """Register a command. ``run`` gets the prefix argument (C-u 3 -> 3,
    plain C-u -> 4), or None. Raising an exception shows it as an error."""
    _call(commands.register, name, doc)
    _commands[name] = run


def run_command(name: str) -> None:
    """Run any command (built-in or a plugin's) after this call returns."""
    _call(commands.run, name)


def read_string(prompt: str, initial: str, then: Callable[[Optional[str]], None]) -> None:
    """Ask for a line of text; ``then`` gets None if the user pressed C-g."""
    _answers[minibuffer.read_string(prompt, initial)] = then


def completing_read(prompt: str, candidates: List[str], require_match: bool, then: Callable[[Optional[str]], None]) -> None:
    """Ask with TAB completion over ``candidates``."""
    _answers[minibuffer.completing_read(prompt, candidates, require_match)] = then


def y_or_n(prompt: str, then: Callable[[bool], None]) -> None:
    """Ask a y-or-n question; ``then`` gets True for y (False for n or C-g)."""
    _answers[minibuffer.y_or_n(prompt)] = lambda a: then(a == "y")


_KINDS = {
    "buffer-opened": (events.Kind.BUFFER_OPENED, events.Event_BufferOpened),
    "buffer-killed": (events.Kind.BUFFER_KILLED, events.Event_BufferKilled),
    "buffer-changed": (events.Kind.BUFFER_CHANGED, events.Event_BufferChanged),
    "buffer-saved": (events.Kind.BUFFER_SAVED, events.Event_BufferSaved),
    "mode-changed": (events.Kind.MODE_CHANGED, events.Event_ModeChanged),
}


def on(kind: str, f: Callable[[int], None]) -> None:
    """Run ``f(buffer_id)`` for every event of ``kind``: "buffer-opened",
    "buffer-killed", "buffer-changed" (after typing pauses), "buffer-saved",
    "mode-changed"."""
    if kind not in _KINDS:
        raise ForgeError(f"unknown event {kind!r} (known: {', '.join(_KINDS)})")
    events.subscribe(_KINDS[kind][0])
    _events.append((kind, f))


def echo(text: str) -> None:
    """Show a message in the echo area."""
    minibuffer.message(text)


def bind(keys: str, command_name: str, mode: Optional[str] = None, buffer: Optional[int] = None) -> None:
    """Bind keys ("C-c C-t") to a command, globally or in a mode's or a
    buffer's keymap. Plugins can't bind C-c followed by a plain letter."""
    if buffer is not None:
        scope = keymap.Scope_Buffer(buffer)
    elif mode is not None:
        scope = keymap.Scope_Mode(mode)
    else:
        scope = keymap.Scope_Global()
    _call(keymap.bind, scope, keys, command_name)


def define_minor_mode(name: str) -> None:
    _call(modes.define_minor_mode, name)


def define_major_mode(name: str, parent: Optional[str] = None, extensions: Optional[List[str]] = None) -> None:
    _call(modes.define_major_mode, name, parent, extensions or [])


def debug(message: str) -> None:
    _log.log(_log.Level.DEBUG, str(message))


def info(message: str) -> None:
    """Write to Forge's log (*Messages*). print() goes there too."""
    _log.log(_log.Level.INFO, str(message))


def warn(message: str) -> None:
    _log.log(_log.Level.WARN, str(message))


def error(message: str) -> None:
    _log.log(_log.Level.ERROR, str(message))


# ------------------------------------------------------ what Forge calls


def _raise_err(e: BaseException):
    if isinstance(e, Err):
        raise e
    raise Err(str(e) or type(e).__name__)


class WitWorld(wit_world.WitWorld):
    def init(self) -> None:
        if _setup is None:
            raise Err("no @plugin setup function: decorate one with @forge_plugin.plugin")
        try:
            _setup()
        except BaseException as e:
            _raise_err(e)


class Handler(exports.Handler):
    def run_command(self, name: str, prefix_arg: Optional[int]) -> None:
        f = _commands.get(name)
        if f is None:
            raise Err(f"this plugin has no command {name}")
        try:
            f(prefix_arg)
        except BaseException as e:
            _raise_err(e)

    def on_event(self, event: events.Event) -> None:
        for kind, f in list(_events):
            if isinstance(event, _KINDS[kind][1]):
                try:
                    f(event.value)
                except Exception as e:
                    error(f"{kind} handler: {e}")

    def on_answer(self, request: int, answer: Optional[str]) -> None:
        f = _answers.pop(request, None)
        if f is not None:
            f(answer)
