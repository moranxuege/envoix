#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_CAPTURE_SECONDS=60
readonly DEFAULT_SAMPLE_INTERVAL_SECONDS=1
readonly DEFAULT_IPHONE_ID="1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1"
readonly IPHONE_BUNDLE_ID="com.xiaomi.hyperConnect"
readonly ANDROID_ACTIVITY="com.miui.mishare.connectivity/com.miui.mishare.activity.MiShareNearShareActivity"

capture_seconds="$DEFAULT_CAPTURE_SECONDS"
sample_interval_seconds="$DEFAULT_SAMPLE_INTERVAL_SECONDS"
output_dir=""
android_serial="${ANDROID_SERIAL:-}"
iphone_id="${XIAOMI_PROBE_IPHONE_ID:-$DEFAULT_IPHONE_ID}"
launch_apps=0
adb_bin="${ADB:-}"
logcat_pid=""
sampling_pid=""
capture_finished=0

usage() {
  cat <<'EOF'
Usage: scripts/xiaomi-interconnect-path-probe.sh [options]

Collects read-only evidence while a Xiaomi Interconnect transfer is performed.

Options:
  --seconds N       Capture duration in seconds (default: 60)
  --interval N      Network sampling interval in seconds (default: 1)
  --output DIR      New output directory (default: /private/tmp/...timestamp)
  --android SERIAL  Android adb serial (auto-detected when exactly one is online)
  --iphone ID       iPhone CoreDevice identifier (default: the lab iPhone)
  --launch-apps     Bring Xiaomi Interconnect to the foreground on both devices
  -h, --help        Show this help

The probe does not change radio state, pairing state, application data, or logs.
EOF
}

die() {
  echo "error: $*" >&2
  exit 2
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    die "$name must be a positive integer"
  fi
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --seconds)
      [[ "$#" -ge 2 ]] || die "--seconds requires a value"
      capture_seconds="$2"
      shift 2
      ;;
    --interval)
      [[ "$#" -ge 2 ]] || die "--interval requires a value"
      sample_interval_seconds="$2"
      shift 2
      ;;
    --output)
      [[ "$#" -ge 2 ]] || die "--output requires a value"
      output_dir="$2"
      shift 2
      ;;
    --android)
      [[ "$#" -ge 2 ]] || die "--android requires a value"
      android_serial="$2"
      shift 2
      ;;
    --iphone)
      [[ "$#" -ge 2 ]] || die "--iphone requires a value"
      iphone_id="$2"
      shift 2
      ;;
    --launch-apps)
      launch_apps=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

require_positive_integer "--seconds" "$capture_seconds"
require_positive_integer "--interval" "$sample_interval_seconds"

if [[ -z "$adb_bin" ]]; then
  adb_bin="$(command -v adb || true)"
fi
if [[ -z "$adb_bin" && -n "${ANDROID_SDK_ROOT:-}" ]]; then
  adb_bin="$ANDROID_SDK_ROOT/platform-tools/adb"
fi
if [[ -z "$adb_bin" && -n "${ANDROID_HOME:-}" ]]; then
  adb_bin="$ANDROID_HOME/platform-tools/adb"
fi
[[ -x "$adb_bin" ]] || die "adb not found; set ADB, ANDROID_SDK_ROOT, or ANDROID_HOME"

if [[ -z "$android_serial" ]]; then
  android_serial="$({ env ADB_LIBUSB=0 "$adb_bin" devices 2>/dev/null || true; } \
    | awk 'NR > 1 && $2 == "device" { print $1 }')"
  if [[ -z "$android_serial" ]]; then
    die "no online Android device found"
  fi
  if [[ "$android_serial" == *$'\n'* ]]; then
    die "multiple Android devices found; select one with --android SERIAL"
  fi
fi

if [[ -z "$output_dir" ]]; then
  output_dir="/private/tmp/envoix-xiaomi-probe-$(date +%Y%m%d-%H%M%S)"
