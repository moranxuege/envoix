#!/usr/bin/env bash
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
android_dir="$repo_root/android"

if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" == "1" \
      && "${ENVOIX_BUILD_LEASE_MODE:-writer}" == "reader" ]]; then
  echo "error: the transfer matrix needs a writer build lease" >&2
  exit 3
fi
if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
  exec "$repo_root/scripts/with-build-cache-guard.sh" "$0" "$@"
fi

adb_bin="${ADB:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb}"
adb_serial="${ANDROID_SERIAL:-}"
android_home="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
apple_cache_root="${ENVOIX_APPLE_CACHE_ROOT:-${TMPDIR:-/tmp}/envoix-apple-cache}"
ios_destination="${ENVOIX_IOS_DESTINATION:-}"
macos_destination="platform=macOS"
repeat_count="${ENVOIX_MATRIX_REPEAT:-2}"
large_bytes="${ENVOIX_MATRIX_LARGE_BYTES:-134217728}"
ready_timeout="${ENVOIX_MATRIX_READY_TIMEOUT_SECONDS:-120}"
transfer_timeout_ms="${ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS:-600000}"
receiver_settle_seconds="${ENVOIX_MATRIX_RECEIVER_SETTLE_SECONDS:-1}"
base_run_id="${ENVOIX_MATRIX_RUN_ID:-$(date +%Y%m%d-%H%M%S)-$$}"
scenario_text="${ENVOIX_MATRIX_SCENARIOS:-single_file multiple_files folder multiple_folders image large_file collision overlap unicode_empty same_name_roots share}"
direction_text="${ENVOIX_MATRIX_DIRECTIONS:-android:ios ios:android android:macos macos:android ios:macos macos:ios}"
skip_build="${ENVOIX_SKIP_BUILD:-0}"
log_dir="${ENVOIX_MATRIX_LOG_DIR:-${TMPDIR:-/tmp}/envoix-transfer-matrix-$base_run_id}"
results_file="$log_dir/results.tsv"
test_runner="dev.envoix.app.test/androidx.test.runner.AndroidJUnitRunner"
main_apk="$android_dir/app/build/outputs/apk/debug/app-debug.apk"
test_apk="$android_dir/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"

read -r -a scenarios <<< "$scenario_text"
read -r -a directions <<< "$direction_text"

usage() {
  cat <<'EOF'
Usage: scripts/cross-device-transfer-matrix.sh

Runs actual Manifest-v2 destination writes for Android, iPhone, and Mac in all
six directed pairs. Every selected case is repeated twice by default.

Environment:
  ENVOIX_MATRIX_SCENARIOS="single_file image ..."  Override scenario set
  ENVOIX_MATRIX_DIRECTIONS="android:ios ..."       Override directed pairs
  ENVOIX_MATRIX_REPEAT=2                            Required consecutive runs
  ENVOIX_MATRIX_LARGE_BYTES=134217728               Large fixture size
  ENVOIX_IOS_DESTINATION="platform=iOS,id=..."      Required physical iPhone
  ANDROID_SERIAL=...                                Physical Android device
  ENVOIX_SKIP_BUILD=1                               Reuse existing artifacts
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi
if [[ "$#" -ne 0 ]]; then
  usage >&2
  exit 2
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

require_positive_integer ENVOIX_MATRIX_REPEAT "$repeat_count"
require_positive_integer ENVOIX_MATRIX_LARGE_BYTES "$large_bytes"
require_positive_integer ENVOIX_MATRIX_READY_TIMEOUT_SECONDS "$ready_timeout"
require_positive_integer ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS "$transfer_timeout_ms"
require_non_negative_integer ENVOIX_MATRIX_RECEIVER_SETTLE_SECONDS "$receiver_settle_seconds"
if [[ "$skip_build" != "0" && "$skip_build" != "1" ]]; then
  echo "error: ENVOIX_SKIP_BUILD must be 0 or 1" >&2
  exit 2
fi
if [[ -z "$ios_destination" ]]; then
  echo "error: ENVOIX_IOS_DESTINATION must identify the physical iPhone" >&2
  exit 2
fi
if [[ ! "$base_run_id" =~ ^[A-Za-z0-9_-]+$ || "${#base_run_id}" -gt 48 ]]; then
  echo "error: ENVOIX_MATRIX_RUN_ID must be at most 48 letters, digits, '-' or '_'" >&2
  exit 2
fi

for scenario in "${scenarios[@]}"; do
  case "$scenario" in
    single_file|multiple_files|folder|multiple_folders|image|large_file|collision|overlap|unicode_empty|same_name_roots|share)
      ;;
    *)
      echo "error: unsupported scenario '$scenario'" >&2
      exit 2
      ;;
  esac
