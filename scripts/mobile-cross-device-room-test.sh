#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
android_dir="$repo_root/android"

adb_bin="${ADB:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb}"
main_apk="$android_dir/app/build/outputs/apk/debug/app-debug.apk"
test_apk="$android_dir/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
test_runner="dev.envoix.app.test/androidx.test.runner.AndroidJUnitRunner"
ios_project="$repo_root/apps/envoix-apple/Envoix.xcodeproj"
ios_cross_device_scheme="${ENVOIX_IOS_CROSS_DEVICE_SCHEME:-Envoix-iOS-CrossDevice}"
ios_derived_data="$repo_root/apps/envoix-apple/build-ios-ui-test"
ios_destination="${ENVOIX_IOS_DESTINATION:-platform=iOS,id=00008130-00043154346B803A}"
start_delay_long="${ENVOIX_CROSS_DEVICE_START_DELAY_LONG:-${ENVOIX_CROSS_DEVICE_START_DELAY:-18}}"
start_delay_short="${ENVOIX_CROSS_DEVICE_START_DELAY_SHORT:-6}"
ready_timeout_long="${ENVOIX_CROSS_DEVICE_READY_TIMEOUT_LONG:-${ENVOIX_CROSS_DEVICE_READY_TIMEOUT:-120}}"
ready_timeout_short="${ENVOIX_CROSS_DEVICE_READY_TIMEOUT_SHORT:-20}"

# Quick-fail then one long retry for unstable links.
if [[ "$start_delay_long" -lt "$start_delay_short" ]]; then
  echo "warning: long start delay (${start_delay_long}s) is shorter than short start delay (${start_delay_short}s); using ${start_delay_short}s as long start delay."
  start_delay_long="$start_delay_short"
fi
if [[ "$ready_timeout_long" -lt "$ready_timeout_short" ]]; then
  echo "warning: long ready timeout (${ready_timeout_long}s) is shorter than short ready timeout (${ready_timeout_short}s); using ${ready_timeout_short}s as long timeout."
  ready_timeout_long="$ready_timeout_short"
fi
android_verbose_log="${ENVOIX_CROSS_DEVICE_VERBOSE_LOG:-1}"
android_logcat_format="${ENVOIX_ANDROID_LOGCAT_FORMAT:-threadtime}"
android_logcat_cross_level="${ENVOIX_ANDROID_LOGCAT_CROSS_LEVEL:-V}"
android_logcat_core_level="${ENVOIX_ANDROID_LOGCAT_CORE_LEVEL:-V}"
log_dir="${TMPDIR:-/tmp}/envoix-cross-device-$(date +%Y%m%d-%H%M%S)"
android_invite_file="cache/envoix-cross-device-ios-to-android.invite"

mkdir -p "$log_dir"

if [[ ! -x "$adb_bin" ]]; then
  echo "error: adb not found at $adb_bin" >&2
  exit 2
fi

prepare_android_device() {
  "$adb_bin" shell input keyevent WAKEUP >/dev/null 2>&1 || true
  "$adb_bin" shell wm dismiss-keyguard >/dev/null 2>&1 || true
  "$adb_bin" shell svc power stayon usb >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global window_animation_scale 0 >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global transition_animation_scale 0 >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global animator_duration_scale 0 >/dev/null 2>&1 || true
}

install_apk() {
  local apk="$1"
  local log_file="$log_dir/$(basename "$apk").install.log"
  if ! "$adb_bin" install -r -d -t -g "$apk" >"$log_file" 2>&1; then
    cat "$log_file" >&2
    if grep -q "INSTALL_FAILED_USER_RESTRICTED" "$log_file"; then
      echo "error: Android blocked adb install. Confirm the prompt on the device, then rerun." >&2
      exit 3
    fi
    exit 1
  fi
}

