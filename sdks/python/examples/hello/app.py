# SPDX-License-Identifier: MIT OR Apache-2.0
"""An example Forge plugin in Python: the same as plugins/hello in Rust,
plus a word count.

    python -m forge_plugin_build build app.py -o hello-py.wasm
    forge --plugin hello-py.wasm          (then C-c C-p)
"""

import re

from forge_plugin import Buffer, bind, command, echo, info, on, plugin, read_string

# componentize-py looks for these two names in this module.
from forge_plugin import Handler, WitWorld  # noqa: F401


def greet(prefix):
    times = max(prefix or 1, 1)

    def insert(name):
        if name:
            Buffer.current().insert(f"Hello, {name}!\n" * times)

    read_string("Greet whom? ", "world", insert)


def count_words(_prefix):
    text = Buffer.current().text()
    echo(f"{len(re.findall(r'\S+', text))} words")


def saved(buffer_id):
    b = Buffer.from_id(buffer_id)
    if b:
        info(f"saved {b.name()}")


@plugin
def setup():
    command("hello-py-greet", "Ask for a name and insert a greeting at point.", greet)
    command("hello-py-count-words", "Show how many words the buffer has.", count_words)
    bind("C-c C-p", "hello-py-greet")
    on("buffer-saved", saved)
    print("Hello from Python! Try C-c C-p.")
