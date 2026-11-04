declare module 'forge:editor/modes@0.1.19' {
  /**
   * Define a major mode (the name must end in "-mode"). The parent's
   * keymap applies under this mode's. Files whose names end in
   * ".EXT" for one of `extensions` open in this mode.
   */
  export function defineMajorMode(name: string, parent: string | undefined, extensions: Array<string>): void;
  export function defineMinorMode(name: string): void;
  /**
   * Known major and minor mode names.
   */
  export function majorModes(): Array<string>;
  export function minorModes(): Array<string>;
  /**
   * ---- 0.1.3
   * What the mode line shows for one of this plugin's minor modes
   * (default: its name without "-mode"; "" hides it).
   */
  export function setLighter(mode: string, lighter: string): void;
}
