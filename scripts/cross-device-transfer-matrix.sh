#!/usr/bin/env bash
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
android_dir="$repo_root/android"
registry="$repo_root/tests/e2e/matrix/cases.v1.json"
contract="$repo_root/scripts/matrix_contract.py"
apple_evidence="$repo_root/scripts/apple_matrix_evidence.py"

adb_bin="${ADB:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb}"
adb_serial="${ANDROID_SERIAL:-}"
android_home="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
apple_cache_root="${ENVOIX_APPLE_CACHE_ROOT:-${TMPDIR:-/tmp}/envoix-apple-cache}"
ios_destination="${ENVOIX_IOS_DESTINATION:-}"
macos_destination="platform=macOS"
repeat_count="${ENVOIX_MATRIX_REPEAT:-}"
large_bytes="${ENVOIX_MATRIX_LARGE_BYTES:-134217728}"
ready_timeout="${ENVOIX_MATRIX_READY_TIMEOUT_SECONDS:-120}"
transfer_timeout_override_ms="${ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS:-}"
transfer_timeout_ms=0
receiver_settle_seconds="${ENVOIX_MATRIX_RECEIVER_SETTLE_SECONDS:-1}"
base_run_id="${ENVOIX_MATRIX_RUN_ID:-$(date +%Y%m%d-%H%M%S)-$$}"
scenario_text="${ENVOIX_MATRIX_SCENARIOS:-}"
direction_text="${ENVOIX_MATRIX_DIRECTIONS:-}"
skip_build="${ENVOIX_SKIP_BUILD:-0}"
output_dir="${ENVOIX_MATRIX_LOG_DIR:-${TMPDIR:-/tmp}/envoix-transfer-matrix-$base_run_id}"
tested_commit="$(git -C "$repo_root" rev-parse HEAD)"
build_variant="debug"
dry_run=0
action="run"
selected_gate=""
selected_tag=""
selected_cases=()
legacy_selection=0
original_args=("$@")
test_runner="dev.envoix.app.test/androidx.test.runner.AndroidJUnitRunner"
android_build_type=""
android_task_suffix=""
apple_build_configuration=""
apple_configuration_slug=""
main_apk=""
test_apk=""

usage() {
  cat <<'EOF'
Usage: scripts/cross-device-transfer-matrix.sh [selection] [options]

Selection (choose one):
  --case CASE_ID              Select one explicit case; may be repeated
  --gate GATE                 Select one registry gate
  --tag TAG                   Select cases with one registry tag

Inspection:
  --list                      List the versioned case registry
  --validate                  Validate the registry and runner syntax

Execution options:
  --dry-run                   Plan and report without building or executing
  --commit SHA                Record the exact tested commit (default: HEAD)
  --run-id ID                 Stable run ID
  --output-directory PATH     Result root
  --android-device SERIAL     Select the physical Android device
  --ios-destination VALUE     Select the physical iPhone destination
  --build-variant VARIANT     debug or release_equivalent

With no selection, the current physical-harness gate is used. The legacy
ENVOIX_MATRIX_SCENARIOS and ENVOIX_MATRIX_DIRECTIONS variables are mapped to
explicit registry cases for one migration period; unregistered combinations
are warned and never synthesized.

Environment:
  ENVOIX_MATRIX_SCENARIOS="single_file image ..."  Override scenario set
  ENVOIX_MATRIX_DIRECTIONS="android:ios ..."       Override directed pairs
  ENVOIX_MATRIX_REPEAT=2                            Increase repetitions only
  ENVOIX_MATRIX_LARGE_BYTES=134217728               Large fixture size
  ENVOIX_IOS_DESTINATION="platform=iOS,id=..."      Required physical iPhone
  ANDROID_SERIAL=...                                Physical Android device
  ENVOIX_SKIP_BUILD=1                               Reuse existing artifacts
EOF
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --list)
      action="list"
      shift
      ;;
    --validate)
      action="validate"
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --case)
      [[ "$#" -ge 2 ]] || { echo "error: --case requires a value" >&2; exit 2; }
      selected_cases+=("$2")
      shift 2
      ;;
    --gate)
      [[ "$#" -ge 2 ]] || { echo "error: --gate requires a value" >&2; exit 2; }
      selected_gate="$2"
      shift 2
      ;;
    --tag)
      [[ "$#" -ge 2 ]] || { echo "error: --tag requires a value" >&2; exit 2; }
      selected_tag="$2"
      shift 2
      ;;
    --commit)
      [[ "$#" -ge 2 ]] || { echo "error: --commit requires a value" >&2; exit 2; }
      tested_commit="$2"
      shift 2
      ;;
    --run-id)
      [[ "$#" -ge 2 ]] || { echo "error: --run-id requires a value" >&2; exit 2; }
      base_run_id="$2"
      shift 2
      ;;
    --output-directory)
      [[ "$#" -ge 2 ]] || { echo "error: --output-directory requires a value" >&2; exit 2; }
      output_dir="$2"
      shift 2
      ;;
    --android-device)
      [[ "$#" -ge 2 ]] || { echo "error: --android-device requires a value" >&2; exit 2; }
      adb_serial="$2"
      shift 2
      ;;
    --ios-destination)
      [[ "$#" -ge 2 ]] || { echo "error: --ios-destination requires a value" >&2; exit 2; }
      ios_destination="$2"
      shift 2
      ;;
    --build-variant)
      [[ "$#" -ge 2 ]] || { echo "error: --build-variant requires a value" >&2; exit 2; }
      build_variant="$2"
      shift 2
      ;;
    *)
      echo "error: unknown argument '$1'" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$action" == "list" ]]; then
  exec python3 "$contract" list-cases "$registry"
