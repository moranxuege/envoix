#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
ffi_dir="$repo_root/crates/envoix-ffi"
package_dir="$ffi_dir/EnvoixCore"
input_stamp="$package_dir/.envoix-inputs.sha256"
core_target="${ENVOIX_APPLE_CORE_TARGET:-}"
core_profile="${ENVOIX_APPLE_CORE_PROFILE:-release}"
generated_package_copies=(
  "generated/headers/envoix_ffiFFI.h"
  "generated/headers/module.modulemap"
  "generated/sources/envoix_ffi.swift"
)

case "$core_target" in
  ""|aarch64-apple-darwin|aarch64-apple-ios|aarch64-apple-ios-sim) ;;
  *)
    echo "error: ENVOIX_APPLE_CORE_TARGET must be empty, aarch64-apple-darwin, aarch64-apple-ios, or aarch64-apple-ios-sim" >&2
    exit 2
    ;;
esac
case "$core_profile" in
  debug|release) ;;
  *)
    echo "error: ENVOIX_APPLE_CORE_PROFILE must be debug or release" >&2
    exit 2
    ;;
esac

if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" == "1" \
      && "${ENVOIX_BUILD_LEASE_MODE:-writer}" == "reader" ]]; then
  echo "error: build-apple-core cannot mutate products under a reader lease" >&2
  exit 3
fi
if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
  exec "$repo_root/scripts/with-build-cache-guard.sh" "$0" "$@"
fi

apple_core_is_current() {
  [[ -f "$package_dir/Package.swift" ]] || return 1
  [[ -f "$input_stamp" ]] || return 1
  local recorded_digest
  IFS= read -r recorded_digest < "$input_stamp" || return 1
  [[ "$recorded_digest" == "$(apple_core_input_digest)" ]]
}

apple_core_input_digest() {
  {
    printf 'target=%s\n' "${core_target:-all}"
    printf 'profile=%s\n' "$core_profile"
    local input
    for input in \
      "$repo_root/Cargo.toml" \
      "$repo_root/Cargo.lock" \
      "$repo_root/scripts/build-apple-core.sh" \
      "$repo_root/scripts/configure-apple-package.sh" \
      "$repo_root/scripts/postprocess-apple-binding.py"; do
      printf '%s\n' "${input#$repo_root/}"
      shasum -a 256 "$input" | cut -d ' ' -f 1
    done

    while IFS= read -r input; do
      printf '%s\n' "${input#$repo_root/}"
      shasum -a 256 "$input" | cut -d ' ' -f 1
    done < <(find "$repo_root/crates" "$repo_root/vendor" \
      -path "$package_dir" -prune -o \
      -type f \( \
        -name '*.rs' -o \
        -name '*.toml' -o \
        -name '*.udl' -o \
        -name '*.swift' -o \
        -name '*.c' -o \
        -name '*.cc' -o \
        -name '*.cpp' -o \
        -name '*.h' -o \
        -name '*.modulemap' \
      \) -print | LC_ALL=C sort)
  } | shasum -a 256 | cut -d ' ' -f 1
}

if [[ "${ENVOIX_APPLE_FORCE_CORE_REBUILD:-0}" != "1" ]] && apple_core_is_current; then
  echo "Apple core inputs unchanged; reusing $package_dir (profile=$core_profile, target=${core_target:-all})"
  exit 0
fi

if ! cargo swift --version >/dev/null 2>&1; then
  echo "error: cargo-swift is required to build the Apple UniFFI package" >&2
  exit 2
fi

remove_generated_package_copies() {
  local binding
  for binding in "${generated_package_copies[@]}"; do
    rm -f "$ffi_dir/$binding"
  done
  rmdir "$ffi_dir/generated/headers" "$ffi_dir/generated/sources" 2>/dev/null || true
}

cleanup() {
  remove_generated_package_copies
}
trap cleanup EXIT INT TERM

generate_apple_package() {
  (
    cd "$ffi_dir"
    local -a package_args=(
      package
      --platforms macos@13 ios@16
      --name EnvoixCore
      --lib-type static
      --swift-tools-version 5.7
      --accept-all
    )
    if [[ -n "$core_target" ]]; then
      package_args+=(--target "$core_target")
    else
      package_args+=(--exclude-arch x86_64-apple-ios)
    fi
    if [[ "$core_profile" == "release" ]]; then
      package_args+=(--release)
    fi
    env \
      MACOSX_DEPLOYMENT_TARGET=13.0 \
      IPHONEOS_DEPLOYMENT_TARGET=16.0 \
      CFLAGS_aarch64_apple_darwin="-mmacosx-version-min=13.0" \
      CFLAGS_x86_64_apple_darwin="-mmacosx-version-min=13.0" \
      CFLAGS_aarch64_apple_ios="-miphoneos-version-min=16.0" \
      CFLAGS_aarch64_apple_ios_sim="-mios-simulator-version-min=16.0" \
      cargo swift "${package_args[@]}"
  )
}

