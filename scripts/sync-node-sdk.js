'use strict';
// Keep the two distribution entry points identical to the authoritative native SDK.
const fs = require('node:fs');
const path = require('node:path');
const source = path.join(__dirname, '..', 'sdks', 'nodejs', 'index.js');
const target = path.join(__dirname, '..', 'packages', 'npm', 'index.js');
const declarationSource = path.join(__dirname, '..', 'sdks', 'nodejs', 'index.d.ts');
const declarationTarget = path.join(__dirname, '..', 'packages', 'npm', 'index.d.ts');
const testSource = path.join(__dirname, '..', 'sdks', 'nodejs', 'test.js');
const testTarget = path.join(__dirname, '..', 'packages', 'npm', 'test.js');
if (process.argv.includes('--check')) {
  for (const [expected, actual] of [[source, target], [declarationSource, declarationTarget], [testSource, testTarget]]) {
    if (fs.readFileSync(expected, 'utf8') !== fs.readFileSync(actual, 'utf8')) {
      throw new Error(`Node SDK distribution files differ: ${path.basename(expected)}`);
    }
  }
} else {
  fs.copyFileSync(source, target);
  fs.copyFileSync(declarationSource, declarationTarget);
  fs.copyFileSync(testSource, testTarget);
}
