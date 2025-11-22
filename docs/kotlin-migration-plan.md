# Kotlin Migration Plan – Core Engine via JNI

## 1. Goals and Constraints
- Keep the Rust-based UI/CLI (`code-rs/tui`, `code-rs/cli`) and existing `./build-fast.sh` pipeline untouched while the core logic begins moving toward Kotlin.
- Treat `codex-rs` as a read-only mirror; all new JNI hooks and Kotlin code must live alongside the actively developed `code-rs` tree.
- Kotlin will initially **call into the existing Rust crates over JNI**. Only once correctness and parity are proven will Kotlin replace portions of the Rust core.
- Architectural invariant: the full Rust core (`codex-rs`, `code-rs/core`, `code_auto_drive_core`, etc.) stays in-tree, at the same paths, as a first-class engine. Kotlin always acts as a sister system that talks to the Rust engine via JNI; we do not delete, move, or fork the Rust crates when mirroring behaviour.

### 1.1 Rust + Kotlin dual-engine contract

- Every upstream sync from `openai/codex` must continue to apply cleanly because the Rust layout never changes; Kotlin mirrors sit beside it rather than replacing files.
- Production builds always contain both engines. Kotlin now owns the default flow, while setting `CODE_ENGINE=rust` (or `engine_mode = "rust"` in config) forces the legacy Rust path for regression triage.
- When a Kotlin path “owns” a flow it still forwards to Rust for anything the Kotlin side does not yet implement, so parity tests can compare both outputs.
- Removal/deletion of Rust crates is out of scope for the migration; when Kotlin eventually becomes authoritative we will add explicit cutover documentation instead of erasing the Rust history.

## 2. Current Core Engine Surface
- `code-rs/core` (`code_core` crate) owns auth, model execution, conversation management, exec orchestration, and protocol/event plumbing (see `code-rs/core/src/lib.rs`). Key entry points that UI layers use today:
  - `codex::Codex` and `conversation_manager` for lifecycle management and history feeding.
  - `exec` module for shell command execution and streaming output control.
- `code-rs/code-auto-drive-core` provides the autonomous execution brain:
  - `auto_coordinator` coordinates model-mediated planning/approval cycles.
  - `controller` exposes the deterministic Auto Drive state machine (`AutoController`, `AutoControllerEffect`).

### 2.1 Kotlin-Owned Surface (November 2025)
- The `core-kotlin` Gradle project now exposes a single façade (`CoreEngineFacade`) that groups every Kotlin-side mirror behind two namespaces:
  - `AutoDrive`: mirrors `AutoDriveController::update_continue_mode`, `handle_countdown_tick`, and the multi-step `auto_drive_sequence` flow. Kotlin drives `KotlinAutoDriveControllerState` locally, while JNI calls (`CoreEngineApi.autoDriveSequence`) invoke the real Rust controller. Parity tests compare both sides on countdown ticks, continue-mode transitions, transient pauses, and full sequences.
  - `Conversation`: mirrors two history transforms that `conversation_manager` and `conversation_history` use when resuming/forking conversations. The Kotlin mirror calls `ConversationControllerKt.pruneHistory` / `filterHistory`, while the JNI bridge forwards to `prune_history_after_dropping_last_user_turns` and `retain_api_messages_only` in Rust.
- The façade ensures tests (and future clients) use one API for both local mirrors and JNI-backed calls, keeping parity checks consistent without scattering references to `AutoDriveControllerKt`/`ConversationControllerKt` across the codebase.
- Parity coverage today:
  - Auto Drive countdown tick & continue-mode ops.
  - Stateful Auto Drive sequences (including transient failure stop/reset).
  - Conversation prune (drop last N user turns), history filtering (drop non-API/system items), and snapshot coalescing (dedupe assistant messages sharing a stream id).
  - Auto Drive planner seed bootstrap (`PlannerSeedControllerKt` mirrors `build_initial_planning_seed` so Kotlin can preview coordinator prompts/goals before invoking Rust).
