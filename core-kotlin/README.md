# Core Kotlin Bridge

The Kotlin engine implemented under `core-kotlin/` is a **sister system** to the
existing Rust engine inside `code-rs/` and the upstream mirror under
`codex-rs/`. Keep these invariants in mind:

- The Rust core remains fully in-tree, at the same paths, for the lifetime of
  the migration. Never delete or relocate `code-rs/core`, `code_auto_drive_core`,
  or their upstream counterparts.
- Kotlin implementations call into the Rust engine over JNI via
  `code-rs/code-core-jni/`. Feature flags only decide which engine handles a
  flow—they do **not** remove the Rust version.
- Upstream merges from `openai/codex` must continue to apply cleanly, so do not
  restructure the Rust workspace while adding Kotlin mirrors.
- `scripts/run-core-kotlin-tests.sh` (invoked by CI and `./build-fast.sh`)
  builds the shared library and executes the Kotlin parity suite. Keep it
  green before landing Kotlin changes.
- The same script also produces `build/libs/code-kotlin-engine-all.jar` and
  copies it to `code-rs/target/kotlin/code-kotlin-engine.jar`. The CLI now
  auto-detects the jar (preferring a copy next to the built `code` binary and
  falling back to `target/kotlin`) so manual `CODE_KOTLIN_CLASSPATH`
  configuration is optional during development.

Use this module to add mirrors/parity tests first, then gate new Kotlin-backed
flows behind explicit feature flags in the CLI/TUI.
