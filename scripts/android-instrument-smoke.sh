#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
android_dir="$repo_root/android"

adb_bin="${ADB:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb}"
main_apk="$android_dir/app/build/outputs/apk/debug/app-debug.apk"
test_apk="$android_dir/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
test_class="${ENVOIX_ANDROID_TEST_CLASS:-dev.envoix.app.InviteCodecInstrumentedTest}"
test_runner="dev.envoix.app.test/androidx.test.runner.AndroidJUnitRunner"

if [[ ! -x "$adb_bin" ]]; then
  echo "error: adb not found at $adb_bin" >&2
  echo "set ADB=/path/to/adb or ANDROID_HOME=/path/to/sdk" >&2
  exit 2
fi

prepare_device() {
  "$adb_bin" shell input keyevent WAKEUP >/dev/null 2>&1 || true
  "$adb_bin" shell wm dismiss-keyguard >/dev/null 2>&1 || true
  "$adb_bin" shell svc power stayon usb >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global window_animation_scale 0 >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global transition_animation_scale 0 >/dev/null 2>&1 || true
  "$adb_bin" shell settings put global animator_duration_scale 0 >/dev/null 2>&1 || true
}

if [[ "${ENVOIX_SKIP_BUILD:-0}" != "1" ]]; then
  (
    cd "$android_dir"
    env \
      ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}" \
      ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}" \
      ./gradlew :app:assembleDebug :app:assembleDebugAndroidTest --no-daemon
  )
fi

prepare_device

install_apk() {
  local apk="$1"
  local log_file
  log_file="$(mktemp "${TMPDIR:-/tmp}/envoix-adb-install.XXXXXX")"
  if ! "$adb_bin" install -r -d -t -g "$apk" 2>&1 | tee "$log_file"; then
    if grep -q "INSTALL_FAILED_USER_RESTRICTED" "$log_file"; then
      echo "error: Android blocked adb install with INSTALL_FAILED_USER_RESTRICTED." >&2
      echo "confirm the install prompt on the device, then rerun this script." >&2
      rm -f "$log_file"
      exit 3
    fi
    rm -f "$log_file"
    exit 1
  fi
  rm -f "$log_file"
}

install_apk "$main_apk"
install_apk "$test_apk"
"$adb_bin" shell pm grant dev.envoix.app android.permission.CAMERA >/dev/null 2>&1 || true
"$adb_bin" shell pm grant dev.envoix.app android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
"$adb_bin" shell input keyevent BACK >/dev/null 2>&1 || true
"$adb_bin" shell am force-stop dev.envoix.app >/dev/null 2>&1 || true
"$adb_bin" shell am force-stop dev.envoix.app.test >/dev/null 2>&1 || true
"$adb_bin" shell am force-stop com.lbe.security.miui >/dev/null 2>&1 || true
"$adb_bin" shell am start -W -n dev.envoix.app/.MainActivity
"$adb_bin" shell pidof dev.envoix.app >/dev/null
"$adb_bin" shell am instrument -w -e class "$test_class" "$test_runner"

if [[ "${ENVOIX_TEST_CLEANUP:-0}" == "1" ]]; then
  "$adb_bin" uninstall dev.envoix.app.test >/dev/null || true
  "$adb_bin" uninstall dev.envoix.app >/dev/null || true
fi