build_and_install_android() {
  if [[ "${ENVOIX_SKIP_BUILD:-0}" != "1" ]]; then
    (
      cd "$android_dir"
      env \
        ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}" \
        ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}" \
        ./gradlew :app:assembleDebug :app:assembleDebugAndroidTest --no-daemon
    )
  fi

  prepare_android_device
  install_apk "$main_apk"
  install_apk "$test_apk"
  "$adb_bin" shell pm grant dev.envoix.app android.permission.CAMERA >/dev/null 2>&1 || true
  "$adb_bin" shell pm grant dev.envoix.app android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
  "$adb_bin" shell am force-stop dev.envoix.app >/dev/null 2>&1 || true
  "$adb_bin" shell am force-stop dev.envoix.app.test >/dev/null 2>&1 || true
}

run_android_test() {
  local method="$1"
  shift
  local timeout_ms="${1:-${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-}}"
  if [[ "$#" -gt 0 ]]; then
    shift
  fi
  local -a transfer_args
  transfer_args=()
  if [[ -n "$timeout_ms" ]]; then
    transfer_args+=(-e envoixCrossDeviceTimeoutMs "$timeout_ms")
  fi
  if [[ -n "${ENVOIX_ANDROID_TO_IOS_CODE:-}" ]]; then
    transfer_args+=(-e envoixAndroidToIosCode "$ENVOIX_ANDROID_TO_IOS_CODE")
  fi
  if [[ -n "${ENVOIX_IOS_TO_ANDROID_CODE:-}" ]]; then
    transfer_args+=(-e envoixIosToAndroidCode "$ENVOIX_IOS_TO_ANDROID_CODE")
  fi
  if [[ -n "${ENVOIX_ANDROID_TO_IOS_BYTES:-}" ]]; then
    transfer_args+=(-e envoixAndroidToIosBytes "$ENVOIX_ANDROID_TO_IOS_BYTES")
  fi
  if [[ -n "${ENVOIX_IOS_TO_ANDROID_BYTES:-}" ]]; then
    transfer_args+=(-e envoixIosToAndroidBytes "$ENVOIX_IOS_TO_ANDROID_BYTES")
  fi
  if [[ "${#transfer_args[@]}" -gt 0 ]]; then
    "$adb_bin" shell am instrument -w \
      -e envoixCrossDevice 1 \
      -e envoixVerboseLog "$android_verbose_log" \
      "${transfer_args[@]}" \
      "$@" \
      -e class "dev.envoix.app.CrossDeviceRoomInstrumentedTest#$method" \
      "$test_runner"
  else
    "$adb_bin" shell am instrument -w \
      -e envoixCrossDevice 1 \
      -e envoixVerboseLog "$android_verbose_log" \
      "$@" \
      -e class "dev.envoix.app.CrossDeviceRoomInstrumentedTest#$method" \
      "$test_runner"
  fi
}

print_log_tail() {
  local title="$1"
  local file="$2"
  echo "--- $title ($file) ---" >&2
  if [[ -f "$file" ]]; then
    tail -n 180 "$file" >&2
  else
    echo "missing log file" >&2
  fi
}

wait_for_log() {
  local file="$1"
  local pattern="$2"
  local label="$3"
  local timeout="${4:-$ready_timeout_long}"
  local waited=0
  while [[ "$waited" -lt "$timeout" ]]; do
    if [[ -f "$file" ]] && grep -Eq "$pattern" "$file"; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "error: timed out waiting for $label after ${timeout}s" >&2
  print_log_tail "$label" "$file"
  return 1
}

