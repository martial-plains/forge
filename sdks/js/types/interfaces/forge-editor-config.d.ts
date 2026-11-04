declare module 'forge:editor/config@0.1.19' {
  /**
   * The plugin's settings as a JSON object (`{}` if the user gave none).
   * Lists are arrays, tables are objects, and Lua's text, numbers and
   * booleans are their JSON kinds. Read when the plugin loads: a change
   * in init.rhai applies the next time the plugin loads. A plugin should
   * refuse a key it doesn't know, naming it, so a typo isn't silent.
   */
  export function get(): string;
}
