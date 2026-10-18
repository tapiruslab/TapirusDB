'use strict';

// Exercise the npm tarball in a separate consumer directory and a second process.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');
const nodeDirectory = path.dirname(fs.realpathSync(process.execPath));
const npmCli = [
  process.env.npm_execpath,
  path.join(nodeDirectory, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
  path.resolve(nodeDirectory, '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
].find(candidate => candidate && fs.existsSync(candidate));
assert.ok(npmCli, 'Run via npm run test:package or install npm alongside Node');
assert.ok(fs.existsSync(npmCli), 'Run via npm run test:package or make npm-cli.js available beside Node');
const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-npm-consumer-'));
const npmEnvironment = { ...process.env, npm_config_cache: path.join(directory, 'cache'), npm_config_update_notifier: 'false' };
try {
  const packed = JSON.parse(execFileSync(process.execPath, [npmCli, 'pack', '--json', '--pack-destination', directory], { cwd: path.join(root, 'sdks', 'nodejs'), env: npmEnvironment, encoding: 'utf8' }));
  const consumer = path.join(directory, 'consumer');
  fs.mkdirSync(consumer);
  fs.writeFileSync(path.join(consumer, 'package.json'), JSON.stringify({ name: 'tapirus-install-check', version: '0.0.0', private: true }));
  execFileSync(process.execPath, [npmCli, 'install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', path.join(directory, packed[0].filename)], { cwd: consumer, env: npmEnvironment, stdio: 'inherit' });
  const environment = { ...process.env, TAPIRUS_TEST_PACKAGE: path.join(consumer, 'node_modules', 'tapirus') };
  delete environment.TAPIRUS_NATIVE_PATH;
  delete environment.TAPIRUS_TEST_REOPEN_PATH;
  execFileSync(process.execPath, [path.join(root, 'sdks', 'nodejs', 'test.js')], { cwd: consumer, env: environment, stdio: 'inherit' });
  console.log('npm tarball installation and independent consumer checks passed.');
} finally {
  // directory is a fresh task-owned child of os.tmpdir().
  fs.rmSync(directory, { recursive: true, force: true });
}
