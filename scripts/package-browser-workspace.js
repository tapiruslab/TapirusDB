'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');
const source = path.join(root, 'crates', 'tapirus-wasm');
const targets = process.argv.includes('--site') ? [path.join(root, 'tapirusdb-site', 'studio')] : [path.join(root, 'studio', 'public', 'native')];
for (const destination of targets) {
  const assets = path.join(destination, 'wasm');
  fs.mkdirSync(assets, { recursive: true });
  for (const file of ['tapirus_wasm.js', 'tapirus_wasm_bg.wasm', 'LICENSE']) fs.copyFileSync(path.join(source, 'pkg', file), path.join(assets, file));
  for (const file of ['workspace.js', 'workspace.css']) fs.copyFileSync(path.join(source, 'demo', file), path.join(destination, file));
  fs.writeFileSync(path.join(destination, 'index.html'), fs.readFileSync(path.join(source, 'demo', 'index.html'), 'utf8').replace('../pkg/tapirus_wasm.js', './wasm/tapirus_wasm.js').replace('../pkg/LICENSE', './wasm/LICENSE'));
  const sha256 = file => crypto.createHash('sha256').update(fs.readFileSync(path.join(assets, file))).digest('hex');
  fs.writeFileSync(path.join(destination, 'engine-manifest.json'), JSON.stringify({ version: require(path.join(source, 'pkg', 'package.json')).version, target: 'wasm32-unknown-unknown', wasmSha256: sha256('tapirus_wasm_bg.wasm'), glueSha256: sha256('tapirus_wasm.js'), sourceHead: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(), workingTreeDirty: execFileSync('git', ['status', '--porcelain', '--untracked-files=no'], { cwd: root, encoding: 'utf8' }).trim().length > 0 }, null, 2) + '\n');
  console.log(`Packaged real Rust workspace at ${path.relative(root, destination)}`);
}