fi
[[ ! -e "$output_dir" ]] || die "output path already exists: $output_dir"
mkdir -p "$output_dir"

adb_device() {
  env ADB_LIBUSB=0 "$adb_bin" -s "$android_serial" "$@"
}

capture_android_snapshot() {
  local label="$1"
  local snapshot_dir="$output_dir/android-$label"
  mkdir -p "$snapshot_dir"

  adb_device shell date >"$snapshot_dir/device-time.txt" 2>&1 || true
  adb_device shell getprop >"$snapshot_dir/getprop.txt" 2>&1 || true
  adb_device shell settings get global bluetooth_on \
    >"$snapshot_dir/bluetooth-on.txt" 2>&1 || true
  adb_device shell settings get global wifi_on \
    >"$snapshot_dir/wifi-on.txt" 2>&1 || true
  adb_device shell ip -br link >"$snapshot_dir/ip-link.txt" 2>&1 || true
  adb_device shell ip -br addr >"$snapshot_dir/ip-address.txt" 2>&1 || true
  adb_device shell ip rule >"$snapshot_dir/ip-rule.txt" 2>&1 || true
  adb_device shell ip route show table all >"$snapshot_dir/ip-route.txt" 2>&1 || true
  adb_device shell ip -6 route show table all >"$snapshot_dir/ip6-route.txt" 2>&1 || true
  adb_device shell dumpsys connectivity >"$snapshot_dir/connectivity.txt" 2>&1 || true
  adb_device shell dumpsys wifi >"$snapshot_dir/wifi.txt" 2>&1 || true
  adb_device shell dumpsys wifip2p >"$snapshot_dir/wifip2p.txt" 2>&1 || true
  adb_device shell dumpsys MultiWifiP2pService \
    >"$snapshot_dir/multi-wifi-p2p.txt" 2>&1 || true
  adb_device shell dumpsys MiuiWifiService \
    >"$snapshot_dir/miui-wifi.txt" 2>&1 || true
  adb_device shell dumpsys xiaomi.InterconnectionService \
    >"$snapshot_dir/interconnection.txt" 2>&1 || true
  adb_device shell service list >"$snapshot_dir/services.txt" 2>&1 || true
  adb_device shell ps -A >"$snapshot_dir/processes.txt" 2>&1 || true
}

capture_iphone_processes() {
  local label="$1"
  local json_file="$output_dir/iphone-processes-$label.json"
  local log_file="$output_dir/iphone-processes-$label.log"

  if [[ -z "$iphone_id" ]] || ! command -v xcrun >/dev/null 2>&1; then
    return
  fi
  xcrun devicectl device info processes \
    --device "$iphone_id" \
    --timeout 15 \
    --json-output "$json_file" \
    --log-output "$log_file" \
    >/dev/null 2>&1 || true
}

launch_xiaomi_apps() {
  adb_device shell am start -W -n "$ANDROID_ACTIVITY" \
    >"$output_dir/android-app-launch.txt" 2>&1 || true
  if [[ -n "$iphone_id" ]] && command -v xcrun >/dev/null 2>&1; then
    xcrun devicectl device process launch \
      --device "$iphone_id" \
      --activate "$IPHONE_BUNDLE_ID" \
      >"$output_dir/iphone-app-launch.txt" 2>&1 || true
  fi
}

sample_android_network() {
  local elapsed=0
  while [[ "$elapsed" -lt "$capture_seconds" ]]; do
    {
      echo "===== host=$(date -u +%Y-%m-%dT%H:%M:%SZ) elapsed=${elapsed}s ====="
      adb_device shell 'date; ip -br link; ip -br addr; cat /proc/net/dev; ip rule; ip route show table all; dumpsys wifip2p; dumpsys MultiWifiP2pService'
    } >>"$output_dir/android-network-timeline.txt" 2>&1 || true
    sleep "$sample_interval_seconds"
    elapsed=$((elapsed + sample_interval_seconds))
  done
}