done
for direction in "${directions[@]}"; do
  case "$direction" in
    android:ios|ios:android|android:macos|macos:android|ios:macos|macos:ios)
      ;;
    *)
      echo "error: unsupported direction '$direction'" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$log_dir"
printf 'case\tkind\tsender\treceiver\tscenario\trepetition\tstatus\n' > "$results_file"

adb_command() {
  if [[ -n "$adb_serial" ]]; then
    "$adb_bin" -s "$adb_serial" "$@"
  else
    "$adb_bin" "$@"
  fi
}

matrix_uses_macos() {
  local direction
  for direction in "${directions[@]}"; do
    if [[ "$direction" == macos:* || "$direction" == *:macos ]]; then
      return 0
    fi
  done
  return 1
}

print_log_tail() {
  local title="$1"
  local file="$2"
  echo "--- $title: $file ---" >&2
  if [[ -f "$file" ]]; then
    tail -n 120 "$file" >&2
  else
    echo "missing log" >&2
  fi
}

prepare_builds() {
  if [[ ! -x "$adb_bin" ]]; then
    echo "error: adb not found at $adb_bin" >&2
    return 1
  fi
  if ! adb_command get-state >/dev/null 2>&1; then
    echo "error: no usable Android device; set ANDROID_SERIAL if more than one is attached" >&2
    return 1
  fi

  if [[ "$skip_build" != "1" ]]; then
    echo "build: Android application and instrumented tests"
    if ! (
      cd "$android_dir"
      env ANDROID_HOME="$android_home" ANDROID_SDK_ROOT="$android_home" \
        ./gradlew :app:ktlintCheck :app:assembleDebug :app:assembleDebugAndroidTest --no-daemon
    ); then
      return 1
    fi

    echo "build: iPhone hosted tests"
    if ! env ENVOIX_APPLE_CACHE_ROOT="$apple_cache_root" ENVOIX_IOS_SIM_DESTINATION="$ios_destination" \
      "$repo_root/scripts/apple-dev.sh" ios-test-build hosted -quiet; then
      return 1
    fi

    if matrix_uses_macos; then
      echo "build: Mac hosted tests"
      if ! env ENVOIX_APPLE_CACHE_ROOT="$apple_cache_root" \
        "$repo_root/scripts/apple-dev.sh" macos-test-build -quiet; then
        return 1
      fi
    fi
  fi

  echo "deploy: Android application and test APK"
  adb_command shell input keyevent WAKEUP >/dev/null 2>&1 || true
  adb_command shell wm dismiss-keyguard >/dev/null 2>&1 || true
  adb_command shell svc power stayon usb >/dev/null 2>&1 || true
  if ! adb_command install -r -d -t -g "$main_apk" > "$log_dir/android-main.install.log" 2>&1; then
    print_log_tail "Android main APK install" "$log_dir/android-main.install.log"
    return 1
  fi
  if ! adb_command install -r -d -t -g "$test_apk" > "$log_dir/android-test.install.log" 2>&1; then
    print_log_tail "Android test APK install" "$log_dir/android-test.install.log"
    return 1
  fi
  adb_command shell pm grant dev.envoix.app android.permission.CAMERA >/dev/null 2>&1 || true
  adb_command shell pm grant dev.envoix.app android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
}