fi
if [[ "$action" == "validate" ]]; then
  python3 "$contract" validate-registry "$registry"
  bash -n "$0"
  exit 0
fi
if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" == "1" \
      && "${ENVOIX_BUILD_LEASE_MODE:-writer}" == "reader" ]]; then
  echo "error: the transfer matrix needs a writer build lease" >&2
  exit 3
fi
if [[ "$dry_run" != "1" && "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
  exec "$repo_root/scripts/with-build-cache-guard.sh" "$0" "${original_args[@]}"
fi

require_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: $name must be a positive integer" >&2
    exit 2
  fi
}

require_non_negative_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "error: $name must be a non-negative integer" >&2
    exit 2
  fi
}

if [[ -n "$repeat_count" ]]; then
  require_positive_integer ENVOIX_MATRIX_REPEAT "$repeat_count"
fi
require_positive_integer ENVOIX_MATRIX_LARGE_BYTES "$large_bytes"
require_positive_integer ENVOIX_MATRIX_READY_TIMEOUT_SECONDS "$ready_timeout"
if [[ -n "$transfer_timeout_override_ms" ]]; then
  require_positive_integer \
    ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS "$transfer_timeout_override_ms"
fi
require_non_negative_integer ENVOIX_MATRIX_RECEIVER_SETTLE_SECONDS "$receiver_settle_seconds"
if [[ "$skip_build" != "0" && "$skip_build" != "1" ]]; then
  echo "error: ENVOIX_SKIP_BUILD must be 0 or 1" >&2
  exit 2
fi
if [[ "$build_variant" != "debug" && "$build_variant" != "release_equivalent" ]]; then
  echo "error: --build-variant must be debug or release_equivalent" >&2
  exit 2
fi
if [[ "$build_variant" == "release_equivalent" ]]; then
  android_build_type="release"
  android_task_suffix="Release"
  apple_build_configuration="Release"
  apple_configuration_slug="release"
else
  android_build_type="debug"
  android_task_suffix="Debug"
  apple_build_configuration="Debug"
  apple_configuration_slug="debug"
fi
main_apk="$android_dir/app/build/outputs/apk/$android_build_type/app-$android_build_type.apk"
test_apk="$android_dir/app/build/outputs/apk/androidTest/$android_build_type/app-$android_build_type-androidTest.apk"
if [[ ! "$base_run_id" =~ ^[A-Za-z0-9_.-]+$ || "${#base_run_id}" -gt 96 ]]; then
  echo "error: run ID must be at most 96 letters, digits, '.', '-' or '_'" >&2
  exit 2
fi

selection_modes=0
[[ "${#selected_cases[@]}" -gt 0 ]] && selection_modes=$((selection_modes + 1))
[[ -n "$selected_gate" ]] && selection_modes=$((selection_modes + 1))
[[ -n "$selected_tag" ]] && selection_modes=$((selection_modes + 1))
if [[ -n "$scenario_text" || -n "$direction_text" ]]; then
  legacy_selection=1
  selection_modes=$((selection_modes + 1))
fi
if [[ "$selection_modes" -gt 1 ]]; then
  echo "error: select only one of --case, --gate, --tag, or legacy environment inputs" >&2
  exit 2
fi
if [[ "$selection_modes" -eq 0 ]]; then
  selected_gate="current-physical-harness"
fi

resolve_args=(
  resolve-cases "$registry"
  --run-id "$base_run_id"
  --commit "$tested_commit"
  --build-variant "$build_variant"
  --output "$output_dir/matrix-plan.json"
)
[[ "$dry_run" == "1" ]] && resolve_args+=(--dry-run)
[[ -n "$repeat_count" ]] && resolve_args+=(--repetitions "$repeat_count")
if [[ "${#selected_cases[@]}" -gt 0 ]]; then
  for case_id in "${selected_cases[@]}"; do
    resolve_args+=(--case "$case_id")
  done
fi
[[ -n "$selected_gate" ]] && resolve_args+=(--gate "$selected_gate")
[[ -n "$selected_tag" ]] && resolve_args+=(--tag "$selected_tag")
if [[ "$legacy_selection" == "1" ]]; then
  scenario_text="${scenario_text:-single_file multiple_files folder multiple_folders image large_file collision overlap unicode_empty same_name_roots share}"
  direction_text="${direction_text:-android:ios ios:android android:macos macos:android ios:macos macos:ios}"
  read -r -a scenarios <<< "$scenario_text"
  read -r -a directions <<< "$direction_text"
  for scenario in "${scenarios[@]}"; do
    resolve_args+=(--legacy-scenario "$scenario")
  done
  for direction in "${directions[@]}"; do
    resolve_args+=(--legacy-direction "$direction")
  done
fi

umask 077
mkdir -p "$output_dir/cases" "$output_dir/sanitized/cases" "$output_dir/private/cases"
chmod 700 "$output_dir/private"
python3 "$contract" "${resolve_args[@]}" || exit 2
plan_file="$output_dir/matrix-plan.json"
result_file="$output_dir/matrix-result.json"
report_file="$output_dir/matrix-report.md"

adb_command() {
  if [[ -n "$adb_serial" ]]; then
    "$adb_bin" -s "$adb_serial" "$@"
  else
    "$adb_bin" "$@"
  fi
}

