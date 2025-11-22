# Kotlin Coordinator Slice – November 22, 2025

This change introduces the first production-safe slice of a Kotlin-hosted
coordinator loop as outlined in `docs/kotlin-migration-plan.md` (§9.3) and
`docs/kotlin-coordinator-roadmap.md`:

1. **Coordinator abstraction** – `core-kotlin` now exposes
   `KotlinCoordinatorDecision`, `KotlinCoordinatorInput`, and
   `SimpleKotlinCoordinator`. The coordinator consumes the same history stream
   used by `simple_model_turn`, interprets thinking/answer deltas, and (for now)
   heuristically flags when the assistant is attempting to run shell commands or
   provide a patch. The abstraction keeps the JNI protocol unchanged by emitting
   those decisions as ordinary agent messages so the Rust CLI still receives the
   exact `EventMsg` kinds it expects.
2. **CoreEngineHost integration** – Tool-free turns now flow through the new
   Kotlin coordinator bridge. When the coordinator is healthy we emit its
   thinking/answer/exec/patch decisions; if it cannot stream (no history,
   missing auth, etc.) we fall back to the previous synthesized lines so CI and
   offline test environments behave exactly as before.
3. **Rust insertion point** – On the Rust side the `code-auto-drive-core` crate
   defines an `AutoCoordinatorRuntime` trait, now implemented by both the
   existing Rust loop and an experimental `KotlinAutoCoordinatorRuntime`
   (selectable via the `CODEX_EXPERIMENTAL_KOTLIN_COORDINATOR=1` env var), and a
   `RustAutoCoordinatorRuntime` implementation. `ChatWidget` routes auto-drive
   turn creation through this runtime, which keeps current behavior but clearly
   marks where a Kotlin-hosted coordinator can plug in next without rewriting
   the TUI or approval plumbing.
4. **Stop/StopAck lifecycle** – The Kotlin host now accepts `control`
   submissions (starting with a `stop` command) and emits a
   `kotlin_coordinator_event` entry whose `decisions` list includes a
   `stop_ack`. The experimental runtime forwards that payload as the existing
   `AutoCoordinatorEvent::StopAck`, so cancellation semantics in the TUI remain
   unchanged even though Kotlin now owns the lifecycle acknowledgement.

### Follow-up slices

* Teach the Kotlin coordinator to emit structured exec/patch approvals over the
  JNI event stream rather than as placeholder agent messages.
* Implement a Kotlin runtime that satisfies `AutoCoordinatorRuntime` and feeds
  the trait with Kotlin-produced `AutoCoordinatorEvent`s. Once in place, the
  TUI can toggle between Rust and Kotlin runtimes without invasive plumbing.
* Move approval queue + stdout/diff routing into Kotlin, aligning with roadmap
  items “Own tool approvals and streaming telemetry” and “Emit first-class
  streaming data”.
* Teach the Kotlin runtime to honor decision ACKs so it can safely stream the
  next decision only after the TUI confirms receipt, enabling multi-step turns
  that stay ordered across JNI.
