declare module 'forge:editor/menus@0.1.19' {
  /**
   * Add a menu, after the built-in ones. Fails if this plugin already
   * has one, or if the title is taken.
   */
  export function define(title: string, items: Array<Item>): void;
  /**
   * Add items to a menu someone else defined — a built-in one, or
   * another plugin's — after the ones already there.
   */
  export function extend(title: string, items: Array<Item>): void;
  /**
   * Add items to the submenu called `label` in the menu called
   * `title`, after the ones already there. A missing label becomes a
   * submenu header; a label on a plain item fails, since opening it
   * would hide the command it runs. Since 0.1.8.
   */
  export function extendSubmenu(menu: string, label: string, items: Array<Item>): void;
  /**
   * Take back what this plugin added to `title`: the whole menu if it
   * defined it, otherwise just its items.
   */
  export function remove(title: string): void;
  /**
   * Turn the bar on or off in the current buffer. A plugin that wants a
   * menu can turn it on as it loads. With `menu-bar-mode` on, F10 and
   * clicking work; off, neither does and nothing is drawn.
   */
  export function setEnabled(enabled: boolean): void;
  /**
   * Whether the bar is on in the current buffer.
   */
  export function enabled(): boolean;
  /**
   * Open a menu by title, or close the open one when `title` is empty.
   */
  export function open(title: string): void;
  /**
   * Close whatever is open.
   */
  export function close(): void;
  /**
   * The open menu's title, or none.
   */
  export function openTitle(): string | undefined;
  /**
   * Run the item under the cursor in the open menu. False when no menu
   * is open or the cursor is on a separator.
   */
  export function choose(): boolean;
  /**
   * The item under the cursor, for showing what it does.
   */
  export function selection(): ItemSelection | undefined;
  /**
   * One entry: what it says and what it runs. An empty `command` is a
   * separator (a rule between groups).
   */
  export interface Item {
    label: string,
    /**
     * A command name; empty for a separator.
     */
    command: string,
  }
  /**
   * The item under the cursor in the open menu, if any. Since 0.1.5.
   */
  export interface ItemSelection {
    /**
     * The menu it is in.
     */
    menu: string,
    label: string,
    command: string,
    /**
     * 1-based, and how many the menu has.
     */
    index: number,
    total: number,
    /**
     * False when the command isn't defined (a plugin unloaded).
     */
    available: boolean,
  }
}