- The session-oriented JNI host (`CoreEngineHost`) now speaks a `chat_turn` payload emitted by the Rust CLI: it ingests the filtered conversation history, summarizes the latest user prompt, invokes `CoreEngineApi.autoDriveSequence`, and streams the resulting Kotlin-generated summaries/events back to the CLI instead of relying on a stubbed `auto_drive_sequence` request.
- Every mirrored operation has a corresponding JNI path and Kotlin test in `CoreEngineApiTest`, so regressions in either language fail CI automatically.

### 2.2 Engine Sandbox (experimental)
- A minimal CLI (`EngineSandboxKt`) now lives inside `core-kotlin` and exposes the façade through a tiny JSON protocol. Each request contains an `operation` field:
  - `"auto_drive_sequence"` requires `initial_state` + `operations` and returns both the Kotlin-only and Rust/JNI `AutoDriveSequenceResponse` payloads.
  - `"conversation_prune_history"` accepts `history` + optional `drop_last_user_turns` and emits parity responses for the prune helper.
  - `"conversation_filter_history"` filters the provided history via both implementations.
  - `"conversation_coalesce_snapshot"` deduplicates assistant snapshot records sharing a stream id.
  - `"auto_drive_turn_flow"` runs an end-to-end turn (Auto Drive sequence → prune → filter → snapshot coalesce) so we can verify Kotlin orchestrates multi-stage flows before owning the underlying logic.
  - `"conversation_snapshot_summary"` and `"conversation_fork_history"` mirror the resume/fork helpers that feed conversation restarts.
  - `"conversation_filter_popular_commands"` strips "Popular commands" scaffolding before the coordinator begins planning.
  - `"auto_coordinator_planning_seed"` takes a `goal` plus `include_agents` flag and returns both the Kotlin mirror and Rust planner seed JSON so developers can confirm coordinator bootstrapping parity.
- The CLI automatically initialises/shuts down `CoreEngineApi` and prints a JSON response describing both codepaths. Run it via `scripts/run-kotlin-engine-sandbox.sh`, either piping JSON into stdin or passing it as the first argument, e.g.:

```bash
scripts/run-kotlin-engine-sandbox.sh '{"operation":"conversation_filter_history","history":[{"type":"message","role":"system","content":[]}]}'
```

- This sandbox gives developers a quick way to exercise the mirrored operations without touching the Rust UI, and it will expand as we migrate more of the core engine into Kotlin.

### 2.3 Enabling the Kotlin engine inside the CLI

- The Rust CLI exposes an `engine_mode` toggle, but Kotlin is now the default. Set `CODE_ENGINE=rust` (or `engine_mode = "rust"` / CLI overrides) to opt back into the Rust engine when debugging; the legacy `CODE_USE_KOTLIN_ENGINE=1` flag is still supported but no longer required.
- Session configuration events emit a background notice when Kotlin is active so testers can confirm which engine served a conversation.
- The `code debug kotlin-turn` helper automatically sets `CODE_ENGINE=kotlin` for you and streams a single experimental turn, but the flag also works for full interactive sessions.

## 3. First JNI Candidate: Auto Drive Controller Slice
- **Why**: `code_auto_drive_core::controller` (see `code-rs/code-auto-drive-core/src/controller.rs`) is mostly stateful but self-contained; it transforms inputs (current turn metadata, settings) into `AutoControllerEffect`s consumed by the TUI. Its boundary already serialises through enums and structs, making it a good pilot for Kotlin orchestration.
- **Initial JNI contract**:
  - Kotlin submits an `AutoControllerRequest` JSON blob describing the current Auto Drive phase, timers, and queue state.
  - Rust returns an `AutoControllerEffectList` JSON blob (mirroring the existing enum) that the Kotlin layer can consume while still delegating to Rust for actual effect handling during Phase 0.
  - JNI entrypoints: `initialize_auto_controller`, `apply_auto_controller_inputs`, `shutdown_auto_controller` (these can be implemented by calling the existing Rust functions without re‑implementing logic yet).

