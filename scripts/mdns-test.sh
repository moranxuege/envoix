#!/usr/bin/env bash
set -eu

readonly SERIAL_A="emulator-5554"
readonly SERIAL_B="emulator-5556"
readonly PACKAGE="dev.envoix.app"
readonly ACTIVITY="$PACKAGE/.MainActivity"
readonly SERVICE="$PACKAGE/.TransferService"
readonly ACTION_START="$PACKAGE.START"
readonly DEVICE_INPUT="/data/user/0/$PACKAGE/cache/mdns-test-input"
readonly DEVICE_OUTPUT_DIR="/sdcard/Download/Envoix"
readonly TRANSFER_RATE_KBITS=4194 # Approximately 512 KiB/s.
readonly TRANSFER_QUEUE_BYTES=33554432 # 32 MiB.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
emulator="$sdk_root/emulator/emulator"
adb="$sdk_root/platform-tools/adb"
apk="${APK:-$repo_root/android/app/build/outputs/apk/debug/app-debug.apk}"
started=0
passed=0
failed=0
timeout=120
verbose=0
keep_emulators=0
diagnostic_pids=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [options] <avd-a> <avd-b> <test-file> [apk]

Launch two rootable Android emulators and transfer the test file twice:
first with internet access, then with internet blocked while shared Wi-Fi and
mDNS remain available. Each received file is checked against the source SHA-256.

Both AVDs must use x86_64 Google APIs images, not Google Play images.
The sender's shared Wi-Fi bandwidth is limited to approximately 512 KiB/s.
Live diagnostics are written to android/build/mdns-test.
The APK defaults to:
  $apk

Options:
  --timeout SECONDS   Per-transfer timeout (default: $timeout)
  --verbose           Print transfer states and always capture diagnostics
  --keep-emulators    Leave both emulators running after the test
  -h, --help          Show this help
EOF
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

avd_config() {
    local ini="$HOME/.android/avd/$1.ini"
    local path

    [ -f "$ini" ] || die "AVD '$1' does not exist"
    path="$(sed -n 's/^path=//p' "$ini" | head -n 1)"
    [ -n "$path" ] || path="$HOME/.android/avd/$1.avd"
    printf '%s/config.ini\n' "$path"
}

check_avd() {
    local config
    config="$(avd_config "$1")"

    [ -f "$config" ] || die "missing config.ini for AVD '$1'"
    grep -Fqx 'abi.type=x86_64' "$config" || die "AVD '$1' is not x86_64"
    ! grep -Fqx 'PlayStore.enabled=true' "$config" ||
        die "AVD '$1' uses a Google Play image; use a rootable Google APIs AVD"
}

