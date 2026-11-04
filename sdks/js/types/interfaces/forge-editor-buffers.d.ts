/// <reference path="./forge-editor-types.d.ts" />
declare module 'forge:editor/buffers@0.1.19' {
  export type Position = import('forge:editor/types@0.1.19').Position;
  export type Span = import('forge:editor/types@0.1.19').Span;
  
  export class Buffer implements Disposable {
    /**
     * This type does not have a public constructor.
     */
    private constructor();
    /**
    * The buffer in the selected window.
    */
    static current(): Buffer;
    /**
    * Every buffer, oldest first.
    */
    static all(): Array<Buffer>;
    static find(name: string): Buffer | undefined;
    static fromId(id: number): Buffer | undefined;
    /**
    * A new, empty buffer. The name gets <2>, <3>, ... if it's taken.
    * It isn't shown until `show` is called.
    */
    static create(name: string): Buffer;
    /**
    * Stable for the buffer's life; events refer to buffers by id.
    */
    id(): number;
    alive(): boolean;
    name(): string;
    /**
    * The file the buffer visits, if any.
    */
    path(): string | undefined;
    /**
    * Unsaved changes?
    */
    modified(): boolean;
    /**
    * Grows by at least one with every change to the text.
    */
    version(): bigint;
    text(): string;
    textIn(span: Span): string;
    lineCount(): number;
    /**
    * One line without its newline ("" past the end).
    */
    line(line: number): string;
    point(): Position;
    setPoint(position: Position): void;
    mark(): Position | undefined;
    /**
    * Set (and highlight) or clear the mark.
    */
    setMark(position: Position | undefined): void;
    /**
    * The highlighted region, if the mark is active.
    */
    region(): Span | undefined;
    /**
    * Insert at point; point ends up after the text.
    */
    insert(text: string): void;
    replace(span: Span, text: string): void;
    /**
    * Delete a span and return what was there.
    */
    'delete'(span: Span): string;
    /**
    * Literal search, case-insensitive unless `query` has capitals
    * (like C-s). Forward finds the first match at or after `start`;
    * backward the last one ending at or before it.
    */
    searchForward(query: string, start: Position): Span | undefined;
    searchBackward(query: string, start: Position): Span | undefined;
    /**
    * First match of a regular expression (Rust regex syntax) at or
    * after `start`. Err if the pattern is invalid.
    */
    searchRegexp(pattern: string, start: Position): Span | undefined;
    majorMode(): string;
    setMajorMode(mode: string): void;
    /**
    * Enabled minor modes, highest keymap priority first. Set a mode's
    * priority with `keymap.set-priority` to change where it sits.
    */
    minorModes(): Array<string>;
    enableMinorMode(mode: string): void;
    disableMinorMode(mode: string): void;
    /**
    * Show this buffer in the selected window.
    */
    show(): void;
    /**
    * Kill the buffer, discarding unsaved changes.
    */
    kill(): void;
    /**
    * ---- 0.1.1
    * Set a buffer option. An empty
    * value goes back to the default. Options: "line-numbers" (off,
    * absolute, relative), "indent-width" (1-16), "indent-tabs"
    * (true, false), "fill-column", and since 0.1.9 "read-only"
    * (true, false): commands may not change the text (typing says
    * "Buffer is read-only"), but this API still can, so a plugin
    * keeps rewriting a listing it owns.
    */
    setOption(name: string, value: string): void;
    /**
    * An option's value; none if it's unknown or unset.
    */
    getOption(name: string): string | undefined;
    /**
    * Re-indent a line by the major mode's rules (what TAB does).
    * False if the mode has none.
    */
    indentLine(line: number): boolean;
    /**
    * ---- 0.1.2
    * Open a file in a buffer (or find the buffer already visiting
    * it), without showing it. Needs a directory grant covering the
    * file, or the processes permission (which trusts the plugin
    * with more than that anyway).
    */
    static visit(path: string): Buffer;
    /**
    * ---- 0.1.10
    * Text this plugin shows in the buffer's mode line, after the
    * version-control part: a language
    * server's name and its diagnostics, say. None removes it.
    */
    setModeLineInfo(text: string | undefined): void;
    /**
    * What this plugin set with `set-mode-line-info`.
    */
    modeLineInfo(): string | undefined;
    [Symbol.dispose](): void;
  }
}
