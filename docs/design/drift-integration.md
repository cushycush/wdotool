# wdotool inside Drift

A brainstorm of what "wdotool as a first-class citizen of Drift" could mean concretely. Read this once, pick the ones worth doing, and I'll seed those as Subtasks under WDOTOOL-7 (Drift integration) and WDOTOOL-8 (Wflow integration).

The framing: wdotool is the input substrate. wflow is one consumer of it. Drift is the OS that wants to ship both pre-configured. So wdotool's job inside Drift is two-sided: be the input plane every other tool writes through, and disappear so the user never has to think about it.

## The big picture

Drift today sits on Hyprland with our overlay, which means wdotool's `wlr-protocols` backend already handles input and window ops perfectly. There is nothing structurally broken to fix. What we can do is sand off the seams: bundle, pre-configure, theme, integrate, and (longer term) give Drift its own backend slot so it can extend the surface beyond what wlr-protocols offers.

The work falls in three buckets.

**Bucket 1: wdotool just shipping with Drift.** Nothing about wdotool itself changes; we just put it on the ISO so the user never installs it. This is the lowest-effort, highest-value bucket. Most of it is one-line ISO additions and a couple of systemd units.

**Bucket 2: Drift exposes wdotool's state to the user.** A Drift Settings panel, a Drift Launcher quick-action, themed consent dialogs. Drift owns the GUI; wdotool exposes data over `diag --json` and the capabilities schema.

**Bucket 3: wdotool grows a `drift` backend.** Same shape `kde` and `gnome` already have: composes libei (for input) with a Drift-specific IPC path (for window ops, IPC events, anything else Drift's compositor surfaces that wlr-foreign-toplevel doesn't). This is the most invasive piece and the most strategic, because it lets Drift evolve its protocols without forcing the wlr-* compatibility scope on every distro.

## Concrete pieces

### Bundling and first-boot

**1. Ship wdotool in the Drift ISO.** Bundle the prebuilt binary, the Cargo features that make sense for the distro (`libei`, `wlroots`, `uinput`; skip `kde` / `gnome` since Drift uses its own compositor). Adds `wdotool` to the path on first boot. Mirrors DRIFT-56 (Bundling user-owned tools) and DRIFT-46 (Bundling wflow + wdotool in the ISO).

**2. First-boot wdotool diag.** During Drift's first-boot wizard, run `wdotool diag --json`, surface the result to the user as "input automation: ready." Anything red gets a one-click "fix this" handler (add user to uinput group, install missing udev rule, set up portal token).

**3. Prime as a Drift systemd user service.** `wdotool prime` is the long-running mode that holds wlroots virtual devices alive for the session. Ship `wdotool-prime.service` user unit in Drift dotfiles, default-on. Result: every wdotool invocation in the session skips the 30-100ms virtual-device creation step. The cost is one persistent process. Toggle from Drift Settings if the user doesn't want it.

**4. Portal consent priming.** First time the user runs anything that needs libei (a wflow workflow, an ad-hoc script), the portal pops the consent dialog. Drift could prime this once at first boot under a "set up automation" wizard step, so the dialog appears in a context the user understands rather than mid-workflow.

**5. uinput group + udev rule in the base install.** No `usermod -aG uinput` after first install; the user is already in the group, the udev rule already exists. One less footgun.

### Drift exposes wdotool to the user

**6. Drift Settings "Automation" panel.** Drift Settings GUI gets an Automation tab that parses `wdotool diag --json` (using `@wdotool/capabilities` for types) and shows the user what works on their setup. Per-row UI: backend, available, source-of-truth (auto-picked or manually overridden), with buttons to reset consent, run a smoke test, copy a diag report for a bug filing. Same panel surfaces wflow's library too, so "Automation" is one tab covering both tools.

**7. Drift Launcher quick-action.** Type "diag" or "test input" in the Drift Launcher, hit enter, get a one-shot result card. Same `wdotool diag --copy` machinery, just rendered in Drift's shell instead of dumped to a terminal.

**8. Themed consent dialog.** When the portal asks for consent the first time, the dialog is whatever the desktop's portal frontend draws. If Drift owns xdg-desktop-portal-* (or a thin wrapper over one), the consent dialog can wear Drift's brand. Lower-priority polish but a visible touchpoint.

### Drift compositor as a wdotool peer

**9. Synthetic input provenance.** Drift compositor wants to tag injected events with the injecting PID so downstream tools can attribute them (closes DRIFT-27, "Synthetic input provenance"). wdotool's side has two paths, and we'd likely do both:

- Set a stable, parseable client name on the libei session so any Drift IPC event carries `source={kind: synthetic, tool: "wdotool", pid: N}`. Independent of any protocol extension; works today.
- Define a small Drift-specific protocol extension (or use an existing libei pid hint if one exists) that pins down PID + tool name on the wire, not just in a free-form name field.

This is one of the strongest "feels first-class" wins: tools downstream of the compositor's event stream can debounce, debug, or filter their own injections out without parsing names heuristically.

**10. A `drift` backend in wdotool.** Same shape as `kde.rs` and `gnome.rs`: a backend that composes libei (input) with Drift's own IPC for window ops. Detector picks it when `XDG_CURRENT_DESKTOP=drift` or when the `drift-session` compositor-identity hint is present (DRIFT-9, "compositor-identity"). What this buys us:

