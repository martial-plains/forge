// A plugin that runs a program: the buffer goes to wc's stdin, and its
// answer comes back later, when the editor is idle.
import { plugin, command, echo, Buffer } from '@forge-editor/plugin';
import { spawn } from '@forge-editor/plugin/process';

plugin(() => {
  command('run-wc', 'Count words with wc.', () => {
    let out = '';
    const p = spawn('wc', ['-w'], {
      onOutput: (text) => (out += text),
      onExit: (code) => echo(`wc: ${out.trim()} words (exit ${code})`),
    });
    p.write(Buffer.current().text());
    p.closeInput();
  });
});
