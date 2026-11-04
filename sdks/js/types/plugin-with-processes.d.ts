/// <reference path="./interfaces/forge-editor-buffers.d.ts" />
/// <reference path="./interfaces/forge-editor-commands.d.ts" />
/// <reference path="./interfaces/forge-editor-completion.d.ts" />
/// <reference path="./interfaces/forge-editor-config.d.ts" />
/// <reference path="./interfaces/forge-editor-decorations.d.ts" />
/// <reference path="./interfaces/forge-editor-environment.d.ts" />
/// <reference path="./interfaces/forge-editor-events.d.ts" />
/// <reference path="./interfaces/forge-editor-files.d.ts" />
/// <reference path="./interfaces/forge-editor-handler.d.ts" />
/// <reference path="./interfaces/forge-editor-keymap.d.ts" />
/// <reference path="./interfaces/forge-editor-log.d.ts" />
/// <reference path="./interfaces/forge-editor-menus.d.ts" />
/// <reference path="./interfaces/forge-editor-minibuffer.d.ts" />
/// <reference path="./interfaces/forge-editor-modes.d.ts" />
/// <reference path="./interfaces/forge-editor-process-handler.d.ts" />
/// <reference path="./interfaces/forge-editor-processes.d.ts" />
/// <reference path="./interfaces/forge-editor-types.d.ts" />
declare module 'forge:editor/plugin-with-processes@0.1.19' {
  export type * as ForgeEditorBuffers0119 from 'forge:editor/buffers@0.1.19'; // import forge:editor/buffers@0.1.19
  export type * as ForgeEditorCommands0119 from 'forge:editor/commands@0.1.19'; // import forge:editor/commands@0.1.19
  export type * as ForgeEditorCompletion0119 from 'forge:editor/completion@0.1.19'; // import forge:editor/completion@0.1.19
  export type * as ForgeEditorConfig0119 from 'forge:editor/config@0.1.19'; // import forge:editor/config@0.1.19
  export type * as ForgeEditorDecorations0119 from 'forge:editor/decorations@0.1.19'; // import forge:editor/decorations@0.1.19
  export type * as ForgeEditorEnvironment0119 from 'forge:editor/environment@0.1.19'; // import forge:editor/environment@0.1.19
  export type * as ForgeEditorEvents0119 from 'forge:editor/events@0.1.19'; // import forge:editor/events@0.1.19
  export type * as ForgeEditorFiles0119 from 'forge:editor/files@0.1.19'; // import forge:editor/files@0.1.19
  export type * as ForgeEditorKeymap0119 from 'forge:editor/keymap@0.1.19'; // import forge:editor/keymap@0.1.19
  export type * as ForgeEditorLog0119 from 'forge:editor/log@0.1.19'; // import forge:editor/log@0.1.19
  export type * as ForgeEditorMenus0119 from 'forge:editor/menus@0.1.19'; // import forge:editor/menus@0.1.19
  export type * as ForgeEditorMinibuffer0119 from 'forge:editor/minibuffer@0.1.19'; // import forge:editor/minibuffer@0.1.19
  export type * as ForgeEditorModes0119 from 'forge:editor/modes@0.1.19'; // import forge:editor/modes@0.1.19
  export type * as ForgeEditorProcesses0119 from 'forge:editor/processes@0.1.19'; // import forge:editor/processes@0.1.19
  export type * as ForgeEditorTypes0119 from 'forge:editor/types@0.1.19'; // import forge:editor/types@0.1.19
  export * as processHandler from 'forge:editor/process-handler@0.1.19'; // export forge:editor/process-handler@0.1.19
  export * as handler from 'forge:editor/handler@0.1.19'; // export forge:editor/handler@0.1.19
  /**
  * Runs once, right after loading: register commands, define modes,
  * bind keys, subscribe to events. An error unloads the plugin.
  */
  export function init(): void;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
