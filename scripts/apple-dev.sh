#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
apple_dir="$repo_root/apps/envoix-apple"
project="$apple_dir/Envoix.xcodeproj"
xcodegen_input_stamp="$project/.envoix-inputs.sha256"
resolve_cache_root() {
  local configured="${ENVOIX_APPLE_CACHE_ROOT:-}"
  local default_root="${TMPDIR:-/tmp}/envoix-apple-cache"
  local github_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"

  if [[ "${GITHUB_ACTIONS:-0}" == "1" ]]; then
    configured="${configured:-$github_root/envoix-apple-cache/$GITHUB_RUN_ID}"
    case "$configured" in
      "$github_root/"*|"/private/tmp/"*|"/tmp/"*)
        ;;
      *)
        echo "warning: ENVOIX_APPLE_CACHE_ROOT=$configured is outside runner temp; using $github_root/envoix-apple-cache/$GITHUB_RUN_ID" >&2
        configured="$github_root/envoix-apple-cache/$GITHUB_RUN_ID"
        ;;
    esac
  else
    configured="${configured:-$default_root}"
  fi

  printf '%s\n' "${configured%/}"
}

cache_root="$(resolve_cache_root)"
apple_build_configuration="${ENVOIX_APPLE_BUILD_CONFIGURATION:-Debug}"
case "$apple_build_configuration" in
  Debug)
    apple_configuration_slug="debug"
    ;;
  Release)
    apple_configuration_slug="release"
    ;;
  *)
    echo "error: ENVOIX_APPLE_BUILD_CONFIGURATION must be Debug or Release" >&2
    exit 2
    ;;
esac
ios_sim_cache="$cache_root/ios-simulator-$apple_configuration_slug"
ios_device_cache="$cache_root/ios-device-$apple_configuration_slug"
macos_cache="$cache_root/macos-$apple_configuration_slug"
macos_release_cache="$cache_root/macos-release"
ios_sim_destination="${ENVOIX_IOS_SIM_DESTINATION:-}"
macos_release_team_id="6638TTB2SF"
macos_helper_bundle_id="com.envoix.app.engine-helper"
macos_helper_keychain_group="$macos_release_team_id.com.envoix.engine.credentials"
required_shared_schemes=(
  Envoix
  Envoix-iOS
  Envoix-iOS-Hosted
  Envoix-iOS-AppUI
  Envoix-macOS-Hosted
  Envoix-macOS-Clipboard
  Envoix-EngineHelper
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
  macos-clipboard-test [...]     Run unhosted macOS clipboard and local credential tests
  macos-helper-test [...]        Run isolated Agent helper host/control tests
  macos-release                  Archive, notarize, staple, and verify a Developer ID build
  core-force                     Force regeneration of the Rust-to-Swift package

Space commands:
  guard-status                  Show free-space cleanup watermarks
  cache-size                     Show Envoix Xcode, generated core, and Cargo cache sizes
  trim-cache                     Remove Xcode logs/indexes but retain compiled products
  trim-rust-incremental          Remove Cargo incremental state but retain dependency artifacts
  clean-cache                    Remove only Envoix Xcode caches and legacy Apple build folders

Environment:
  ENVOIX_APPLE_CACHE_ROOT        Stable cache root (default: $TMPDIR/envoix-apple-cache)
  ENVOIX_APPLE_BUILD_CONFIGURATION
                                Xcode configuration: Debug (default) or Release
  ENVOIX_APPLE_CORE_PROFILE      Rust core profile: release (default) or debug
  ENVOIX_IOS_SIM_DESTINATION    Simulator destination passed to xcodebuild
  ENVOIX_IOS_DEVICE_DESTINATION Required by ios-device-build
  ENVOIX_XCRESULT_PATH          Optional milestone-only .xcresult output path
  ENVOIX_MACOS_DEVELOPER_ID     Full Developer ID Application identity for Team 6638TTB2SF
  ENVOIX_MACOS_NOTARY_PROFILE   notarytool Keychain profile name
  ENVOIX_MACOS_RELEASE_DIR      New absolute output directory (default: dist/macos/<timestamp>)
  ENVOIX_BUILD_CACHE_MIN_FREE_GIB
                                Hard free-space minimum (default: 32)
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

apple_build_timestamp() {
  case "$apple_build_configuration" in
    Debug) date '+%Y-%m-%dT%H:%M:%S%z' ;;
    Release) date '+%Y-%m-%d' ;;
  esac
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
  local -a build_timestamp_args=()
  if [[ "$action" != "test-without-building" ]]; then
    build_timestamp_args=("ENVOIX_BUILD_TIMESTAMP=$(apple_build_timestamp)")
  fi
  local -a result_args=()
  while IFS= read -r argument; do
    result_args+=("$argument")
  done < <(result_bundle_args)

  xcodebuild \
    -project "$project" \
    -scheme "$scheme" \
    -configuration "$apple_build_configuration" \
    -destination "$destination" \
    -derivedDataPath "$ios_sim_cache" \
    ${build_timestamp_args[@]+"${build_timestamp_args[@]}"} \
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

