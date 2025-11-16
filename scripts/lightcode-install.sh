#!/usr/bin/env bash
set -euo pipefail

# Build (if needed) and install the Lightcode CLI to /usr/bin/lightcode.

SCRIPT_PATH="${BASH_SOURCE[0]}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${SCRIPT_PATH}")" >/dev/null 2>&1 && pwd)"
SCRIPT_ABS="${SCRIPT_DIR}/$(basename -- "${SCRIPT_PATH}")"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
BUILD_SCRIPT="${REPO_ROOT}/build-fast.sh"
INSTALL_PATH="/usr/bin/lightcode"
INSTALL_DIR="$(dirname -- "${INSTALL_PATH}")"

if [ ! -x "${BUILD_SCRIPT}" ]; then
  echo "Build script not found at ${BUILD_SCRIPT}" >&2
  exit 1
fi

if [ "${LIGHTCODE_INSTALL_REEXEC:-0}" != "1" ] && [ ! -w "${INSTALL_DIR}" ]; then
  if command -v sudo >/dev/null 2>&1; then
    exec sudo LIGHTCODE_INSTALL_REEXEC=1 "${SCRIPT_ABS}" "$@"
  fi
  echo "Write access to ${INSTALL_DIR} is required. Please rerun with sudo." >&2
  exit 1
fi

find_binary() {
  local override="${LIGHTCODE_BIN:-}"
  if [ -n "${override}" ]; then
    if [ ! -x "${override}" ]; then
      echo "LIGHTCODE_BIN is set to '${override}', but it is not executable." >&2
      return 1
    fi
    printf '%s\n' "${override}"
    return 0
  fi

  local candidates=(
    "${REPO_ROOT}/code-rs/bin/code"
    "${REPO_ROOT}/code-rs/target/dev-fast/code"
    "${REPO_ROOT}/code-rs/target/debug/code"
    "${REPO_ROOT}/code-rs/target/release/code"
  )

  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    candidates+=(
      "${CARGO_TARGET_DIR}/dev-fast/code"
      "${CARGO_TARGET_DIR}/debug/code"
      "${CARGO_TARGET_DIR}/release/code"
    )
  fi

  local candidate
  for candidate in "${candidates[@]}"; do
    if [ -x "${candidate}" ]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done
  return 1
}

ensure_binary() {
  if find_binary >/dev/null; then
    return 0
  fi
  echo "Local binary missing. Building via ./build-fast.sh ..."
  (cd "${REPO_ROOT}" && "${BUILD_SCRIPT}")
  if ! find_binary >/dev/null; then
    echo "Build did not produce an executable binary." >&2
    exit 1
  fi
}

ensure_binary
BIN_PATH="$(find_binary)"

install -Dm755 "${BIN_PATH}" "${INSTALL_PATH}"
echo "Installed ${BIN_PATH} -> ${INSTALL_PATH}"
