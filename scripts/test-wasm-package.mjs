// Install the actual WASM tarball and execute it outside the source tree.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const nodeDirectory = path.dirname(fs.realpathSync(process.execPath));
const npmCli = [process.env.npm_execpath,
  path.join(nodeDirectory, 'node_modules/npm/bin/npm-cli.js'),
  path.resolve(nodeDirectory, '../lib/node_modules/npm/bin/npm-cli.js'),
].find(candidate => candidate && fs.existsSync(candidate));
assert.ok(npmCli, 'npm must be installed alongside Node.js');
const output = path.join(root, 'target/stack-packages/wasm');
fs.mkdirSync(output, { recursive: true });
const consumer = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-wasm-package-'));
const env = { ...process.env, npm_config_cache: path.join(consumer, 'cache'), npm_config_update_notifier: 'false' };
try {
  fs.writeFileSync(path.join(consumer, 'package.json'), JSON.stringify({ name: 'tapirus-wasm-install-check', version: '0.0.0', private: true }));
  const source = path.join(root, 'crates/tapirus-wasm/pkg');
  const packed = JSON.parse(execFileSync(process.execPath,
    [npmCli, 'pack', '--json', '--pack-destination', output], { cwd: source, env, encoding: 'utf8' }))[0];
  for (const name of ['LICENSE', 'README.md', 'tapirus_wasm.js', 'tapirus_wasm.d.ts', 'tapirus_wasm_bg.wasm']) {
    assert.ok(packed.files.some(file => file.path === name), `WASM tarball must contain ${name}`);
  }
  const tarball = path.join(output, packed.filename);
  execFileSync(process.execPath, [npmCli, 'install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', tarball],
    { cwd: consumer, env, stdio: 'inherit' });
  const installed = path.join(consumer, 'node_modules/@tapirus/wasm');
  const { default: init, TapirusWasm } = await import(pathToFileURL(path.join(installed, 'tapirus_wasm.js')));
  await init({ module_or_path: fs.readFileSync(path.join(installed, 'tapirus_wasm_bg.wasm')) });
  const expected = JSON.parse(fs.readFileSync(path.join(source, 'package.json'), 'utf8')).version;
  assert.equal(TapirusWasm.version(), expected);
  let db = new TapirusWasm();
  try {
    db.execute('CREATE TABLE packaged (id INTEGER PRIMARY KEY, note TEXT)');
    db.execute_params('INSERT INTO packaged VALUES (?, ?)', JSON.stringify([1, "quote ' ? ;"]));
    db.execute('BEGIN');
    db.execute_params('INSERT INTO packaged VALUES (?, ?)', '[2,"rolled back"]');
    db.execute('ROLLBACK');
    const snapshot = db.export_bytes();
    db.free();
    db = TapirusWasm.import_bytes(snapshot);
    assert.deepEqual(JSON.parse(db.query_json('SELECT * FROM packaged')), [{ id: 1, note: "quote ' ? ;" }]);
    assert.throws(() => db.query_json('INVALID SQL'));
  } finally {
    db.free();
  }
  console.log(`WASM ${expected}: installed tarball, bound SQL, rollback, snapshot reopen and errors passed. ${tarball}`);
} finally {
  fs.rmSync(consumer, { recursive: true, force: true });
}
