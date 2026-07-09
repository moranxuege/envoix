#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

unexpected_symbols="$(
  rg -n "Java_dev_envoix_app_" "$repo_root/crates/envoix-ffi/src/lib.rs" |
    rg -v "Java_dev_envoix_app_NativeBootstrap_(initContext|initLogging|setLogLevel)" || true
)"

unexpected_external="$(
  rg -n "external fun" "$repo_root/android/app/src/main/java/dev/envoix/app" \
    --glob '!**/ffi/**' |
    rg -v "Native.kt:.*(initLogging|setLogLevel|initContext)" || true
)"

if [[ -n "$unexpected_symbols$unexpected_external" ]]; then
  echo "Unexpected hand-written Android JNI surface detected." >&2
  echo "Only NativeBootstrap initContext/initLogging/setLogLevel are allowed;" >&2
  echo "transfers must go through UniFFI." >&2
  if [[ -n "$unexpected_symbols" ]]; then
    echo "$unexpected_symbols" >&2
  fi
  if [[ -n "$unexpected_external" ]]; then
    echo "$unexpected_external" >&2
  fi
  exit 1
fi

echo "Android JNI transfer surface check passed."
