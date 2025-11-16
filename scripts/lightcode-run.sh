#!/usr/bin/env bash
set -euo pipefail

# Run the locally compiled Lightcode binary with the caller's arguments.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"

if [ -n "${LIGHTCODE_BIN:-}" ] && [ ! -x "${LIGHTCODE_BIN}" ]; then
  echo "LIGHTCODE_BIN is set to '${LIGHTCODE_BIN}', but it is not executable." >&2
  exit 1
fi

candidate_bins=()
if [ -n "${LIGHTCODE_BIN:-}" ]; then
  candidate_bins+=("${LIGHTCODE_BIN}")
fi
candidate_bins+=(
  "${REPO_ROOT}/code-rs/bin/code"
  "${REPO_ROOT}/code-rs/target/dev-fast/code"
  "${REPO_ROOT}/code-rs/target/debug/code"
  "${REPO_ROOT}/code-rs/target/release/code"
)

if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  candidate_bins+=(
    "${CARGO_TARGET_DIR}/dev-fast/code"
    "${CARGO_TARGET_DIR}/debug/code"
    "${CARGO_TARGET_DIR}/release/code"
  )
fi

BIN_PATH=""
for candidate in "${candidate_bins[@]}"; do
  if [ -x "${candidate}" ]; then
    BIN_PATH="${candidate}"
    break
  fi
done

if [ -z "${BIN_PATH}" ]; then
  cat >&2 <<'ERR'
Unable to find a compiled Lightcode binary.
Run ./build-fast.sh (workspace: code) and try again.
ERR
  exit 1
fi

exec "${BIN_PATH}" "$@"
