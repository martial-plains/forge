declare module 'forge:editor/files@0.1.19' {
  export function exists(path: string): boolean;
  /**
   * A text file's contents (`~` is the home directory): a plugin's
   * configuration, say. Allowed in granted directories (read or write)
   * and, with the processes permission, anywhere.
   */
  export function read(path: string): string;
  /**
   * Make an empty file (and its directory). With `overwrite`, an
   * existing file is emptied; without, it is left alone.
   */
  export function create(path: string, overwrite: boolean): void;
  /**
   * Move a file or directory. Fails if `target` exists, unless `overwrite`.
   */
  export function rename(source: string, target: string, overwrite: boolean): void;
  /**
   * Remove a file, or a directory (with `recursive`, and its contents).
   */
  export { _delete as delete };
  function _delete(path: string, recursive: boolean): void;
}