verify_macos_release_bundle() {
  local application="$1"
  local helper="$application/Contents/Library/LoginItems/EnvoixEngineHelper.app"
  local evidence_directory="$2"
  local application_details helper_details application_requirement helper_requirement
  local application_identifier helper_identifier helper_group

  [[ -d "$application" ]] || {
    echo "error: archived application is missing: $application" >&2
    exit 4
  }
  [[ -d "$helper" ]] || {
    echo "error: embedded Engine helper is missing: $helper" >&2
    exit 4
  }

  codesign --verify --deep --strict --verbose=2 "$application"
  application_details="$(codesign -dv --verbose=4 "$application" 2>&1)"
  helper_details="$(codesign -dv --verbose=4 "$helper" 2>&1)"
  [[ "$application_details" == *"TeamIdentifier=$macos_release_team_id"* ]] || {
    echo "error: main application Team ID does not match $macos_release_team_id" >&2
    exit 4
  }
  [[ "$helper_details" == *"TeamIdentifier=$macos_release_team_id"* ]] || {
    echo "error: helper Team ID does not match $macos_release_team_id" >&2
    exit 4
  }
  [[ "$application_details" == *"(runtime)"* ]] || {
    echo "error: main application is not signed with hardened runtime" >&2
    exit 4
  }
  [[ "$helper_details" == *"(runtime)"* ]] || {
    echo "error: helper is not signed with hardened runtime" >&2
    exit 4
  }

  application_identifier="$(plutil -extract CFBundleIdentifier raw -o - "$application/Contents/Info.plist")"
  helper_identifier="$(plutil -extract CFBundleIdentifier raw -o - "$helper/Contents/Info.plist")"
  [[ "$application_identifier" == "com.envoix.app" ]] || {
    echo "error: unexpected main application bundle identifier: $application_identifier" >&2
    exit 4
  }
  [[ "$helper_identifier" == "$macos_helper_bundle_id" ]] || {
    echo "error: unexpected helper bundle identifier: $helper_identifier" >&2
    exit 4
  }

  codesign -d --entitlements :- "$application" \
    > "$evidence_directory/application-entitlements.plist"
  codesign -d --entitlements :- "$helper" \
    > "$evidence_directory/helper-entitlements.plist"
  if plutil -extract keychain-access-groups xml1 -o - \
      "$evidence_directory/application-entitlements.plist" >/dev/null 2>&1; then
    echo "error: GUI application must not have a Keychain access group" >&2
    exit 4
  fi
  helper_group="$(plutil -extract keychain-access-groups.0 raw -o - \
    "$evidence_directory/helper-entitlements.plist")"
  [[ "$helper_group" == "$macos_helper_keychain_group" ]] || {
    echo "error: helper Keychain access group does not match $macos_helper_keychain_group" >&2
    exit 4
  }
  if plutil -extract keychain-access-groups.1 raw -o - \
      "$evidence_directory/helper-entitlements.plist" >/dev/null 2>&1; then
    echo "error: helper must have exactly one Keychain access group" >&2
    exit 4
  fi
  if plutil -extract com.apple.security.get-task-allow raw -o - \
      "$evidence_directory/application-entitlements.plist" 2>/dev/null \
      | rg -x 'true' >/dev/null; then
    echo "error: release application must not allow debugger attachment" >&2
    exit 4
  fi
  if plutil -extract com.apple.security.get-task-allow raw -o - \
      "$evidence_directory/helper-entitlements.plist" 2>/dev/null \
      | rg -x 'true' >/dev/null; then
    echo "error: release helper must not allow debugger attachment" >&2
    exit 4
  fi

  application_requirement="$(codesign -dr - "$application" 2>&1)"
  helper_requirement="$(codesign -dr - "$helper" 2>&1)"
  [[ "$application_requirement" == *"identifier \"com.envoix.app\""* \
        && "$application_requirement" == *"$macos_release_team_id"* ]] || {
    echo "error: main application designated requirement is incomplete" >&2
    exit 4
  }
  [[ "$helper_requirement" == *"identifier \"$macos_helper_bundle_id\""* \
        && "$helper_requirement" == *"$macos_release_team_id"* ]] || {
    echo "error: helper designated requirement is incomplete" >&2
    exit 4
  }
}

