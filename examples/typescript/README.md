# TypeScript native SDK example

Uses the same Rust addon and declarations as the `tapirus` npm package. Node.js
22+ is required. From the repository root:

```sh
node scripts/build-node-sdk.js --debug
cd examples/typescript
npm install
npm run check
npm start
```

The local file dependency uses `sdks/nodejs`; this does not verify a published
registry version. For an application outside this repository, use `npm install
tapirus` once the desired native release is published. SQL values are bound with
parameters. See the [native SDK guide](../../sdks/nodejs/README.md).
