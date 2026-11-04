// Write Forge plugins in JavaScript or TypeScript.
//
// A plugin is a WebAssembly component for the `forge:editor/plugin` world
// (wit/forge-editor.wit). The raw API is importable as ES modules
// ('forge:editor/buffers@0.1.30', ...); this module adds the plumbing that
// makes the callback style pleasant, like the Rust SDK:
//
//   import { plugin, command, readString, Buffer } from '@forge-editor/plugin';
//
//   plugin(() => {
//     command('greet', 'Ask for a name and insert a greeting.', () => {
//       readString('Name: ', '', (name) => {
//         if (name) Buffer.current().insert(`Hello, ${name}!`);
//       });
//     });
//   });
//
// Build with `npx forge-plugin build plugin.js -o greet.wasm`.
//
// Forge calls a plugin and waits for it to return; a plugin never waits
// for the user. So questions take a function that runs when the answer
// arrives, and prompts, messages and commands.run take effect after the
// current call returns. Functions that can fail throw an Error.

import * as commands from 'forge:editor/commands@0.1.30';
import * as events from 'forge:editor/events@0.1.30';
import * as minibuffer from 'forge:editor/minibuffer@0.1.30';
import { log as hostLog } from 'forge:editor/log@0.1.30';
export { Buffer } from 'forge:editor/buffers@0.1.30';
export * as keymap from 'forge:editor/keymap@0.1.30';
export * as modes from 'forge:editor/modes@0.1.30';
export * as decorations from 'forge:editor/decorations@0.1.30';
export { commands, events, minibuffer };

const commandFns = new Map();
const answerFns = new Map();
const eventFns = [];
let initFn = () => {};

/// Errors from Forge (ComponentError) carry their message in `payload`.
function unwrap(fn) {
  try {
    return fn();
  } catch (e) {
    throw new Error(e?.payload ?? String(e));
  }
}

/// A thrown value as the string Forge shows.
function message(e) {
  return e instanceof Error ? e.message : String(e);
}

/** The plugin's setup, run once when Forge loads it. */
export function plugin(init) {
  initFn = init;
}

/**
 * Register a command. `run` gets the prefix argument (C-u 3 -> 3, plain
 * C-u -> 4), or undefined without one. Throwing shows the error.
 */
export function command(name, doc, run, { background = false } = {}) {
  unwrap(() => (background ? commands.registerBackground(name, doc) : commands.register(name, doc)));
  commandFns.set(name, run);
}

/** Run any command (built-in or a plugin's) after this call returns. */
export function runCommand(name) {
  unwrap(() => commands.run(name));
}

/** Ask for a line of text; `then` gets it (undefined if the user pressed C-g). */
export function readString(prompt, initial, then) {
  answerFns.set(minibuffer.readString(prompt, initial ?? ''), then);
}

/** Ask with TAB completion over `candidates`. */
export function completingRead(prompt, candidates, requireMatch, then) {
  answerFns.set(minibuffer.completingRead(prompt, candidates, requireMatch), then);
}

/** Ask a y-or-n question; `then` gets true for y (false for n or C-g). */
export function yOrN(prompt, then) {
  answerFns.set(minibuffer.yOrN(prompt), (a) => then(a === 'y'));
}

/**
 * Run `f(bufferId)` for every event of `kind`: 'buffer-opened',
 * 'buffer-killed', 'buffer-changed' (after typing pauses), 'buffer-saved',
 * 'mode-changed'.
 */
export function on(kind, f, { background = false } = {}) {
  if (background) events.subscribeBackground(kind);
  else events.subscribe(kind);
  eventFns.push([kind, f]);
}

/** Show a message in the echo area. */
export function echo(text) {
  minibuffer.message(text);
}

/** Forge's log (*Messages*). console.log goes there too. */
export const log = {
  debug: (m) => hostLog('debug', String(m)),
  info: (m) => hostLog('info', String(m)),
  warn: (m) => hostLog('warn', String(m)),
  error: (m) => hostLog('error', String(m)),
};

// ------------------------------------------------ what Forge calls

export function init() {
  try {
    initFn();
  } catch (e) {
    throw message(e);
  }
}

export const handler = {
  runCommand(name, prefix) {
    const f = commandFns.get(name);
    if (!f) throw `this plugin has no command ${name}`;
    try {
      f(prefix === undefined ? undefined : Number(prefix));
    } catch (e) {
      throw message(e);
    }
  },
  onEvent(event) {
    for (const [kind, f] of eventFns) {
      if (kind === event.tag) {
        try {
          f(event.val);
        } catch (e) {
          log.error(`${kind} handler: ${message(e)}`);
        }
      }
    }
  },
  onAnswer(request, answer) {
    const f = answerFns.get(request);
    answerFns.delete(request);
    if (f) f(answer);
  },
};
