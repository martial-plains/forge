/// <reference path="./forge-editor-processes.d.ts" />
declare module 'forge:editor/process-handler@0.1.19' {
  /**
   * Output from a process's stdout, in order.
   */
  export function onOutput(process: Process, data: Uint8Array): void;
  /**
   * The process ended: its exit code (none if killed by a signal).
   * Comes after all its output.
   */
  export function onExit(process: Process, code: number | undefined): void;
  export type Process = import('forge:editor/processes@0.1.19').Process;
}
