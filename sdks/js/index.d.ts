/// <reference path="./types/plugin-with-processes.d.ts" />
// Types for @forge-editor/plugin. The raw API ('forge:editor/...')
// is described in types/, generated from wit/ with `npm run types`.

export { Buffer } from 'forge:editor/buffers@0.1.19';
export * as keymap from 'forge:editor/keymap@0.1.19';
export * as modes from 'forge:editor/modes@0.1.19';
export * as decorations from 'forge:editor/decorations@0.1.19';
export * as commands from 'forge:editor/commands@0.1.19';
export * as events from 'forge:editor/events@0.1.19';
export * as minibuffer from 'forge:editor/minibuffer@0.1.19';

export type EventKind = import('forge:editor/events@0.1.19').Kind;

/** The plugin's setup, run once when Forge loads it. */
export function plugin(init: () => void): void;
/**
 * Register a command; `run` gets the prefix argument, if any. Throws if the name is taken.
 * With `background`, it runs on a snapshot of the editor without blocking it (Forge 0.1.2;
 * see commands.register-background in the WIT).
 */
export function command(
  name: string,
  doc: string,
  run: (prefix: number | undefined) => void,
  options?: { background?: boolean },
): void;
/** Run any command after this call returns. */
export function runCommand(name: string): void;
/** Ask for a line of text; `then` gets undefined if the user pressed C-g. */
export function readString(prompt: string, initial: string, then: (answer: string | undefined) => void): void;
/** Ask with TAB completion over `candidates`. */
export function completingRead(
  prompt: string,
  candidates: string[],
  requireMatch: boolean,
  then: (answer: string | undefined) => void,
): void;
/** Ask a y-or-n question. */
export function yOrN(prompt: string, then: (yes: boolean) => void): void;
/** Run `f` with the buffer id whenever an event of `kind` happens. */
export function on(kind: EventKind, f: (bufferId: number) => void, options?: { background?: boolean }): void;
/** Show a message in the echo area. */
export function echo(text: string): void;
/** Forge's log (*Messages*). */
export const log: {
  debug(message: unknown): void;
  info(message: unknown): void;
  warn(message: unknown): void;
  error(message: unknown): void;
};