- Drift can evolve its window/IPC protocols without being constrained by the wlr-foreign-toplevel surface.
- "Run this command only when window X on workspace Y is focused" is a Drift IPC event that wdotool exposes back to wflow as a trigger type (DRIFT-42, "IPC triggers"). wdotool grows the consumer side; wflow grows the trigger card.
- `wdotool getwindowgeometry` and `getmouselocation`, currently unsupported on wlroots because the protocols don't expose them, work on Drift via the IPC path. Same `kde` and `gnome` already do.

The cost is real (a new backend is a multi-week piece, plus protocol coordination with `drift-compositor/`), and the wlr-protocols backend already does most of the work today, so this is a strategic investment for v0.7+ rather than near-term. Splitting the work into "probe Drift IPC and decide" (WDOTOOL-73) and "implement" (WDOTOOL-74) lets the first half land cheaply.

**11. Drift IPC events into wflow trigger types.** Indirect through wdotool: Drift compositor emits `window-opened`, `workspace-switched`, `app-focused`, `idle-threshold` (DRIFT-42). wdotool-core could expose these as a `Stream<Event>` other Rust code subscribes to, the same way the recorder module already does for input. wflow consumes the stream and turns each event into a trigger card. Drift's compositor stays the source of truth; wdotool is the consumer-side Rust facade so other tools don't each re-implement the IPC client.

**12. Shared portal session.** Today wdotool, wflow, and any other libei consumer each open their own `org.freedesktop.portal.RemoteDesktop` session and each pay one consent prompt. A shared session (mediated by Drift's portal frontend or a tiny in-session daemon) cuts the prompt count from N to 1. Faster startup too: the consent dialog is the slowest part of a cold libei session. Trade-off: more shared state to debug when something goes wrong.

### Surface the capabilities schema everywhere

**13. `@wdotool/capabilities` as the schema for "what can this session do."** The npm package already exists and is type-safe. Drift Settings reads it; wflow could read it (today wflow probes wdotool via subprocess); Drift's docs site could render a per-backend feature matrix from it. One source of truth, three consumers.

**14. Capability descriptors per CLI action.** wflow's first-run trust dialog wants to show "this workflow uses: type, click, windowclose" before the user accepts it. wdotool's side: a small per-action capability metadata table (action name, side effects, what permission it needs). wflow's KDL decoder maps workflow actions onto these descriptors and renders them in the trust prompt. Same data could power a `wdotool capabilities --action type` for the trust-prompt UX even outside wflow.

### Wflow side, but wdotool-shaped

These are arguably wflow tickets, but they need wdotool changes to land:

**15. Recorder module migration (shipped).** Already done in v0.4.0, called out here for completeness. wflow's bundled recorder is now `wdotool-core::recorder`.

**16. Stable consumer API.** Each wdotool-core release should run the wflow build before pushing the tag. Trivially scriptable in CI; saves a coordinated revert after the fact. (WDOTOOL-47.)

**17. Trust-prompt cooperation.** See #14. Capability descriptors plus a per-workflow audit hook on the wdotool side make wflow's trust prompt land as something the user can reason about rather than a yes/no on raw shell + wdotool calls.

## What I'd do first

If I had to pick the top three:

1. **WDOTOOL-38 (Bundle in the ISO) + WDOTOOL-40 (First-boot setup) + WDOTOOL-43 (prime as a systemd user service).** Together these are the bucket-1 wins. Maybe one afternoon of work between us, and the result is "automation just works on a fresh Drift install." This is the foundation.
2. **WDOTOOL-42 (Synthetic input provenance), at least the libei client-name half.** Closes a tracked Drift ticket (DRIFT-27), is cheap on the wdotool side, and unlocks downstream tools that want to filter their own injections out of the compositor event stream.
3. **WDOTOOL-41 (Drift Settings Automation panel).** This is what makes the integration visible to users. They see wdotool's state without thinking about it. Powered by data that already exists (`wdotool diag --json` and `@wdotool/capabilities`); the work is on the Drift Settings side, with wdotool just making sure the schema stays stable.

The `drift` backend (WDOTOOL-39 / 74) is the big strategic piece but doesn't need to land soon. Hyprland's wlr-protocols backend does the job for now. When Drift compositor's IPC starts having features wlr-foreign-toplevel doesn't, that's the moment.

## Open questions for you

A few decisions I'd like input on before turning the strongest ideas into tickets:

- **Should the `drift` backend exist at all, or do we lean on wlr-protocols indefinitely?** The answer probably depends on how far Drift compositor's IPC diverges from wlroots over the next year. If Drift sticks close, wlr-protocols stays sufficient and we save the multi-week backend work. If Drift's IPC grows real divergence (per-app input policies, richer window queries, named event streams), a `drift` backend pays for itself.
- **Where does the IPC-event-to-trigger-stream code live?** wdotool-core gets a new `events` module, or wflow grows it directly, or it's a new tiny crate. I lean toward wdotool-core because the consumer surface is identical to the recorder module shape we already have.
- **Shared portal session: real win, or premature complexity?** One consent prompt is annoying. N consent prompts in a row is worse. But a multi-process portal broker is a piece of infra that has to not break. Defer until N gets uncomfortable?
