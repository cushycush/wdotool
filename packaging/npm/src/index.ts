// Public exports for `@wdotool/capabilities`.
//
// Two surfaces:
//   - Types for the capabilities report (import from `@wdotool/capabilities`)
//   - The JSON Schema document itself (import from `@wdotool/capabilities/schema`)
//
// Typical use:
//
//   import { isCapabilitiesReport, type CapabilitiesReport } from "@wdotool/capabilities";
//   const raw = JSON.parse(stdout);
//   if (!isCapabilitiesReport(raw)) throw new Error("not a v1 report");
//   // raw is now typed as CapabilitiesReport.
//
// For full schema validation, pair with ajv:
//
//   import { schema } from "@wdotool/capabilities/schema";
//   import Ajv from "ajv/dist/2020.js";
//   const ajv = new Ajv();
//   const validate = ajv.compile(schema);
//   if (!validate(raw)) console.error(validate.errors);

export type {
  BackendInfo,
  BackendKind,
  CapabilitiesReport,
  ExtrasCapabilities,
  InputCapabilities,
  Modifiers,
  PlatformInfo,
  RecordCapability,
  RecordSource,
  SchemaVersion,
  TypeUnicode,
  WindowCapabilities,
  WindowMatchBy,
} from "./types.js";

import type { CapabilitiesReport } from "./types.js";

// Lightweight runtime check that an unknown value matches the
// schema_version=1 contract structurally. Use this before treating
// untyped JSON (e.g. parsed `wdotool capabilities` stdout) as a typed
// report. This is NOT a full JSON Schema validation; for that, import
// the schema from `@wdotool/capabilities/schema` and feed it to ajv.
export function isCapabilitiesReport(
  value: unknown,
): value is CapabilitiesReport {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    v["schema_version"] === 1 &&
    typeof v["wdotool_version"] === "string" &&
    typeof v["backend"] === "object" &&
    v["backend"] !== null &&
    typeof v["input"] === "object" &&
    v["input"] !== null &&
    typeof v["window"] === "object" &&
    v["window"] !== null &&
    typeof v["extras"] === "object" &&
    v["extras"] !== null &&
    typeof v["platform"] === "object" &&
    v["platform"] !== null
  );
}