wait_for_boot() {
    local serial="$1"
    local deadline=$((SECONDS + 180))

    "$adb" -s "$serial" wait-for-device
    while [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" != "1" ]; do
        [ "$SECONDS" -lt "$deadline" ] || die "$serial did not boot within 180 seconds"
        sleep 2
    done
}

root_device() {
    local serial="$1"
    local output

    output="$("$adb" -s "$serial" root 2>&1 || true)"
    "$adb" -s "$serial" wait-for-device
    [ "$("$adb" -s "$serial" shell id -u | tr -d '\r')" = "0" ] ||
        die "$serial is not rootable ($output)"
}

connect_wifi() {
    local serial="$1"
    local deadline=$((SECONDS + 60))

    "$adb" -s "$serial" shell svc wifi enable
    "$adb" -s "$serial" shell cmd wifi connect-network AndroidWifi open -r persistent >/dev/null
    until "$adb" -s "$serial" shell ip -4 -o address show dev wlan0 scope global 2>/dev/null | grep -q 'inet '; do
        [ "$SECONDS" -lt "$deadline" ] || die "$serial did not join shared Wi-Fi"
        sleep 1
    done
}

reconnect_wifi() {
    local serial="$1"
    local deadline=$((SECONDS + 30))

    "$adb" -s "$serial" shell svc wifi disable
    while "$adb" -s "$serial" shell ip -4 -o address show dev wlan0 scope global 2>/dev/null | grep -q 'inet '; do
        [ "$SECONDS" -lt "$deadline" ] || die "$serial Wi-Fi did not disconnect"
        sleep 1
    done
    connect_wifi "$serial"
}

wait_for_validation() {
    local serial="$1"
    local expected="$2"
    local deadline=$((SECONDS + 45))
    local validated

    while true; do
        if "$adb" -s "$serial" shell cmd wifi status 2>/dev/null |
            grep 'NetworkCapabilities:' | grep -q 'VALIDATED'; then
            validated=yes
        else
            validated=no
        fi
        [ "$validated" = "$expected" ] && return
        [ "$SECONDS" -lt "$deadline" ] ||
            die "$serial Wi-Fi validation remained '$validated' (expected '$expected')"
        sleep 2
    done
}

configure_peer_routes() {
    local serial="$1"

    "$adb" -s "$serial" shell \
        'ip rule del pref 9000 2>/dev/null || true; ip rule del pref 9001 2>/dev/null || true; ip -6 rule del pref 9000 2>/dev/null || true; ip -6 rule del pref 9001 2>/dev/null || true; ip rule add pref 9000 to 10.0.2.16/28 lookup wlan0; ip rule add pref 9001 to 224.0.0.0/4 lookup wlan0; ip -6 rule add pref 9000 to fec0::/64 lookup wlan0; ip -6 rule add pref 9001 to ff00::/8 lookup wlan0'
}

clear_bandwidth_limit() {
    "$adb" -s "$1" shell 'tc qdisc del dev wlan0 root 2>/dev/null || true'
}

limit_bandwidth() {
    "$adb" -s "$1" shell \
        "tc qdisc replace dev wlan0 root tbf rate ${TRANSFER_RATE_KBITS}kbit burst 64kb limit $TRANSFER_QUEUE_BYTES"
}

wifi_address() {
    "$adb" -s "$1" shell ip -4 -o address show dev wlan0 scope global |
        awk '{print $4}' | head -n 1 | cut -d/ -f1 | tr -d '\r'
}

disable_firewall() {
    "$adb" -s "$1" shell \
        'iptables -D OUTPUT -j ENVOIX_OFFLINE 2>/dev/null || true; iptables -F ENVOIX_OFFLINE 2>/dev/null || true; iptables -X ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -D OUTPUT -j ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -F ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -X ENVOIX_OFFLINE 2>/dev/null || true'
}

enable_firewall() {
    disable_firewall "$1"
    "$adb" -s "$1" shell \
        'iptables -N ENVOIX_OFFLINE; iptables -A ENVOIX_OFFLINE -o lo -j ACCEPT; iptables -A ENVOIX_OFFLINE -d 10.0.2.0/24 -j ACCEPT; iptables -A ENVOIX_OFFLINE -d 224.0.0.0/4 -j ACCEPT; iptables -A ENVOIX_OFFLINE -j REJECT; iptables -I OUTPUT 1 -j ENVOIX_OFFLINE; ip6tables -N ENVOIX_OFFLINE; ip6tables -A ENVOIX_OFFLINE -o lo -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -d fe80::/10 -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -d ff00::/8 -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -j REJECT; ip6tables -I OUTPUT 1 -j ENVOIX_OFFLINE'
}

reset_app() {
    local serial="$1"

    "$adb" -s "$serial" shell pm clear "$PACKAGE" >/dev/null
    "$adb" -s "$serial" shell am start -W -n "$ACTIVITY" >/dev/null
    "$adb" -s "$serial" shell pm grant "$PACKAGE" android.permission.POST_NOTIFICATIONS 2>/dev/null || true
}

start_transfer() {
    local serial="$1"
    local direction="$2"
    local room="$3"
    local path="$4"

    "$adb" -s "$serial" shell am start-foreground-service \
        -n "$SERVICE" -a "$ACTION_START" \
        --es direction "$direction" --es room "$room" --es path "$path" >/dev/null
}

device_hashes() {
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'mdns-test-input*' -exec sha256sum {} \\; 2>/dev/null" |
        tr -d '\r'
}

record_state() {
    local serial="$1"
    local progress_log="$2"
    local snapshot

    snapshot="$("$adb" -s "$serial" shell \
        "cat /data/user/0/$PACKAGE/files/records/*.json 2>/dev/null" 2>&1 || true)"
    {
        printf '\n=== %s ===\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf '%s\n' "${snapshot:-<no transfer record>}"
    } >>"$progress_log"
    printf '%s\n' "$snapshot" |
        sed -n 's/.*"state": *"\([^"]*\)".*/\1/p' | tail -n 1 | tr -d '\r'
}

stop_live_diagnostics() {
    local pid

    for pid in "${diagnostic_pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    for pid in "${diagnostic_pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    diagnostic_pids=()
}

start_live_diagnostics() {
    local environment="$1"
    local role serial

    stop_live_diagnostics
    for role in sender receiver; do
        if [ "$role" = sender ]; then serial="$SERIAL_A"; else serial="$SERIAL_B"; fi
        : >"$log_dir/$environment-$role-progress.log"
        "$adb" -s "$serial" logcat -b all -v threadtime \
            >"$log_dir/$environment-$role-live-logcat.log" 2>&1 &
        diagnostic_pids+=("$!")
        "$adb" -s "$serial" exec-out tail -n +1 -f \
            "/data/user/0/$PACKAGE/files/logs/core.log" \
            >"$log_dir/$environment-$role-live-core.log" 2>&1 &
        diagnostic_pids+=("$!")
    done
}

capture_diagnostics() {
    local environment="$1"
    local role serial

    for role in sender receiver; do
        if [ "$role" = sender ]; then serial="$SERIAL_A"; else serial="$SERIAL_B"; fi
        "$adb" -s "$serial" logcat -d >"$log_dir/$environment-$role.log" 2>&1 || true
        "$adb" -s "$serial" shell \
            "ip -brief address; ip route show table all; dumpsys wifi | grep -E 'mNetworkInfo|mWifiInfo|Wi-Fi is'; tc -s qdisc show dev wlan0; iptables -nvL ENVOIX_OFFLINE 2>/dev/null; ip6tables -nvL ENVOIX_OFFLINE 2>/dev/null" \
            >"$log_dir/$environment-$role-network.log" 2>&1 || true
        rm -rf "$log_dir/$environment-$role-files"
        "$adb" -s "$serial" pull "/data/user/0/$PACKAGE/files" \
            "$log_dir/$environment-$role-files" >/dev/null 2>&1 || true
    done
}

run_test() {
    local environment="$1"
    local room="$(printf '%06d' $((RANDOM % 1000000)))-mdns-test"
    local deadline
    local actual=""
    local app_uid
    local sender_state=""
    local receiver_state=""

    printf '\n[%s] Preparing devices...\n' "$environment"
    clear_bandwidth_limit "$SERIAL_A"
    disable_firewall "$SERIAL_A"
    disable_firewall "$SERIAL_B"
    if [ "$environment" = "lan-only" ]; then
        "$adb" -s "$SERIAL_A" shell cmd connectivity airplane-mode enable >/dev/null
        "$adb" -s "$SERIAL_B" shell cmd connectivity airplane-mode enable >/dev/null
    else
        "$adb" -s "$SERIAL_A" shell cmd connectivity airplane-mode disable >/dev/null
        "$adb" -s "$SERIAL_B" shell cmd connectivity airplane-mode disable >/dev/null
    fi
    reset_app "$SERIAL_A"
    reset_app "$SERIAL_B"
    "$adb" -s "$SERIAL_A" logcat -c
    "$adb" -s "$SERIAL_B" logcat -c
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'mdns-test-input*' -exec rm -f {} \\; 2>/dev/null || true"
    "$adb" -s "$SERIAL_A" push "$test_file" /data/local/tmp/mdns-test-input >/dev/null
    app_uid="$("$adb" -s "$SERIAL_A" shell stat -c %u "/data/user/0/$PACKAGE" | tr -d '\r')"
    "$adb" -s "$SERIAL_A" shell \
        "cp /data/local/tmp/mdns-test-input '$DEVICE_INPUT'; chown $app_uid:$app_uid '$DEVICE_INPUT'"

    if [ "$environment" = "lan-only" ]; then
        enable_firewall "$SERIAL_A"
        enable_firewall "$SERIAL_B"
    fi
    # Reassociate Wi-Fi in the selected environment, then override Android's
    # overlapping eth0/wlan0 policy only for shared-Wi-Fi peers and multicast.
    reconnect_wifi "$SERIAL_A"
    reconnect_wifi "$SERIAL_B"
    configure_peer_routes "$SERIAL_A"
    configure_peer_routes "$SERIAL_B"
    if [ "$environment" = "lan-only" ]; then
        wait_for_validation "$SERIAL_A" no
        wait_for_validation "$SERIAL_B" no
    else
        "$adb" -s "$SERIAL_A" shell \
            'nc -w 10 67.230.187.238 8445 </dev/null' ||
            die "$SERIAL_A cannot reach the internet"
        "$adb" -s "$SERIAL_B" shell \
            'nc -w 10 67.230.187.238 8445 </dev/null' ||
            die "$SERIAL_B cannot reach the internet"
    fi

    printf '[%s] Limiting sender Wi-Fi to approximately 512 KiB/s...\n' "$environment"
    limit_bandwidth "$SERIAL_A"
    start_live_diagnostics "$environment"
    start_transfer "$SERIAL_B" receive "$room" "unused"
    sleep 2
    start_transfer "$SERIAL_A" send "$room" "$DEVICE_INPUT"

    deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        actual="$(device_hashes || true)"
        sender_state="$(record_state "$SERIAL_A" "$log_dir/$environment-sender-progress.log" || true)"
        receiver_state="$(record_state "$SERIAL_B" "$log_dir/$environment-receiver-progress.log" || true)"
        if [ "$verbose" -eq 1 ]; then
            printf '[%s] sender=%s receiver=%s\r' "$environment" "${sender_state:-unknown}" "${receiver_state:-unknown}"
        fi
        if printf '%s\n' "$actual" | grep -q "^$expected_hash " &&
            [ "$sender_state" = "completed" ] && [ "$receiver_state" = "completed" ]; then
            stop_live_diagnostics
            [ "$verbose" -eq 0 ] || printf '\n'
            printf '[%s] PASS: both peers completed; received SHA-256 matches %s\n' "$environment" "$expected_hash"
            passed=$((passed + 1))
            [ "$verbose" -eq 0 ] || capture_diagnostics "$environment"
            return
        fi
        if [ "$sender_state" = "failed" ] || [ "$receiver_state" = "failed" ]; then
            break
        fi
        sleep 2
    done

    stop_live_diagnostics
    [ "$verbose" -eq 0 ] || printf '\n'
    printf '[%s] FAIL after %s seconds: sender=%s receiver=%s\n' \
        "$environment" "$timeout" "${sender_state:-unknown}" "${receiver_state:-unknown}" >&2
    [ -z "$actual" ] || printf '[%s] received candidates:\n%s\n' "$environment" "$actual" >&2
    capture_diagnostics "$environment"
    failed=$((failed + 1))
}

cleanup() {
    stop_live_diagnostics
    clear_bandwidth_limit "$SERIAL_A" >/dev/null 2>&1 || true
    disable_firewall "$SERIAL_A" >/dev/null 2>&1 || true
    disable_firewall "$SERIAL_B" >/dev/null 2>&1 || true
    if [ "$started" -eq 1 ] && [ "$keep_emulators" -eq 0 ]; then
        "$adb" -s "$SERIAL_A" emu kill >/dev/null 2>&1 || true
        "$adb" -s "$SERIAL_B" emu kill >/dev/null 2>&1 || true
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --timeout)
            [ "$#" -ge 2 ] || die "--timeout requires a value"
            timeout="$2"
            shift 2
            ;;
        --verbose) verbose=1; shift ;;
        --keep-emulators) keep_emulators=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*) die "unknown option: $1" ;;
        *) break ;;
    esac
