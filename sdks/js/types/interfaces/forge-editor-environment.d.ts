declare module 'forge:editor/environment@0.1.19' {
  /**
   * The value of an environment variable, or none when it is unset.
   * Needs the `environment` permission.
   */
  export function get(name: string): string | undefined;
  /**
   * Every variable as a (name, value) pair, in order. Needs the
   * `environment` permission.
   */
  export function variables(): Array<[string, string]>;
  /**
   * The kind of system Forge was built for: "gnu/linux", "darwin",
   * "windows-nt", "gnu", or the target's OS as a fallback.
   */
  export function systemType(): string;
  /**
   * The canonical name Forge was built for, e.g.
   * "x86_64-unknown-linux-gnu".
   */
  export function systemConfiguration(): string;
  /**
   * This machine's hostname.
   */
  export function systemName(): string;
  /**
   * This process's id.
   */
  export function pid(): number;
  /**
   * Where output is discarded: "/dev/null", or "NUL" on Windows.
   */
  export function nullDevice(): string;
  /**
   * The separator between directories in a path list: ":" or ";".
   */
  export function pathSeparator(): string;
  /**
   * The name Forge was invoked as, without its directory.
   */
  export function invocationName(): string;
  /**
   * Split a path list on `path-separator` into its directories. An
   * empty component, meaning the current directory, comes back as a
   * path of "".
   */
  export function parsePath(path: string): Array<string>;
}