require_android_network_route() {
  local route
  # Route lookup is local to Android and sends no traffic. Use a general public
  # IPv4 target instead of assuming any Wi-Fi interface, gateway, or subnet.
  if ! route="$(adb_command shell ip -4 route get 1.1.1.1 2>&1)"; then
    route="${route//$'\r'/}"
    echo "environment error: the selected Android device has no usable public IPv4 network route" >&2
    echo "Connect Wi-Fi or mobile data before running the cross-device matrix." >&2
    [[ -z "$route" ]] || echo "Android route probe: $route" >&2
    return 1
  fi
  route="${route//$'\r'/}"
  if [[ -z "$route" \
        || "$route" == *"unreachable"* \
        || "$route" == *"blackhole"* \
        || "$route" == *"prohibit"* \
        || ! "$route" =~ (^|[[:space:]])dev[[:space:]][^[:space:]]+ ]]; then
    echo "environment error: the selected Android device has no usable public IPv4 network route" >&2
    echo "Connect Wi-Fi or mobile data before running the cross-device matrix." >&2
    [[ -z "$route" ]] || echo "Android route probe: $route" >&2
    return 1
  fi
}

matrix_uses_macos() {
  [[ "${MATRIX_USES_MACOS:-0}" == "1" ]]
}

sanitize_log() {
  local source="$1"
  local relative sanitized
  [[ -f "$source" ]] || return 0
  relative="${source#"$output_dir/private/"}"
  sanitized="$output_dir/sanitized/$relative"
  python3 "$contract" redact "$source" "$sanitized"
}

print_log_tail() {
  local title="$1"
  local file="$2"
  local relative sanitized
  relative="${file#"$output_dir/private/"}"
  sanitized="$output_dir/sanitized/$relative"
  sanitize_log "$file"
  echo "--- $title: sanitized/$relative ---" >&2
  if [[ -f "$file" ]]; then
    tail -n 120 "$sanitized" >&2
  else
    echo "missing log" >&2
  fi
}

prepare_builds() {
  local build_private="$output_dir/private/build"
  local -a android_build_args=(
    :app:ktlintCheck
    ":app:assemble$android_task_suffix"
    ":app:assemble${android_task_suffix}AndroidTest"
    --no-daemon
  )
  mkdir -p "$build_private"
  if [[ ! -x "$adb_bin" ]]; then
    echo "error: adb not found at $adb_bin" >&2
    return 1
  fi
  if ! adb_command get-state >/dev/null 2>&1; then
    echo "error: no usable Android device; set ANDROID_SERIAL if more than one is attached" >&2
    return 1
  fi
  require_android_network_route || return 1

  if [[ "$skip_build" != "1" ]]; then
    echo "build: Android $android_build_type application and instrumented tests"
    if ! (
      cd "$android_dir"
      env ANDROID_HOME="$android_home" ANDROID_SDK_ROOT="$android_home" \
        ./gradlew \
          -Penvoix.testBuildType="$android_build_type" \
          "${android_build_args[@]}"
    ); then
      return 1
    fi

    echo "build: iPhone $apple_build_configuration hosted tests"
    if ! env \
      ENVOIX_APPLE_CACHE_ROOT="$apple_cache_root" \
      ENVOIX_APPLE_BUILD_CONFIGURATION="$apple_build_configuration" \
      ENVOIX_IOS_SIM_DESTINATION="$ios_destination" \
      "$repo_root/scripts/apple-dev.sh" ios-test-build hosted -quiet; then
      return 1
    fi

    if matrix_uses_macos; then
      echo "build: Mac $apple_build_configuration hosted tests"
      if ! env \
        ENVOIX_APPLE_CACHE_ROOT="$apple_cache_root" \
        ENVOIX_APPLE_BUILD_CONFIGURATION="$apple_build_configuration" \
        "$repo_root/scripts/apple-dev.sh" macos-test-build -quiet; then
        return 1
      fi
    fi
  fi

  echo "deploy: Android application and test APK"
  adb_command shell input keyevent WAKEUP >/dev/null 2>&1 || true
  adb_command shell wm dismiss-keyguard >/dev/null 2>&1 || true
  adb_command shell svc power stayon usb >/dev/null 2>&1 || true
  if ! adb_command install -r -d -t -g "$main_apk" > "$build_private/android-main.install.log" 2>&1; then
    print_log_tail "Android main APK install" "$build_private/android-main.install.log"
    return 1
  fi
  if ! adb_command install -r -d -t -g "$test_apk" > "$build_private/android-test.install.log" 2>&1; then
    print_log_tail "Android test APK install" "$build_private/android-test.install.log"
    return 1
  fi
  if ! sanitize_log "$build_private/android-main.install.log" \
      || ! sanitize_log "$build_private/android-test.install.log"; then
    echo "error: could not sanitize Android installation logs" >&2
    return 1
  fi
  rm -f "$build_private/android-main.install.log" "$build_private/android-test.install.log"
  adb_command shell pm grant dev.envoix.app android.permission.CAMERA >/dev/null 2>&1 || true
  adb_command shell pm grant dev.envoix.app android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
}

ios_products="$apple_cache_root/ios-simulator-$apple_configuration_slug/Build/Products"
macos_products="$apple_cache_root/macos-$apple_configuration_slug/Build/Products"
ios_xctestrun=""
macos_xctestrun=""
ACTIVE_PATCHED_XCTESTRUN=""

locate_apple_artifacts() {
  ios_xctestrun="$(find "$ios_products" -maxdepth 1 -name 'Envoix-iOS-Hosted_*.xctestrun' -print -quit 2>/dev/null)"
  macos_xctestrun="$(find "$macos_products" -maxdepth 1 -name 'Envoix-macOS-Hosted_*.xctestrun' -print -quit 2>/dev/null)"
  if [[ -z "$ios_xctestrun" ]]; then
    echo "error: missing iOS xctestrun artifact under $ios_products" >&2
    return 1
  fi
  if matrix_uses_macos && [[ -z "$macos_xctestrun" ]]; then
    echo "error: missing macOS xctestrun artifact under $macos_products" >&2
    return 1
  fi
}

