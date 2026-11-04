declare module 'forge:editor/keymap@0.1.19' {
  /**
   * Keys use key notation: "C-c C-t", "M-g g", "S-a", "<f5>".
   * Plugins can't bind C-c followed by a letter: those keys belong to
   * the user.
   */
  export function bind(scope: Scope, keys: string, command: string): void;
  /**
   * Returns whether there was a binding to remove.
   */
  export function unbind(scope: Scope, keys: string): boolean;
  /**
   * Keys that would run `command` run `replacement` instead.
   */
  export function remap(scope: Scope, command: string, replacement: string): void;
  /**
   * The command `keys` runs in the current buffer, if any.
   */
  export function lookup(keys: string): string | undefined;
  /**
   * ---- 0.1.3
   * Is `keys` the start of a longer binding in the current buffer
   * ("C-x" is)? For reading key sequences one key at a time.
   */
  export function isPrefix(keys: string): boolean;
  /**
   * Handle `keys` as if typed, once the current call returns (after
   * commands.run and questions). For modal editing and keypads.
   */
  export function executeKeys(keys: string): void;
  /**
   * ---- 0.1.7
   * Bind `keys` to `command` in the region map `name`, the keymap a
   * stretch of text brings with it: it is searched while point is
   * inside a decoration that names it (`decorations.set-regions`, or
   * a buffer's own regions), at `keymap.set-priority`'s default for it.
   * 
   * A click is a key like any other, so `<mouse-1>` and `<mouse-2>`
   * are keys this takes. This is where a clickable name, a magit
   * section or a column header gets its behaviour.
   * 
   * Not a `scope` case, for the reason `bind-transient` isn't: use
   * this rather than `bind(scope.region(name), ...)`.
   */
  export function bindRegion(name: string, keys: string, command: string): void;
  /**
   * Drop the region map `name` and its bindings. Err if there was
   * none.
   */
  export function removeRegion(name: string): boolean;
  /**
   * Bind `keys` to `command` in a project's own keymap, applying to
   * every file under `root` (the longest such root that is above the
   * file wins). This is how `.dir-locals.el` bindings are built in: a
   * checkout can have keys of its own without the user rebinding them
   * buffer by buffer.
   * 
   * Not a `scope` case, for the reason `bind-transient` isn't: use
   * this rather than `bind(scope.project(root), ...)`.
   */
  export function bindProject(root: string, keys: string, command: string): void;
  /**
   * Drop the project map for `root` and its bindings. Err if there
   * was none. A project's own `init.rhai` is a separate thing: this
   * forgets only the bindings.
   */
  export function removeProject(root: string): boolean;
  /**
   * The keys that run `command` in the current buffer ("C-x C-f"),
   * most direct first.
   */
  export function whereIs(command: string): Array<string>;
  /**
   * ---- 0.1.6
   * Put a layer's bindings at `priority`, above or below the layers they
   * would otherwise be searched among. Higher wins. Meow uses this to sit
   * its modal maps above the major mode without rebinding the keys it
   * wants. Returns the priority the scope had before, which is the default
   * for that scope when nothing has overridden it.
   */
  export function setPriority(scope: Scope, priority: number): number;
  /**
   * Drop the override, going back to the default for that scope. Returns
   * whether there was one.
   */
  export function resetPriority(scope: Scope): boolean;
  /**
   * Bind `keys` to `command` in the transient map `name`. The map is
   * remembered but only searched while it is up (`push-transient`).
   */
  export function bindTransient(name: string, keys: string, command: string): void;
  /**
   * Bind `keys` to `command` in the prompt map `name`, which is named by
   * what is asking: "find-file", "dired", "m-x", "completion-popup". A
   * prompt with no map of its own uses the default one, so binding into
   * a named prompt only shadows the keys named here. A key the map
   * doesn't bind is typed, or drops to the buffer as usual.
   * 
   * Not a `scope` case, for the reason `bind-transient` isn't: use this
   * rather than `bind(scope.prompt(name), ...)`.
   */
  export function bindPrompt(name: string, keys: string, command: string): void;
  /**
   * Bind `keys` to `command` in the conditional layer `name`, and set the
   * condition that layer applies while, written the way Forge writes a
   * condition (the same text it prints for one):
   * 
   *   region-active, read-only, in-comment, in-string, in-git-repo
   *   major-derives-from(MODE), minor-mode-on(MODE)
   *   not(CONDITION), all(CONDITION ...), any(CONDITION ...)
   * 
   * Only built-in conditions, since each is answered from a snapshot
   * Forge takes once per keypress; a predicate a plugin supplied would
   * run on every key instead.
   * 
   * A layer replaces its condition but keeps the bindings already in it,
   * so one can be narrowed. Like `bind-transient` and `bind-prompt`,
   * this is a function rather than a `scope` case.
   */
  export function bindLayer(name: string, when: string, keys: string, command: string): void;
  /**
   * Drop the layer `name` and its bindings. Returns whether it was there.
   */
  export function removeLayer(name: string): boolean;
  /**
   * Put a transient map on top of every other layer. Pushing the same
   * name again makes it the outermost one. The map stays up until it is
   * popped, until a one-shot key resolves in it, until its timeout
   * passes, or until its `on-unbound` policy fires.
   */
  export function pushTransient(name: string, opts: Array<TransientOpts>): void;
  /**
   * Take a transient map down. Err if it wasn't up.
   */
  export function popTransient(name: string): void;
  /**
   * The transient maps that are up, outermost first.
   */
  export function transients(): Array<string>;
  /**
   * Which keymap a binding goes in. Buffer-local maps beat minor modes,
   * which beat the major mode, which beats global.
   * Not a `scope` case: a transient map can't go here, because adding a
   * case to `scope` would change its type and stop older plugins (0.1.0
   * to 0.1.6) from loading. Bind into one with `bind-transient`.
   */
  export type Scope = ScopeGlobal | ScopeMode | ScopeBuffer;
  export interface ScopeGlobal {
    tag: 'global',
  }
  /**
   * A major or minor mode's keymap, by mode name.
   */
  export interface ScopeMode {
    tag: 'mode',
    val: string,
  }
  /**
   * One buffer's local keymap, by buffer id.
   */
  export interface ScopeBuffer {
    tag: 'buffer',
    val: number,
  }
  /**
   * What a key that doesn't reach a live transient map does.
   * # Variants
   * 
   * ## `"stay"`
   * 
   * Pass it to the layers below; the map stays up.
   * ## `"swallow"`
   * 
   * Drop it; the map stays up. For a keypad that owns the keyboard
   * while it is open. Since 0.1.9 Forge lists such a map's keys
   * (with their commands' docs) while it is up.
   * ## `"exit"`
   * 
   * Take the map down, then let the key resolve again in what's left.
   */
  export type OnUnbound = 'stay' | 'swallow' | 'exit';
  /**
   * A map installed on top of every other layer for as long as it is
   * live, the way transient maps work. Bind into it with
   * `bind(keymap.scope.transient(name), ...)` first.
   */
  export type TransientOpts = TransientOptsOneShot | TransientOptsOnUnbound | TransientOptsTimeout;
  /**
   * Ends after the first key it resolves.
   */
  export interface TransientOptsOneShot {
    tag: 'one-shot',
  }
  /**
   * What a key that the map doesn't bind should do.
   */
  export interface TransientOptsOnUnbound {
    tag: 'on-unbound',
    val: OnUnbound,
  }
  /**
   * Seconds; absent (or 0) means it stays until popped.
   */
  export interface TransientOptsTimeout {
    tag: 'timeout',
    val: number,
  }
}
