/// <reference path="./forge-editor-events.d.ts" />
/// <reference path="./forge-editor-minibuffer.d.ts" />
declare module 'forge:editor/handler@0.1.19' {
  /**
   * One of the plugin's registered commands was run. `prefix-arg` is
   * the numeric value of C-u / M-5 etc. (plain C-u is 4), if given.
   */
  export function runCommand(name: string, prefixArg: bigint | undefined): void;
  export function onEvent(event: Event): void;
  export function onAnswer(request: Request, answer: string | undefined): void;
  export type Event = import('forge:editor/events@0.1.19').Event;
  export type Request = import('forge:editor/minibuffer@0.1.19').Request;
}
