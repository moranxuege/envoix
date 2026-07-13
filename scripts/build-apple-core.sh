#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
ffi_dir="$repo_root/crates/envoix-ffi"
backup_dir="$(mktemp -d "${TMPDIR:-/tmp}/envoix-apple-bindings.XXXXXX")"
generated_bindings=(
  "generated/envoix_ffi.swift"
  "generated/envoix_ffiFFI.h"
  "generated/envoix_ffiFFI.modulemap"
  "generated/headers/envoix_ffiFFI.h"
  "generated/headers/module.modulemap"
  "generated/sources/envoix_ffi.swift"
)

if ! cargo swift --version >/dev/null 2>&1; then
  echo "error: cargo-swift is required to build the Apple UniFFI package" >&2
  exit 2
fi

restore_bindings() {
  local binding
  for binding in "${generated_bindings[@]}"; do
    if [[ -f "$backup_dir/$binding" ]]; then
      cp "$backup_dir/$binding" "$ffi_dir/$binding"
    fi
  done
}

cleanup() {
  restore_bindings
  rm -rf "$backup_dir"
}
trap cleanup EXIT INT TERM

for binding in "${generated_bindings[@]}"; do
  mkdir -p "$backup_dir/$(dirname "$binding")"
  cp "$ffi_dir/$binding" "$backup_dir/$binding"
done

# BLAKE3 compiles platform-specific C/NEON objects. Cargo can otherwise reuse
# objects produced before deployment-target flags were introduced, leaving an
# iOS 26/macOS 26 minimum-version load command inside an iOS 16/macOS 13 app.
for target in \
  aarch64-apple-darwin \
  x86_64-apple-darwin \
  aarch64-apple-ios \
  aarch64-apple-ios-sim; do
  cargo clean -p blake3 --target "$target"
done

(
  cd "$ffi_dir"
  env \
    MACOSX_DEPLOYMENT_TARGET=13.0 \
    IPHONEOS_DEPLOYMENT_TARGET=16.0 \
    CFLAGS_aarch64_apple_darwin="-mmacosx-version-min=13.0" \
    CFLAGS_x86_64_apple_darwin="-mmacosx-version-min=13.0" \
    CFLAGS_aarch64_apple_ios="-miphoneos-version-min=16.0" \
    CFLAGS_aarch64_apple_ios_sim="-mios-simulator-version-min=16.0" \
    cargo swift package \
      --platforms macos@13 ios@16 \
      --name EnvoixCore \
      --lib-type static \
      --exclude-arch x86_64-apple-ios \
      --swift-tools-version 5.7 \
      --accept-all
)

apple_binding_postprocessor="$repo_root/scripts/postprocess-apple-binding.py"
python3 "$apple_binding_postprocessor" "$ffi_dir/generated/envoix_ffi.swift"
python3 "$apple_binding_postprocessor" "$ffi_dir/generated/sources/envoix_ffi.swift"
python3 "$apple_binding_postprocessor" "$ffi_dir/EnvoixCore/Sources/EnvoixCore/envoix_ffi.swift"

semantic_binding_change=0
for binding in "${generated_bindings[@]}"; do
  if ! diff -q -w "$backup_dir/$binding" "$ffi_dir/$binding" >/dev/null; then
    echo "error: UniFFI generated a semantic change in $binding; regenerate and review the binding before building the app." >&2
    semantic_binding_change=1
  fi
done
if [[ "$semantic_binding_change" -ne 0 ]]; then
  exit 1
fi

restore_bindings
"$repo_root/scripts/configure-apple-package.sh"

echo "Apple core package generated at $ffi_dir/EnvoixCore"
