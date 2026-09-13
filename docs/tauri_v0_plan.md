# Tauri v0 Layer — Minimal Backend↔Frontend Vertical Slice

## Context

`backend/` (pcap → UDP/TCP replay, TOML scenarios, speed control) and `frontend/`
(bare SolidJS + Vite scaffold) exist as separate repos, each buildable/testable
standalone. Per the earlier decision, Tauri's `src-tauri` shell lives in the
**parent repo** (`RePlayrer/`), not in either — it's the composition layer, not
part of either product. The parent repo's own git/submodule wiring is deferred
(agreed separately); this plan only concerns the `src-tauri` files themselves,
which can be written to disk before that housekeeping happens.

Before building any real frontend views (Scenario Editor, Replay Control
Panel, etc.), we want the smallest possible vertical slice through the Tauri
bridge — same philosophy as the backend's own v0 pcap→UDP slice. This proves
the actual unknown (process bridging, command/event shapes, cross-repo Cargo
path dependencies, packaging) before investing in UI that would otherwise be
built against imagined command/event shapes.

**Deferred, not in scope for this slice:**
- Separate `Load Scenario`/`Save Scenario`/`Import Recording` commands (the
  system doc's full Command Model) — folded into `start_replay`'s path
  argument for now.
- Pause/Resume (already deferred at the `replay-engine` level itself).
- `Output Connected`/`Output Disconnected`/`Error Events` from the system
  doc's Event Model — only `Started`/`Position`/`Stopped` for v0.
- Scenario file picker (`@tauri-apps/plugin-dialog`) — plain text path input
  instead.
- Multi-window support, concurrent/multiple simultaneous replays.
- Parent-repo git init / submodule conversion (separately deferred).

## Scope

- **One command pair**: `start_replay(scenario_path)`, `stop_replay()`.
- **One event trio**: `replay://started` (carries the scenario's `speed`),
  `replay://position` (dispatched event count only), `replay://stopped`.
  Elapsed *replay time* (for a scrubber/playhead) is **not** in this
  payload — see note below.
- **Frontend**: a bare view — text input for scenario path, Start/Stop
  buttons, a live counter/log driven by the three events. No styling
  investment; this is a wiring proof, not a real screen.
- **Single global replay slot**: calling `start_replay` while one is already
  running is a command error (`Err("replay already running")`), not a second
  concurrent engine.

## Layout

```
RePlayrer/
├── backend/           # existing, untouched — own git repo/workspace
├── frontend/           # existing, untouched at the Vite/Solid layer
└── src-tauri/          # new — this plan's deliverable
    ├── Cargo.toml       # path deps into ../backend/crates/*, plus tauri
    ├── tauri.conf.json  # devUrl -> frontend's vite dev server,
    │                     # frontendDist -> ../frontend/dist,
    │                     # beforeDevCommand/beforeBuildCommand run in ../frontend
    ├── build.rs
    └── src/
        ├── main.rs
        ├── state.rs     # AppState: Mutex<Option<ReplayEngineHandle>>
        └── commands.rs  # start_replay / stop_replay
```

Frontend gains a thin wrapper (inline in `App.tsx`, or a small `src/replay.ts`)
around `invoke`/`listen` from `@tauri-apps/api` — no other new frontend files
or dependencies.

## Why path dependencies, not a git dependency

`src-tauri/Cargo.toml` points directly at `../backend/crates/replay-core`
etc. via relative path. This works identically whether `backend/` is a plain
subdirectory (today) or a git submodule checkout (once the parent repo is set
up) — a submodule is just a checked-out directory at that path, from Cargo's
point of view. Nothing here needs to change when that conversion happens.

`src-tauri` is its own Cargo package, **not** added to backend's
`[workspace] members`, and gets its own `Cargo.lock` — backend stays
independently buildable with zero awareness a Tauri shell exists on top of it.

## Command/Event Surface (v0)

```rust
// commands.rs
#[tauri::command]
async fn start_replay(
    app: AppHandle,
    state: State<'_, AppState>,
    scenario_path: String,
) -> Result<(), String>;

#[tauri::command]
async fn stop_replay(state: State<'_, AppState>) -> Result<(), String>;
```

```rust
// state.rs
#[derive(Default)]
struct AppState(Mutex<Option<ReplayEngineHandle>>);
```

`start_replay`, mirroring `replay-cli::main`'s pipeline almost line-for-line:

1. Lock `AppState`; if it already holds a handle, return
   `Err("replay already running")`.
2. `load_scenario(&path)` — a copy of `replay-cli`'s helper (load, parse
   TOML, `validate()`). Small enough to duplicate for v0; the natural first
   thing to extract into a shared crate if a third consumer ever needs the
   same wiring (a test harness, a future REST binding).
3. Build `TimelineManager` (one cursor per recording path), construct
   `outputs: HashMap<String, Arc<dyn Output>>`, wire the `Dispatcher` —
   exactly as `replay-cli` does today.
4. `replay_engine::spawn(Box::new(timeline_manager), speed)`, store the
   returned `ReplayEngineHandle` in `AppState`, call `.start()`.
5. `tokio::spawn` a bridging task:
   ```rust
   while let Some(event) = events.recv().await {
       dispatcher.dispatch(&event).await;
       count += 1;
       app.emit("replay://position", Position { count })?;
   }
   // channel closed: replay finished naturally, or Stop drained it
   *state.0.lock().unwrap() = None;
   app.emit("replay://stopped", ())?;
   ```
6. Emit `replay://started` (`Started { speed: scenario.speed }`) and return
   `Ok(())` once the engine is spawned and `.start()` has been called (not
   once replay finishes).

**Note on position/time, not count:** `replay_engine::spawn`'s channel only
ever forwards a raw `Event` — it discards `ScheduledEvent.replay_at` (the
normalized, offset-adjusted, scenario-relative elapsed time) after using it
internally for the sleep deadline. `Event.timestamp` is each recording's own
*absolute captured* time, not comparable across sources with different
offsets — useless as a scrubber position. Rather than widen
`replay_engine`'s public channel type for this v0 slice, the frontend
computes elapsed replay-time itself: capture `Date.now()` when
`replay://started` fires, then `elapsed_replay_ms = (Date.now() - startedAt)
* speed`. This is exact as long as v0's constraints hold (static speed, no
pause) — it stops being valid the moment Pause or live speed changes exist,
which is already flagged in `backend/docs/frontend_integration_gaps.md`
(Gaps 1–2) as needing its own position-reporting design at that point.
`replay://position`'s `count` is only for a dispatched-event
counter/log, not for driving the scrubber.

