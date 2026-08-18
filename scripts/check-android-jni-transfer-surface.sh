#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
kotlin_native="$repo_root/android/app/src/main/java/dev/envoix/app/Native.kt"
rust_jni_dir="$repo_root/crates/envoix-ffi/src/android_jni"

kotlin_symbols="$(
  grep -Eo 'external fun [A-Za-z0-9_]+' "$kotlin_native" |
    awk '{print $3}' |
    sort -u
)"
rust_symbols="$(
  find "$rust_jni_dir" -maxdepth 1 -type f -name '*.rs' \
    -exec grep -hEo 'fn Java_dev_envoix_app_Native_[A-Za-z0-9_]+' {} + |
    sed 's/fn Java_dev_envoix_app_Native_//' |
    sort -u
)"
unexpected_external="$(
  find "$repo_root/android/app/src/main/java/dev/envoix/app" \
    -type f -name '*.kt' ! -name 'Native.kt' ! -path '*/ffi/*' \
    -exec grep -nH 'external fun' {} + || true
)"

if ! grep -Fq 'System.loadLibrary("envoix_ffi")' "$kotlin_native"; then
  echo "Android JNI and UniFFI must load from the single envoix_ffi library." >&2
  exit 1
fi

if grep -Fq 'System.loadLibrary("envoix_jni")' "$kotlin_native"; then
  echo "The removed envoix_jni library must not be loaded." >&2
  exit 1
fi

if [[ -n "$unexpected_external" ]]; then
  echo "JNI declarations must stay in Native.kt so the boundary remains auditable." >&2
  echo "$unexpected_external" >&2
  exit 1
fi

if [[ "$kotlin_symbols" != "$rust_symbols" ]]; then
  echo "Android JNI declarations and exported Rust symbols do not match." >&2
  echo "Kotlin Native.kt:" >&2
  echo "$kotlin_symbols" >&2
  echo "Rust envoix-ffi exceptional JNI:" >&2
  echo "$rust_symbols" >&2
  exit 1
fi

echo "Android JNI boundary check passed: Kotlin and Rust expose the same symbols."
