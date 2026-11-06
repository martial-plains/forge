// Running programs from a Forge plugin (a language server, a formatter).
// Needs Forge 0.1.2 and the "processes" permission naming the programs:
//
//   "forge": { "permissions": { "processes": ["pyright-langserver"] } }
//
// forge-plugin build then targets the plugin-with-processes world and
// exports the handler below. Output arrives when the editor is idle.
//
//   import { spawn } from '@forge-editor/plugin/process';
//   const p = spawn('wc', ['-w'], { onOutput: (bytes) => ..., onExit: (code) => ... });
//   p.write('some text');
//   p.closeInput();

import * as processes from 'forge:editor/processes@0.1.31';

const handlers = new Map();
const decoder = new TextDecoder();
const encoder = new TextEncoder();

/** Start a program. Throws if it can't (not permitted, not found). */
export function spawn(program, args = [], { dir, onOutput, onExit, text = true } = {}) {
  let id;
  try {
    id = processes.spawn(program, args, dir);
  } catch (e) {
    throw new Error(e?.payload ?? String(e));
  }
  handlers.set(id, { onOutput, onExit, text });
  return {
    id,
    write(data) {
      try {
        processes.write(id, typeof data === 'string' ? encoder.encode(data) : data);
      } catch (e) {
        throw new Error(e?.payload ?? String(e));
      }
    },
    closeInput: () => processes.closeInput(id),
    kill: () => processes.kill(id),
  };
}

/** The project a file is in (see the WIT), or undefined. */
export const projectRoot = (path) => processes.projectRoot(path);

/** What Forge calls (exported by the build). */
export const processHandler = {
  onOutput(id, data) {
    const h = handlers.get(id);
    if (h?.onOutput) h.onOutput(h.text ? decoder.decode(data, { stream: true }) : data);
  },
  onExit(id, code) {
    const h = handlers.get(id);
    handlers.delete(id);
    if (h?.onExit) h.onExit(code);
  },
};
