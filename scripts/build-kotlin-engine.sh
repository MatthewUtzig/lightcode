#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/code-rs/target}"
RAW_PROFILE="${KOTLIN_JNI_PROFILE:-${PROFILE:-release}}"
if [ "${RAW_PROFILE}" = "debug" ]; then
  KOTLIN_JNI_PROFILE="dev"
else
  KOTLIN_JNI_PROFILE="${RAW_PROFILE}"
fi
LIB_DIR="${TARGET_DIR}/${KOTLIN_JNI_PROFILE}"

echo "[core-kotlin] Preparing Kotlin engine jar (${KOTLIN_JNI_PROFILE})"

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
./gradlew --no-daemon --stacktrace shadowJar "$@"
JAR_SRC="${ROOT_DIR}/core-kotlin/build/libs/code-kotlin-engine-all.jar"
JAR_DEST="${TARGET_DIR}/kotlin/code-kotlin-engine.jar"
if [[ -f "${JAR_SRC}" ]]; then
  mkdir -p "$(dirname "${JAR_DEST}")"
  cp "${JAR_SRC}" "${JAR_DEST}"
else
  echo "[core-kotlin] WARNING: expected jar ${JAR_SRC} not found" >&2
fi
popd >/dev/null
