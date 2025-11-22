#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/code-rs/target}"
KOTLIN_JNI_PROFILE="${KOTLIN_JNI_PROFILE:-${PROFILE:-release}}"
LIB_DIR="${TARGET_DIR}/${KOTLIN_JNI_PROFILE}"

echo "[core-kotlin] Building codex_core_jni (${KOTLIN_JNI_PROFILE})"
pushd "${ROOT_DIR}/code-rs" >/dev/null
BUILD_FLAGS=(cargo build --package code-core-jni --locked)
if [ "${KOTLIN_JNI_PROFILE}" = "release" ]; then
  BUILD_FLAGS+=(--release)
else
  BUILD_FLAGS+=(--profile "${KOTLIN_JNI_PROFILE}")
fi
"${BUILD_FLAGS[@]}"
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
chmod +x ./gradlew >/dev/null 2>&1 || true
./gradlew --no-daemon --stacktrace test shadowJar "$@"
JAR_SRC="${ROOT_DIR}/core-kotlin/build/libs/code-kotlin-engine-all.jar"
JAR_DEST="${TARGET_DIR}/kotlin/code-kotlin-engine.jar"
if [[ -f "${JAR_SRC}" ]]; then
  mkdir -p "$(dirname "${JAR_DEST}")"
  cp "${JAR_SRC}" "${JAR_DEST}"
else
  echo "[core-kotlin] WARNING: expected jar ${JAR_SRC} not found" >&2
fi
popd >/dev/null