set_xctestrun_environment() {
  local file="$1"
  local target="$2"
  local key="$3"
  local value="$4"
  local scope
  for scope in EnvironmentVariables TestingEnvironmentVariables; do
    /usr/libexec/PlistBuddy -c "Add :$target:$scope:$key string $value" "$file" >/dev/null 2>&1 \
      || /usr/libexec/PlistBuddy -c "Set :$target:$scope:$key $value" "$file" >/dev/null
  done
}

run_apple_method() {
  local platform="$1"
  local method="$2"
  local scenario="$3"
  local invitation="$4"
  local run_id="$5"
  local case_id="$6"
  local repetition="$7"
  local role="$8"
  local private_case_dir="$9"
  local log_file="${10}"
  local endpoint_role="sender"
  local source_xctestrun target destination product_directory derived_data patched result_bundle status=0

  [[ "$role" == "receive" ]] && endpoint_role="receiver"

  if [[ "$platform" == "ios" ]]; then
    source_xctestrun="$ios_xctestrun"
    target="Envoix-iOSUITests"
    destination="$ios_destination"
    product_directory="$ios_products"
    derived_data="$apple_cache_root/ios-simulator-$apple_configuration_slug"
  else
    source_xctestrun="$macos_xctestrun"
    target="Envoix-macOSUITests"
    destination="$macos_destination"
    product_directory="$macos_products"
    derived_data="$apple_cache_root/macos-$apple_configuration_slug"
  fi

  patched="$product_directory/.envoix-matrix-$run_id-$platform-$method.xctestrun"
  result_bundle="$private_case_dir/apple-$endpoint_role.xcresult"
  if [[ -e "$patched" || -e "$result_bundle" ]]; then
    echo "error: refusing to overwrite existing Apple matrix test artifact" > "$log_file"
    return 20
  fi
  cp "$source_xctestrun" "$patched"
  ACTIVE_PATCHED_XCTESTRUN="$patched"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE 1
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_RUN_ID "$run_id"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_CASE_ID "$case_id"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_REPETITION "$repetition"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_BUILD_VARIANT "$build_variant"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_SCENARIO "$scenario"
  if [[ -n "$invitation" ]]; then
    set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_INVITATION "$invitation"
  fi
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_LARGE_BYTES "$large_bytes"
  set_xctestrun_environment "$patched" "$target" ENVOIX_LOG "envoix=info,iroh=warn,warn"

  xcodebuild \
    test-without-building \
    -xctestrun "$patched" \
    -destination "$destination" \
    -derivedDataPath "$derived_data" \
    -parallel-testing-enabled NO \
    -only-testing:"$target/ManifestV2PhysicalTransferTests/$method" \
    -resultBundlePath "$result_bundle" \
    > "$log_file" 2>&1 || status=$?
  rm -f "$patched"
  ACTIVE_PATCHED_XCTESTRUN=""

  if [[ "$status" -ne 0 ]]; then
    if grep -Eq 'Executed 1 test, with [1-9][0-9]* failures?' "$log_file"; then
      return 10
    fi
    return 20
  fi
  if ! grep -Eq 'Executed 1 test, with 0 failures' "$log_file"; then
    return 20
  fi
}

run_android_method() {
  local method="$1"
  local scenario="$2"
  local invitation="$3"
  local run_id="$4"
  local case_id="$5"
  local repetition="$6"
  local log_file="$7"
  local status=0
  local -a invitation_args=()

  if [[ -n "$invitation" ]]; then
    invitation_args=(-e envoixCrossDeviceInvitation "$invitation")
  fi

  adb_command shell am instrument -w \
    -e envoixCrossDevice 1 \
    -e envoixCrossDeviceRunId "$run_id" \
    -e envoixCrossDeviceCaseId "$case_id" \
    -e envoixCrossDeviceRepetition "$repetition" \
    -e envoixCrossDeviceBuildVariant "$build_variant" \
    -e envoixCrossDeviceScenario "$scenario" \
    ${invitation_args[@]+"${invitation_args[@]}"} \
    -e envoixCrossDeviceLargeBytes "$large_bytes" \
    -e envoixCrossDeviceTimeoutMs "$transfer_timeout_ms" \
    -e class "dev.envoix.app.ManifestV2CrossDeviceInstrumentedTest#$method" \
    "$test_runner" > "$log_file" 2>&1 || status=$?

  if grep -Eq '^FAILURES!!!|^Tests run: [1-9][0-9]*, +Failures: [1-9][0-9]*' "$log_file"; then
    return 10
  fi
  if [[ "$status" -ne 0 ]] || ! grep -Eq '^OK \(1 test\)' "$log_file"; then
    return 20
  fi
}

run_endpoint_role() {
  local platform="$1"
  local role="$2"
  local scenario="$3"
  local test_layer="$4"
  local invitation="$5"
  local run_id="$6"
  local case_id="$7"
  local repetition="$8"
  local log_file="$9"
  local method private_case_dir
  private_case_dir="$(dirname "$log_file")"

  if [[ "$role" == "send" ]]; then
    method="sendScenarioManifestV2Room"
    [[ "$platform" != "android" ]] && method="testSendScenarioManifestV2Room"
    if [[ "$test_layer" == "l2_physical" ]]; then
      method="sendScenarioProductActivityRoom"
      [[ "$platform" != "android" ]] && method="testSendScenarioProductActivityRoom"
    fi
  else
    method="receiveScenarioManifestV2Room"
    [[ "$platform" != "android" ]] && method="testReceiveScenarioManifestV2Room"
    if [[ "$test_layer" == "l2_physical" ]]; then
      method="receiveScenarioProductActivityRoom"
      [[ "$platform" != "android" ]] && method="testReceiveScenarioProductActivityRoom"
    fi
  fi

  if [[ "$platform" == "android" ]]; then
    run_android_method \
      "$method" "$scenario" "$invitation" "$run_id" "$case_id" "$repetition" "$log_file"
  else
    run_apple_method \
      "$platform" "$method" "$scenario" "$invitation" "$run_id" \
      "$case_id" "$repetition" "$role" "$private_case_dir" "$log_file"
  fi
}

