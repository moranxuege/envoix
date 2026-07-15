#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
apple_dir="$repo_root/apps/envoix-apple"
project="$apple_dir/Envoix.xcodeproj"
xcodegen_input_stamp="$project/.envoix-inputs.sha256"
cache_root="${ENVOIX_APPLE_CACHE_ROOT:-${TMPDIR:-/tmp}/envoix-apple-cache}"
ios_sim_cache="$cache_root/ios-simulator-debug"
ios_device_cache="$cache_root/ios-device-debug"
macos_cache="$cache_root/macos-debug"
ios_sim_destination="${ENVOIX_IOS_SIM_DESTINATION:-}"
required_shared_schemes=(
  Envoix
  Envoix-iOS
  Envoix-iOS-Hosted
  Envoix-iOS-AppUI
  Envoix-macOS-Hosted
)

usage() {
  cat <<'EOF'
Usage: scripts/apple-dev.sh <command> [arguments]

Build commands:
  prepare                         Refresh the Rust package only when inputs changed, then run XcodeGen
  ios-build [xcodebuild args]     Incrementally build the iOS Simulator app
  ios-test-build <suite> [...]   Build an iOS test suite without running it
  ios-test <hosted|ui|all> [...] Incrementally build and run one test suite
  ios-test-rerun <suite> [...]   Rerun an already-built suite with test-without-building
  ios-device-build [...]         Build for ENVOIX_IOS_DEVICE_DESTINATION
  macos-build [...]              Incrementally build the macOS app
  macos-test-build [...]         Build the macOS App-hosted tests without running them
  macos-test [...]               Run the macOS App-hosted test target
  macos-test-rerun [...]         Rerun the built macOS App-hosted tests
  core-force                     Force regeneration of the Rust-to-Swift package

Space commands:
  guard-status                  Show free-space cleanup watermarks
  cache-size                     Show Envoix Xcode, generated core, and Cargo cache sizes
  trim-cache                     Remove Xcode logs/indexes but retain compiled products
  trim-rust-incremental          Remove Cargo incremental state but retain dependency artifacts
  clean-cache                    Remove only Envoix Xcode caches and legacy Apple build folders

Environment:
  ENVOIX_APPLE_CACHE_ROOT        Stable cache root (default: $TMPDIR/envoix-apple-cache)
  ENVOIX_IOS_SIM_DESTINATION    Simulator destination passed to xcodebuild
  ENVOIX_IOS_DEVICE_DESTINATION Required by ios-device-build
  ENVOIX_XCRESULT_PATH          Optional milestone-only .xcresult output path
  ENVOIX_BUILD_CACHE_MIN_FREE_GIB
                                Hard free-space minimum (default: 64)
  ENVOIX_BUILD_CACHE_TARGET_FREE_GIB
                                Auto-clean target (default: 96)
  ENVOIX_APPLE_FORCE_PROJECT_REBUILD
                                Force XcodeGen even when project inputs match
EOF
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: $1 is required" >&2
    exit 2
  fi
}

