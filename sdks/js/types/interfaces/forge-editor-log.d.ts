declare module 'forge:editor/log@0.1.19' {
  export function log(level: Level, message: string): void;
  /**
   * # Variants
   * 
   * ## `"debug"`
   * 
   * ## `"info"`
   * 
   * ## `"warn"`
   * 
   * ## `"error"`
   */
  export type Level = 'debug' | 'info' | 'warn' | 'error';
}
