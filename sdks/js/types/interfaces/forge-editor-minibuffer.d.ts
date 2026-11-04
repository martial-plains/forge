declare module 'forge:editor/minibuffer@0.1.19' {
  export function readString(prompt: string, initial: string): Request;
  /**
   * TAB completes from `candidates`. With `require-match`, only a
   * candidate is accepted.
   */
  export function completingRead(prompt: string, candidates: Array<string>, requireMatch: boolean): Request;
  /**
   * Answer is "y" or "n".
   */
  export function yOrN(prompt: string): Request;
  /**
   * Show a message in the echo area.
   */
  export function message(text: string): void;
  /**
   * ---- 0.1.3
   * Read one key; the answer is its description ("a", "C-x", "RET"),
   * none for C-g. `prompt` shows in the echo area meanwhile.
   */
  export function readKey(prompt: string): Request;
  /**
   * ---- 0.1.10
   * What the echo area shows right now, if anything: so documentation shown when idle doesn't cover
   * "Wrote file" and the like.
   */
  export function currentMessage(): string | undefined;
  export type Request = number;
}