## 4. Migration Phases
| Phase | What Stays in Rust | What Kotlin Owns | Notes |
| --- | --- | --- | --- |
| 0. Kotlin shim | Entire core engine (`code_core`, `code_auto_drive_core`). Kotlin only loads the shared library and forwards JSON APIs (see `core-kotlin` module). | Kotlin `CoreEngine` exposes JVM-friendly APIs (`initialize`, `execute`, `shutdown`) that wrap JNI. | Validates JNI plumbing while Rust delivers authoritative behaviour. |
| 1. Controller parity | `codex`, `exec`, networking, Auto Drive coordinator remain Rust. | Kotlin mirrors `AutoController` in pure Kotlin but can fall back to Rust implementation via JNI while tests gain parity coverage. | Swap call sites behind a feature flag to compare effect streams. |
| 2. Conversation & scheduling | Low-level exec stays Rust initially. | Kotlin takes over conversation management (`conversation_manager.rs`) and scheduling policies while still issuing exec requests back to Rust through JNI. | Requires extra JNI surfaces for streaming events and command IO. |
| 3. Exec / tool runtime | Rust continues to provide sandboxing until Kotlin/native strategy is settled. | Kotlin hosts planning, conversation, Auto Drive, and queue management. JNI boundaries become thinner (only for exec + system integration). | Allows exploring multiplatform front-ends. |

## 5. JNI Call Shape (Phase 0)
1. Kotlin serialises requests/responses as JSON so the Rust side can reuse `serde_json` with existing structs (no new ABI-specific structs needed immediately).
2. JNI signatures (see `core-kotlin/src/main/kotlin/ai/lightcode/core/jni/RustCoreBridge.kt`):
   - `initialize(configJson: String)` → `()`
   - `execute(requestJson: String): String`
   - `shutdown()` → `()`
3. Rust implements matching JNI functions in a new `cdylib` crate (e.g., `code-rs/core-jni`) that internally calls into `code_core` and `code_auto_drive_core`.
4. Kotlin exposes `CoreEngine` so non-JNI callers never invoke `System.loadLibrary` directly.

## 6. Kotlin ↔ Rust Ownership During Phases
- **Remain Rust-only for now**: model execution, token accounting, CLI command execution, auth/token caching, project filesystem mutations.
- **Move to Kotlin first**: Auto Drive controller state machine, queue/timeout logic, eventually conversation orchestration.
- **Bridge utilities**: configuration loading, logging/tracing hooks, and protocol event serialization will be shared by encoding/decoding JSON through JNI until Kotlin implementations are ready.

## 7. Next Steps
1. Land the `core-kotlin` Gradle module (see `/core-kotlin` in this repo) that loads `codex_core_jni` and exposes `CoreEngine`.
2. Add a lightweight `code-core-jni` crate in Rust that exports JNI functions and forwards them to the existing code paths (future work).
3. Define JSON schemas for `AutoControllerRequest` and `AutoControllerEffectList`, lift them into shared Kotlin/Rust representations, and exercise them through integration tests before porting logic.
4. Gate Kotlin-backed flows behind feature flags so `./build-fast.sh` stays green throughout the migration.

## 8. Running the Kotlin JNI smoke tests
- Use `scripts/run-core-kotlin-tests.sh` to build the `codex_core_jni` cdylib (`cargo build -p code-core-jni --release --locked`) and run `./core-kotlin/gradlew test` with the produced library on the JVM search path (`LD_LIBRARY_PATH`/`DYLD_LIBRARY_PATH`/`PATH`). This script is the single entry point for developers and CI.
- The main preview build workflow (see `.github/workflows/preview-build.yml`) now provisions JDK 21 and runs this script as a sibling required job. The release workflow (`.github/workflows/release.yml`) and the local `./build-fast.sh` helper also invoke it so Kotlin regressions block the same gates as Rust regressions.

This document should be refined as the Kotlin codebase grows, but it captures the agreed initial boundary and phases for JNI adoption.

## 9. Current Status (November 2025)

