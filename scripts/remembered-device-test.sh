#!/usr/bin/env bash
set -euo pipefail

readonly SERIAL_A="emulator-5554"
readonly SERIAL_B="emulator-5556"
readonly PACKAGE="dev.envoix.app"
readonly ACTIVITY="$PACKAGE/.MainActivity"
readonly TEST_RECEIVER="$PACKAGE/.NatTestReceiver"
readonly ACTION_CREATE_RECEIVER_INVITE="$PACKAGE.NAT_TEST_CREATE_RECEIVER_INVITE"
readonly ACTION_START_SENDER="$PACKAGE.NAT_TEST_START_SENDER"
readonly ACTION_START_REMEMBERED_RECEIVER="$PACKAGE.NAT_TEST_START_REMEMBERED_RECEIVER"
readonly ACTION_QUERY_REMEMBERED="$PACKAGE.NAT_TEST_QUERY_REMEMBERED"
readonly ACTION_QUERY_TRANSFER="$PACKAGE.NAT_TEST_QUERY_TRANSFER"
readonly RELAY_URL="https://envoix.chkxwlyh.us:8444"
readonly DEVICE_OUTPUT_DIR="/sdcard/Download/Envoix"
readonly DEVICE_STAGING="/data/local/tmp/envoix-remembered-test-input"
readonly LABEL_ON_SENDER="receiver-emulator"
readonly LABEL_ON_RECEIVER="sender-emulator"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
emulator="$sdk_root/emulator/emulator"
adb="$sdk_root/platform-tools/adb"
gradlew="$repo_root/android/gradlew"
guard="$repo_root/scripts/with-build-cache-guard.sh"
apk="$repo_root/android/app/build/outputs/apk/debug/app-debug.apk"
broker_binary="$repo_root/target/release/envoix-rendezvous-server"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
log_dir="$repo_root/android/build/remembered-device-test/$run_id"
broker_log="$log_dir/broker.log"
broker_key="$log_dir/rendezvous-secret.key"

transfer_timeout=180
boot_timeout=240
broker_pid=""
emulator_a_pid=""
emulator_b_pid=""
broker_endpoint=""
device_input=""

usage() {
    cat <<EOF
Usage: $(basename "$0") [--timeout SECONDS] <avd-a> <avd-b> <test-file>

Build and launch two x86_64 Android emulators, establish a remembered
relationship through a local rendezvous broker, restart both apps, and verify
a second transfer through the remembered relationship.

The emulators use their default user-mode network. The local broker is reached
through Android's host alias (10.0.2.2), while the existing Envoix public relay
provides the data path. No root, sudo, TAP devices, or custom certificates are
required. Both AVDs must be distinct x86_64 images with Internet access.
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
    grep -Eq '^abi\.type[[:space:]]*=[[:space:]]*x86_64[[:space:]]*$' "$config" ||
        die "AVD '$1' is not x86_64"
}

cleanup() {
    set +e
    "$adb" -s "$SERIAL_A" shell rm -f "$DEVICE_STAGING" >/dev/null 2>&1
    if [ -n "$emulator_a_pid" ]; then
        "$adb" -s "$SERIAL_A" emu kill >/dev/null 2>&1
    fi
    if [ -n "$emulator_b_pid" ]; then
        "$adb" -s "$SERIAL_B" emu kill >/dev/null 2>&1
    fi
    if [ -n "$emulator_a_pid" ]; then
        wait "$emulator_a_pid" >/dev/null 2>&1
    fi
    if [ -n "$emulator_b_pid" ]; then
        wait "$emulator_b_pid" >/dev/null 2>&1
    fi
    if [ -n "$broker_pid" ]; then
        kill "$broker_pid" >/dev/null 2>&1
        wait "$broker_pid" >/dev/null 2>&1
    fi
}

capture_diagnostics() {
    local phase="$1"

    "$adb" -s "$SERIAL_A" logcat -d >"$log_dir/$phase-sender.log" 2>&1 || true
    "$adb" -s "$SERIAL_B" logcat -d >"$log_dir/$phase-receiver.log" 2>&1 || true
    "$adb" -s "$SERIAL_A" shell dumpsys activity services "$PACKAGE" \
        >"$log_dir/$phase-sender-services.log" 2>&1 || true
    "$adb" -s "$SERIAL_B" shell dumpsys activity services "$PACKAGE" \
        >"$log_dir/$phase-receiver-services.log" 2>&1 || true
}

