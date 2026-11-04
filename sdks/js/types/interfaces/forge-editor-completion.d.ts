/// <reference path="./forge-editor-types.d.ts" />
declare module 'forge:editor/completion@0.1.19' {
  /**
   * Minibuffer completion: `vertical` lists up to `lines` candidates
   * under the prompt, updated as you type (C-n / C-p to move, RET
   * picks, TAB inserts). Otherwise TAB completes in the prompt.
   */
  export function setMinibufferUi(vertical: boolean, lines: number): void;
  /**
   * Whether C-n at the last candidate wraps around to the first (and
   * C-p at the first to the last) in the minibuffer list, and similarly
   * for page moves. Off by default (vertico-cycle). Since 0.1.19.
   */
  export function setMinibufferCycle(cycle: boolean): void;
  /**
   * How typed text matches candidates, tried in order: "orderless"
   * (space-separated parts, in any order), "prefix", "substring",
   * "flex" (the letters in order). Default: prefix.
   */
  export function setMatching(styles: Array<string>): void;
  /**
   * Annotate candidates of `category` ("command", "buffer", "file",
   * "symbol", "package", "plugin" or a plugin question's): Forge calls
   * completion-handler.annotate once per list.
   */
  export function annotateCategory(category: string): void;
  /**
   * Completion at point: `popup` shows candidates in a list at point
   * (else the echo area). `auto` opens it by itself once typing pauses
   * `delay-ms` after at least `min-prefix` word characters.
   */
  export function setAtPointUi(popup: boolean, auto: boolean, delayMs: number, minPrefix: number, lines: number): void;
  /**
   * Provide candidates at point in buffers whose major mode is or
   * derives from one of `modes` (none: every buffer). Forge calls
   * completion-handler.complete when completion starts.
   */
  export function registerSource(name: string, modes: Array<string>): void;
  /**
   * ---- 0.1.4
   * How candidates are ordered once they match. Tried in order; the
   * first one applies (unknown names are ignored, so an older Forge
   * still runs a plugin that asks for more).
   *   "length"       shortest first, then alphabetical
   *   "alpha"        alphabetical
   *   "history"      most recently used first, then length
   *   "history-length" most recently used first, then length
   *   "position"     the order the source produced (no reordering)
   * Since 0.1.4. Without it, matches come back with exact hits first,
   * then prefix hits, then the rest.
   */
  export function setSorting(styles: Array<string>): void;
  /**
   * The candidate the cursor is on in the minibuffer's vertical list,
   * or in a popup at point. None when no list is open. Since 0.1.4.
   */
  export function selected(): Selection | undefined;
  /**
   * Ask the source that gave the selected candidate to say where it is
   * defined (corfu-info-location, M-g): the source answers with
   * completion-handler.resolve-location, and reports the place itself
   * (a definition can need a round trip to a language server). True
   * when a source was asked (a popup is open); false when there is no
   * selection, so the caller can fall back. Since 0.1.17.
   */
  export function locate(): boolean;
  /**
   * Widen the popup at point to fit its candidates, between
   * `min-width` and `max-width` columns (0 for either leaves that end
   * to Forge). Since 0.1.4. A terminal has no borders to draw, so
   * `border` is accepted and ignored.
   */
  export function setPopupFrame(minWidth: number, maxWidth: number, border: number): void;
  /**
   * Answer a completion-handler.complete request: the text to replace
   * and what could replace it. During that call or later (a language
   * server answers later); a late answer for a finished completion is
   * ignored.
   */
  export function deliver(request: number, span: Span, candidates: Array<Candidate>): void;
  /**
   * Answer a completion-handler.complete request with decorated
   * candidates (a margin and a deprecation flag). Since 0.1.15. Same as
   * `deliver`, but each candidate may carry a margin (shown before it)
   * and say that it is deprecated. A plugin that doesn't decorate keeps
   * using `deliver`.
   */
  export function deliverDecorated(request: number, span: Span, candidates: Array<DecoratedCandidate>): void;
  /**
   * Answer a completion-handler.complete request with detailed
   * candidates (decoration plus documentation and a location). Since
   * 0.1.16. Supersedes `deliver-decorated`; the extra fields reach the
   * selection a plugin reads back with `selected` (corfu's M-h / M-g).
   */
  export function deliverDetailed(request: number, span: Span, candidates: Array<DetailedCandidate>): void;
  /**
   * ---- 0.1.14: driving the popup (a completion plugin like corfu owns
   * the commands and policy; the popup itself is the editor's).
   * The popup's session at point, or none. Since 0.1.14.
   */
  export function current(): SessionInfo | undefined;
  /**
   * Set the popup's policy (see `options`). Replaces the whole set.
   * Since 0.1.14.
   */
  export function setOptions(options: Options): void;
  /**
   * Move the selection `n` candidates (negative: back). Since 0.1.14.
   */
  export function next(n: number): void;
  export function previous(n: number): void;
  /**
   * Select the first or last candidate. Since 0.1.14.
   */
  export function first(): void;
  export function last(): void;
  /**
   * Move a page down (or up). Since 0.1.14.
   */
  export function scroll(down: boolean): void;
  /**
   * Take the selected candidate and finish (corfu-complete). Since
   * 0.1.14.
   */
  export function accept(): void;
  /**
   * Grow the input to what every candidate shares (corfu-expand). Since
   * 0.1.14.
   */
  export function expand(): void;
  /**
   * Close the popup, keeping what was typed (corfu-quit). Since 0.1.14.
   */
  export function quit(): void;
  /**
   * Deselect the candidate; again, quit (corfu-reset). Since 0.1.14.
   */
  export function reset(): void;
  /**
   * Take the candidate on popup row `row` (corfu-indexed's C-u N TAB).
   * Since 0.1.14.
   */
  export function acceptRow(row: number): void;
  /**
   * Wait for the next key to name a candidate: "insert", "complete" or
   * "jump" (corfu-quick-*). Since 0.1.14.
   */
  export function armQuick(action: string): void;
  /**
   * Insert the component separator and keep completing
   * (corfu-insert-separator). Since 0.1.14.
   */
  export function insertSeparator(): void;
  /**
   * ---- 0.1.18: an auxiliary popup beside the completion popup
   * (corfu-popupinfo). The plugin asks for it and drives it; Forge draws
   * it from the selection's documentation and location.
   * Ask for an auxiliary popup beside the completion popup showing the
   * selected candidate's documentation and/or location
   * (corfu-popupinfo). `show-documentation` and `show-location` pick the
   * parts; `delay-ms` waits after the selection settles before showing
   * (0: at once); `max-width` and `max-height` cap it (0: Forge's
   * default). Nothing shows when the selection has neither, when no
   * popup is open, or while popup-info is toggled off. Since 0.1.18.
   */
  export function setPopupInfo(showDocumentation: boolean, showLocation: boolean, delayMs: number, maxWidth: number, maxHeight: number): void;
  /**
   * Turn the auxiliary popup off, or back on, for the popup that is
   * open (corfu-popupinfo-toggle). Since 0.1.18.
   */
  export function togglePopupInfo(): void;
  /**
   * Scroll the auxiliary popup a page down (or up)
   * (corfu-popupinfo-scroll-down / -scroll-up). Since 0.1.18.
   */
  export function scrollPopupInfo(down: boolean): void;
  /**
   * Scroll the auxiliary popup to its first line
   * (corfu-popupinfo-beginning). Since 0.1.18.
   */
  export function popupInfoBeginning(): void;
  /**
   * Scroll the auxiliary popup to its last line (corfu-popupinfo-end).
   * Since 0.1.18.
   */
  export function popupInfoEnd(): void;
  export type Position = import('forge:editor/types@0.1.19').Position;
  export type Span = import('forge:editor/types@0.1.19').Span;
  /**
   * A candidate for completion at point.
   */
  export interface Candidate {
    text: string,
    /**
     * Shown next to it ("fn", "keyword", a signature).
     */
    annotation?: string,
  }
  /**
   * A candidate with display decoration. Since 0.1.15. A plugin that
   * shows an icon or a kind before a candidate, or marks some as
   * deprecated, delivers these instead of `candidate`s.
   */
  export interface DecoratedCandidate {
    text: string,
    /**
     * Shown next to it ("fn", "keyword", a signature).
     */
    annotation?: string,
    /**
     * Shown before it: an icon or a short kind ("fn", "var", "mod").
     */
    margin?: string,
    /**
     * The candidate is deprecated: drawn dimmed / struck through.
     */
    deprecated: boolean,
  }
  /**
   * Where a candidate is defined: the file, when it is not the buffer
   * the list is in, and a place in it. Since 0.1.17. Matches the
   * `(buffer-or-file . position)` company's `location` command
   * returns (corfu-info-location).
   */
  export interface Location {
    /**
     * The file it is defined in. None: the current buffer.
     */
    path?: string,
    /**
     * 0-based line and column in that file (see `position`).
     */
    line: number,
    column: number,
  }
  /**
   * A candidate with everything a completion-at-point source can give:
   * display decoration, documentation and a source location. Since
   * 0.1.16. Supersedes `decorated-candidate` (which has no
   * documentation or location).
   */
  export interface DetailedCandidate {
    text: string,
    /**
     * Shown next to it ("fn", "keyword", a signature).
     */
    annotation?: string,
    /**
     * Shown before it: an icon or a short kind ("fn", "var", "mod").
     */
    margin?: string,
    /**
     * The candidate is deprecated: drawn dimmed / struck through.
     */
    deprecated: boolean,
    /**
     * Longer text shown for the selection (corfu's M-h).
     */
    documentation?: string,
    /**
     * Where the candidate is defined (corfu's M-g).
     */
    location?: Location,
  }
  /**
   * The candidate under the cursor in an open list, and where it is.
   * Since 0.1.4. A plugin uses it to show documentation about the
   * selection (corfu's M-h) without owning the list.
   */
  export interface Selection {
    /**
     * The candidate's text, as it would be inserted.
     */
    text: string,
    /**
     * Its annotation, if it has one.
     */
    annotation?: string,
    /**
     * The category it came from ("command", "buffer", ...).
     */
    category: string,
    /**
     * 1-based position in the list, and how many match.
     */
    index: number,
    total: number,
    /**
     * The selection's documentation, if the source gave one. Since
     * 0.1.16.
     */
    documentation?: string,
    /**
     * Where the selection is defined, if the source said. Since
     * 0.1.17 (was a bare position in 0.1.16).
     */
    location?: Location,
  }
  /**
   * The popup's session at point, for a plugin driving it. Since 0.1.14.
   */
  export interface SessionInfo {
    /**
     * A completion session is open.
     */
    open: boolean,
    /**
     * It is drawn in a popup (not the echo area).
     */
    popup: boolean,
    /**
     * 0-based position of the selected candidate among the matches.
     */
    index: number,
    total: number,
    /**
     * Nothing is selected (preselect 'prompt, or after a reset).
     */
    deselected: boolean,
    /**
     * How many rows the popup shows.
     */
    lines: number,
    /**
     * The input typed into the popup.
     */
    input: string,
  }
  /**
   * The popup's policy: what a completion plugin (corfu) chooses. Since
   * 0.1.14.
   */
  export interface Options {
    /**
     * C-n on the last candidate wraps to the first.
     */
    cycle: boolean,
    /**
     * Nothing is selected until you move.
     */
    preselectPrompt: boolean,
    /**
     * The component separator (corfu-separator).
     */
    separator: string,
    /**
     * "never", "always" or "separator".
     */
    quitAtBoundary: string,
    /**
     * "never", "always" or "separator".
     */
    quitNoMatch: string,
    /**
     * Names (or `name*` prefixes) that keep completion open at the
     * boundary.
     */
    continueCommands: Array<string>,
    /**
     * Number the popup's rows.
     */
    indexed: boolean,
    /**
     * Quickselect keys: the first labels the first candidates, the
     * second groups the rest.
     */
    quick1: string,
    quick2: string,
    /**
     * Show the selected candidate's annotation after a pause.
     */
    echo: boolean,
    echoDelayMs: number,
    /**
     * Click a row to take it.
     */
    mouse: boolean,
    /**
     * Rank candidates you took before first.
     */
    history: boolean,
  }
}