**What’s working today**
- `scripts/run-core-kotlin-tests.sh` builds the JNI jar (`code-core-jni` + Gradle tests) on every `./build-fast.sh`, preview build, and release run, so Kotlin regressions fail the same gates as Rust.
- `code-kotlin-parity-tests` compares AutoDrive effect sequences between Kotlin and Rust, keeping the controller mirrors honest.
- Kotlin is the default CLI engine; the TUI’s Misc settings panel shows the active engine and lets users persist `engine_mode` toggles.
- When `EngineMode::Kotlin` is selected, the CLI/TUI now renders at least one assistant message sourced directly from Kotlin: every auto-drive step summary is streamed to the UI via `AgentMessageDelta`/`AgentMessage` events without routing through the Rust `run_turn` path.
- The Kotlin host now emits `exec_request` / `patch_request` metadata on `AutoDriveEffectPayload` (seeded by a minimal demo path) so the Rust bridge can surface those requests via `ExecApprovalRequest` / `ApplyPatchApprovalRequest` events instead of using bespoke plan heuristics.
- TUI tests cover both the Kotlin chat turn smoke test and the new exec+patch history rendering path (even when Kotlin is selected).

**What’s still missing before Kotlin can replace Rust**
- Kotlin still depends on Rust for the full Auto Drive coordinator loop. The Kotlin controller can emit a single canned exec/patch effect, but goal derivation, planner prompts, queue retries, approvals, and restart semantics continue to live in `code_auto_drive_core`. Kotlin never consumes real `ResponseItem::LocalShellCall` output from the model, so coordinator parity stops at deterministic helpers.
- Tool execution remains Rust-owned end-to-end. Kotlin can now raise `exec_request` / `patch_request` metadata, but stdout/stderr streaming, patch diffs, sandboxing, turn diff emission, and approval state tracking are all handled inside `code-rs/core`. Kotlin does not yet source these requests from model decisions or observe the streaming lifecycle directly.
- Conversation and history orchestration are still backed by the Rust `conversation_manager`. Kotlin mirrors prune/filter/fork helpers, but resume/snapshot state, fork metadata, and final assistant output continue to be serialized in Rust.
- `run_kotlin_engine_turn` only reports background step/status events plus a summary message. The Kotlin path does not generate user-visible assistant responses, debugging deltas, or raw reasoning streams; when EngineMode::Kotlin is active, the TUI still waits for Rust to produce the final conversation items.
- End-to-end parity coverage exists for controller helpers and a single exec/patch slice, but there is no test that proves Kotlin alone can plan, approve, execute, and render a multi-turn Auto Drive session. Broader scenarios (agent queueing, pause/resume flows, streaming reasoning) still rely on Rust fixtures.

Bridging these gaps requires Kotlin to own the coordinator state machine, surface real tool calls sourced from model output, stream results back to the UI without Rust intermediaries, and prove parity across the entire turn lifecycle.

### 9.1 Next Incremental Step (design sketch)

Goal: replace the current exec/patch demo injections with a minimal real semantic flow that exercises the existing Rust exec/apply-patch pipeline end-to-end while still keeping the Kotlin controller simple.

1. **Kotlin decision output**
   - Extend the Kotlin auto-drive controller so that one concrete operation (e.g., “Apply first fix suggestion”) emits a structured representation of the tool action it would like to perform. Instead of pattern-matching on goal text, populate the `exec_request` / `patch_request` fields when the controller transitions into a “run command” or “apply patch” state. For now this can be a hard-coded branch (e.g., when continuing from a known countdown id) but it must be driven by actual controller state rather than arbitrary heuristics.
   - Mirror the Rust `LocalShellCall` schema: the exec request should carry a command vector, cwd, and optional reason; the patch request should carry a map of file changes (add/update/delete) plus optional `grant_root`.

2. **Rust bridge consumption**
   - Update `run_kotlin_engine_turn` so that it only emits `ExecApprovalRequest` and `ApplyPatchApprovalRequest` events when those request structs are present. Do **not** fabricate `ExecCommandBegin`/`End` or `PatchApply*`; instead hand the approval request to the existing exec/apply-patch machinery, letting it stream stdout/stderr, honor approvals, and emit history cells exactly as the Rust engine does today.
   - Propagate the call_ids from Kotlin so the approval and exec lifecycles stay aligned.