analyze_capture() {
  local iface
  local relevant_log="$output_dir/android-logcat-relevant.txt"
  local summary_file="$output_dir/summary.txt"
  local pattern='OneHop|MiShare|mishare|Lyra|NetBus|00370E2E|WifiAware|Wi-Fi Aware|wifiaware|NAN|nan_|MiWill|miwill|miw_oem|WifiP2p|P2P|p2p[0-9]|hotspot|SoftAp|BLE_APPLE|MDNS|mDNS'

  grep -Eai "$pattern" "$output_dir/android-logcat-all.txt" >"$relevant_log" || true
  {
    echo "Xiaomi Interconnect path probe"
    echo "host_finished=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "android_serial=$android_serial"
    echo "iphone_id=$iphone_id"
    echo "capture_seconds=$capture_seconds"
    echo
    echo "Observed interface-up samples:"
    grep -Ea '^(miw_oem|p2p|aware|nan)[^[:space:]]*[[:space:]]+UP' \
      "$output_dir/android-network-timeline.txt" | sort -u || true
    echo
    echo "Observed P2P group evidence:"
    grep -Ea 'groupFormed: true|curState=(GroupCreatedState|GroupCreatingState)|mGroup [^n]' \
      "$output_dir/android-network-timeline.txt" | sort -u || true
    echo
    echo "Interface byte deltas (includes background traffic):"
    for iface in wlan0 p2p0 p2p1 miw_oem0 aware_data0; do
      awk -v iface="$iface" '
        $1 == iface ":" {
          if (samples == 0) {
            first_rx = $2
            first_tx = $10
          }
          last_rx = $2
          last_tx = $10
          samples++
        }
        END {
          if (samples > 0) {
            printf "%s rx_delta=%.0f tx_delta=%.0f samples=%d\n", iface,
              last_rx - first_rx, last_tx - first_tx, samples
          }
        }
      ' "$output_dir/android-network-timeline.txt"
    done
    echo
    echo "Relevant Android log lines: $(wc -l <"$relevant_log" | tr -d ' ')"
    echo "Review android-logcat-relevant.txt and android-network-timeline.txt before drawing a conclusion."
  } >"$summary_file"
}

finish_capture() {
  if [[ "$capture_finished" == "1" ]]; then
    return
  fi
  capture_finished=1

  if [[ -n "$sampling_pid" ]]; then
    wait "$sampling_pid" 2>/dev/null || true
  fi
  if [[ -n "$logcat_pid" ]]; then
    kill "$logcat_pid" 2>/dev/null || true
    wait "$logcat_pid" 2>/dev/null || true
  fi

  capture_android_snapshot "after"
  capture_iphone_processes "after"
  analyze_capture
  echo "Capture complete: $output_dir"
}

trap finish_capture EXIT INT TERM

if ! adb_device get-state >/dev/null 2>&1; then
  die "Android device is not reachable: $android_serial"
fi

{
  echo "host_started=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "android_serial=$android_serial"
  echo "iphone_id=$iphone_id"
  echo "capture_seconds=$capture_seconds"
  echo "sample_interval_seconds=$sample_interval_seconds"
  echo "launch_apps=$launch_apps"
  echo "privacy_note=Logs can contain device names, IP addresses, and network identifiers."
} >"$output_dir/metadata.txt"

capture_android_snapshot "before"
capture_iphone_processes "before"

adb_device logcat -b all -v threadtime -T 1 \
  >"$output_dir/android-logcat-all.txt" 2>&1 &
logcat_pid=$!

sample_android_network &
sampling_pid=$!

if [[ "$launch_apps" == "1" ]]; then
  launch_xiaomi_apps
fi

echo "Capture started for ${capture_seconds}s. Perform one Xiaomi Interconnect transfer now."
echo "Output: $output_dir"
wait "$sampling_pid"
sampling_pid=""
finish_capture
trap - EXIT INT TERM
