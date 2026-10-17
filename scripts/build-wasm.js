'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');
const lock = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
const expected = lock.match(/name = "wasm-bindgen"\r?\nversion = "([^"]+)"/)[1];
const bindgen = process.env.WASM_BINDGEN_BIN || 'wasm-bindgen';
const actual = execFileSync(bindgen, ['--version'], { encoding: 'utf8' }).trim();
if (actual !== `wasm-bindgen ${expected}`) throw new Error(`Install wasm-bindgen-cli ${expected}; found ${actual}`);
const flags = process.argv.includes('--offline') ? ['--offline'] : [];
const debug = process.argv.includes('--debug');
execFileSync('cargo', ['build', '--locked', '-p', 'tapirus-wasm', '--target', 'wasm32-unknown-unknown', ...(debug ? [] : ['--release']), ...flags], { cwd: root, stdio: 'inherit' });
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--format-version=1', ...flags], { cwd: root, encoding: 'utf8' }));
const destination = path.join(root, 'crates', 'tapirus-wasm', 'pkg');
execFileSync(bindgen, [path.join(metadata.target_directory, 'wasm32-unknown-unknown', debug ? 'debug' : 'release', 'tapirus_wasm.wasm'), '--target', 'web', '--typescript', '--out-dir', destination, '--out-name', 'tapirus_wasm'], { cwd: root, stdio: 'inherit' });
const version = fs.readFileSync(path.join(root, 'crates', 'tapirus-wasm', 'Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)[1];
fs.copyFileSync(path.join(root, 'LICENSE'), path.join(destination, 'LICENSE'));
fs.writeFileSync(path.join(destination, 'package.json'), JSON.stringify({ name: '@tapirus/wasm', version, type: 'module', main: 'tapirus_wasm.js', types: 'tapirus_wasm.d.ts', files: ['tapirus_wasm.js', 'tapirus_wasm.d.ts', 'tapirus_wasm_bg.wasm', 'tapirus_wasm_bg.wasm.d.ts', 'LICENSE', 'README.md'], license: 'BUSL-1.1' }, null, 2) + '\n');
if (process.argv.includes('--studio')) {
  execFileSync(process.execPath, [path.join(root, 'scripts', 'package-browser-workspace.js')], { cwd: root, stdio: 'inherit' });
}
console.log(`Generated Rust WASM engine v${version} with matching JavaScript glue.`);
