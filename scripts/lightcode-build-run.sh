#!/usr/bin/env bash
set -euo pipefail

# Rebuild the CLI and run it. Use "--" to separate build args from runtime args.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
BUILD_SCRIPT="${REPO_ROOT}/build-fast.sh"
RUN_SCRIPT="${SCRIPT_DIR}/lightcode-run.sh"

if [ ! -x "${BUILD_SCRIPT}" ]; then
  echo "Build script not found at ${BUILD_SCRIPT}" >&2
  exit 1
fi

split_run_args=0
build_args=()
run_args=()
for arg in "$@"; do
  if [ "${arg}" = "--" ]; then
    split_run_args=1
    continue
  fi
  if [ ${split_run_args} -eq 0 ]; then
    build_args+=("${arg}")
  else
    run_args+=("${arg}")
  fi
done

(cd "${REPO_ROOT}" && "${BUILD_SCRIPT}" "${build_args[@]}")

"${RUN_SCRIPT}" "${run_args[@]}"
