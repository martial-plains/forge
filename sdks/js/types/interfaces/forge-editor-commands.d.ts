declare module 'forge:editor/commands@0.1.19' {
  /**
   * Offer a command. M-x NAME runs it and keys can be bound to it;
   * Forge calls `handler.run-command` with the name. Fails if a
   * built-in or another plugin already has the name.
   */
  export function register(name: string, doc: string): void;
  /**
   * Run any command, built-in or from a plugin, once the current call
   * returns. Fails if no command has that name.
   */
  export function run(name: string): void;
  export function exists(name: string): boolean;
  /**
   * ---- 0.1.2
   * Like `register`, but the command runs in the background: Forge
   * calls the plugin on another thread with a snapshot of the editor
   * and doesn't wait. Its buffer edits are merged in when it returns
   * (dropped with a message if the same text changed meanwhile), and
   * so are its decorations, messages and questions. Calls to the plugin
   * meanwhile wait their turn. In a background call a plugin can't
   * register, bind, define modes, subscribe, or create, show or kill
   * buffers (those fail or trap), and point and mark moves aren't kept.
   * Background calls may run for 60 seconds.
   */
  export function registerBackground(name: string, doc: string): void;
  /**
   * ---- 0.1.3
   * A command's documentation (the first line is the summary).
   */
  export function doc(name: string): string | undefined;
  /**
   * ---- 0.1.10
   * Run this plugin's command `name` once each time the editor has been
   * idle for `seconds` after a key or click: for things
   * like showing documentation for what's at point. 0 (or less) stops
   * it. Fails if the plugin doesn't own the command.
   */
  export function runWhenIdle(name: string, seconds: number): void;
}