wait_for_boot() {
    local serial="$1"
    local deadline=$((SECONDS + boot_timeout))

    while [ "$SECONDS" -lt "$deadline" ]; do
        if "$adb" -s "$serial" get-state >/dev/null 2>&1 &&
            [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; then
            "$adb" -s "$serial" shell input keyevent 82 >/dev/null 2>&1 || true
            return
        fi
        sleep 2
    done
    die "$serial did not finish booting within $boot_timeout seconds"
}

start_broker() {
    local deadline endpoint_id

    stdbuf -oL -eL "$broker_binary" \
        --bind 0.0.0.0:8445 \
        --secret-key "$broker_key" \
        >"$broker_log" 2>&1 &
    broker_pid=$!
    deadline=$((SECONDS + 30))
    endpoint_id=""
    while [ -z "$endpoint_id" ]; do
        endpoint_id="$(sed -n 's/^rendezvous endpoint id: //p' "$broker_log" | head -n 1)"
        kill -0 "$broker_pid" 2>/dev/null ||
            die "local broker exited; see $broker_log"
        [ "$SECONDS" -lt "$deadline" ] ||
            die "local broker did not become ready; see $broker_log"
        [ -n "$endpoint_id" ] || sleep 1
    done
    broker_endpoint="$endpoint_id@10.0.2.2:8445"
    printf 'Local broker: %s\nRelay:        %s\n' "$broker_endpoint" "$RELAY_URL"
}

reset_app() {
    local serial="$1"

    "$adb" -s "$serial" shell pm clear "$PACKAGE" >/dev/null
    "$adb" -s "$serial" shell am start -W -n "$ACTIVITY" >/dev/null
    "$adb" -s "$serial" shell pm grant \
        "$PACKAGE" android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
}

restart_app() {
    local serial="$1"

    "$adb" -s "$serial" shell am force-stop "$PACKAGE"
    "$adb" -s "$serial" shell am start -W -n "$ACTIVITY" >/dev/null
}

broadcast_data() {
    local serial="$1"
    local action="$2"
    local output data
    shift 2

    output="$("$adb" -s "$serial" shell am broadcast \
        -n "$TEST_RECEIVER" -a "$action" "$@" 2>&1)"
    data="$(printf '%s\n' "$output" |
        sed -n 's/.*result=-1, data="\([^"]*\)".*/\1/p' |
        tail -n 1 | tr -d '\r')"
    [ -n "$data" ] || die "debug bridge action $action failed on $serial: $output"
    printf '%s\n' "$data"
}

query_transfer_field() {
    local serial="$1"
    local direction="$2"
    local field="$3"
    local output

    output="$("$adb" -s "$serial" shell am broadcast \
        -n "$TEST_RECEIVER" -a "$ACTION_QUERY_TRANSFER" \
        --es direction "$direction" --es field "$field" 2>/dev/null || true)"
    printf '%s\n' "$output" |
        sed -n 's/.*result=-1, data="\([^"]*\)".*/\1/p' |
        tail -n 1 | tr -d '\r'
}

query_transfer_state() {
    query_transfer_field "$1" "$2" state
}

query_remembered() {
    broadcast_data "$1" "$ACTION_QUERY_REMEMBERED" --es remembered_label "$2"
}

wait_for_remembered() {
    local serial="$1"
    local label="$2"
    local deadline=$((SECONDS + 20))
    local record

    while [ "$SECONDS" -lt "$deadline" ]; do
        record="$(query_remembered "$serial" "$label" 2>/dev/null || true)"
        if [[ "$record" =~ ^[0-9a-fA-F-]+:[0-9]+:-?[0-9]+$ ]]; then
            printf '%s\n' "$record"
            return
        fi
        sleep 1
    done
    die "$serial did not persist remembered device '$label'"
}

clear_receiver_outputs() {
    "$adb" -s "$SERIAL_B" shell \
        "content delete --user 0 --uri content://media/external/file --where \"_data LIKE '/storage/emulated/0/Download/Envoix/envoix-remembered-test-input%'\"" \
        >/dev/null 2>&1 || true
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'envoix-remembered-test-input*' -exec rm -f {} \\; 2>/dev/null || true"
}

device_hashes() {
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'envoix-remembered-test-input*' -exec sha256sum {} \\; 2>/dev/null" |
        tr -d '\r'
}

