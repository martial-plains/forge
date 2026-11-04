/// <reference path="./forge-editor-buffers.d.ts" />
/// <reference path="./forge-editor-types.d.ts" />
declare module 'forge:editor/decorations@0.1.19' {
  /**
   * Replace this plugin's decorations in a buffer. They move with
   * edits, and go away with the text they're on or when the plugin is
   * unloaded. Fails (setting nothing) on an unknown face.
   */
  export function set(buffer: Buffer, decorations: Array<Decoration>): void;
  /**
   * Remove this plugin's decorations from a buffer.
   */
  export function clear(buffer: Buffer): void;
  /**
   * The syntax highlighting of a span, as decorations with a syntax
   * face (no message), merged into runs. Empty if the buffer's major
   * mode has no grammar. A spell checker uses this to look only at
   * comments and strings.
   */
  export function syntax(buffer: Buffer, span: Span): Array<Decoration>;
  /**
   * Attach region maps to stretches of text, replacing the ones this
   * plugin set before. They move with edits, and go away with the text
   * they're on or when the plugin is unloaded. A region map only
   * applies while point is inside its stretch, which is what makes a
   * name in a listing clickable in its own way from the line around it.
   * 
   * Separate from `set` because a keymap is not a face: the same text
   * can be coloured by one call and made clickable by another.
   */
  export function setRegions(buffer: Buffer, regions: Array<Region>): void;
  /**
   * Show stretches of text as something else, replacing the
   * replacements this plugin set before (its faces and region maps
   * stay). They move with edits and go away with the text or the
   * plugin. The line point is on is always shown as it is, so you edit
   * the real text. Spans that overlap are not combined: the first one
   * wins. Fails (setting nothing) on an unknown face. Since 0.1.11.
   */
  export function setDisplay(buffer: Buffer, replacements: Array<Replacement>): void;
  export type Span = import('forge:editor/types@0.1.19').Span;
  export type Buffer = import('forge:editor/buffers@0.1.19').Buffer;
  /**
   * `face` is one of "error", "warning", "info", "spell-error",
   * "highlight", or a syntax face: "keyword", "string", "escape",
   * "comment", "function", "type", "constant", "builtin", "property",
   * "attribute", or (since 0.1.9) a bracket-depth face for
   * rainbow-delimiters: "rainbow-depth-1" to "rainbow-depth-9",
   * "rainbow-unmatched", "rainbow-mismatched", or (since 0.1.11) one of
   * org-modern's: "org-modern-todo", "org-modern-done", "org-modern-tag",
   * "org-modern-priority", "org-modern-label", "org-modern-date-active",
   * "org-modern-time-active", "org-modern-date-inactive",
   * "org-modern-time-inactive", "org-modern-progress-complete",
   * "org-modern-progress-incomplete", "org-modern-block-name",
   * "org-modern-symbol", "org-modern-horizontal-rule",
   * "org-modern-target". The frontend picks the colors.
   */
  export interface Decoration {
    span: Span,
    face: string,
    /**
     * Shown in the echo area while point is on the text.
     */
    message?: string,
  }
  /**
   * A stretch of text and the region map that goes with it, so a key
   * bound in that map (`keymap.bind-region`) applies while point is
   * inside the stretch. Since 0.1.7.
   */
  export interface Region {
    span: Span,
    /**
     * The region map's name.
     */
    keymap: string,
  }
  /**
   * ---- 0.1.11
   * A stretch of text to show as something else, or not at all: Org's
   * `#+` hidden, a TODO keyword drawn as a label, a bullet for a dash.
   * The text in the buffer doesn't change.
   */
  export interface Replacement {
    span: Span,
    /**
     * What to show instead; none hides the span.
     */
    text?: string,
    /**
     * The face of what is shown, as for `decoration` (none: the
     * text's own).
     */
    face?: string,
  }
}