ENDPOINT_EVIDENCE_ERROR=""

collect_android_evidence() {
  local role="$1"
  local run_id="$2"
  local case_id="$3"
  local repetition="$4"
  local private_case_dir="$5"
  local endpoint_role="sender"
  local app_path private_path public_path
  local read_status=0 validation_status=0 cleanup_status=0

  [[ "$role" == "receive" ]] && endpoint_role="receiver"
  app_path="files/envoix-matrix/$run_id/$case_id/$endpoint_role.json"
  private_path="$private_case_dir/android-$endpoint_role.json"
  public_path="$output_dir/cases/$case_id/r$repetition/$endpoint_role.json"

  adb_command exec-out run-as dev.envoix.app cat "$app_path" \
    > "$private_path" 2>/dev/null || read_status=$?
  if [[ "$read_status" -ne 0 || ! -s "$private_path" ]]; then
    ENDPOINT_EVIDENCE_ERROR="missing_android_endpoint_result"
    validation_status=1
  elif ! python3 "$contract" validate-endpoint-result "$private_path" \
    --run-id "$run_id" \
    --case "$case_id" \
    --repetition "$repetition" \
    --role "$endpoint_role" \
    --platform android \
    --output "$public_path"; then
    ENDPOINT_EVIDENCE_ERROR="invalid_android_endpoint_result"
    validation_status=1
  else
    rm -f "$private_path"
  fi

  adb_command shell run-as dev.envoix.app \
    rm -f "$app_path" "files/envoix-matrix/$run_id/$case_id/.$endpoint_role.json.tmp" \
    >/dev/null 2>&1 || cleanup_status=1
  adb_command shell run-as dev.envoix.app \
    rmdir "files/envoix-matrix/$run_id/$case_id" \
    >/dev/null 2>&1 || true
  adb_command shell run-as dev.envoix.app \
    rmdir "files/envoix-matrix/$run_id" \
    >/dev/null 2>&1 || true
  if [[ "$cleanup_status" -ne 0 ]]; then
    ENDPOINT_EVIDENCE_ERROR="android_endpoint_cleanup_failed"
    return 1
  fi
  return "$validation_status"
}

collect_apple_evidence() {
  local platform="$1"
  local role="$2"
  local run_id="$3"
  local case_id="$4"
  local repetition="$5"
  local private_case_dir="$6"
  local endpoint_role="sender"
  local result_bundle export_directory export_log private_path public_path
  local extraction_status=0 validation_status=0

  [[ "$role" == "receive" ]] && endpoint_role="receiver"
  result_bundle="$private_case_dir/apple-$endpoint_role.xcresult"
  export_directory="$private_case_dir/apple-$endpoint_role-attachments"
  export_log="$private_case_dir/apple-$endpoint_role-attachment-export.log"
  private_path="$private_case_dir/apple-$endpoint_role.json"
  public_path="$output_dir/cases/$case_id/r$repetition/$endpoint_role.json"

  if [[ ! -d "$result_bundle" ]]; then
    ENDPOINT_EVIDENCE_ERROR="missing_apple_result_bundle"
    return 1
  fi
  if [[ -e "$export_directory" ]]; then
    ENDPOINT_EVIDENCE_ERROR="apple_attachment_export_conflict"
    return 1
  fi
  if ! xcrun xcresulttool export attachments \
      --path "$result_bundle" \
      --output-path "$export_directory" \
      > "$export_log" 2>&1; then
    ENDPOINT_EVIDENCE_ERROR="apple_attachment_export_failed"
    return 1
  fi
  python3 "$apple_evidence" "$export_directory" \
    --run-id "$run_id" \
    --case "$case_id" \
    --repetition "$repetition" \
    --role "$endpoint_role" \
    --platform "$platform" \
    --output "$private_path" || extraction_status=1
  if [[ "$extraction_status" -ne 0 || ! -s "$private_path" ]]; then
    ENDPOINT_EVIDENCE_ERROR="missing_apple_endpoint_result"
    validation_status=1
  elif ! python3 "$contract" validate-endpoint-result "$private_path" \
    --run-id "$run_id" \
    --case "$case_id" \
    --repetition "$repetition" \
    --role "$endpoint_role" \
    --platform "$platform" \
    --output "$public_path"; then
    ENDPOINT_EVIDENCE_ERROR="invalid_apple_endpoint_result"
    validation_status=1
  fi

  if [[ "$validation_status" -eq 0 ]]; then
    rm -f "$private_path" "$export_log"
    rm -rf -- "$result_bundle" "$export_directory"
  fi
  return "$validation_status"
}

remove_android_evidence_files() {
  local run_id="$1"
  local case_id="$2"
  local directory="files/envoix-matrix/$run_id/$case_id"

  adb_command shell run-as dev.envoix.app rm -f \
    "$directory/sender.json" \
    "$directory/receiver.json" \
    "$directory/.sender.json.tmp" \
    "$directory/.receiver.json.tmp" \
    >/dev/null 2>&1 || true
  adb_command shell run-as dev.envoix.app rmdir "$directory" \
    >/dev/null 2>&1 || true
  adb_command shell run-as dev.envoix.app rmdir "files/envoix-matrix/$run_id" \
    >/dev/null 2>&1 || true
}