wait_for_transfer() {
    local phase="$1"
    local deadline=$((SECONDS + transfer_timeout))
    local sender_state receiver_state sender_error receiver_error hashes

    sender_state=""
    receiver_state=""
    hashes=""
    while [ "$SECONDS" -lt "$deadline" ]; do
        sender_state="$(query_transfer_state "$SERIAL_A" sender || true)"
        receiver_state="$(query_transfer_state "$SERIAL_B" receiver || true)"
        hashes="$(device_hashes || true)"
        if [ "$sender_state" = delivered ] &&
            [ "$receiver_state" = delivered ] &&
            printf '%s\n' "$hashes" | grep -q "^$expected_hash "; then
            printf '[%s] PASS: delivered SHA-256 %s\n' "$phase" "$expected_hash"
            return
        fi
        if [ "$sender_state" = failed ] || [ "$receiver_state" = failed ]; then
            break
        fi
        sleep 1
    done

    capture_diagnostics "$phase"
    printf '[%s] sender=%s receiver=%s\n' \
        "$phase" "${sender_state:-unknown}" "${receiver_state:-unknown}" >&2
    sender_error="$(query_transfer_field "$SERIAL_A" sender error || true)"
    receiver_error="$(query_transfer_field "$SERIAL_B" receiver error || true)"
    [ -z "$sender_error" ] || printf '[%s] sender error: %s\n' "$phase" "$sender_error" >&2
    [ -z "$receiver_error" ] || printf '[%s] receiver error: %s\n' "$phase" "$receiver_error" >&2
    [ -z "$hashes" ] || printf '[%s] received candidates:\n%s\n' "$phase" "$hashes" >&2
    die "$phase transfer failed; see $log_dir"
}

stage_input() {
    local app_data

    "$adb" -s "$SERIAL_A" push "$test_file" "$DEVICE_STAGING" >/dev/null
    "$adb" -s "$SERIAL_A" shell run-as "$PACKAGE" mkdir -p cache/remembered-test
    "$adb" -s "$SERIAL_A" shell run-as "$PACKAGE" \
        cp "$DEVICE_STAGING" cache/remembered-test/envoix-remembered-test-input
    app_data="$("$adb" -s "$SERIAL_A" shell run-as "$PACKAGE" pwd | tr -d '\r')"
    device_input="$app_data/cache/remembered-test/envoix-remembered-test-input"
}

start_initial_pairing() {
    local room

    room="$(broadcast_data "$SERIAL_B" "$ACTION_CREATE_RECEIVER_INVITE" \
        --es broker "$broker_endpoint" \
        --es relay "$RELAY_URL" \
        --es remember_label "$LABEL_ON_RECEIVER")"
    [[ "$room" =~ ^[0-9]{6}-[a-z0-9]{4}-[a-z0-9]{4}$ ]] ||
        die "receiver returned an invalid Room Code"
    sleep 1
    broadcast_data "$SERIAL_A" "$ACTION_START_SENDER" \
        --es room "$room" \
        --es path "$device_input" \
        --es broker "$broker_endpoint" \
        --es relay "$RELAY_URL" \
        --es remember_label "$LABEL_ON_SENDER" >/dev/null
}

start_remembered_transfer() {
    broadcast_data "$SERIAL_B" "$ACTION_START_REMEMBERED_RECEIVER" \
        --es remembered_label "$LABEL_ON_RECEIVER" >/dev/null
    sleep 1
    broadcast_data "$SERIAL_A" "$ACTION_START_SENDER" \
        --es path "$device_input" \
        --es remembered_label "$LABEL_ON_SENDER" >/dev/null
}

assert_generation() {
    local record="$1"
    local expected_generation="$2"
    local expected_previous="$3"
    local relationship generation previous

    IFS=: read -r relationship generation previous <<<"$record"
    [ -n "$relationship" ] || die "remembered relationship id is missing"
    [ "$generation" = "$expected_generation" ] ||
        die "expected generation $expected_generation, got $generation"
    [ "$previous" = "$expected_previous" ] ||
        die "expected previous generation $expected_previous, got $previous"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --timeout)
            [ "$#" -ge 2 ] || die "--timeout requires a value"
            transfer_timeout="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        -*)
            die "unknown option: $1"
            ;;
        *)
            break
            ;;
    esac
done

[ "$#" -eq 3 ] || {
    usage
    exit 2
}
[ "$transfer_timeout" -gt 0 ] 2>/dev/null ||
    die "--timeout must be a positive integer"