ios_products="$apple_cache_root/ios-simulator-debug/Build/Products"
macos_products="$apple_cache_root/macos-debug/Build/Products"
ios_xctestrun=""
macos_xctestrun=""

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
  local code="$4"
  local run_id="$5"
  local log_file="$6"
  local source_xctestrun target destination product_directory derived_data patched status=0

  if [[ "$platform" == "ios" ]]; then
    source_xctestrun="$ios_xctestrun"
    target="Envoix-iOSUITests"
    destination="$ios_destination"
    product_directory="$ios_products"
    derived_data="$apple_cache_root/ios-simulator-debug"
  else
    source_xctestrun="$macos_xctestrun"
    target="Envoix-macOSUITests"
    destination="$macos_destination"
    product_directory="$macos_products"
    derived_data="$apple_cache_root/macos-debug"
  fi

  patched="$product_directory/.envoix-matrix-$run_id-$platform-$method.xctestrun"
  cp "$source_xctestrun" "$patched"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE 1
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_RUN_ID "$run_id"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_SCENARIO "$scenario"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_CODE "$code"
  set_xctestrun_environment "$patched" "$target" ENVOIX_CROSS_DEVICE_LARGE_BYTES "$large_bytes"
  set_xctestrun_environment "$patched" "$target" ENVOIX_LOG "envoix=info,iroh=warn,warn"

  xcodebuild \
    test-without-building \
    -xctestrun "$patched" \
    -destination "$destination" \
    -derivedDataPath "$derived_data" \
    -parallel-testing-enabled NO \
    -only-testing:"$target/ManifestV2PhysicalTransferTests/$method" \
    > "$log_file" 2>&1 || status=$?
  rm -f "$patched"

  if [[ "$status" -ne 0 ]] || ! grep -Eq 'Executed 1 test, with 0 failures' "$log_file"; then
    return 1
  fi
}

run_android_method() {
  local method="$1"
  local scenario="$2"
  local code="$3"
  local run_id="$4"
  local log_file="$5"
  local status=0

  adb_command shell am instrument -w \
    -e envoixCrossDevice 1 \
    -e envoixCrossDeviceRunId "$run_id" \
    -e envoixCrossDeviceScenario "$scenario" \
    -e envoixCrossDeviceCode "$code" \
    -e envoixCrossDeviceLargeBytes "$large_bytes" \
    -e envoixCrossDeviceTimeoutMs "$transfer_timeout_ms" \
    -e class "dev.envoix.app.ManifestV2CrossDeviceInstrumentedTest#$method" \
    "$test_runner" > "$log_file" 2>&1 || status=$?

  if [[ "$status" -ne 0 ]] || ! grep -Eq '^OK \(1 test\)' "$log_file"; then
    return 1
  fi
}

run_endpoint_role() {
  local platform="$1"
  local role="$2"
  local scenario="$3"
  local code="$4"
  local run_id="$5"
  local log_file="$6"
  local method

  if [[ "$role" == "send" ]]; then
    method="sendScenarioManifestV2Room"
    [[ "$platform" != "android" ]] && method="testSendScenarioManifestV2Room"
  else
    method="receiveScenarioManifestV2Room"
    [[ "$platform" != "android" ]] && method="testReceiveScenarioManifestV2Room"
  fi

  if [[ "$platform" == "android" ]]; then
    run_android_method "$method" "$scenario" "$code" "$run_id" "$log_file"
  else
    run_apple_method "$platform" "$method" "$scenario" "$code" "$run_id" "$log_file"
  fi
}

remove_endpoint_patch() {
  local platform="$1"
  local role="$2"
  local run_id="$3"
  local method product_directory
  [[ "$platform" == "android" ]] && return
  if [[ "$role" == "send" ]]; then
    method="testSendScenarioManifestV2Room"
  else
    method="testReceiveScenarioManifestV2Room"
  fi
  if [[ "$platform" == "ios" ]]; then
    product_directory="$ios_products"
  else
    product_directory="$macos_products"
  fi
  rm -f "$product_directory/.envoix-matrix-$run_id-$platform-$method.xctestrun"
}

ANDROID_LOGCAT_PID=""
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
  local code="$4"
  local run_id="$5"
  local case_id="$6"
  local sender_log="$log_dir/$case_id.sender.log"
  local receiver_log="$log_dir/$case_id.receiver.log"
  local logcat_log="$log_dir/$case_id.android.logcat.log"
  local ready_log ready_pattern receiver_pid sender_status=0 receiver_status=0

  if [[ "$sender" == "android" || "$receiver" == "android" ]]; then
    start_android_logcat "$logcat_log"
  fi

  run_endpoint_role "$receiver" receive "$scenario" "$code" "$run_id" "$receiver_log" &
  receiver_pid=$!
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
    remove_endpoint_patch "$receiver" receive "$run_id"
    stop_android_tests
    stop_android_logcat
    return 1
  fi

  if [[ "$receiver_settle_seconds" -gt 0 ]]; then
    sleep "$receiver_settle_seconds"
  fi
  run_endpoint_role "$sender" send "$scenario" "$code" "$run_id" "$sender_log" || sender_status=$?
  if [[ "$sender_status" -ne 0 ]]; then
    kill "$receiver_pid" >/dev/null 2>&1 || true
  fi
  wait "$receiver_pid" || receiver_status=$?
  remove_endpoint_patch "$sender" send "$run_id"
  remove_endpoint_patch "$receiver" receive "$run_id"
  stop_android_tests
  stop_android_logcat

  if [[ "$sender_status" -ne 0 || "$receiver_status" -ne 0 ]]; then
    print_log_tail "$sender sender" "$sender_log"
    print_log_tail "$receiver receiver" "$receiver_log"
    [[ -f "$logcat_log" ]] && print_log_tail "Android logcat" "$logcat_log"
    return 1
  fi
}

