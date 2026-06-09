#!/usr/bin/env node
// Build the XenoClaw backend and stage it as a Tauri sidecar.
//
// Tauri resolves sidecars as `binaries/<name>-<target-triple>[.exe]`, so we
// build `cargo -p xenoclaw` and copy the result to the platform-suffixed path
// the bundler / dev runner expects.
//
// Usage:
//   node src-tauri/scripts/build-sidecar.mjs            # release build
//   node src-tauri/scripts/build-sidecar.mjs --debug    # debug build (faster)

import { execSync } from 'node:child_process';
import { copyFileSync, mkdirSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SRC_TAURI = join(__dirname, '..');
const REPO_ROOT = join(SRC_TAURI, '..', '..');
const BIN_DIR = join(SRC_TAURI, 'binaries');

const debug = process.argv.includes('--debug');

function hostTriple() {
  const out = execSync('rustc -Vv', { encoding: 'utf8' });
  const m = out.match(/host:\s*(\S+)/);
  if (!m) throw new Error('Could not determine Rust host triple from `rustc -Vv`');
  return m[1];
}

const triple = hostTriple();
const isWindows = triple.includes('windows');
const ext = isWindows ? '.exe' : '';
const profile = debug ? 'debug' : 'release';

console.log(`Building xenoclaw backend (${profile}) for ${triple}…`);
execSync(`cargo build -p xenoclaw ${debug ? '' : '--release'}`, {
  cwd: REPO_ROOT,
  stdio: 'inherit',
});

const built = join(REPO_ROOT, 'target', profile, `xenoclaw${ext}`);
if (!existsSync(built)) {
  throw new Error(`Expected binary not found: ${built}`);
}

mkdirSync(BIN_DIR, { recursive: true });
const dest = join(BIN_DIR, `xenoclaw-${triple}${ext}`);
copyFileSync(built, dest);
console.log(`Staged sidecar: ${dest}`);
