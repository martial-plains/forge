#!/usr/bin/env node
// forge-plugin build ENTRY [-o OUT.wasm]
//
// Bundle a plugin (JavaScript or TypeScript) with esbuild, turn it into a
// WebAssembly component with ComponentizeJS, and embed its manifest from
// the "forge" field of the nearest package.json:
//
//   "forge": {
//     "name": "spell",                       // default: the package name
//     "startup": false,                      // true: never start lazily
//     "permissions": {
//       "network": false,
//       "memory-mb": 256,
//       "directories": [{ "path": "~/dict", "why": "word lists", "write": false }]
//     }
//   }

import { build } from 'esbuild';
import { componentize } from '@bytecodealliance/componentize-js';
import { existsSync, readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const sdk = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

function usage(msg) {
  if (msg) console.error(`forge-plugin: ${msg}`);
  console.error('usage: forge-plugin build ENTRY.(js|ts) [-o OUT.wasm]');
  process.exit(2);
}

function nearestPackage(dir) {
  for (let d = dir; ; d = path.dirname(d)) {
    const p = path.join(d, 'package.json');
    if (existsSync(p)) return JSON.parse(readFileSync(p, 'utf8'));
    if (path.dirname(d) === d) return {};
  }
}

// ---------------------------------------------------------- manifest

const toml = {
  str: (s) => JSON.stringify(String(s)),
  value(v) {
    if (typeof v === 'string') return toml.str(v);
    if (typeof v === 'number' || typeof v === 'boolean') return String(v);
    if (Array.isArray(v)) return `[${v.map((x) => toml.value(x)).join(', ')}]`;
    throw new Error(`can't put ${JSON.stringify(v)} in a manifest`);
  },
};

const KNOWN = ['name', 'startup', 'permissions'];

export function manifest(pkg) {
  const forge = pkg.forge ?? {};
  const unknown = Object.keys(forge).find((k) => !KNOWN.includes(k));
  if (unknown) throw new Error(`package.json "forge": unknown key ${JSON.stringify(unknown)} (known: ${KNOWN.join(', ')})`);
  const name = forge.name ?? pkg.name?.replace(/^@[^/]+\//, '');
  const lines = [];
  if (name) lines.push(`name = ${toml.str(name)}`);
  if (pkg.version) lines.push(`version = ${toml.str(pkg.version)}`);
  if (pkg.description) lines.push(`description = ${toml.str(pkg.description)}`);
  if (forge.startup !== undefined) lines.push(`startup = ${toml.value(forge.startup)}`);
  const perms = forge.permissions ?? {};
  const { directories = [], ...rest } = perms;
  if (Object.keys(perms).length) {
    lines.push('', '[permissions]');
    for (const [k, v] of Object.entries(rest)) lines.push(`${k} = ${toml.value(v)}`);
    for (const d of directories) {
      lines.push('', '[[permissions.directories]]');
      for (const [k, v] of Object.entries(typeof d === 'string' ? { path: d } : d)) lines.push(`${k} = ${toml.value(v)}`);
    }
  }
  return lines.join('\n') + '\n';
}

/** Append a custom section (allowed anywhere in a component). */
function withCustomSection(bytes, name, data) {
  const leb = (n) => {
    const out = [];
    do {
      let b = n & 0x7f;
      n >>>= 7;
      if (n) b |= 0x80;
      out.push(b);
    } while (n);
    return out;
  };
  const enc = new TextEncoder();
  const nameBytes = enc.encode(name);
  const body = [...leb(nameBytes.length), ...nameBytes, ...enc.encode(data)];
  return Buffer.concat([Buffer.from(bytes), Buffer.from([0, ...leb(body.length), ...body])]);
}

// ------------------------------------------------------------ build

async function buildPlugin(entry, out) {
  const abs = path.resolve(entry);
  const pkg = nearestPackage(path.dirname(abs));
  const tmp = mkdtempSync(path.join(tmpdir(), 'forge-plugin-'));
  try {
    const bundle = path.join(tmp, 'plugin.js');
    // A plugin allowed to run programs gets their output (Forge 0.1.2).
    const processes = (pkg.forge?.permissions?.processes ?? []).length > 0;
    const extra = processes ? `export { processHandler } from '@forge-editor/plugin/process';\n` : '';
    // The entry, plus the exports Forge calls (from the SDK).
    await build({
      stdin: {
        contents: `import ${JSON.stringify(abs)};\nexport { init, handler } from '@forge-editor/plugin';\n${extra}`,
        resolveDir: path.dirname(abs),
        loader: 'js',
      },
      bundle: true,
      format: 'esm',
      platform: 'neutral',
      target: 'es2022',
      outfile: bundle,
      external: ['forge:*', 'wasi:*'],
      alias: {
        '@forge-editor/plugin/process': path.join(sdk, 'process.js'),
        '@forge-editor/plugin': path.join(sdk, 'index.js'),
      },
      logLevel: 'warning',
    });
    const { component } = await componentize({
      sourcePath: bundle,
      witPath: path.join(sdk, 'wit'),
      worldName: processes ? 'plugin-with-processes' : 'plugin',
      // No fetch(): Forge doesn't provide wasi:http.
      disableFeatures: ['http', 'fetch-event'],
    });
    const bytes = withCustomSection(component, 'forge-manifest', manifest(pkg));
    writeFileSync(out, bytes);
    console.log(`${out}: ${(bytes.length / 1e6).toFixed(1)} MB`);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

const [cmd, entry, ...rest] = process.argv.slice(2);
if (cmd !== 'build' || !entry) usage();
let out = path.basename(entry).replace(/\.[jt]s$/, '') + '.wasm';
for (let i = 0; i < rest.length; i++) {
  if (rest[i] === '-o' && rest[i + 1]) out = rest[++i];
  else usage(`unknown argument ${rest[i]}`);
}
buildPlugin(entry, out).catch((e) => {
  console.error(`forge-plugin: ${e.message ?? e}`);
  process.exit(1);
});