3. **Testing**
   - Add/adjust tests so that when `CODE_ENGINE=kotlin`, at least one end-to-end scenario drives the new flow: the Kotlin controller emits an `exec_request`, the Rust bridge raises the approval request, and the standard exec infrastructure runs the command and streams output. A similar test should cover the patch request path by reusing the existing TUI harness or `TestCodexHarness` utilities.

Once this vertical slice works, we can iterate toward sourcing the exec/patch requests from real Kotlin Auto Drive decisions (or planner output) instead of a stubbed branch.

### 9.2 Ownership inventory on the path to Rust parity

The table below groups the remaining Rust dependencies into a few concrete epics so workstreams can chip away at them without losing sight of the larger cutover goal.

| Epic | Kotlin today | What still routes through Rust | Suggested mileposts |
| --- | --- | --- | --- |
| **Coordinator & approval ownership** | Kotlin mirrors deterministic controller helpers (`AutoDriveControllerKt`) and can emit a single exec/patch effect. | `code_auto_drive_core::auto_coordinator` still owns planning prompts, queue retries, approval bookkeeping, interrupts, and restart semantics. Kotlin never ingests real `ResponseItem` tool calls or sends approval decisions back to the model. | (1) Build a Kotlin-hosted coordinator harness that consumes real model output, (2) stream `AutoCoordinatorEvent`s over JNI instead of calling back into Rust, (3) migrate approval/queue state to Kotlin while keeping the Rust implementation as a failsafe. |
| **Tool execution + streaming telemetry** | Kotlin can raise `exec_request` / `patch_request` and let the Rust bridge run the tool. | stdout/stderr chunks, patch diffs, sandbox choices, and turn-diff events are emitted exclusively by the Rust exec/apply-patch crates. Kotlin never sees the running command lifecycle, so it cannot pause/cancel or summarize results. | (1) Extend the Kotlin host to subscribe to exec stream events, (2) expose a Kotlin-native tool runner (or structured telemetry) so coordinator decisions can react without a Rust bridge, (3) add parity tests that compare streaming order and tail buffering between engines. |
| **Conversation + snapshot orchestration** | Kotlin mirrors prune/filter/fork/snapshot helpers for parity tests. | `conversation_manager`, history persistence, fork metadata, and resume snapshots are still produced in Rust. Kotlin emits only a synthetic summary message when EngineMode::Kotlin is active. | (1) Move `ConversationManager` responsibilities into Kotlin so it can persist history locally, (2) surface assistant messages / reasoning streams directly from Kotlin to the TUI, (3) add resume/fork fixtures that run solely against Kotlin data. |
| **CLI/TUI turn flow & streaming** | The CLI can flip `engine_mode = "kotlin"`, but `run_kotlin_engine_turn` only returns background summaries. | User-visible assistant responses, streaming deltas, token accounting, and error handling (interrupts, retries, auto-compaction) still run through the Rust `run_turn` path even when Kotlin is selected. | (1) Teach Kotlin to emit `AgentMessageDelta`/`AgentReasoningDelta` events with proper `OrderMeta`, (2) wire Kotlin turn retries, compaction, and scheduler hooks without calling back into Rust, (3) gate the Rust `run_turn` fallback behind an opt-in flag so Kotlin owns the happy path. |

Tracking progress against these epics will tell us when Kotlin truly equals (and can eventually supplant) the Rust engine instead of forwarding through it.

### 9.3 Kotlin cutover checklist