build_macos_release() {
  require_command rg
  require_command security
  require_command codesign
  require_command ditto
  require_command plutil
  require_command shasum
  require_command spctl
  require_command xcodebuild
  require_command xcrun

  local identity="${ENVOIX_MACOS_DEVELOPER_ID:-}"
  local notary_profile="${ENVOIX_MACOS_NOTARY_PROFILE:-}"
  local release_stamp release_directory archive_path application submission_zip artifact_zip
  [[ -n "$identity" ]] || {
    echo "error: set ENVOIX_MACOS_DEVELOPER_ID to the full Developer ID Application identity" >&2
    exit 2
  }
  [[ -n "$notary_profile" ]] || {
    echo "error: set ENVOIX_MACOS_NOTARY_PROFILE to a notarytool Keychain profile" >&2
    exit 2
  }
  case "$identity" in
    "Developer ID Application:"*"($macos_release_team_id)") ;;
    *)
      echo "error: ENVOIX_MACOS_DEVELOPER_ID must be a Developer ID Application identity for Team $macos_release_team_id" >&2
      exit 2
      ;;
  esac
  if ! security find-identity -v -p codesigning \
      | rg -F -- "\"$identity\"" >/dev/null; then
    echo "error: Developer ID identity is not available in the current macOS Keychain" >&2
    exit 2
  fi

  release_stamp="$(date '+%Y%m%dT%H%M%S%z')"
  release_directory="${ENVOIX_MACOS_RELEASE_DIR:-$repo_root/dist/macos/$release_stamp}"
  case "$release_directory" in
    /*) ;;
    *)
      echo "error: ENVOIX_MACOS_RELEASE_DIR must be an absolute path" >&2
      exit 2
      ;;
  esac
  [[ ! -e "$release_directory" ]] || {
    echo "error: release output already exists: $release_directory" >&2
    exit 2
  }
  mkdir -p "$release_directory"

  archive_path="$release_directory/Envoix.xcarchive"
  application="$archive_path/Products/Applications/Envoix.app"
  submission_zip="$release_directory/Envoix-notary-submission.zip"
  artifact_zip="$release_directory/Envoix-0.3.0-macos-notarized.zip"

  xcodebuild \
    -project "$project" \
    -scheme Envoix \
    -configuration Release \
    -destination 'generic/platform=macOS' \
    -derivedDataPath "$macos_release_cache" \
    -archivePath "$archive_path" \
    DEVELOPMENT_TEAM="$macos_release_team_id" \
    CODE_SIGN_STYLE=Manual \
    CODE_SIGNING_ALLOWED=YES \
    CODE_SIGNING_REQUIRED=YES \
    CODE_SIGN_IDENTITY="$identity" \
    CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO \
    ENABLE_HARDENED_RUNTIME=YES \
    OTHER_CODE_SIGN_FLAGS=--timestamp \
    ENVOIX_BUILD_TIMESTAMP="$(date '+%Y-%m-%d')" \
    ARCHS='arm64 x86_64' \
    ONLY_ACTIVE_ARCH=NO \
    COMPILER_INDEX_STORE_ENABLE=NO \
    archive

  verify_macos_release_bundle "$application" "$release_directory"
  ditto -c -k --keepParent "$application" "$submission_zip"
  xcrun notarytool submit "$submission_zip" \
    --keychain-profile "$notary_profile" \
    --wait
  xcrun stapler staple "$application"
  xcrun stapler validate "$application"
  spctl --assess --type execute --verbose=4 "$application"
  verify_macos_release_bundle "$application" "$release_directory"
  ditto -c -k --keepParent "$application" "$artifact_zip"
  shasum -a 256 "$artifact_zip"
  echo "Notarized macOS artifact: $artifact_zip"
}

command_name="${1:-help}"
if [[ "$#" -gt 0 ]]; then
  shift
fi

case "$command_name" in
  prepare|ios-build|ios-test-build|ios-test|ios-device-build|macos-build|macos-test-build|macos-test|macos-clipboard-test|macos-helper-test|macos-release|core-force)
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
    envoix_build_timestamp="$(apple_build_timestamp)"
    xcodebuild \
      -project "$project" \
      -scheme Envoix-iOS \
      -configuration "$apple_build_configuration" \
      -destination "$ENVOIX_IOS_DEVICE_DESTINATION" \
      -derivedDataPath "$ios_device_cache" \
      -allowProvisioningUpdates \
      ENVOIX_BUILD_TIMESTAMP="$envoix_build_timestamp" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      build \
      "$@"
    ;;
  macos-build)
    prepare_project
    envoix_build_timestamp="$(apple_build_timestamp)"
    xcodebuild \
      -project "$project" \
      -scheme Envoix \
      -configuration "$apple_build_configuration" \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      CODE_SIGNING_ALLOWED=YES \
      CODE_SIGNING_REQUIRED=YES \
      CODE_SIGN_IDENTITY=- \
      CODE_SIGN_ENTITLEMENTS= \
      ENVOIX_BUILD_TIMESTAMP="$envoix_build_timestamp" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      build \
      "$@"
    ;;
  macos-test)
    prepare_project
    envoix_build_timestamp="$(apple_build_timestamp)"
    result_args=()
    while IFS= read -r argument; do
      result_args+=("$argument")
    done < <(result_bundle_args)
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Hosted \
      -configuration "$apple_build_configuration" \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      CODE_SIGN_ENTITLEMENTS= \
      ENVOIX_BUILD_TIMESTAMP="$envoix_build_timestamp" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      ${result_args[@]+"${result_args[@]}"} \
      test \
      "$@"
    ;;
  macos-test-build)
    prepare_project
    envoix_build_timestamp="$(apple_build_timestamp)"
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Hosted \
      -configuration "$apple_build_configuration" \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      CODE_SIGN_ENTITLEMENTS= \
      ENVOIX_BUILD_TIMESTAMP="$envoix_build_timestamp" \
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
      -configuration "$apple_build_configuration" \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      ${result_args[@]+"${result_args[@]}"} \
      test-without-building \
      "$@"
    ;;
  macos-clipboard-test)
    prepare_project
    envoix_build_timestamp="$(apple_build_timestamp)"
    xcodebuild \
      -project "$project" \
      -scheme Envoix-macOS-Clipboard \
      -configuration "$apple_build_configuration" \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      ENVOIX_BUILD_TIMESTAMP="$envoix_build_timestamp" \
      COMPILER_INDEX_STORE_ENABLE=NO \
      test \
      "$@"
    ;;
  macos-helper-test)
    prepare_project
    result_args=()
    while IFS= read -r argument; do
      result_args+=("$argument")
    done < <(result_bundle_args)
    xcodebuild \
      -project "$project" \
      -scheme Envoix-EngineHelper \
      -configuration Debug \
      -destination 'platform=macOS,arch=arm64' \
      -derivedDataPath "$macos_cache" \
      CODE_SIGNING_ALLOWED=YES \
      CODE_SIGNING_REQUIRED=YES \
      CODE_SIGN_IDENTITY=- \
      COMPILER_INDEX_STORE_ENABLE=NO \
      ${result_args[@]+"${result_args[@]}"} \
      test \
      "$@"
    ;;
  macos-release)
    [[ "$#" -eq 0 ]] || {
      echo "error: macos-release accepts configuration through environment variables only" >&2
      exit 2
    }
    prepare_project
    build_macos_release
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