remove_endpoint_patch() {
  local platform="$1"
  local role="$2"
  local run_id="$3"
  local test_layer="$4"
  local method product_directory
  [[ "$platform" == "android" ]] && return
  if [[ "$role" == "send" ]]; then
    method="testSendScenarioManifestV2Room"
    [[ "$test_layer" == "l2_physical" ]] && method="testSendScenarioProductActivityRoom"
  else
    method="testReceiveScenarioManifestV2Room"
    [[ "$test_layer" == "l2_physical" ]] && method="testReceiveScenarioProductActivityRoom"
  fi
  if [[ "$platform" == "ios" ]]; then
    product_directory="$ios_products"
  else
    product_directory="$macos_products"
  fi
  rm -f "$product_directory/.envoix-matrix-$run_id-$platform-$method.xctestrun"
}

ANDROID_LOGCAT_PID=""
ACTIVE_RECEIVER_PID=""
ACTIVE_RECEIVER_PLATFORM=""
ACTIVE_RECEIVER_RUN_ID=""
ACTIVE_RECEIVER_TEST_LAYER=""
ACTIVE_CASE_ID=""

start_android_logcat() {
  local log_file="$1"
  adb_command logcat -c >/dev/null 2>&1 || true
  adb_command logcat -v threadtime EnvoixCrossDevice:V Envoix:V AndroidRuntime:E '*:S' \
    > "$log_file" 2>&1 &
  ANDROID_LOGCAT_PID=$!
}

stop_android_logcat() {
  if [[ -n "$ANDROID_LOGCAT_PID" ]]; then
    kill "$ANDROID_LOGCAT_PID" >/dev/null 2>&1 || true
    wait "$ANDROID_LOGCAT_PID" >/dev/null 2>&1 || true
    ANDROID_LOGCAT_PID=""
  fi
}

stop_android_tests() {
  adb_command shell am force-stop dev.envoix.app >/dev/null 2>&1 || true
  adb_command shell am force-stop dev.envoix.app.test >/dev/null 2>&1 || true
}

cleanup_runner() {
  if [[ -n "$ACTIVE_PATCHED_XCTESTRUN" ]]; then
    rm -f "$ACTIVE_PATCHED_XCTESTRUN"
  fi
  if [[ -n "$ACTIVE_RECEIVER_PID" ]]; then
    kill "$ACTIVE_RECEIVER_PID" >/dev/null 2>&1 || true
    wait "$ACTIVE_RECEIVER_PID" >/dev/null 2>&1 || true
    remove_endpoint_patch \
      "$ACTIVE_RECEIVER_PLATFORM" receive "$ACTIVE_RECEIVER_RUN_ID" \
      "$ACTIVE_RECEIVER_TEST_LAYER"
  fi
  if [[ -n "$ACTIVE_RECEIVER_RUN_ID" && -n "$ACTIVE_CASE_ID" ]]; then
    remove_android_evidence_files "$ACTIVE_RECEIVER_RUN_ID" "$ACTIVE_CASE_ID"
  fi
  stop_android_tests
  stop_android_logcat
}