avd_a="$1"
avd_b="$2"
test_file="$3"
[ "$avd_a" != "$avd_b" ] || die "use two distinct AVDs"
[ -f "$test_file" ] || die "test file not found at $test_file"
[ -x "$emulator" ] || die "Android Emulator not found at $emulator"
[ -x "$adb" ] || die "adb not found at $adb"
[ -x "$gradlew" ] || die "Gradle wrapper not found at $gradlew"
for command_name in awk cargo date grep sed sha256sum stdbuf; do
    command -v "$command_name" >/dev/null || die "$command_name is required"
done
cargo ndk --version >/dev/null 2>&1 || die "cargo-ndk is required"
check_avd "$avd_a"
check_avd "$avd_b"

mkdir -p "$log_dir"
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

"$adb" start-server >/dev/null
if "$adb" devices | grep -Eq '^emulator-(5554|5556)[[:space:]]'; then
    die "emulator ports 5554/5556 are already in use"
fi

expected_hash="$(sha256sum "$test_file" | awk '{print $1}')"
printf 'Building the local broker...\n'
"$guard" cargo build --release -p envoix-rendezvous-server
printf 'Building the x86_64 debug APK...\n'
ENVOIX_ANDROID_ABIS=x86_64 \
    "$guard" "$gradlew" --project-dir "$repo_root/android" :app:assembleDebug --no-daemon
[ -f "$apk" ] || die "Gradle did not produce $apk"

start_broker

"$emulator" "@$avd_a" -port 5554 -no-snapshot -no-window -no-audio -gpu swiftshader \
    >"$log_dir/emulator-5554.log" 2>&1 &
emulator_a_pid=$!
"$emulator" "@$avd_b" -port 5556 -no-snapshot -no-window -no-audio -gpu swiftshader \
    >"$log_dir/emulator-5556.log" 2>&1 &
emulator_b_pid=$!

printf 'Waiting for emulators to boot...\n'
wait_for_boot "$SERIAL_A"
wait_for_boot "$SERIAL_B"
for serial in "$SERIAL_A" "$SERIAL_B"; do
    install_output="$("$adb" -s "$serial" install -r -t "$apk" 2>&1)" ||
        die "failed to install APK on $serial: $install_output"
    reset_app "$serial"
    "$adb" -s "$serial" logcat -c
done

stage_input
clear_receiver_outputs

printf '\n[initial-pairing] Establishing a mutually remembered relationship...\n'
start_initial_pairing
wait_for_transfer initial-pairing

sender_record="$(wait_for_remembered "$SERIAL_A" "$LABEL_ON_SENDER")"
receiver_record="$(wait_for_remembered "$SERIAL_B" "$LABEL_ON_RECEIVER")"
assert_generation "$sender_record" 0 -1
assert_generation "$receiver_record" 0 -1

printf '\n[restart] Restarting both apps without clearing protected storage...\n'
restart_app "$SERIAL_A"
restart_app "$SERIAL_B"
sleep 2
[ "$(query_remembered "$SERIAL_A" "$LABEL_ON_SENDER")" = "$sender_record" ] ||
    die "sender remembered relationship changed after restart"
[ "$(query_remembered "$SERIAL_B" "$LABEL_ON_RECEIVER")" = "$receiver_record" ] ||
    die "receiver remembered relationship changed after restart"

printf '\n[remembered-reconnect] Transferring without a Room Code or QR payload...\n'
start_remembered_transfer
wait_for_transfer remembered-reconnect

sender_rotated="$(wait_for_remembered "$SERIAL_A" "$LABEL_ON_SENDER")"
receiver_rotated="$(wait_for_remembered "$SERIAL_B" "$LABEL_ON_RECEIVER")"
assert_generation "$sender_rotated" 1 0
assert_generation "$receiver_rotated" 1 0

restart_app "$SERIAL_A"
restart_app "$SERIAL_B"
sleep 2
[ "$(query_remembered "$SERIAL_A" "$LABEL_ON_SENDER")" = "$sender_rotated" ] ||
    die "sender rotation did not survive restart"
[ "$(query_remembered "$SERIAL_B" "$LABEL_ON_RECEIVER")" = "$receiver_rotated" ] ||
    die "receiver rotation did not survive restart"

if grep -Eq 'r1_[A-Za-z0-9_-]{43}' "$broker_log"; then
    die "broker log exposed a remembered room locator"
fi

printf '\nPASS: remembered relationship survived restart, reconnected, rotated, and transferred.\n'
printf 'Logs: %s\n' "$log_dir"
