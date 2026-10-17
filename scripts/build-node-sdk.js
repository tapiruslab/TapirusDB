'use strict';

// Build the host addon without downloading a JavaScript build tool.
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');
fs.copyFileSync(path.join(root, 'LICENSE'), path.join(root, 'sdks', 'nodejs', 'CORE_LICENSE'));
if (process.argv.includes('--license-only')) process.exit(0);
const debug = process.argv.includes('--debug');
const flags = process.argv.includes('--offline') ? ['--offline'] : [];
execFileSync('cargo', ['build', '--locked', '-p', 'tapirus-nodejs', ...(debug ? [] : ['--release']), ...flags], { cwd: root, stdio: 'inherit' });
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--format-version=1', ...flags], { cwd: root, encoding: 'utf8' }));
const library = process.platform === 'win32' ? 'tapirus_nodejs.dll' : process.platform === 'darwin' ? 'libtapirus_nodejs.dylib' : 'libtapirus_nodejs.so';
const suffix = process.platform === 'win32' ? '-msvc' : process.platform === 'linux' ? (process.report.getReport().header.glibcVersionRuntime ? '-gnu' : '-musl') : '';
const destination = path.join(root, 'sdks', 'nodejs', `tapirus.${process.platform}-${process.arch}${suffix}.node`);
fs.copyFileSync(path.join(metadata.target_directory, debug ? 'debug' : 'release', library), destination);
console.log(`Built ${path.relative(root, destination)}`);