1. **Replace the Rust auto coordinator loop** – Introduce a Kotlin-hosted coordinator that subsumes `code-rs/code-auto-drive-core/src/auto_coordinator.rs` and the `ChatWidget` wiring around `AutoCoordinatorHandle` (`code-rs/tui/src/chatwidget.rs:16010`). The Kotlin loop must consume real `ResponseItem` streams, produce `AutoCoordinatorEvent` equivalents, and own pause/restart/queue semantics so Rust no longer mediates planning.
2. **Own tool approvals and streaming telemetry** – Today `run_kotlin_engine_turn` still funnels exec/apply-patch work through `handle_kotlin_exec_request` / `handle_kotlin_patch_request` (`code-rs/core/src/codex.rs:6614`). Kotlin needs a queue that watches stdout/stderr/diffs directly, emits approval prompts itself, and reconciles turn diffs without relying on Rust’s exec/apply-patch pipeline.
3. **Manage conversation history and snapshots natively** – Conversation creation, replay, and persistence remain in `conversation_manager.rs` (`code-rs/core/src/conversation_manager.rs:32`) and `conversation_history.rs`. Kotlin must take over history storage, snapshot replay, fork/resume metadata, and project-hook lifecycle events so Rust no longer serializes the transcript.
4. **Emit first-class streaming + telemetry data** – The `simple_model_turn` JNI path still shells into Rust’s `ModelClient` (`code-rs/code-core-jni/src/lib.rs:625`) which aggregates reasoning/answer deltas, token usage, and rate-limit events. Kotlin needs its own model client (or a thinner bridge) that streams `AgentMessageDelta`, reasoning content, usage metrics, and rate-limit snapshots directly, ensuring order metadata is assigned on the Kotlin side.
5. **Surface configuration/auth without Rust helpers** – Kotlin currently reuses Rust’s `Config`/`AuthManager` via JNI initialization (`code-rs/code-core-jni/src/lib.rs:681`). Completing the migration requires a Kotlin-owned config/auth loader so auto-drive turns, coordinator restarts, and approvals no longer bootstrap through Rust-only types.

## 3. Coverage Snapshot (Nov 2025)

| Subsystem | Kotlin coverage today | Parity hooks/tests | Next Kotlin candidates |
| --- | --- | --- | --- |
| **Auto Drive controller** | `AutoDriveControllerKt` + `KotlinAutoDriveControllerState` mirror countdown, continue-mode, launch, stop, and transient failure branches, with `TurnFlowOrchestrator` driving multi-step flows via `CoreEngineFacade.AutoDrive`. | `CoreEngineApiTest.autoDriveSequence*`, sandbox `auto_drive_sequence` / `auto_drive_turn_flow`, and JNI bridging through `CoreEngineApi.autoDriveSequence`. | Mirror a thin slice of the Auto Drive coordinator (e.g., goal-derivation or timeout helpers) so Kotlin can pre-flight turn decisions before delegating to Rust. |
| **Auto Drive coordinator** | Kotlin now mirrors the deterministic helpers that run before the model loop starts: the popular-command filter (`ConversationControllerKt`) and the planner seed bootstrapper (`PlannerSeedControllerKt`). Rust still owns the live coordinator loop in `code-auto-drive-core/src/auto_coordinator.rs`. | Sandbox requests `conversation_filter_popular_commands` + `auto_coordinator_planning_seed` and tests `CoreEngineApiTest.conversationPopularCommandsFilterMatchesRust` / `plannerSeedMatchesRust*`. | Next helpers: timeout/agent selection policies so Kotlin can predict `AutoCoordinatorEvent`s before mirroring the full state machine. |
| **Conversation manager/history** | Kotlin mirrors prune, filter, fork, and popular-command helpers via `ConversationControllerKt`, with `CoreEngineFacade.Conversation` exposing both Kotlin and Rust paths. | `CoreEngineApiTest` coverage (`:175`, `:232`, `:310`, `:319`) plus sandbox ops `conversation_prune_history` / `conversation_fork_history`. | Expand into resume/fork metadata (e.g., building full `InitialHistory` payloads) or coordinator goal heuristics. |
| **Snapshots** | `HistorySnapshotControllerKt` mirrors coalescing + summary logic; JNI routes (`conversation_coalesce_snapshot`, `conversation_snapshot_summary`) return Rust results for parity tests/sandbox. | Tests `conversationSnapshotCoalesceMatchesRust`, `conversationSnapshotSummaryMatchesRust` and sandbox ops keep mirrors aligned. | Round-trip full `HistorySnapshot` (serialize/deserialize) so Kotlin can simulate UI/state restoration independently. |
| **Exec / scheduling** | None—Kotlin only wraps JNI via `CoreEngine.execute`. | No parity tests; Rust’s `code-rs/core/src/exec.rs` owns execution. | Mirror lightweight scheduling heuristics or metadata validation so Kotlin can preflight exec runs before calling Rust. |