`stop_replay` just calls `handle.stop()` on the stored handle. The bridging
task's channel closing as a result is what actually fires `replay://stopped`
— a single source of truth for that event, whether the replay was stopped
manually or ran to completion on its own.

## Milestones

1. **M0 — scaffold.** `cargo tauri init` (or hand-written if the `tauri-cli`
   isn't available in this environment) in `RePlayrer/`, `tauri.conf.json`
   pointed at `../frontend`. Verify: `cargo tauri dev` opens a window showing
   the stock SolidJS/Vite starter page.
2. **M1 — path deps.** Add `replay-core`, `replay-engine`, `format-pcap`,
   `output-udp`, `output-tcp` as path dependencies. Verify: `cargo check`
   inside `src-tauri` succeeds standalone — proves the cross-repo path-dependency
   approach before any command logic exists.
3. **M2 — commands + bridging.** Implement `AppState`, `start_replay`,
   `stop_replay`, the bridging task, the three events. Verify: drive it via
   the running app's devtools console (`window.__TAURI__.core.invoke(...)`)
   against a real scenario TOML + UDP loopback output; confirm events fire
   and packets still flow exactly as via `replay-cli`.
4. **M3 — frontend wiring.** Path input, Start/Stop buttons, live position
   counter/log via `listen`. Verify manually in the app window.
5. **M4 — E2E.** Real scenario TOML + external UDP listener
   (`tcpdump`/Wireshark) alongside the running Tauri app: confirm
   payload/timing correctness is unchanged from the pure-CLI path, and that
   Start → Stop → Start-again behaves correctly through the UI (including the
   "already running" error path).

## Risks / Gotchas

- **Duplicated pipeline wiring** between `replay-cli::main` and
  `src-tauri`'s `start_replay` — deliberate for v0 (mirrors `replay-cli`
  itself not being consumed as a library). First candidate for extraction
  into a shared crate once a third consumer needs it.
- **Webview filesystem sandboxing**: the plain-text scenario-path input
  sidesteps needing `@tauri-apps/plugin-dialog`/fs allowlist entirely for
  v0; a real file picker belongs to the later Scenario Editor work.
- **Single-slot `AppState`**: a second `start_replay` while one is running
  must fail cleanly, not silently spawn a second engine against the same
  outputs — test this path explicitly.
- **Tauri v2 API specifics** (`emit` vs. `emit_all`, exact `State`/`AppHandle`
  extraction signatures) should be checked against whatever `cargo tauri
  init` actually scaffolds when M0 runs — this plan describes the intended
  shape, not verified exact method names.
- **No Output Connected/Disconnected or Error events in v0** — a failure
  during `start_replay`'s setup (e.g. UDP bind failure) surfaces as the
  command's `Err(String)`; a failure *after* start (e.g. a TCP client
  permanently down) has no frontend-visible signal beyond backend logs.
  Matches this slice's deliberately non-exhaustive event set.

## Verification

- `cargo check`/`cargo build` inside `src-tauri` after M1.
- Manual `cargo tauri dev` runs after M2–M4, per milestone above.
- Full E2E per M4, same manual-verification style as the backend's own v0
  slice (real capture + external listener, eyeballed for correctness).
- **Automated, on both sides of the boundary, but not across it**: a
  `tauri::test::MockRuntime`-based Rust test (`src-tauri/src/commands.rs`)
  replays a synthetic pcap scenario over real loopback UDP end to end,
  bypassing the frontend entirely; a Playwright suite
  (`frontend/tests/tauri-ipc.spec.ts`, against `tests/e2e-harness.tsx`)
  drives the real rendered UI with Tauri's IPC mocked
  (`@tauri-apps/api/mocks`), bypassing the Rust backend entirely. Together
  they cover both sides of the command/event contract (right names, right
  payload shapes, right UI reaction) without needing a real window — but
  neither proves the two sides actually agree at runtime through the real
  IPC bridge. That last link is still only the manual `cargo tauri dev`
  check above.
