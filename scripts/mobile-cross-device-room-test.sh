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
start_delay="${ENVOIX_CROSS_DEVICE_START_DELAY:-18}"
log_dir="${TMPDIR:-/tmp}/envoix-cross-device-$(date +%Y%m%d-%H%M%S)"

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
  "$adb_bin" shell am instrument -w \
    -e envoixCrossDevice 1 \
    -e class "dev.envoix.app.CrossDeviceRoomInstrumentedTest#$method" \
    "$test_runner"
}

run_ios_test() {
  local method="$1"
  local log_file="$2"
  xcodebuild test \
    OTHER_SWIFT_FLAGS='$(inherited) -D ENVOIX_CROSS_DEVICE_TESTING' \
    -allowProvisioningUpdates \
    -project "$ios_project" \
    -scheme "$ios_cross_device_scheme" \
    -configuration Debug \
    -destination "$ios_destination" \
    -derivedDataPath "$ios_derived_data" \
    -only-testing:"Envoix-iOSUITests/EnvoixIOSLoopbackTests/$method" \
    >"$log_file" 2>&1
  if ! grep -q "Executed 1 test, with 0 test skipped" "$log_file"; then
    echo "error: iOS cross-device test did not execute as expected." >&2
    cat "$log_file" >&2
    return 1
  fi
}

run_android_to_ios() {
  local ios_log="$log_dir/android-to-ios.ios.log"
  local android_log="$log_dir/android-to-ios.android.log"
  echo "android -> ios: receiver starting on iOS; logs in $log_dir"
  run_ios_test testCrossDeviceReceiveAndroidToIosRoom "$ios_log" &
  local ios_pid=$!
  sleep "$start_delay"
  echo "android -> ios: sender starting on Android"
  if ! run_android_test sendAndroidToIosRoom >"$android_log" 2>&1; then
    cat "$android_log" >&2
    wait "$ios_pid" || true
    return 1
  fi
  if ! wait "$ios_pid"; then
    cat "$ios_log" >&2
    return 1
  fi
}

run_ios_to_android() {
  local ios_log="$log_dir/ios-to-android.ios.log"
  local android_log="$log_dir/ios-to-android.android.log"
  echo "ios -> android: receiver starting on Android; logs in $log_dir"
  run_android_test receiveIosToAndroidRoom >"$android_log" 2>&1 &
  local android_pid=$!
  sleep "$start_delay"
  echo "ios -> android: sender starting on iOS"
  if ! run_ios_test testCrossDeviceSendIosToAndroidRoom "$ios_log"; then
    cat "$ios_log" >&2
    wait "$android_pid" || true
    return 1
  fi
  if ! wait "$android_pid"; then
    cat "$android_log" >&2
    return 1
  fi
}

direction="${1:-both}"
build_and_install_android

case "$direction" in
  android-to-ios)
    run_android_to_ios
    ;;
  ios-to-android)
    run_ios_to_android
    ;;
  both)
    run_android_to_ios
    run_ios_to_android
    ;;
  *)
    echo "usage: $0 [android-to-ios|ios-to-android|both]" >&2
    exit 2
    ;;
esac

echo "cross-device room tests passed; logs in $log_dir"
