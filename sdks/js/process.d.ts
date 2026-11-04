// Types for @forge-editor/plugin/process (see process.js).
export interface Process {
  id: number;
  write(data: string | Uint8Array): void;
  closeInput(): void;
  kill(): void;
}
export function spawn(
  program: string,
  args?: string[],
  options?: {
    dir?: string;
    onOutput?: (data: string | Uint8Array) => void;
    onExit?: (code: number | undefined) => void;
    /** Decode output as UTF-8 (default true). */
    text?: boolean;
  },
): Process;
export function projectRoot(path: string): string | undefined;
