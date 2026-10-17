'use strict';

const fs = require('node:fs');
const path = require('node:path');
if (!fs.existsSync(path.join(__dirname, 'CORE_LICENSE'))) throw new Error('Build the SDK to include the embedded engine license before packing.');
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