wait_for_log() {
  local file="$1"
  local pattern="$2"
  local waited=0
  while [[ "$waited" -lt "$ready_timeout" ]]; do
    if [[ -f "$file" ]] && grep -Eq "$pattern" "$file"; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

run_pair() {
  local sender="$1"
  local receiver="$2"
  local scenario="$3"
  local test_layer="$4"
  local invitation="$5"
  local run_id="$6"
  local case_id="$7"
  local repetition="$8"
  local private_case_dir="$9"
  local sender_log="$private_case_dir/sender.log"
  local receiver_log="$private_case_dir/receiver.log"
  local logcat_log="$private_case_dir/android-logcat.log"
  local ready_log ready_pattern published_invitation receiver_pid
  local sender_status=0 receiver_status=0 sanitization_status=0 evidence_status=0

  LAST_FAILURE_STATUS=""
  LAST_FAILURE_CODE=""
  ENDPOINT_EVIDENCE_ERROR=""
  mkdir -p "$private_case_dir"

  if [[ "$sender" == "android" || "$receiver" == "android" ]]; then
    start_android_logcat "$logcat_log"
  fi

  run_endpoint_role \
    "$receiver" receive "$scenario" "$test_layer" "$invitation" "$run_id" \
    "$case_id" "$repetition" "$receiver_log" &
  receiver_pid=$!
  ACTIVE_RECEIVER_PID="$receiver_pid"
  ACTIVE_RECEIVER_PLATFORM="$receiver"
  ACTIVE_RECEIVER_RUN_ID="$run_id"
  ACTIVE_RECEIVER_TEST_LAYER="$test_layer"
  ACTIVE_CASE_ID="$case_id"
  case "$receiver" in
    android)
      ready_log="$logcat_log"
      ready_pattern="EnvoixCrossDevice:.*Android receiver ready scenario=$scenario"
      ;;
    ios)
      ready_log="$receiver_log"
      ready_pattern="\\[cross-device\\] iOS receiver ready scenario=$scenario"
      ;;
    macos)
      ready_log="$receiver_log"
      ready_pattern="\\[cross-device\\] macOS receiver ready scenario=$scenario"
      ;;
  esac

  if ! wait_for_log "$ready_log" "$ready_pattern"; then
    echo "fail: receiver did not become ready for $case_id" >&2
    print_log_tail "$receiver receiver" "$receiver_log"
    [[ -f "$logcat_log" ]] && print_log_tail "Android logcat" "$logcat_log"
    kill "$receiver_pid" >/dev/null 2>&1 || true
    wait "$receiver_pid" >/dev/null 2>&1 || true
    ACTIVE_RECEIVER_PID=""
    remove_endpoint_patch "$receiver" receive "$run_id" "$test_layer"
    stop_android_tests
    stop_android_logcat
    remove_android_evidence_files "$run_id" "$case_id"
    ACTIVE_CASE_ID=""
    sanitize_log "$sender_log"
    sanitize_log "$receiver_log"
    sanitize_log "$logcat_log"
    LAST_FAILURE_STATUS="infrastructure_failure"
    LAST_FAILURE_CODE="receiver_not_ready"
    return 1
  fi
  if ! wait_for_log "$ready_log" '\[cross-device\] invitation=[^[:space:]]+'; then
    echo "fail: receiver did not publish its complete InviteV2 URI for $case_id" >&2
    print_log_tail "$receiver receiver" "$receiver_log"
    [[ -f "$logcat_log" ]] && print_log_tail "Android logcat" "$logcat_log"
    kill "$receiver_pid" >/dev/null 2>&1 || true
    wait "$receiver_pid" >/dev/null 2>&1 || true
    ACTIVE_RECEIVER_PID=""
    remove_endpoint_patch "$receiver" receive "$run_id" "$test_layer"
    stop_android_tests
    stop_android_logcat
    remove_android_evidence_files "$run_id" "$case_id"
    ACTIVE_CASE_ID=""
    sanitize_log "$sender_log"
    sanitize_log "$receiver_log"
    sanitize_log "$logcat_log"
    LAST_FAILURE_STATUS="infrastructure_failure"
    LAST_FAILURE_CODE="invitation_not_published"
    return 1
  fi
  published_invitation="$(
    sed -nE 's/.*\[cross-device\] invitation=([^[:space:]]+).*/\1/p' "$ready_log" |
      tail -n 1
  )"
  if ! [[ "$published_invitation" =~ ^envoix://invite/v2/[^[:space:]]+$ ]]; then
    echo "fail: receiver published an unreadable InviteV2 URI for $case_id" >&2
    kill "$receiver_pid" >/dev/null 2>&1 || true
    wait "$receiver_pid" >/dev/null 2>&1 || true
    ACTIVE_RECEIVER_PID=""
    remove_endpoint_patch "$receiver" receive "$run_id" "$test_layer"
    stop_android_tests
    stop_android_logcat
    remove_android_evidence_files "$run_id" "$case_id"
    ACTIVE_CASE_ID=""
    sanitize_log "$sender_log"
    sanitize_log "$receiver_log"
    sanitize_log "$logcat_log"
    LAST_FAILURE_STATUS="infrastructure_failure"
    LAST_FAILURE_CODE="invitation_unreadable"
    return 1
  fi

  if [[ "$receiver_settle_seconds" -gt 0 ]]; then
    sleep "$receiver_settle_seconds"
  fi
  run_endpoint_role \
    "$sender" send "$scenario" "$test_layer" "$published_invitation" "$run_id" \
    "$case_id" "$repetition" "$sender_log" ||
    sender_status=$?
  published_invitation=""
  if [[ "$sender_status" -ne 0 ]]; then
    kill "$receiver_pid" >/dev/null 2>&1 || true
  fi
  wait "$receiver_pid" || receiver_status=$?
  ACTIVE_RECEIVER_PID=""
  remove_endpoint_patch "$sender" send "$run_id" "$test_layer"
  remove_endpoint_patch "$receiver" receive "$run_id" "$test_layer"
  if [[ "$sender" == "android" ]]; then
    collect_android_evidence \
      send "$run_id" "$case_id" "$repetition" "$private_case_dir" || evidence_status=1
  else
    collect_apple_evidence \
      "$sender" send "$run_id" "$case_id" "$repetition" "$private_case_dir" || evidence_status=1
  fi
  if [[ "$receiver" == "android" ]]; then
    collect_android_evidence \
      receive "$run_id" "$case_id" "$repetition" "$private_case_dir" || evidence_status=1
  else
    collect_apple_evidence \
      "$receiver" receive "$run_id" "$case_id" "$repetition" "$private_case_dir" || evidence_status=1
  fi
  ACTIVE_CASE_ID=""
  stop_android_tests
  stop_android_logcat
  sanitize_log "$sender_log" || sanitization_status=1
  sanitize_log "$receiver_log" || sanitization_status=1
  sanitize_log "$logcat_log" || sanitization_status=1

  if [[ "$sanitization_status" -ne 0 ]]; then
    LAST_FAILURE_STATUS="infrastructure_failure"
    LAST_FAILURE_CODE="log_sanitization_failed"
    return 1
  fi

  if [[ "$evidence_status" -ne 0 ]]; then
    LAST_FAILURE_STATUS="infrastructure_failure"
    LAST_FAILURE_CODE="$ENDPOINT_EVIDENCE_ERROR"
    return 1
  fi

  if [[ "$sender_status" -ne 0 || "$receiver_status" -ne 0 ]]; then
    print_log_tail "$sender sender" "$sender_log"
    print_log_tail "$receiver receiver" "$receiver_log"
    [[ -f "$logcat_log" ]] && print_log_tail "Android logcat" "$logcat_log"
    if [[ "$sender_status" -eq 10 || "$receiver_status" -eq 10 ]]; then
      LAST_FAILURE_STATUS="product_failure"
      LAST_FAILURE_CODE="endpoint_assertion_failed"
    else
      LAST_FAILURE_STATUS="infrastructure_failure"
      LAST_FAILURE_CODE="endpoint_runner_failed"
    fi
    return 1
  fi
  rm -f "$sender_log" "$receiver_log" "$logcat_log"
}

case_index=0
CURRENT_RUN_ID=""
CURRENT_INVITATION=""
LAST_FAILURE_STATUS=""
LAST_FAILURE_CODE=""

next_execution() {
  local repetition="$1"
  case_index=$((case_index + 1))
  CURRENT_RUN_ID="$base_run_id-c$case_index-r$repetition"
  # The receiver publishes the complete InviteV2 URI before its sender starts.
  CURRENT_INVITATION=""
}

record_execution() {
  local case_id="$1"
  local repetition="$2"
  local status="$3"
  local failure_code="${4:-}"
  local record_path="$output_dir/cases/$case_id/r$repetition/result.json"
  local sanitized_dir="$output_dir/sanitized/cases/$case_id/r$repetition"
  local args=(
    record-result
    --run-id "$base_run_id"
    --case "$case_id"
    --repetition "$repetition"
    --status "$status"
    --output "$record_path"
  )
  local log
  [[ -n "$failure_code" ]] && args+=(--failure-code "$failure_code")
  for log in sender.log receiver.log android-logcat.log; do
    if [[ -f "$sanitized_dir/$log" ]]; then
      args+=(--sanitized-log "sanitized/cases/$case_id/r$repetition/$log")
    fi
  done
  local endpoint
  for endpoint in sender.json receiver.json; do
    if [[ -f "$output_dir/cases/$case_id/r$repetition/$endpoint" ]]; then
      args+=(--endpoint-result "cases/$case_id/r$repetition/$endpoint")
    fi
  done
  python3 "$contract" "${args[@]}"
}

aggregate_and_check() {
  local aggregate_status=0
  local public_files=()
  python3 "$contract" aggregate-run \
    "$registry" "$plan_file" "$output_dir/cases" \
    --json-output "$result_file" \
    --report-output "$report_file" || aggregate_status=$?
  while IFS= read -r file; do
    public_files+=("$file")
  done < <(
    find "$output_dir" \
      -path "$output_dir/private" -prune -o \
      -type f -print
  )
  python3 "$contract" redaction-check "${public_files[@]}" || return 1
  return "$aggregate_status"
}

execution_rows=()
while IFS= read -r row; do
  execution_rows+=("$row")
done < <(python3 "$contract" list-executions "$registry" "$plan_file")

MATRIX_USES_MACOS=0
requires_execution=0
for row in "${execution_rows[@]}"; do
  IFS=$'\t' read -r _ _ sender receiver _ _ _ disposition <<< "$row"
  if [[ "$disposition" == "execute" ]]; then
    requires_execution=1
    if [[ "$sender" == "macos" || "$receiver" == "macos" ]]; then
      MATRIX_USES_MACOS=1
    fi
  fi
done

if [[ "$dry_run" == "1" || "$requires_execution" == "0" ]]; then
  for row in "${execution_rows[@]}"; do
    IFS=$'\t' read -r case_id repetition _ _ _ _ _ disposition <<< "$row"
    record_execution "$case_id" "$repetition" "$disposition"
  done
  if aggregate_and_check; then
    echo "matrix plan complete: no physical executions"
    echo "report: $report_file"
    exit 0
  fi
  exit 1
fi

trap cleanup_runner EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -z "$ios_destination" ]]; then
  echo "error: ENVOIX_IOS_DESTINATION or --ios-destination must identify the physical iPhone" >&2
  preparation_failure="ios_destination_missing"