version_exceeds() {
  awk -v actual="$1" -v maximum="$2" 'BEGIN {
    split(actual, a, ".")
    split(maximum, b, ".")
    if ((a[1] + 0) > (b[1] + 0)) exit 0
    if ((a[1] + 0) == (b[1] + 0) && (a[2] + 0) > (b[2] + 0)) exit 0
    exit 1
  }'
}

validate_library_minimum_versions() {
  local library="$1"
  local maximum="$2"
  local minimum
  local inspected=0
  while IFS= read -r minimum; do
    inspected=$((inspected + 1))
    if version_exceeds "$minimum" "$maximum"; then
      echo "error: $library contains an object requiring OS $minimum (maximum $maximum)" >&2
      return 1
    fi
  done < <(xcrun otool -l "$library" | awk '$1 == "minos" { print $2 }')
  if [[ "$inspected" -eq 0 ]]; then
    echo "error: could not inspect deployment versions in $library" >&2
    return 1
  fi
}

validate_apple_package_minimum_versions() {
  if [[ "$core_target" == "aarch64-apple-darwin" ]]; then
    validate_single_apple_library 13.0
    return
  fi
  if [[ "$core_target" == "aarch64-apple-ios-sim" ]]; then
    validate_single_apple_library 16.0
    return
  fi
  if [[ "$core_target" == "aarch64-apple-ios" ]]; then
    validate_single_apple_library 16.0
    return
  fi
  validate_library_minimum_versions \
    "$package_dir/envoix_ffiFFI.xcframework/macos-arm64_x86_64/libenvoix_ffi.a" \
    13.0 || return 1
  validate_library_minimum_versions \
    "$package_dir/envoix_ffiFFI.xcframework/ios-arm64/libenvoix_ffi.a" \
    16.0 || return 1
  validate_library_minimum_versions \
    "$package_dir/envoix_ffiFFI.xcframework/ios-arm64-simulator/libenvoix_ffi.a" \
    16.0 || return 1
}

validate_single_apple_library() {
  local maximum="$1"
  local -a libraries=()
  while IFS= read -r library; do
    libraries+=("$library")
  done < <(find "$package_dir/envoix_ffiFFI.xcframework" -type f -name 'libenvoix_ffi.a' -print)
  if [[ "${#libraries[@]}" -ne 1 ]]; then
    echo "error: expected one Apple core library for $core_target, found ${#libraries[@]}" >&2
    return 1
  fi
  validate_library_minimum_versions "${libraries[0]}" "$maximum"
}

clean_blake3_apple_targets() {
  if [[ -n "$core_target" ]]; then
    cargo clean -p blake3 --target "$core_target"
    return
  fi
  local target
  for target in \
    aarch64-apple-darwin \
    x86_64-apple-darwin \
    aarch64-apple-ios \
    aarch64-apple-ios-sim; do
    cargo clean -p blake3 --target "$target"
  done
}

echo "Generating Apple core package (profile=$core_profile, target=${core_target:-all})"
generate_apple_package

# BLAKE3 compiles platform-specific C/NEON objects. Reuse the Cargo cache on
# the normal path, but inspect every archived object before accepting it. If an
# older build left an iOS/macOS 26 load command behind, clean only BLAKE3 and
# regenerate once with the explicit deployment flags above.
if ! validate_apple_package_minimum_versions; then
  echo "Apple archive deployment validation failed; rebuilding BLAKE3 objects." >&2
  clean_blake3_apple_targets
  generate_apple_package
  validate_apple_package_minimum_versions
fi

apple_binding_postprocessor="$repo_root/scripts/postprocess-apple-binding.py"
python3 "$apple_binding_postprocessor" "$ffi_dir/generated/envoix_ffi.swift"
python3 "$apple_binding_postprocessor" "$ffi_dir/generated/sources/envoix_ffi.swift"
python3 "$apple_binding_postprocessor" "$ffi_dir/EnvoixCore/Sources/EnvoixCore/envoix_ffi.swift"

semantic_binding_change=0
if ! diff -q -w \
  "$ffi_dir/generated/envoix_ffi.swift" \
  "$ffi_dir/generated/sources/envoix_ffi.swift" >/dev/null; then
  echo "error: cargo-swift generated divergent Swift binding copies." >&2
  semantic_binding_change=1
fi
if ! diff -q -w \
  "$ffi_dir/generated/envoix_ffiFFI.h" \
  "$ffi_dir/generated/headers/envoix_ffiFFI.h" >/dev/null; then
  echo "error: cargo-swift generated divergent C header copies." >&2
  semantic_binding_change=1
fi
if ! diff -q -w \
  "$ffi_dir/generated/envoix_ffiFFI.modulemap" \
  "$ffi_dir/generated/headers/module.modulemap" >/dev/null; then
  echo "error: cargo-swift generated divergent module-map copies." >&2
  semantic_binding_change=1
fi
if [[ "$semantic_binding_change" -ne 0 ]]; then
  exit 1
fi

remove_generated_package_copies
"$repo_root/scripts/configure-apple-package.sh"
apple_core_input_digest > "$input_stamp"

echo "Apple core package generated at $ffi_dir/EnvoixCore"
