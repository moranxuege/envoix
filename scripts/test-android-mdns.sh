#!/usr/bin/env bash
set -eu

readonly SERIAL_A="emulator-5554"
readonly SERIAL_B="emulator-5556"
readonly PACKAGE="dev.envoix.app"
readonly ACTIVITY="$PACKAGE/.MainActivity"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
emulator="$sdk_root/emulator/emulator"
adb="$sdk_root/platform-tools/adb"
apk="${APK:-$repo_root/android/app/build/outputs/apk/debug/app-debug.apk}"
started=0

usage() {
    cat <<EOF
Usage: $(basename "$0") <avd-a> <avd-b> [apk]

Launch two rootable Android Emulator AVDs on their shared virtual Wi-Fi,
install Envoix, and switch both devices between internet-enabled and
LAN-only modes for mDNS transfer testing.

Both AVDs must use x86_64 Google APIs images, not Google Play images.
The APK defaults to:
  $apk
EOF
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

avd_config() {
    local avd="$1"
    local ini="$HOME/.android/avd/$avd.ini"
    local path

    [ -f "$ini" ] || die "AVD '$avd' does not exist"
    path="$(sed -n 's/^path=//p' "$ini" | head -n 1)"
    [ -n "$path" ] || path="$HOME/.android/avd/$avd.avd"
    printf '%s/config.ini\n' "$path"
}

check_avd() {
    local avd="$1"
    local config
    config="$(avd_config "$avd")"

    [ -f "$config" ] || die "missing config.ini for AVD '$avd'"
    grep -Fqx 'abi.type=x86_64' "$config" || die "AVD '$avd' is not x86_64"
    if grep -Fqx 'PlayStore.enabled=true' "$config"; then
        die "AVD '$avd' uses a Google Play image; create a rootable Google APIs AVD"
    fi
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

wait_for_wifi_address() {
    local serial="$1"
    local deadline=$((SECONDS + 60))

    until "$adb" -s "$serial" shell ip -4 -o address show dev wlan0 scope global 2>/dev/null | grep -q 'inet '; do
        [ "$SECONDS" -lt "$deadline" ] ||
            die "$serial did not join the shared emulator Wi-Fi (wlan0 has no IPv4 address)"
        sleep 1
    done
}

connect_wifi() {
    local serial="$1"

    "$adb" -s "$serial" shell svc wifi enable
    "$adb" -s "$serial" shell cmd wifi connect-network AndroidWifi open -r persistent >/dev/null
    wait_for_wifi_address "$serial"
}

wifi_address() {
    "$adb" -s "$1" shell ip -4 -o address show dev wlan0 scope global |
        awk '{print $4}' | cut -d/ -f1 | tr -d '\r'
}

show_network() {
    local serial="$1"
    printf '\n%s\n' "$serial"
    "$adb" -s "$serial" shell ip -4 -brief address show | tr -d '\r'
    "$adb" -s "$serial" shell ip -4 route show | tr -d '\r'
    "$adb" -s "$serial" shell ip -4 route show table all | sed -n '/^default /p' | tr -d '\r'
    if "$adb" -s "$serial" shell iptables -S ENVOIX_OFFLINE >/dev/null 2>&1; then
        printf '[LAN-only firewall active]\n'
    fi
}

disable_firewall() {
    local serial="$1"

    "$adb" -s "$serial" shell \
        'iptables -D OUTPUT -j ENVOIX_OFFLINE 2>/dev/null || true; iptables -F ENVOIX_OFFLINE 2>/dev/null || true; iptables -X ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -D OUTPUT -j ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -F ENVOIX_OFFLINE 2>/dev/null || true; ip6tables -X ENVOIX_OFFLINE 2>/dev/null || true'
}

enable_firewall() {
    local serial="$1"

    disable_firewall "$serial"
    "$adb" -s "$serial" shell \
        'iptables -N ENVOIX_OFFLINE; iptables -A ENVOIX_OFFLINE -o lo -j ACCEPT; iptables -A ENVOIX_OFFLINE -d 10.0.2.0/24 -j ACCEPT; iptables -A ENVOIX_OFFLINE -d 224.0.0.0/4 -j ACCEPT; iptables -A ENVOIX_OFFLINE -j REJECT; iptables -I OUTPUT 1 -j ENVOIX_OFFLINE; ip6tables -N ENVOIX_OFFLINE; ip6tables -A ENVOIX_OFFLINE -o lo -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -d fe80::/10 -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -d ff00::/8 -j ACCEPT; ip6tables -A ENVOIX_OFFLINE -j REJECT; ip6tables -I OUTPUT 1 -j ENVOIX_OFFLINE'
}

go_offline() {
    enable_firewall "$SERIAL_A"
    enable_firewall "$SERIAL_B"
    sleep 8
    printf '\nLAN-only mode enabled. Non-LAN traffic is blocked; Wi-Fi and mDNS multicast remain allowed.\n'
    show_network "$SERIAL_A"
    show_network "$SERIAL_B"
}

go_online() {
    disable_firewall "$SERIAL_A"
    disable_firewall "$SERIAL_B"
    printf '\nInternet mode restored.\n'
    show_network "$SERIAL_A"
    show_network "$SERIAL_B"
}

cleanup() {
    if [ "$started" -eq 1 ]; then
        "$adb" -s "$SERIAL_A" emu kill >/dev/null 2>&1 || true
        "$adb" -s "$SERIAL_B" emu kill >/dev/null 2>&1 || true
    fi
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] || {
    usage
    exit 2
}

avd_a="$1"
avd_b="$2"
[ "$avd_a" != "$avd_b" ] || die "use two distinct AVDs so their app data and identities are independent"
[ "$#" -lt 3 ] || apk="$3"

[ -x "$emulator" ] || die "Android Emulator not found at $emulator"
[ -x "$adb" ] || die "adb not found at $adb"
[ -f "$apk" ] || die "APK not found at $apk; build it first or pass its path as the third argument"
check_avd "$avd_a"
check_avd "$avd_b"

if "$adb" devices | grep -Eq '^emulator-(5554|5556)[[:space:]]'; then
    die "emulator ports 5554/5556 are already in use"
fi

trap cleanup EXIT INT TERM
mkdir -p "$repo_root/android/build/mdns-test"
"$emulator" "@$avd_a" -port 5554 -feature WiFiPacketStream -no-snapshot -netdelay none -netspeed full \
    >"$repo_root/android/build/mdns-test/emulator-5554.log" 2>&1 &
"$emulator" "@$avd_b" -port 5556 -feature WiFiPacketStream -no-snapshot -netdelay none -netspeed full \
    >"$repo_root/android/build/mdns-test/emulator-5556.log" 2>&1 &
started=1

printf 'Waiting for %s and %s to boot...\n' "$avd_a" "$avd_b"
wait_for_boot "$SERIAL_A"
wait_for_boot "$SERIAL_B"
root_device "$SERIAL_A"
root_device "$SERIAL_B"

connect_wifi "$SERIAL_A"
connect_wifi "$SERIAL_B"

for serial in "$SERIAL_A" "$SERIAL_B"; do
    if ! install_output="$("$adb" -s "$serial" install -r -t "$apk" 2>&1)"; then
        die "failed to install APK on $serial: $install_output"
    fi
    "$adb" -s "$serial" shell am start -n "$ACTIVITY" >/dev/null
done

# Package installation and Activity startup can briefly reset connectivity on
# some system images. Reassociate, then require both peers to be ready at once.
connect_wifi "$SERIAL_A"
connect_wifi "$SERIAL_B"
wifi_a="$(wifi_address "$SERIAL_A")"
wifi_b="$(wifi_address "$SERIAL_B")"
[ -n "$wifi_a" ] || die "$SERIAL_A lost its shared Wi-Fi address during setup"
[ -n "$wifi_b" ] || die "$SERIAL_B lost its shared Wi-Fi address during setup"
[ "$wifi_a" != "$wifi_b" ] ||
    die "emulators received the same Wi-Fi address ($wifi_a); shared networking is not active"

printf '\nBoth Envoix instances are running on shared virtual Wi-Fi.\n'
show_network "$SERIAL_A"
show_network "$SERIAL_B"

cat <<'EOF'

Test procedure:
  Online mDNS:  disable "Internet pairing", enable "Local Wi-Fi pairing"
                on both devices, then transfer a file.
  Offline mDNS: enable both pairing modes, choose [o] below, then transfer again.
                The firewall blocks the broker but leaves mDNS and LAN traffic.

Commands: [o] LAN-only/offline  [i] restore internet  [s] show routes  [q] quit
EOF

while true; do
    printf '\nmdns-test> '
    read -r command || command="q"
    case "$command" in
        o|offline) go_offline ;;
        i|online) go_online ;;
        s|status)
            show_network "$SERIAL_A"
            show_network "$SERIAL_B"
            ;;
        q|quit|exit) break ;;
        *) printf 'Use o, i, s, or q.\n' ;;
    esac
done
