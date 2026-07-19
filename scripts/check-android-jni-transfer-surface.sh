#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
kotlin_native="$repo_root/android/app/src/main/java/dev/envoix/app/Native.kt"
rust_jni="$repo_root/apps/envoix-android-jni/src/lib.rs"

kotlin_symbols="$(
  grep -Eo 'external fun [A-Za-z0-9_]+' "$kotlin_native" |
    awk '{print $3}' |
    sort -u
)"
rust_symbols="$(
  grep -Eo 'fn Java_dev_envoix_app_Native_[A-Za-z0-9_]+' "$rust_jni" |
    sed 's/fn Java_dev_envoix_app_Native_//' |
    sort -u
)"
unexpected_external="$(
  find "$repo_root/android/app/src/main/java/dev/envoix/app" \
    -type f -name '*.kt' ! -name 'Native.kt' ! -path '*/ffi/*' \
    -exec grep -nH 'external fun' {} + || true
)"

if [[ -n "$unexpected_external" ]]; then
  echo "JNI declarations must stay in Native.kt so the boundary remains auditable." >&2
  echo "$unexpected_external" >&2
  exit 1
fi

if [[ "$kotlin_symbols" != "$rust_symbols" ]]; then
  echo "Android JNI declarations and exported Rust symbols do not match." >&2
  echo "Kotlin Native.kt:" >&2
  echo "$kotlin_symbols" >&2
  echo "Rust envoix-android-jni:" >&2
  echo "$rust_symbols" >&2
  exit 1
fi

echo "Android JNI boundary check passed: Kotlin and Rust expose the same symbols."