resolve_ios_sim_destination() {
  if [[ -n "$ios_sim_destination" ]]; then
    printf '%s\n' "$ios_sim_destination"
    return
  fi

  local line fallback_id=""
  while IFS= read -r line; do
    if [[ "$line" =~ ^[[:space:]]*iPhone\ 16\ Pro\ \(([0-9A-F-]+)\) ]]; then
      printf 'platform=iOS Simulator,id=%s\n' "${BASH_REMATCH[1]}"
      return
    fi
    if [[ -z "$fallback_id" \
          && "$line" =~ ^[[:space:]]*iPhone[^\(]*\ \(([0-9A-F-]+)\) ]]; then
      fallback_id="${BASH_REMATCH[1]}"
    fi
  done < <(xcrun simctl list devices available)

  if [[ -n "$fallback_id" ]]; then
    printf 'platform=iOS Simulator,id=%s\n' "$fallback_id"
    return
  fi

  echo "error: no available iPhone simulator; set ENVOIX_IOS_SIM_DESTINATION" >&2
  exit 2
}

xcodegen_input_digest() {
  {
    printf '%s\n' "xcodegen=$(xcodegen --version)"
    shasum -a 256 "$apple_dir/project.yml"
    find \
      "$apple_dir/Sources" \
      "$apple_dir/Shared" \
      "$apple_dir/Resources" \
      "$apple_dir/ShareExtension" \
      "$apple_dir/Tests" \
      -type f ! -name '.DS_Store' -print | LC_ALL=C sort
  } | shasum -a 256 | cut -d ' ' -f 1
}

generated_project_is_complete() {
  [[ -f "$project/project.pbxproj" ]] || return 1
  local scheme
  for scheme in "${required_shared_schemes[@]}"; do
    [[ -f "$project/xcshareddata/xcschemes/$scheme.xcscheme" ]] || return 1
  done
}

generate_project_if_needed() {
  local current_digest recorded_digest=""
  current_digest="$(xcodegen_input_digest)"
  if [[ -f "$xcodegen_input_stamp" ]]; then
    IFS= read -r recorded_digest < "$xcodegen_input_stamp" || true
  fi
  if [[ "${ENVOIX_APPLE_FORCE_PROJECT_REBUILD:-0}" != "1" \
        && "$current_digest" == "$recorded_digest" ]] \
        && generated_project_is_complete; then
    echo "Xcode project inputs unchanged; reusing $project"
    return
  fi

  (
    cd "$apple_dir"
    xcodegen generate
  )
  printf '%s\n' "$current_digest" > "$xcodegen_input_stamp"
}

prepare_project() {
  require_command xcodegen
  "$repo_root/scripts/build-apple-core.sh"
  generate_project_if_needed
}

result_bundle_args() {
  if [[ -n "${ENVOIX_XCRESULT_PATH:-}" ]]; then
    printf '%s\n' -resultBundlePath "$ENVOIX_XCRESULT_PATH"
  fi
}

scheme_for_suite() {
  case "$1" in
    hosted) printf '%s\n' Envoix-iOS-Hosted ;;
    ui) printf '%s\n' Envoix-iOS-AppUI ;;
    all) printf '%s\n' Envoix-iOS ;;
    *)
      echo "error: test suite must be hosted, ui, or all" >&2
      exit 2
      ;;
  esac
}

run_ios() {
  local scheme="$1"
  local action="$2"
  shift 2
  local destination
  destination="$(resolve_ios_sim_destination)"
  local -a result_args=()
  while IFS= read -r argument; do
    result_args+=("$argument")
  done < <(result_bundle_args)

  xcodebuild \
    -project "$project" \
    -scheme "$scheme" \
    -configuration Debug \
    -destination "$destination" \
    -derivedDataPath "$ios_sim_cache" \
    COMPILER_INDEX_STORE_ENABLE=NO \
    ${result_args[@]+"${result_args[@]}"} \
    "$action" \
    "$@"
}

show_sizes() {
  local paths=(
    "$cache_root"
    "$apple_dir/build"
    "$apple_dir/build-ios-ui-test"
    "$repo_root/crates/envoix-ffi/EnvoixCore"
    "$repo_root/target"
  )
  local path
  for path in "${paths[@]}"; do
    if [[ -e "$path" ]]; then
      du -sh "$path"
    fi
  done
}

trim_xcode_cache() {
  local directory
  for directory in "$ios_sim_cache" "$ios_device_cache" "$macos_cache"; do
    rm -rf -- "$directory/Index.noindex" "$directory/Logs"
  done
}

clean_xcode_cache() {
  if [[ "$cache_root" != *envoix* || "$cache_root" == "/" ]]; then
    echo "error: refusing to remove unsafe cache root: $cache_root" >&2
    exit 2
  fi
  rm -rf -- "$cache_root" "$apple_dir/build" "$apple_dir/build-ios-ui-test"
}

trim_rust_incremental() {
  local directory
  while IFS= read -r directory; do
    rm -rf -- "$directory"
  done < <(find "$repo_root/target" -type d -name incremental -prune -print 2>/dev/null)
}

command_name="${1:-help}"
if [[ "$#" -gt 0 ]]; then
  shift
fi