case_index=0
pass_count=0
fail_count=0
CURRENT_RUN_ID=""
CURRENT_CODE=""
CURRENT_CASE_ID=""

next_case() {
  local label="$1"
  local repetition="$2"
  case_index=$((case_index + 1))
  CURRENT_RUN_ID="$base_run_id-c$case_index-r$repetition"
  printf -v CURRENT_CODE '%06d-amber-comet' "$((800000 + case_index % 100000))"
  printf -v CURRENT_CASE_ID '%03d-%s-r%d' "$case_index" "$label" "$repetition"
}

record_result() {
  local kind="$1"
  local sender="$2"
  local receiver="$3"
  local scenario="$4"
  local repetition="$5"
  local status="$6"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$CURRENT_CASE_ID" "$kind" "$sender" "$receiver" "$scenario" "$repetition" "$status" \
    >> "$results_file"
  if [[ "$status" == "PASS" ]]; then
    pass_count=$((pass_count + 1))
  else
    fail_count=$((fail_count + 1))
  fi
}

run_local_share_recovery() {
  local repetition platform log_file status
  for repetition in $(seq 1 "$repeat_count"); do
    for platform in android ios; do
      next_case "$platform-share-recovery" "$repetition"
      log_file="$log_dir/$CURRENT_CASE_ID.local.log"
      echo "case $CURRENT_CASE_ID: $platform unreadable Share source -> valid source"
      status="FAIL"
      if [[ "$platform" == "android" ]]; then
        if run_android_method shareSourceFailureDoesNotPoisonNextSelection \
          single_file "$CURRENT_CODE" "$CURRENT_RUN_ID" "$log_file"; then
          status="PASS"
        fi
      elif run_apple_method ios testShareSourceFailureDoesNotPoisonNextSelection \
        single_file "$CURRENT_CODE" "$CURRENT_RUN_ID" "$log_file"; then
        status="PASS"
      fi
      record_result local "$platform" "$platform" unreadable_share "$repetition" "$status"
      if [[ "$status" == "PASS" ]]; then
        echo "pass: $CURRENT_CASE_ID"
      else
        echo "fail: $CURRENT_CASE_ID" >&2
        print_log_tail "$platform local Share recovery" "$log_file"
      fi
    done
  done
}

if ! prepare_builds; then
  echo "error: matrix build/deploy phase failed; logs in $log_dir" >&2
  exit 1
fi
if ! locate_apple_artifacts; then
  exit 1
fi

run_local_share_recovery

for direction in "${directions[@]}"; do
  sender="${direction%%:*}"
  receiver="${direction##*:}"
  for scenario in "${scenarios[@]}"; do
    if [[ "$scenario" == "share" && "$sender" == "macos" ]]; then
      echo "skip: macOS has no Share source provider ($direction)"
      continue
    fi
    for repetition in $(seq 1 "$repeat_count"); do
      next_case "$sender-to-$receiver-$scenario" "$repetition"
      echo "case $CURRENT_CASE_ID: $sender -> $receiver, scenario=$scenario"
      if run_pair "$sender" "$receiver" "$scenario" \
        "$CURRENT_CODE" "$CURRENT_RUN_ID" "$CURRENT_CASE_ID"; then
        record_result transfer "$sender" "$receiver" "$scenario" "$repetition" PASS
        echo "pass: $CURRENT_CASE_ID"
      else
        record_result transfer "$sender" "$receiver" "$scenario" "$repetition" FAIL
        echo "fail: $CURRENT_CASE_ID" >&2
      fi
    done
  done
done

echo "matrix complete: pass=$pass_count fail=$fail_count"
echo "results: $results_file"
echo "logs: $log_dir"
if [[ "$fail_count" -ne 0 ]]; then
  exit 1
fi
