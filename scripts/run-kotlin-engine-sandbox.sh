#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${ROOT_DIR}/code-rs/target"
PROFILE="release"
LIB_DIR="${TARGET_DIR}/${PROFILE}"

echo "[kotlin-sandbox] Building codex_core_jni (${PROFILE})"
pushd "${ROOT_DIR}/code-rs" >/dev/null
cargo build --package code-core-jni --release --locked
popd >/dev/null

case "${OS:-$(uname -s)}" in
  Darwin)
    export DYLD_LIBRARY_PATH="${LIB_DIR}:${DYLD_LIBRARY_PATH:-}"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    export PATH="${LIB_DIR}:${PATH}"
    ;;
  *)
    export LD_LIBRARY_PATH="${LIB_DIR}:${LD_LIBRARY_PATH:-}"
    ;;
esac

pushd "${ROOT_DIR}/core-kotlin" >/dev/null
if [ "$#" -gt 0 ]; then
  ./gradlew --no-daemon --stacktrace run --args "$1"
else
  ./gradlew --no-daemon --stacktrace run
fi
popd >/dev/null
