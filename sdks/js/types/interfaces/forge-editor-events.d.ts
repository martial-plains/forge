declare module 'forge:editor/events@0.1.19' {
  export function subscribe(kind: Kind): void;
  export function unsubscribe(kind: Kind): void;
  /**
   * ---- 0.1.2
   * Like `subscribe`, but the handler runs in the background (see
   * `commands.register-background`).
   */
  export function subscribeBackground(kind: Kind): void;
  /**
   * # Variants
   * 
   * ## `"buffer-opened"`
   * 
   * ## `"buffer-killed"`
   * 
   * ## `"buffer-changed"`
   * 
   * ## `"buffer-saved"`
   * 
   * ## `"mode-changed"`
   */
  export type Kind = 'buffer-opened' | 'buffer-killed' | 'buffer-changed' | 'buffer-saved' | 'mode-changed';
  /**
   * Each carries the buffer id.
   */
  export type Event = EventBufferOpened | EventBufferKilled | EventBufferChanged | EventBufferSaved | EventModeChanged;
  export interface EventBufferOpened {
    tag: 'buffer-opened',
    val: number,
  }
  export interface EventBufferKilled {
    tag: 'buffer-killed',
    val: number,
  }
  export interface EventBufferChanged {
    tag: 'buffer-changed',
    val: number,
  }
  export interface EventBufferSaved {
    tag: 'buffer-saved',
    val: number,
  }
  /**
   * The major mode or the set of minor modes changed.
   */
  export interface EventModeChanged {
    tag: 'mode-changed',
    val: number,
  }
}
