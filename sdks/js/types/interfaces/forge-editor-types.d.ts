declare module 'forge:editor/types@0.1.19' {
  /**
   * A place in a buffer: 0-based line and column. Columns count Unicode
   * scalar values (Rust `char`s), not bytes or screen cells.
   */
  export interface Position {
    line: number,
    column: number,
  }
  /**
   * Text from `start` (included) to `end` (not included).
   */
  export interface Span {
    start: Position,
    end: Position,
  }
}