case "$command_name" in
  prepare|ios-build|ios-test-build|ios-test|ios-device-build|macos-build|macos-test-build|macos-test|core-force)
    if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" == "1" \
          && "${ENVOIX_BUILD_LEASE_MODE:-writer}" == "reader" ]]; then
      echo "error: $command_name cannot mutate products under a reader lease" >&2
      exit 3
    fi
    if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
      exec "$repo_root/scripts/with-build-cache-guard.sh" "$0" "$command_name" "$@"
    fi
    ;;
  ios-test-rerun|macos-test-rerun)
    if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
      exec "$repo_root/scripts/with-build-cache-guard.sh" \
        --preserve-build-products "$0" "$command_name" "$@"
    fi
    ;;
esac

case "$command_name" in
  prepare)
    prepare_project
    ;;
  ios-build)
    prepare_project
    run_ios Envoix-iOS build "$@"
    ;;
  ios-test)
    [[ "$#" -gt 0 ]] || { echo "error: ios-test requires hosted, ui, or all" >&2; exit 2; }
    suite="$1"
    shift
    scheme="$(scheme_for_suite "$suite")"
    prepare_project
    run_ios "$scheme" test "$@"
    ;;
  ios-test-build)
    [[ "$#" -gt 0 ]] || { echo "error: ios-test-build requires hosted, ui, or all" >&2; exit 2; }
    suite="$1"
    shift
    scheme="$(scheme_for_suite "$suite")"
    prepare_project
    run_ios "$scheme" build-for-testing "$@"
    ;;
  ios-test-rerun)
    [[ "$#" -gt 0 ]] || { echo "error: ios-test-rerun requires hosted, ui, or all" >&2; exit 2; }
    suite="$1"
    shift
    scheme="$(scheme_for_suite "$suite")"
    run_ios "$scheme" test-without-building "$@"
    ;;
  ios-device-build)
    [[ -n "${ENVOIX_IOS_DEVICE_DESTINATION:-}" ]] || {
      echo "error: set ENVOIX_IOS_DEVICE_DESTINATION to an iOS device destination" >&2
      exit 2
    }
    prepare_project
    xcodebuild \
      -project "$project" \
      -scheme Envoix-iOS \
      -configuration Debug \
      -destination "$ENVOIX_IOS_DEVICE_DESTINATION" \
      -derivedDataPath "$ios_device_cache" \
      -allowProvisioningUpdates \
      COMPILER_INDEX_STORE_ENABLE=NO \
      build \
      "$@"
    ;;
  macos-build)
    prepare_project
    xcodebuild \
      -project "$project" \
      -scheme Envoix \
      -configuration Debug \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      CODE_SIGNING_ALLOWED=NO \
      COMPILER_INDEX_STORE_ENABLE=NO \
      build \
      "$@"
    ;;
  macos-test)
    prepare_project
    result_args=()
    while IFS= read -r argument; do
      result_args+=("$argument")
    done < <(result_bundle_args)
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Hosted \
      -configuration Debug \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      ${result_args[@]+"${result_args[@]}"} \
      test \
      "$@"
    ;;
  macos-test-build)
    prepare_project
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Hosted \
      -configuration Debug \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      build-for-testing \
      "$@"
    ;;
  macos-test-rerun)
    result_args=()
    while IFS= read -r argument; do
      result_args+=("$argument")
    done < <(result_bundle_args)
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Hosted \
      -configuration Debug \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      ${result_args[@]+"${result_args[@]}"} \
      test-without-building \
      "$@"
    ;;
  core-force)
    ENVOIX_APPLE_FORCE_CORE_REBUILD=1 "$repo_root/scripts/build-apple-core.sh"
    ;;
  guard-status)
    "$repo_root/scripts/build-cache-guard.sh" --status
    ;;
  cache-size)
    show_sizes
    ;;
  trim-cache)
    trim_xcode_cache
    show_sizes
    ;;
  trim-rust-incremental)
    trim_rust_incremental
    show_sizes
    ;;
  clean-cache)
    clean_xcode_cache
    show_sizes
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    echo "error: unknown command: $command_name" >&2
    usage >&2
    exit 2
    ;;
esac
