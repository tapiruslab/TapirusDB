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
const lite = process.argv.includes('--lite');
const cargoArgs = [
  'build',
  '--locked',
  '-p',
  'tapirus-wasm',
  '--target',
  'wasm32-unknown-unknown',
  ...(lite ? ['--no-default-features', '--features', 'lite', '--profile', 'min-size'] : debug ? [] : ['--release']),
  ...flags,
];
execFileSync('cargo', cargoArgs, { cwd: root, stdio: 'inherit' });
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--format-version=1', ...flags], { cwd: root, encoding: 'utf8' }));
const profileDir = lite ? 'min-size' : debug ? 'debug' : 'release';
const destination = path.join(root, 'crates', 'tapirus-wasm', lite ? 'pkg-lite' : 'pkg');
const outName = lite ? 'tapirus_wasm_lite' : 'tapirus_wasm';
const pkgName = lite ? '@tapirus/wasm-lite' : '@tapirus/wasm';
execFileSync(bindgen, [path.join(metadata.target_directory, 'wasm32-unknown-unknown', profileDir, 'tapirus_wasm.wasm'), '--target', 'web', '--typescript', '--out-dir', destination, '--out-name', outName], { cwd: root, stdio: 'inherit' });
const version = fs.readFileSync(path.join(root, 'crates', 'tapirus-wasm', 'Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)[1];
fs.copyFileSync(path.join(root, 'LICENSE'), path.join(destination, 'LICENSE'));
fs.copyFileSync(path.join(root, 'crates', 'tapirus-wasm', 'README.md'), path.join(destination, 'README.md'));
fs.writeFileSync(path.join(destination, 'package.json'), JSON.stringify({ name: pkgName, version, type: 'module', main: `${outName}.js`, types: `${outName}.d.ts`, files: [`${outName}.js`, `${outName}.d.ts`, `${outName}_bg.wasm`, `${outName}_bg.wasm.d.ts`, 'LICENSE', 'README.md'], license: 'BUSL-1.1' }, null, 2) + '\n');
if (!lite && process.argv.includes('--studio')) {
  execFileSync(process.execPath, [path.join(root, 'scripts', 'package-browser-workspace.js')], { cwd: root, stdio: 'inherit' });
}
const wasmBytes = fs.statSync(path.join(destination, `${outName}_bg.wasm`)).size;
console.log(`Generated Rust WASM (${lite ? 'lite' : 'quad'}) engine v${version} (${wasmBytes} bytes) with matching JavaScript glue.`);
