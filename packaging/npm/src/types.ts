// TypeScript types for the wdotool capabilities report (schema_version=1).
//
// Source of truth: docs/capabilities-schema.json in the wdotool repo.
// These types are hand-written so the descriptions stay close to where
// consumers read them. If the JSON Schema changes, update these to match.
// `npm run test` will fail in CI if the shipped schema and these types
// fall out of sync on the locked enums.

export type SchemaVersion = 1;

export type BackendKind = "direct" | "daemon";

export interface BackendInfo {
  selected: string;
  kind: BackendKind;
  delegated_to: string | null;
  fallback_chain: string[];
}

// How well `type` handles non-ASCII text. `full` = arbitrary Unicode
// including astral plane (wlroots transient keymap). `bmp_only` =
// Basic Multilingual Plane only. `ascii_only` = ASCII only with a
// warning on non-ASCII (libei/kde/gnome/uinput, since the EIS server
// or kernel owns the keymap). `none` = no text input.
export type TypeUnicode = "full" | "bmp_only" | "ascii_only" | "none";

// Whether the backend can read the compositor's current modifier state.
// `send-only` is the only value v0.2.x emits. Wayland's security model
// hides modifier reads from clients, so `--clearmodifiers` does an
// unconditional release rather than xdotool's save-and-restore.
export type Modifiers = "send-only";

export interface InputCapabilities {
  key: boolean;
  type_text: boolean;
  type_unicode: TypeUnicode;
  mouse_move_absolute: boolean;
  mouse_move_relative: boolean;
  mouse_button: boolean;
  scroll: boolean;
  modifiers: Modifiers;
}

// Which window attributes the backend can match on for `wdotool search`.
// Closed enum: removing or renaming a value bumps schema_version.
export type WindowMatchBy = "title" | "app_id" | "pid" | "class";

export interface WindowCapabilities {
  list: boolean;
  active: boolean;
  activate: boolean;
  close: boolean;
  match_by: WindowMatchBy[];
}

// Where the recorder sources events. Locked enum; v0.2.x always null.
// v0.4.0+ may emit `libei-receiver`.
export type RecordSource = "libei-receiver" | null;

export interface RecordCapability {
  supported: boolean;
  source: RecordSource;
}

export interface ExtrasCapabilities {
  diag: boolean;
  outputs: boolean;
  record: RecordCapability;
  json_output: boolean;
  pointer_position: boolean;
  window_geometry: boolean;
}

export interface PlatformInfo {
  desktop: string | null;
  session_type: string | null;
  compositor_hints: string[];
}

// Coarse grouping for an action. `input` synthesizes keyboard or
// pointer events. `window` reads or operates on toplevels. `extras` is
// recorder + replay + prime + outputs. `introspection` is read-only
// diagnostic surfaces (info, capabilities, diag). Closed enum; adding
// a value bumps schema_version.
export type ActionCategory =
  | "input"
  | "window"
  | "extras"
  | "introspection";

// What the action does at runtime. Trust prompts can render these
// directly ("synthesize_key" -> "send keystrokes") or roll them up
// into a coarser permission grant. Adding values is forward-compat;
// removing or renaming bumps schema_version.
export type ActionSideEffect =
  | "synthesize_key"
  | "synthesize_pointer_motion"
  | "synthesize_pointer_button"
  | "synthesize_scroll"
  | "activate_window"
  | "close_window"
  | "list_windows"
  | "read_window"
  | "read_pointer_position"
  | "read_outputs"
  | "capture_input"
  | "replay_input"
  | "hold_devices"
  | "diagnostic"
  | "introspection";

export interface ActionDescriptor {
  summary: string;
  category: ActionCategory;
  side_effects: ActionSideEffect[];
  // Dotted paths into the capabilities report that must be truthy for
  // this action to succeed on the selected backend (e.g. `input.key`,
  // `window.close`, `extras.record.supported`). Open string list; the
  // convention is `<top-level>.<field>` matching the report shape.
  requires: string[];
}

// Action descriptors keyed by CLI subcommand name (`type`, `mousemove`,
// `windowclose`, ...). Powers `wdotool capabilities --action <name>`
// and wflow's first-run trust prompt.
export type ActionsTable = Record<string, ActionDescriptor>;

export interface CapabilitiesReport {
  schema_version: SchemaVersion;
  wdotool_version: string;
  backend: BackendInfo;
  input: InputCapabilities;
  window: WindowCapabilities;
  extras: ExtrasCapabilities;
  platform: PlatformInfo;
  actions: ActionsTable;
}
