// Re-exports the schema JSON as a typed module. The actual JSON file
// lives next to this in `schema.json` and is synced from
// `docs/capabilities-schema.json` by `scripts/sync-schema.mjs`.

import schemaJson from "./schema.json" with { type: "json" };

export const schema = schemaJson;
export default schema;
