# Kotlin Auto Drive Coordinator Ownership – Design Outline

This note expands the “Coordinator & approval ownership” epic from
`docs/kotlin-migration-plan.md` with a concrete plan for moving the Auto Drive
coordinator loop into Kotlin. The goal is to let `EngineMode::Kotlin` run a full
turn – from model call → tool queue → approvals → streaming output – without the
Rust coordinator in `code-auto-drive-core`.

## 1. Responsibilities to migrate

1. **Consume model output and emit tool decisions**
   - Today: Rust’s `auto_coordinator` builds the coordinator prompt, feeds it to
     `ModelClient::stream`, and interprets each `ResponseItem` / `ResponseEvent`
     (thinking deltas, function calls, approvals).
   - Kotlin goal: `CoreEngineFacade.AutoDrive` should own the streaming loop: it
     constructs the same prompt, calls into the Kotlin model shim, and produces a
     typed timeline of `CoordinatorDecision` events:
       ```kotlin
       sealed interface CoordinatorDecision {
           data class ThinkingDelta(val text: String, val summaryIndex: Int?) : CoordinatorDecision
           data class ToolCall(val call: LocalShellCallPayload) : CoordinatorDecision
           data class ApplyPatch(val patch: KotlinPatchRequestPayload) : CoordinatorDecision
           data class FinalSummary(val status: AutoCoordinatorStatus, val transcript: List<ResponseItem>) : CoordinatorDecision
       }
       ```
     These decisions are serialized through JNI so the Rust CLI/TUI can render
     them while Kotlin continues to stream the underlying model output.

2. **Manage queued exec / apply-patch actions and approvals**
   - Today: when the model emits `ResponseItem::LocalShellCall`, the Rust
     coordinator enqueues the command, pauses the stream, and waits for the user
     to approve/reject via `Op::ExecApproval`/`Op::PatchApproval`. Rust also
     reroutes the tool outputs back into the prompt on retry.
   - Kotlin goal:
       - Maintain a Kotlin-side queue of pending tool calls keyed by `call_id`.
       - For each decision, emit JNI events that map directly to
         `ExecApprovalRequest` / `ApplyPatchApprovalRequest` (no intermediate
         Rust fabrications).
       - Store the approval result in Kotlin so the coordinator can resume the
         model stream with the tool output stitched into the prompt, matching
         the Rust retry semantics.
       - Use the real Rust exec/apply-patch runners only as transport pipes for
         stdout/stderr/diffs; the decision-making, queue state, and approval
         lifecycle live entirely in Kotlin.

3. **Stream assistant messages / reasoning directly to the UI**
   - Today: even under `EngineMode::Kotlin`, user-visible assistant messages are
     created inside `run_turn` after the Rust coordinator completes.
   - Kotlin goal: when Kotlin owns the coordinator stream, it should emit
     `AgentMessageDelta` and `AgentReasoningDelta` events as soon as the model
     produces them. The Rust CLI simply forwards these events; it no longer needs
     to synthesize final messages for the Kotlin path.

## 2. Proposed architecture

```
┌─────────────────────┐     JNI      ┌───────────────────────────┐
│ Kotlin Host (TUI)   │◀────────────▶│ CoreEngineFacade.AutoDrive │
│ - history snapshot  │              │ - CoordinatorLoop         │
│ - approval opcodes  │              │ - ToolQueue (Kotlin)      │
└─────────────────────┘              │ - Exec/Patch decision bus │
                                     └────────┬──────────────────┘
                                              │
                           stdout/stderr/diff │
                                              ▼
                                     ┌────────────────────────┐
                                     │ Rust exec/apply-patch │
                                     │ (unchanged sandbox)   │
                                     └────────────────────────┘
```

Key points:

1. Kotlin owns the streaming loop and produces a typed decision stream. Each
   decision maps 1:1 to the existing protocol events so the CLI/TUI keeps its
   current rendering logic.
   - Rust now exposes an `AutoCoordinatorRuntime` trait (see
     `code-auto-drive-core`) so the TUI can swap between the current Rust loop
     and a future Kotlin-hosted runtime without invasive plumbing.
2. Approvals travel back over JNI as `Op::ExecApproval` / `Op::PatchApproval`.
   Kotlin listens for these ops, updates the ToolQueue entry, and resumes the
   coordinator without calling into Rust.
3. Exec/apply-patch execution continues to run inside `code-rs/core`, but the
   results (stdout chunks, diffs, failure strings) are surfaced back to Kotlin so
   it can update the coordinator transcript before the next model turn.

## 3. First two increments

1. **Kotlin decision stream + assistant deltas**
   - Build a minimal `CoordinatorLoop` in Kotlin that consumes a static prompt
     (e.g., “return hello world”), calls the configured model via JNI, and emits
     `ThinkingDelta` + `FinalSummary` decisions.
   - Update the Rust CLI to accept these JNI decision events and bypass
     `run_turn` when `EngineMode::Kotlin` is active. Success criteria: the TUI
     shows Kotlin-generated thinking/answer text without touching the Rust
     coordinator.
   - **Status (Nov 2025):** the new `simple_model_turn` JNI request lets the Kotlin
     host stream a short thinking/answer pair sourced from the workspace
     conversation. When fixture data is present (e.g., during tests) the loop
     stays deterministic; otherwise it attempts a live model call and falls
     back to the previous heuristics if auth is unavailable.

2. **Single queued exec action managed in Kotlin**
   - Teach the Kotlin loop to recognize a single `LocalShellCall` decision,
     enqueue it, and send an `ExecApprovalRequest` event through the JNI bridge.
   - On approval, invoke `handle_container_exec_with_params` just like the current
     bridge, but capture the stdout/stderr so Kotlin can resume the coordinator
     with that output stitched into the transcript (mirroring the Rust behavior).
   - Add a regression test (similar to `code-rs/core/tests/kotlin_exec_flow.rs`)
     that proves the coordinator resumes after the approved exec command.

Additional increments (apply-patch queue, multi-turn retries, compaction) can
layer on after the above vertical slices land.

## 4. Risks / mitigations

- **Event ordering** – Kotlin must stamp `OrderMeta` the same way `run_turn`
  does so history cells don’t reorder. We can reuse the new `emit_kotlin_*`
  helpers as canonical wrappers.
- **Backpressure** – tool execution still happens in Rust; Kotlin needs a simple
  backpressure signal (e.g., bounded ToolQueue + “busy” flag) so we don’t flood
  the CLI with approval requests.
- **Fallback** – keep the Rust coordinator accessible behind a config toggle so
  we can fall back quickly (`engine_mode = "rust"`) while Kotlin coordination is
  stabilizing.

This document is intentionally concise; expand it as the Kotlin coordinator work
tracks real progress.
