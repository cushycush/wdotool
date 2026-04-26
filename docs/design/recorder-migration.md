# Recorder migration: lift wflow's capture layer into wdotool-core

## Goal

Move the input-capture half of [wflow's recorder](https://github.com/cushycush/wflow/blob/main/src/recorder.rs) into a new module in `wdotool-core` so anyone who wants to record Wayland input on Linux can `cargo add wdotool-core` and start streaming events instead of vendoring wflow.

The wflow side keeps the parts that are wflow's job: turning raw event streams into wflow `Action` steps, the QML bridge, the RecordPage UI, the workflow generator. Those stay where they are.

After the migration, wflow's `recorder.rs` shrinks from ~1,177 lines to a thin "subscribe to wdotool-core's recorder, coalesce into Actions, push frames at Qt" layer (probably 200-300 lines). And wdotool gets a `wdotool record` CLI built on the same library.

## What's the actual substrate work

Reading wflow/src/recorder.rs, the substrate-shaped pieces are:

- **Three capture backends** with a tiered fallback: portal (`xdg-desktop-portal` RemoteDesktop session + libei in receiver mode), evdev (reads `/dev/input/event*`), simulated (deterministic test script). The selection logic, the per-backend init, the per-backend event pump.
- **EIS event mapping** — turning `reis::event::EiEvent` into a normalized `RecEvent`. Lives in `event_to_rec` (~80 lines) and `keycode_to_chord` (~80 lines).
- **Evdev event mapping** — turning `evdev::InputEvent` into the same `RecEvent`. Lives in `evdev_to_rec` (~140 lines).
- **Throttling** for pointer motion. Both portal and evdev paths accumulate sub-threshold movement and emit at intervals.
- **Tail-trim** — drop the user's stop-recording click from the captured stream so the workflow doesn't replay clicking on the recorder UI.
- The `RecEvent` enum itself (Key, Text, Click, Move, Scroll, WindowFocus, Gap).

The wflow-shaped pieces that should stay there:

- `RecFrame` (Armed / Started / Event / Stopped) — UI status frames. Could be useful in wdotool too, but it's a thin wrapper; defer until a second consumer asks for it.
- `events_to_workflow()` — coalesces raw events into wflow `Action` steps. Specific to wflow's `Action` type, can't move.
- The QML bridge (`src/bridge/recorder.rs`) and RecordPage QML.
- The Hyprland focus-tracking glue (subscribes to `.socket2.sock`). This one's borderline — it's compositor-specific input enrichment, but it's tightly coupled to wflow's `WindowFocus` event variant. I'd leave it in wflow for v1 and revisit if a second consumer needs window-focus events.

## Proposed API in `wdotool-core::recorder`

Behind a new `recorder` Cargo feature so library consumers who only want send-side input don't pull in `evdev`, `ashpd`, the receiver bits of `reis`, etc.

```rust
// wdotool_core::recorder

/// A single captured input event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecEvent {
    /// A chord was pressed (coalesced from modifier + key).
    Key { t_ms: u64, chord: String },
    /// A mouse button was pressed.
    Click { t_ms: u64, button: u8 },
    /// Pointer motion. Absolute when the source provides it (libei
    /// portal); a delta otherwise (evdev).
    Move { t_ms: u64, x: i32, y: i32, kind: MoveKind },
    /// Scroll.
    Scroll { t_ms: u64, dx: i32, dy: i32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    Absolute,
    Delta,
}

/// Configuration for a recording session.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Minimum interval between Move emissions. Sub-threshold motion
    /// accumulates and flushes when the interval elapses. Default 1000.
    pub min_move_interval_ms: u64,
    /// Pointer-motion threshold in pixels. Below this, accumulators
    /// build up and don't emit a Move. Default 4.
    pub move_threshold_px: i32,
    /// Backend choice. Auto = portal → evdev fallback, never simulated.
    pub backend: BackendChoice,
}

#[derive(Debug, Clone, Copy)]
pub enum BackendChoice {
    Auto,
    Portal,
    Evdev,
    /// Deterministic script; for tests and CI.
    Simulated,
}

/// Start a recording session.
pub async fn start(config: RecorderConfig) -> Result<RecorderSession>;

/// A live recording session. Drop or call `stop` to end.
pub struct RecorderSession { /* private */ }

impl RecorderSession {
    /// Returns the source actually selected (Auto resolves to one of
    /// Portal / Evdev / Simulated). Useful for debug and for showing
    /// the user what consent path they took.
    pub fn source(&self) -> BackendChoice;

    /// Stream of captured events. The stream ends when `stop()` is
    /// called or the session is dropped.
    pub fn events(&mut self) -> impl Stream<Item = RecEvent> + Send + '_;

    /// Stop the session and return all events captured so far.
    /// Equivalent to consuming `events()` to completion plus calling
    /// stop, but without the consumer dance.
    pub async fn stop(self) -> Result<Vec<RecEvent>>;
}
```

The two access patterns this supports:

```rust
// Pattern A: live UI streaming (wflow)
let mut session = wdotool_core::recorder::start(config).await?;
let mut events = session.events();
while let Some(ev) = events.next().await {
    bridge.push_frame(RecFrame::Event { event: ev });
}

// Pattern B: capture, write JSON, stop (wdotool record CLI)
let session = wdotool_core::recorder::start(config).await?;
tokio::signal::ctrl_c().await?;
let events = session.stop().await?;
serde_json::to_writer_pretty(out, &events)?;
```

## Open questions

These are the real design decisions I want feedback on before code lands.

**1. Stream vs callback?**

wflow's current shape is a callback (`FrameSink = Arc<dyn Fn(RecFrame) + Send + Sync>`). My proposal is a Stream. Streams compose better (`.filter`, `.take_while`, `.timeout`), but Qt's signal flow through the bridge is callback-shaped. wflow's bridge would either adapt the stream into a callback, or keep using a callback adapter.

Either works. A stream is more general; a callback is what wflow already has. The cleanest answer is probably: ship the Stream API, let the wflow bridge build a tiny `pin_mut!(stream); while let Some(ev) = stream.next().await` adapter. Open to "no, just keep the callback" if Qt integration is hairier than I think.

**2. Where does `RecFrame` live?**

wflow has `RecFrame` (Armed / Started / Event / Stopped) as a UI status wrapper. Two options:

- (a) Keep it in wflow. wdotool-core only ships RecEvent; wflow wraps with its own RecFrame for the bridge.
- (b) Move RecFrame to wdotool-core too. Any consumer streaming to a UI gets the lifecycle frames for free.

I lean (a) for v1. RecFrame is UI-flavored; wdotool-core stays event-shaped. If a second consumer asks for lifecycle frames later, we promote.

**3. Hyprland focus tracking?**

wflow's recorder also subscribes to Hyprland's `.socket2.sock` to add `WindowFocus` events to the stream. This is genuinely useful in workflow recordings (focus changes are the boundaries between "open this app, then do that"). But it's compositor-specific (Hyprland only) and the `RecEvent::WindowFocus` variant is closer to a wflow concept than a wdotool input concept.

Two options:

- (a) Leave focus tracking in wflow. wdotool-core ships pure input events. wflow merges its own focus stream with wdotool-core's event stream before pushing to UI.
- (b) Lift focus tracking too. Add a `RecEvent::WindowFocus` variant. Document as best-effort, Hyprland-only today, KDE/GNOME later.

I lean (a). Focus is a different kind of event than "the user moved the mouse," and lifting it bakes a Hyprland dependency into the substrate that other compositors will need parallel implementations of. wflow is the right place to compose them today.

**4. Async runtime tax?**

wdotool-core today uses tokio for the existing backend trait methods. The recorder will need it too (ashpd is async, libei stream pumping is async, the Hyprland socket reader is async). No new dependency, but the `recorder` Cargo feature pulls a heavier subset of tokio (signal handling for Ctrl-C in the CLI, mpsc for the stream channel, etc.) than the baseline.

Acceptable. The feature gate means library consumers who don't want recording don't pay this cost.

**5. Error model on the migrating events?**

wflow's recorder uses `anyhow::Result` everywhere. wdotool-core uses its own `WdoError` / `Result`. The migration ports the error paths; some `anyhow::anyhow!("...")` strings become `WdoError::Backend { backend, source }` or `WdoError::NotSupported { backend, what }`. Mechanical translation, but it's a real chunk of the diff.

## Migration sequence

Coordinated PR pair, plus follow-ups:

**PR-1 (wdotool):** New `recorder` Cargo feature. Adds `wdotool_core::recorder` module. Ports `RecEvent`, the three capture backends, the EIS and evdev event mapping, the throttling, the tail-trim. Ships with unit tests for the pure functions (event mapping, throttling, tail-trim — wflow already has these). Bumps `wdotool-core` to 0.4.0 (minor bump; existing API unchanged).

**PR-2 (wdotool):** New `wdotool record [--output FILE] [--max-duration SEC] [--backend portal|evdev|simulated]` CLI command built on `wdotool_core::recorder`. Optional `wdotool replay FILE` follow-up; replay is just iterating `RecEvent`s and calling the existing `Backend` trait methods, so it's small.

**PR-3 (wflow):** Bumps wdotool-core dep to 0.4.0. Deletes the now-duplicated capture code from `recorder.rs`. Swaps `events_to_workflow` to consume `wdotool_core::recorder::RecEvent` instead of in-process events. UI behavior unchanged. Net delta: ~−800 lines.

PR-1 and PR-2 land in wdotool first, in either order. PR-3 follows once the new wdotool-core is published. About a session of work each.

## What's not in scope

- **Replay enrichment.** wdotool replay would just dispatch RecEvents through the existing Backend trait. No new replay logic; that's already in wflow's `events_to_workflow` and stays there for the workflow-shaped use case.
- **Permissions setup automation.** "Add yourself to the input group" stays in the user's hands. wflow currently surfaces a clear error message naming the problem; wdotool-core does the same.
- **Cross-platform.** Linux Wayland only. macOS, Windows, X11 are not in scope.
- **Keymap-aware key event mapping.** wflow's `keycode_to_chord` is hard-coded US-en QWERTY with a fallback to `keyNN`. Proper xkb-aware decoding is a separate, layered improvement that affects both wdotool replay (already shipped) and the new recorder; tracked separately if needed.

## Risks and mitigations

- **Tokio runtime conflicts.** wflow links wdotool-core in-process and runs its own tokio runtime via Qt threading. If `wdotool_core::recorder::start` spawns its own runtime where wflow already has one, badness ensues. **Mitigation:** the recorder module uses `tokio::task::spawn` from the caller's runtime, not `Runtime::new()`. No internal runtime construction except where forced (libei's dispatcher already does this; we just inherit that pattern).
- **Symbol clashes.** `wdotool_core` and wflow both currently define a `RecEvent`. After the migration, wflow's needs to either alias or import. **Mitigation:** wflow re-exports `pub use wdotool_core::recorder::RecEvent` and stops defining its own, so the wflow-side type stays the same name to existing wflow code.
- **Test coverage during the lift.** The pure-function tests (trim, coalesce, evdev mapping) come along to wdotool-core. The portal-path test in wflow is integration-flavored and stays in wflow, since it requires a portal session. No coverage loss.

## Sign-off bar

Before PR-1 lands, this doc should answer:

- Stream vs callback (decision in writing)
- Where RecFrame lives (decision in writing)
- Where WindowFocus tracking lives (decision in writing)

After PR-1 lands but before PR-3:

- wdotool-core 0.4.0 publishes to crates.io
- The recorder module gets at least one library consumer beyond wflow (could be the `wdotool record` CLI in PR-2 itself)

After PR-3 lands:

- wflow's CI still passes against the new wdotool-core
- wflow's recorder.rs LoC count drops as expected
