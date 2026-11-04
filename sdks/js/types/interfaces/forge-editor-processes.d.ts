declare module 'forge:editor/processes@0.1.19' {
  /**
   * Start `program` (a name looked up in PATH, or a path) with
   * `args`, in `dir` (default: the project of the current buffer).
   */
  export function spawn(program: string, args: Array<string>, dir: string | undefined): Process;
  /**
   * Send bytes to its stdin.
   */
  export function write(process: Process, data: Uint8Array): void;
  /**
   * Close its stdin.
   */
  export function closeInput(process: Process): void;
  export function kill(process: Process): void;
  /**
   * The project a file is in: the nearest directory above it with
   * .git (.hg, .jj, .project), else with a build file (Cargo.toml,
   * package.json, go.mod, pyproject.toml, Makefile, ...).
   */
  export function projectRoot(path: string): string | undefined;
  /**
   * ---- 0.1.9
   * Like `spawn`, with `env` set on top of the environment Forge gives
   * every process (GIT_EDITOR, LC_ALL, ...).
   */
  export function spawnWith(program: string, args: Array<string>, dir: string | undefined, env: Array<[string, string]>): Process;
  /**
   * What the process has written to stderr so far (the first 64 KiB).
   * Still there during its `on-exit`, gone once that returns: read it
   * there to tell the user why a command failed. Stderr also goes to
   * the plugin's log, as before.
   */
  export function errorOutput(process: Process): Uint8Array;
  export type Process = number;
}
