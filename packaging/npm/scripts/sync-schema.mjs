#!/usr/bin/env node
// Copies docs/capabilities-schema.json -> packaging/npm/src/schema.json so
// the npm package always ships the same schema document the Rust binary
// emits. Run before `tsc` (the package.json `build` script does this).
//
// We commit the synced copy too: that way `npm publish` from a clean
// checkout works without needing the docs/ tree on disk.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const src = resolve(repoRoot, "docs", "capabilities-schema.json");
const dst = resolve(here, "..", "src", "schema.json");

const json = readFileSync(src, "utf8");
// Round-trip through JSON.parse to catch malformed source early.
JSON.parse(json);
writeFileSync(dst, json);
console.log(`synced ${src} -> ${dst}`);