elif ! prepare_builds; then
  echo "error: matrix build/deploy phase failed; private logs remain in $output_dir/private" >&2
  preparation_failure="build_or_deploy_failed"
elif ! locate_apple_artifacts; then
  preparation_failure="apple_artifacts_missing"
else
  preparation_failure=""
fi

if [[ -n "$preparation_failure" ]]; then
  for row in "${execution_rows[@]}"; do
    IFS=$'\t' read -r case_id repetition _ _ _ _ _ disposition <<< "$row"
    if [[ "$disposition" == "execute" ]]; then
      record_execution \
        "$case_id" "$repetition" infrastructure_failure "$preparation_failure"
    else
      record_execution "$case_id" "$repetition" "$disposition"
    fi
  done
  aggregate_and_check || true
  exit 1
fi

for row in "${execution_rows[@]}"; do
  IFS=$'\t' read -r \
    case_id repetition sender receiver scenario timeout_seconds test_layer disposition <<< "$row"
  if [[ "$disposition" != "execute" ]]; then
    record_execution "$case_id" "$repetition" "$disposition"
    continue
  fi

  next_execution "$repetition"
  transfer_timeout_ms=$((timeout_seconds * 1000))
  if [[ -n "$transfer_timeout_override_ms" ]]; then
    if [[ "$transfer_timeout_override_ms" -gt "$transfer_timeout_ms" ]]; then
      echo "error: ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS exceeds the registry timeout for $case_id" >&2
      record_execution \
        "$case_id" "$repetition" infrastructure_failure invalid_timeout_override
      continue
    fi
    transfer_timeout_ms="$transfer_timeout_override_ms"
  fi
  private_case_dir="$output_dir/private/cases/$case_id/r$repetition"
  echo "case $case_id r$repetition: $sender -> $receiver, scenario=$scenario"
  if run_pair "$sender" "$receiver" "$scenario" "$test_layer" \
    "$CURRENT_INVITATION" "$CURRENT_RUN_ID" "$case_id" "$repetition" "$private_case_dir"; then
    record_execution "$case_id" "$repetition" pass
    echo "pass: $case_id r$repetition"
  else
    record_execution \
      "$case_id" "$repetition" "$LAST_FAILURE_STATUS" "$LAST_FAILURE_CODE"
    echo "fail: $case_id r$repetition ($LAST_FAILURE_STATUS)" >&2
  fi
done

if aggregate_and_check; then
  echo "matrix complete"
  echo "report: $report_file"
  exit 0
fi
echo "matrix failed; report: $report_file" >&2
exit 1
