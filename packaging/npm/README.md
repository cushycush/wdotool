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
console.log(raw.backend.selected); // "libei" | "wlroots" | ...
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

## What's in the report

A `wdotool capabilities` call returns a single JSON object describing what the running backend can do. Top-level fields:

- `schema_version` — locked at `1` for this package version. Future incompatible shapes will publish as a new major.
- `wdotool_version` — the cargo package version of the running binary.
- `backend` — which backend was selected (libei, wlroots, kde, gnome, uinput) and the fallback chain the detector considered.
- `input` — what input ops the backend can do (key, type, mouse, scroll) and how well `type` handles non-ASCII text.
- `window` — which window ops are supported (list, active, activate, close) and which attributes `wdotool search` can match on.
- `extras` — opt-in features (diag, json output, future record).
- `platform` — environment markers (`XDG_CURRENT_DESKTOP`, session type, compositor hints).

For the field-by-field reference, read the JSON Schema document directly. Every property has a description.

## Versioning

This package's version tracks wdotool releases for the schema_version=1 contract. When wdotool ships a v2 schema, this package will release as v2 too, with a different module path so callers can import both side by side during a migration.

## License

MIT OR Apache-2.0, matching wdotool itself.
