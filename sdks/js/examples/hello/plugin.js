// An example Forge plugin in JavaScript: the same as plugins/hello in Rust.
//
//   npx forge-plugin build plugin.js -o hello-js.wasm
//   forge --plugin hello-js.wasm          (then C-c C-j)

import { plugin, command, readString, on, keymap, log, Buffer } from '@forge-editor/plugin';

plugin(() => {
  command('hello-js-greet', 'Ask for a name and insert a greeting at point.', (prefix) => {
    const times = Math.max(prefix ?? 1, 1);
    readString('Greet whom? ', 'world', (name) => {
      if (name) Buffer.current().insert(`Hello, ${name}!\n`.repeat(times));
    });
  });
  keymap.bind({ tag: 'global' }, 'C-c C-j', 'hello-js-greet');

  on('buffer-saved', (id) => {
    const b = Buffer.fromId(id);
    if (b) log.info(`saved ${b.name()}`);
  });
  console.log('Hello from JavaScript! Try C-c C-j.');
});