run_ios_test() {
  local method="$1"
  local log_file="$2"
  local timeout_seconds="${3:-${ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS:-}}"
  local transfer_invite="${4:-}"
  local build_args=(
    build-for-testing
    'OTHER_SWIFT_FLAGS=$(inherited) -D ENVOIX_CROSS_DEVICE_TESTING'
    -allowProvisioningUpdates
    -project "$ios_project"
    -scheme "$ios_cross_device_scheme"
    -configuration Debug
    -destination "$ios_destination"
    -derivedDataPath "$ios_derived_data"
  )
  if ! xcodebuild "${build_args[@]}" >"$log_file" 2>&1; then
    print_log_tail "iOS $method build" "$log_file"
    return 1
  fi

  local xctestrun
  xctestrun="$(find "$ios_derived_data/Build/Products" -name "${ios_cross_device_scheme}_*.xctestrun" -print | head -n 1)"
  if [[ -z "$xctestrun" ]]; then
    echo "error: failed to locate generated .xctestrun" >&2
    print_log_tail "iOS $method" "$log_file"
    return 1
  fi

  local patched_xctestrun="$ios_derived_data/Build/Products/$method.xctestrun"
  cp "$xctestrun" "$patched_xctestrun"
  if [[ -n "$transfer_invite" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_TRANSFER_INVITE "$transfer_invite"
  fi
  if [[ -n "${ENVOIX_ANDROID_TO_IOS_CODE:-}" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_ANDROID_TO_IOS_CODE "$ENVOIX_ANDROID_TO_IOS_CODE"
  fi
  if [[ -n "${ENVOIX_IOS_TO_ANDROID_CODE:-}" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_IOS_TO_ANDROID_CODE "$ENVOIX_IOS_TO_ANDROID_CODE"
  fi
  if [[ -n "${ENVOIX_ANDROID_TO_IOS_BYTES:-}" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_ANDROID_TO_IOS_BYTES "$ENVOIX_ANDROID_TO_IOS_BYTES"
  fi
  if [[ -n "${ENVOIX_IOS_TO_ANDROID_BYTES:-}" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_IOS_TO_ANDROID_BYTES "$ENVOIX_IOS_TO_ANDROID_BYTES"
  fi
  if [[ -n "$timeout_seconds" ]]; then
    set_xctestrun_env "$patched_xctestrun" ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS "$timeout_seconds"
  fi

  local test_args=(
    test-without-building
    -xctestrun "$patched_xctestrun"
    -destination "$ios_destination"
    -only-testing:"Envoix-iOSUITests/EnvoixIOSLoopbackTests/$method"
  )
  local test_status=0
  xcodebuild "${test_args[@]}" >>"$log_file" 2>&1 || test_status=$?
  rm -f "$patched_xctestrun"
  if [[ "$test_status" -ne 0 ]] || ! grep -Eq "Executed 1 test, with 0 (failures|test skipped)|\\*\\* TEST SUCCEEDED \\*\\*" "$log_file"; then
    echo "error: iOS cross-device test did not execute as expected." >&2
    print_log_tail "iOS $method" "$log_file"
    return 1
  fi
}

base64_arg() {
  printf '%s' "$1" | base64 | tr -d '\n'
}

set_xctestrun_env() {
  local file="$1"
  local key="$2"
  local value="$3"
  /usr/libexec/PlistBuddy -c "Add :Envoix-iOSUITests:EnvironmentVariables:$key string $value" "$file" >/dev/null 2>&1 \
    || /usr/libexec/PlistBuddy -c "Set :Envoix-iOSUITests:EnvironmentVariables:$key $value" "$file"
  /usr/libexec/PlistBuddy -c "Add :Envoix-iOSUITests:TestingEnvironmentVariables:$key string $value" "$file" >/dev/null 2>&1 \
    || /usr/libexec/PlistBuddy -c "Set :Envoix-iOSUITests:TestingEnvironmentVariables:$key $value" "$file"
}

ios_invite_from_log() {
  local file="$1"
  sed -n 's/^.*\[cross-device\] iOS invite \(envoix:.*\)$/\1/p' "$file" | head -n 1
}

wait_for_android_invite() {
  local timeout="${1:-$ready_timeout_long}"
  local waited=0
  local invite=""
  while [[ "$waited" -lt "$timeout" ]]; do
    invite="$("$adb_bin" shell run-as dev.envoix.app cat "$android_invite_file" 2>/dev/null | tr -d '\r' || true)"
    if [[ -n "$invite" ]]; then
      printf '%s' "$invite"
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "error: timed out waiting for Android invite after ${timeout}s" >&2
  return 1
}

start_android_logcat() {
  local file="$1"
  "$adb_bin" logcat -c >/dev/null 2>&1 || true
  "$adb_bin" logcat -v "$android_logcat_format" \
    EnvoixCrossDevice:"$android_logcat_cross_level" \
    Envoix:"$android_logcat_core_level" \
    AndroidRuntime:E '*:S' >"$file" 2>&1 &
  printf '%s' "$!"
}

stop_android_logcat() {
  local pid="${1:-}"
  if [[ -n "$pid" ]]; then
    kill "$pid" >/dev/null 2>&1 || true
    wait "$pid" >/dev/null 2>&1 || true
  fi
}

stop_android_under_test() {
  "$adb_bin" shell am force-stop dev.envoix.app >/dev/null 2>&1 || true
  "$adb_bin" shell am force-stop dev.envoix.app.test >/dev/null 2>&1 || true
}

run_android_to_ios() {
  local ready_timeout="$1"
  local attempt="${2:-1}"
  local transfer_timeout_ms="${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-$((ready_timeout * 1000))}"
  local ios_test_timeout_seconds="${ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS:-$ready_timeout}"
  local suffix=""
  [[ "$attempt" != "1" ]] && suffix=".attempt-$attempt"

  local ios_log="$log_dir/android-to-ios${suffix}.ios.log"
  local android_log="$log_dir/android-to-ios${suffix}.android.log"
  local android_logcat="$log_dir/android-to-ios${suffix}.logcat.log"
  local logcat_pid
  logcat_pid="$(start_android_logcat "$android_logcat")"
  echo "android -> ios: receiver starting on iOS; logs in $log_dir"
  run_ios_test testCrossDeviceReceiveAndroidToIosRoom "$ios_log" "$ios_test_timeout_seconds" &
  local ios_pid=$!
  if ! wait_for_log "$ios_log" "\\[cross-device\\] iOS receive start" "iOS receiver start" "$ready_timeout"; then
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    wait "$ios_pid" || true
    return 1
  fi
  sleep "${ENVOIX_CROSS_DEVICE_RECEIVER_GRACE:-3}"
  echo "android -> ios: sender starting on Android"
  if ! run_android_test sendAndroidToIosRoom "$transfer_timeout_ms" >"$android_log" 2>&1; then
    print_log_tail "Android sender" "$android_log"
    print_log_tail "iOS receiver" "$ios_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    wait "$ios_pid" || true
    return 1
  fi
  if ! wait "$ios_pid"; then
    print_log_tail "iOS receiver" "$ios_log"
    print_log_tail "Android sender" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    return 1
  fi
  stop_android_logcat "$logcat_pid"
}

run_ios_to_android() {
  local ready_timeout="$1"
  local attempt="${2:-1}"
  local receiver_start_delay="$start_delay_short"
  [[ "$attempt" != "1" ]] && receiver_start_delay="$start_delay_long"
  local transfer_timeout_ms="${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-$((ready_timeout * 1000))}"
  local ios_test_timeout_seconds="${ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS:-$ready_timeout}"
  local suffix=""
  [[ "$attempt" != "1" ]] && suffix=".attempt-$attempt"

  local ios_log="$log_dir/ios-to-android${suffix}.ios.log"
  local android_log="$log_dir/ios-to-android${suffix}.android.log"
  local android_logcat="$log_dir/ios-to-android${suffix}.logcat.log"
  local logcat_pid
  logcat_pid="$(start_android_logcat "$android_logcat")"
  echo "ios -> android: receiver starting on Android; logs in $log_dir"
  run_android_test receiveIosToAndroidRoom "$transfer_timeout_ms" >"$android_log" 2>&1 &
  local android_pid=$!
  sleep "$receiver_start_delay"
  sleep "${ENVOIX_CROSS_DEVICE_RECEIVER_GRACE:-3}"
  echo "ios -> android: sender starting on iOS"
  if ! run_ios_test testCrossDeviceSendIosToAndroidRoom "$ios_log" "$ios_test_timeout_seconds"; then
    print_log_tail "iOS sender" "$ios_log"
    print_log_tail "Android receiver" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_under_test
    stop_android_logcat "$logcat_pid"
    wait "$android_pid" || true
    return 1
  fi
  if ! wait "$android_pid"; then
    print_log_tail "Android receiver" "$android_log"
    print_log_tail "iOS sender" "$ios_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    return 1
  fi
  stop_android_logcat "$logcat_pid"
}

run_android_to_ios_invite() {
  local ready_timeout="$1"
  local attempt="${2:-1}"
  local transfer_timeout_ms="${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-$((ready_timeout * 1000))}"
  local ios_test_timeout_seconds="${ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS:-$ready_timeout}"
  local suffix=""
  [[ "$attempt" != "1" ]] && suffix=".attempt-$attempt"

  local ios_log="$log_dir/android-to-ios-invite${suffix}.ios.log"
  local android_log="$log_dir/android-to-ios-invite${suffix}.android.log"
  local android_logcat="$log_dir/android-to-ios-invite${suffix}.logcat.log"
  local logcat_pid
  logcat_pid="$(start_android_logcat "$android_logcat")"
  echo "android -> ios invite: receiver starting on iOS; logs in $log_dir"
  run_ios_test testCrossDeviceReceiveAndroidToIosInvite "$ios_log" "$ios_test_timeout_seconds" &
  local ios_pid=$!
  if ! wait_for_log "$ios_log" "\\[cross-device\\] iOS invite envoix:" "iOS invite" "$ready_timeout"; then
    print_log_tail "iOS invite receiver" "$ios_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    wait "$ios_pid" || true
    return 1
  fi
  local invite
  invite="$(ios_invite_from_log "$ios_log")"
  if [[ -z "$invite" ]]; then
    echo "error: failed to parse iOS invite" >&2
    print_log_tail "iOS invite receiver" "$ios_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    wait "$ios_pid" || true
    return 1
  fi
  echo "android -> ios invite: sender starting on Android"
  if ! run_android_test sendAndroidToIosInvite "$transfer_timeout_ms" -e envoixTransferInviteBase64 "$(base64_arg "$invite")" >"$android_log" 2>&1; then
    print_log_tail "Android invite sender" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    print_log_tail "iOS invite receiver" "$ios_log"
    stop_android_logcat "$logcat_pid"
    wait "$ios_pid" || true
    return 1
  fi
  if ! wait "$ios_pid"; then
    print_log_tail "iOS invite receiver" "$ios_log"
    print_log_tail "Android invite sender" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    return 1
  fi
  stop_android_logcat "$logcat_pid"
}

run_ios_to_android_invite() {
  local ready_timeout="$1"
  local attempt="${2:-1}"
  local transfer_timeout_ms="${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-$((ready_timeout * 1000))}"
  local ios_test_timeout_seconds="${ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS:-$ready_timeout}"
  local suffix=""
  [[ "$attempt" != "1" ]] && suffix=".attempt-$attempt"

  local ios_log="$log_dir/ios-to-android-invite${suffix}.ios.log"
  local android_log="$log_dir/ios-to-android-invite${suffix}.android.log"
  local android_logcat="$log_dir/ios-to-android-invite${suffix}.logcat.log"
  local logcat_pid
  logcat_pid="$(start_android_logcat "$android_logcat")"
  echo "ios -> android invite: receiver starting on Android; logs in $log_dir"
  "$adb_bin" shell run-as dev.envoix.app rm -f "$android_invite_file" >/dev/null 2>&1 || true
  run_android_test receiveIosToAndroidInvite "$transfer_timeout_ms" >"$android_log" 2>&1 &
  local android_pid=$!
  local invite
  if ! invite="$(wait_for_android_invite "$ready_timeout")"; then
    print_log_tail "Android invite receiver" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_logcat "$logcat_pid"
    wait "$android_pid" || true
    return 1
  fi
  sleep "${ENVOIX_CROSS_DEVICE_RECEIVER_GRACE:-3}"
  echo "ios -> android invite: sender starting on iOS"
  if ! run_ios_test testCrossDeviceSendIosToAndroidInvite "$ios_log" "$ios_test_timeout_seconds" "$invite"; then
    print_log_tail "iOS invite sender" "$ios_log"
    print_log_tail "Android invite receiver" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    stop_android_under_test
    stop_android_logcat "$logcat_pid"
    wait "$android_pid" || true
    return 1
  fi
  if ! wait "$android_pid"; then
    print_log_tail "Android invite receiver" "$android_log"
    print_log_tail "Android logcat" "$android_logcat"
    print_log_tail "iOS invite sender" "$ios_log"
    stop_android_logcat "$logcat_pid"
    return 1
  fi
  stop_android_logcat "$logcat_pid"
}

run_with_timeout_retry() {
  local label="$1"
  local test_fn="$2"
  local short_timeout="$3"
  local long_timeout="$4"
  shift 4
  local attempts_total=1
  if [[ "$short_timeout" -ne "$long_timeout" ]]; then
    attempts_total=2
  fi

  echo "info: testing $label (attempt 1/$attempts_total, short timeout=${short_timeout}s)"
  if "$test_fn" "$short_timeout" 1 "$@"; then
    echo "pass: $label on attempt 1"
    return 0
  fi
  if [[ "$short_timeout" -eq "$long_timeout" ]]; then
    echo "fail: $label (attempt 1, short timeout)"
    return 1
  fi
  echo "warning: $label failed in attempt 1, retrying once with long timeout (${long_timeout}s)."
  echo "info: testing $label (attempt 2/2, long timeout=${long_timeout}s)"
  if "$test_fn" "$long_timeout" 2 "$@"; then
    echo "pass: $label on attempt 2"
    return 0
  fi
  echo "fail: $label (attempt 2, long timeout)"
  return 1
}

direction="${1:-both}"
usage() {
  echo "usage: $0 [android-to-ios|ios-to-android|both|android-to-ios-invite|ios-to-android-invite|invite]"
}

case "$direction" in
  -h|--help)
    usage
    exit 0
    ;;
  android-to-ios|ios-to-android|both|android-to-ios-invite|ios-to-android-invite|invite)
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

build_and_install_android

case "$direction" in
  android-to-ios)
    run_with_timeout_retry "android -> ios" run_android_to_ios "$ready_timeout_short" "$ready_timeout_long"
    ;;
  ios-to-android)
    run_with_timeout_retry "ios -> android" run_ios_to_android "$ready_timeout_short" "$ready_timeout_long"
    ;;
  both)
    run_with_timeout_retry "android -> ios" run_android_to_ios "$ready_timeout_short" "$ready_timeout_long" || exit 1
    run_with_timeout_retry "ios -> android" run_ios_to_android "$ready_timeout_short" "$ready_timeout_long" || exit 1
    ;;
  android-to-ios-invite)
    run_with_timeout_retry "android -> ios invite" run_android_to_ios_invite "$ready_timeout_short" "$ready_timeout_long"
    ;;
  ios-to-android-invite)
    run_with_timeout_retry "ios -> android invite" run_ios_to_android_invite "$ready_timeout_short" "$ready_timeout_long"
    ;;
  invite)
    run_with_timeout_retry "android -> ios invite" run_android_to_ios_invite "$ready_timeout_short" "$ready_timeout_long" || exit 1
    run_with_timeout_retry "ios -> android invite" run_ios_to_android_invite "$ready_timeout_short" "$ready_timeout_long" || exit 1
    ;;
esac

echo "cross-device room tests passed; logs in $log_dir"
