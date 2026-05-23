# @wdotool/capabilities

TypeScript types and the JSON Schema for [wdotool](https://github.com/cushycush/wdotool)'s capabilities report.

If you're writing a JS or TS tool that runs `wdotool capabilities` and parses the output (a dashboard, a workflow runner like [wflows.com](https://wflows.com), a config UI, etc.), this package gives you full type safety on the result without you having to hand-write the types or maintain a JSON Schema fork.

## Install

```bash
npm install @wdotool/capabilities
```

## Use

### Type-check parsed output

```ts
import { isCapabilitiesReport, type CapabilitiesReport } from "@wdotool/capabilities";

const proc = await execFile("wdotool", ["capabilities"]);
const raw = JSON.parse(proc.stdout);

if (!isCapabilitiesReport(raw)) {
  throw new Error("wdotool returned a report this version of @wdotool/capabilities can't parse");
}

// raw is now typed as CapabilitiesReport. Autocomplete works:
console.log(raw.backend.selected); // "libei" | "wlr-protocols" | ...
console.log(raw.input.type_unicode); // "full" | "bmp_only" | "ascii_only" | "none"
```

`isCapabilitiesReport` is a structural check, not a full schema validation. It looks for `schema_version === 1` and the required top-level objects, which is enough for most consumers.

### Full schema validation

If you want strict validation (every enum, every required field), pair this package with `ajv`:

```ts
import { schema } from "@wdotool/capabilities/schema";
import Ajv from "ajv/dist/2020.js";

const ajv = new Ajv();
const validate = ajv.compile(schema);

if (!validate(raw)) {
  console.error("validation errors:", validate.errors);
}
```

The schema is JSON Schema draft 2020-12. It's also available as a raw JSON file at `@wdotool/capabilities/schema.json`.

### Non-JS consumers (QML, Python, Go, etc.)

The schema file ships standalone, so tools that can't (or don't want to) link the TypeScript types can grab the JSON document and feed it to whatever validator they prefer. Two practical paths:

- `node_modules/@wdotool/capabilities/dist/schema.json` after `npm install` (e.g. Drift Settings reads it directly from a QML `XMLHttpRequest` against the file path).
- A vendored copy of `docs/capabilities-schema.json` from the wdotool repo (the file in this package is synced from there byte-for-byte at build time, so the two are interchangeable).

Either way, the contract is the JSON Schema document; the TypeScript types are a convenience layer for JS/TS consumers.

## What's in the report

A `wdotool capabilities` call returns a single JSON object describing what the running backend can do. Top-level fields:

- `schema_version` — locked at `1` for this package version. Future incompatible shapes will publish as a new major.
- `wdotool_version` — the cargo package version of the running binary.
- `backend`. Which backend was selected (libei, wlr-protocols, kde, gnome, uinput) and the fallback chain the detector considered.
- `input` — what input ops the backend can do (key, type, mouse, scroll) and how well `type` handles non-ASCII text.
- `window` — which window ops are supported (list, active, activate, close) and which attributes `wdotool search` can match on.
- `extras` — opt-in features (diag, json output, future record).
- `platform` — environment markers (`XDG_CURRENT_DESKTOP`, session type, compositor hints).

For the field-by-field reference, read the JSON Schema document directly. Every property has a description.

## Versioning

This package's version tracks wdotool releases for the schema_version=1 contract. The npm package's major version mirrors the schema major, so v1.x of `@wdotool/capabilities` ships the v1 schema. When wdotool ships a v2 schema, this package will release as v2 too, with a different module path so callers can import both side by side during a migration.

What does and doesn't bump `schema_version`:

- Adding a new optional field to an existing object: backward-compatible, no bump. Consumers must ignore unknown fields, which is the explicit rule baked into the schema's top-level description.
- Adding a new value to an open enum (e.g. a new backend name in `backend.selected`, a new compositor hint in `platform.compositor_hints`): backward-compatible, no bump.
- Adding a new entry to a backend's `window.match_by` array: backward-compatible, no bump.
- Removing a field, renaming a field, narrowing a type, or removing a value from a closed enum (e.g. `input.type_unicode`, `backend.kind`, `input.modifiers`, `window.match_by` element values, `extras.record.source`): not backward-compatible. Bumps `schema_version` and ships `capabilities/v2.json` alongside `v1.json`, plus a new major release of this package.

CI gates the contract three ways: the Rust source in `wdotool-core/src/capabilities.rs` serializes the report, an integration test asserts the mock-backend output matches a committed fixture, and the npm test suite validates that same fixture against the schema document with ajv. The schema, the Rust types, and the TypeScript types all have to agree or one of those tests fails.

## License

MIT OR Apache-2.0, matching wdotool itself.
