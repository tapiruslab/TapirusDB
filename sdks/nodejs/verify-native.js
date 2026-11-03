'use strict';

const fs = require('node:fs');
const path = require('node:path');
if (!fs.existsSync(path.join(__dirname, 'CORE_LICENSE'))) throw new Error('Build the SDK to include the embedded engine license before packing.');
if (!fs.existsSync(path.join(__dirname, 'LICENSE'))) throw new Error('Include the existing MIT wrapper license before packing.');
if (process.argv.includes('--all-platforms')) {
  const targets = {
    'x86_64-unknown-linux-gnu': 'linux-x64-gnu',
    'aarch64-unknown-linux-gnu': 'linux-arm64-gnu',
    'x86_64-apple-darwin': 'darwin-x64',
    'aarch64-apple-darwin': 'darwin-arm64',
    'x86_64-pc-windows-msvc': 'win32-x64-msvc',
  };
  for (const triple of require('./package.json').napi.triples.additional) {
    if (!targets[triple] || !fs.existsSync(path.join(__dirname, `tapirus.${targets[triple]}.node`))) {
      throw new Error(`Missing release addon for advertised target ${triple}`);
    }
  }
}
const nativeFiles = fs.readdirSync(__dirname).filter(file => /^tapirus(?:\..+)?\.node$/.test(file));
if (nativeFiles.length === 0) {
  throw new Error('Refusing to pack the Node SDK without a built native TapirusDB addon. Build the target binaries before npm pack/publish.');
}
if (!nativeFiles.some(file => file === 'tapirus.node' || file.startsWith(`tapirus.${process.platform}-${process.arch}`))) {
  throw new Error(`No native addon for ${process.platform}/${process.arch} is available in ${path.basename(__dirname)}.`);
}
// A filename alone cannot establish ABI, architecture, or engine version.
const { version } = require('./index.js');
const expected = require('./package.json').version;
if (version() !== expected) throw new Error(`Native engine version must match npm version ${expected}`);
