// Tests run against the compiled output in dist/, so this stays plain JS
// and works with `node --test` without a TS test runner.

import { test } from "node:test";
import { strict as assert } from "node:assert";

import { isCapabilitiesReport } from "../dist/index.js";
import { schema } from "../dist/schema.js";

// A minimal valid v1 report. Mirrors the shape wdotool-core emits.
const validReport = {
  schema_version: 1,
  wdotool_version: "0.2.0",
  backend: {
    selected: "wlroots",
    kind: "direct",
    delegated_to: null,
    fallback_chain: ["libei", "wlroots", "uinput"],
  },
  input: {
    key: true,
    type_text: true,
    type_unicode: "full",
    mouse_move_absolute: true,
    mouse_move_relative: true,
    mouse_button: true,
    scroll: true,
    modifiers: "send-only",
  },
  window: {
    list: true,
    active: true,
    activate: true,
    close: true,
    match_by: ["title", "app_id", "pid"],
  },
  extras: {
    diag: true,
    outputs: false,
    record: { supported: true, source: null },
    json_output: true,
    pointer_position: false,
    window_geometry: false,
  },
  platform: {
    desktop: "Hyprland",
    session_type: "wayland",
    compositor_hints: ["hyprland"],
  },
};

test("isCapabilitiesReport accepts a minimal valid v1 report", () => {
  assert.equal(isCapabilitiesReport(validReport), true);
});

test("isCapabilitiesReport rejects null and primitives", () => {
  assert.equal(isCapabilitiesReport(null), false);
  assert.equal(isCapabilitiesReport(undefined), false);
  assert.equal(isCapabilitiesReport(42), false);
  assert.equal(isCapabilitiesReport("hi"), false);
  assert.equal(isCapabilitiesReport([]), false);
});

test("isCapabilitiesReport rejects wrong schema_version", () => {
  assert.equal(
    isCapabilitiesReport({ ...validReport, schema_version: 2 }),
    false,
  );
  assert.equal(
    isCapabilitiesReport({ ...validReport, schema_version: "1" }),
    false,
  );
});

test("isCapabilitiesReport rejects missing required top-level objects", () => {
  const { backend, ...without } = validReport;
  assert.equal(isCapabilitiesReport(without), false);
});

test("schema export is a JSON Schema 2020-12 document", () => {
  assert.equal(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
  assert.equal(schema["$id"], "https://wdotool.dev/schemas/capabilities/v1.json");
  assert.equal(schema.type, "object");
});

test("schema locks schema_version at the const value 1", () => {
  assert.equal(schema.properties.schema_version.const, 1);
});

test("schema's WindowMatchBy enum matches the TS type", () => {
  // If this fails, src/types.ts WindowMatchBy is out of sync with the
  // schema's enum. Update one or the other.
  const enumFromSchema = schema.properties.window.properties.match_by.items.enum;
  assert.deepEqual([...enumFromSchema].sort(), ["app_id", "class", "pid", "title"]);
});

test("schema's TypeUnicode enum matches the TS type", () => {
  const enumFromSchema = schema.properties.input.properties.type_unicode.enum;
  assert.deepEqual(
    [...enumFromSchema].sort(),
    ["ascii_only", "bmp_only", "full", "none"],
  );
});