done

[ "$#" -ge 3 ] && [ "$#" -le 4 ] || {
    usage
    exit 2
}
[ "$timeout" -gt 0 ] 2>/dev/null || die "--timeout must be a positive integer"

avd_a="$1"
avd_b="$2"
test_file="$3"
[ "$#" -lt 4 ] || apk="$4"
[ "$avd_a" != "$avd_b" ] || die "use two distinct AVDs"
[ -x "$emulator" ] || die "Android Emulator not found at $emulator"
[ -x "$adb" ] || die "adb not found at $adb"
[ -f "$test_file" ] || die "test file not found at $test_file"
[ -f "$apk" ] || die "APK not found at $apk; build it first or pass it as the fourth argument"
command -v sha256sum >/dev/null || die "sha256sum is required"
command -v pkill >/dev/null || die "pkill is required"
check_avd "$avd_a"
check_avd "$avd_b"

pkill -KILL -x netsimd 2>/dev/null || true
if "$adb" devices | grep -Eq '^emulator-(5554|5556)[[:space:]]'; then
    die "emulator ports 5554/5556 are already in use"
fi

expected_hash="$(sha256sum "$test_file" | awk '{print $1}')"
log_dir="$repo_root/android/build/mdns-test"
mkdir -p "$log_dir"
trap cleanup EXIT INT TERM

