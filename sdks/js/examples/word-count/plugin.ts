// An example Forge plugin in TypeScript (esbuild strips the types; run
// `tsc --noEmit` to check them).
//
//   npx forge-plugin build plugin.ts -o word-count.wasm

import { plugin, command, echo, decorations, Buffer } from '@forge-editor/plugin';

const LIMIT = 80;

function countWords(text: string): number {
  return text.split(/\s+/).filter((w) => w.length > 0).length;
}

plugin(() => {
  command('word-count', 'Show how many words, lines and characters the buffer has.', () => {
    const b = Buffer.current();
    const text = b.text();
    echo(`${countWords(text)} words, ${b.lineCount()} lines, ${[...text].length} characters`);
  });

  command('word-count-mark-long-lines', `Underline the part of each line past column ${LIMIT}.`, () => {
    const b = Buffer.current();
    const marks = [];
    for (let line = 0; line < b.lineCount(); line++) {
      const width = [...b.line(line)].length;
      if (width > LIMIT) {
        marks.push({
          span: { start: { line, column: LIMIT }, end: { line, column: width } },
          face: 'warning',
          message: `line ${line + 1} is ${width} characters long`,
        });
      }
    }
    decorations.set(b, marks);
    echo(marks.length === 0 ? 'No long lines' : `${marks.length} long line(s)`);
  });
});