"$emulator" "@$avd_a" -port 5554 -no-snapshot -netdelay none -netspeed full \
    >"$log_dir/emulator-5554.log" 2>&1 &
"$emulator" "@$avd_b" -port 5556 -no-snapshot -netdelay none -netspeed full \
    >"$log_dir/emulator-5556.log" 2>&1 &
started=1

printf 'Waiting for %s and %s to boot...\n' "$avd_a" "$avd_b"
wait_for_boot "$SERIAL_A"
wait_for_boot "$SERIAL_B"
root_device "$SERIAL_A"
root_device "$SERIAL_B"
"$adb" -s "$SERIAL_A" shell 'command -v tc >/dev/null' ||
    die "$SERIAL_A does not provide the tc traffic-control utility"
connect_wifi "$SERIAL_A"
connect_wifi "$SERIAL_B"
[ "$(wifi_address "$SERIAL_A")" != "$(wifi_address "$SERIAL_B")" ] ||
    die "emulators received the same shared Wi-Fi address"

for serial in "$SERIAL_A" "$SERIAL_B"; do
    install_output="$("$adb" -s "$serial" install -r -t "$apk" 2>&1)" ||
        die "failed to install APK on $serial: $install_output"
done
connect_wifi "$SERIAL_A"
connect_wifi "$SERIAL_B"

run_test internet
run_test lan-only

printf '\nResult: %d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ] || exit 1
