#!/usr/bin/env bash
set -eu

readonly SERIAL_A="emulator-5554"
readonly SERIAL_B="emulator-5556"
readonly PACKAGE="dev.envoix.app"
readonly ACTIVITY="$PACKAGE/.MainActivity"
readonly ACTION_CREATE_RECEIVER_INVITE="$PACKAGE.NAT_TEST_CREATE_RECEIVER_INVITE"
readonly ACTION_START_SENDER="$PACKAGE.NAT_TEST_START_SENDER"
readonly ACTION_QUERY_TRANSFER="$PACKAGE.NAT_TEST_QUERY_TRANSFER"
readonly NAT_TEST_RECEIVER="$PACKAGE/.NatTestReceiver"
readonly DEVICE_INPUT="/data/user/0/$PACKAGE/cache/nat-test-input"
readonly DEVICE_OUTPUT_DIR="/sdcard/Download/Envoix"
readonly TRANSFER_RATE_KBITS=4194 # Approximately 512 KiB/s.
readonly TRANSFER_QUEUE_BYTES=33554432 # 32 MiB.
readonly AVAILABLE_TESTS=(
    symmetric-both-ipv4
    friendly-both-ipv4
    symmetric-one-side-ipv4
    symmetric-both-ipv6
)

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
emulator="$sdk_root/emulator/emulator"
adb="$sdk_root/platform-tools/adb"
readonly apk="$repo_root/android/app/build/outputs/apk/debug/app-debug.apk"
readonly relay_host="relay.nat-test.envoix"
readonly relay_url="https://$relay_host:8444"
timeout=120
verbose=0
run_spec=all
started=0
network_ready=0
network_setup_started=0
passed=0
failed=0
dnsmasq_a_pid=""
dnsmasq_b_pid=""
broker_pid=""
relay_pid=""
broker_endpoint=""
jni_replaced=0
jni_had_original=0
device_ip_a=""
device_ip_b=""

# Interface names must fit Linux's 15-character limit. The PID avoids clashes
# with stale names from an interrupted run.
readonly net_id="$((BASHPID % 100000))"
readonly ns_a="enx${net_id}a"
readonly ns_b="enx${net_id}b"
readonly tap_a="ex${net_id}ta"
readonly tap_b="ex${net_id}tb"
readonly lan_bridge_a="ex${net_id}la"
readonly lan_bridge_b="ex${net_id}lb"
readonly wan_bridge="ex${net_id}w"
readonly veth_lan_a="ex${net_id}al"
readonly veth_lan_b="ex${net_id}bl"
readonly veth_wan_a="ex${net_id}aw"
readonly veth_wan_b="ex${net_id}bw"

usage() {
    cat <<EOF
Usage: $(basename "$0") [options] <avd-a> <avd-b> <test-file>

Launch two rootable Android emulators on isolated TAP-backed Wi-Fi networks
and transfer the test file under the selected network profiles.
EOF
    printf '  %s\n' "${AVAILABLE_TESTS[@]}"
    cat <<EOF

The script generates a private test CA, builds a CA-enabled x86_64 JNI library
and APK, and runs a local rendezvous broker and iroh relay on the simulated WAN.
Each received file is checked against the source SHA-256. The selected data
path (direct peer address or relay URL) is read from each app through ADB.
This Linux-only test requires sudo, iproute2, nftables, iptables, dnsmasq,
OpenSSL, cargo-ndk, and the Android SDK/NDK.

Both AVDs must use x86_64 Google APIs images, not Google Play images.
Startup removes stale NAT-test resources and stops the global netsimd process.
The sender's Wi-Fi bandwidth is limited to approximately 512 KiB/s.
In symmetric-one-side-ipv4, side A uses symmetric NAT while side B is directly
routed on the test WAN with no address translation or inbound firewall.

Options:
  --timeout SECONDS   Per-transfer timeout (default: $timeout)
  --run TESTS         Comma-separated test names, or "all" (default: all)
  --list-tests        List available test names and exit
  --verbose           Print transfer states and always capture diagnostics
  -h, --help          Show this help
EOF
}

list_tests() {
    printf '%s\n' "${AVAILABLE_TESTS[@]}"
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

select_tests() {
    local spec="$1"
    local candidate selected
    local -a requested

    if [ "$spec" = all ]; then
        selected_tests=("${AVAILABLE_TESTS[@]}")
        return
    fi
    case "$spec" in
        ""|,*|*,|*,,*) die "--run requires a non-empty comma-separated test list" ;;
    esac

    IFS=',' read -r -a requested <<<"$spec"
    selected_tests=()
    for candidate in "${requested[@]}"; do
        if ! printf '%s\n' "${AVAILABLE_TESTS[@]}" | grep -Fqx "$candidate"; then
            die "unknown test '$candidate'; use --list-tests to see available tests"
        fi
        for selected in "${selected_tests[@]}"; do
            [ "$candidate" != "$selected" ] || die "test '$candidate' is listed more than once"
        done
        selected_tests+=("$candidate")
    done
}

privileged() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    else
        sudo "$@"
    fi
}

in_namespace() {
    privileged ip netns exec "$1" "${@:2}"
}

kill_processes_using_binary() {
    local binary="$1"
    local signal="$2"
    local actual pid proc_exe target

    target="$(readlink -f "$binary" 2>/dev/null || true)"
    # Bare `return` yields the failed test's status (1) under `set -e`, aborting
    # the caller on a clean box where the binary doesn't exist yet. Return 0.
    [ -n "$target" ] || return 0
    for proc_exe in /proc/[0-9]*/exe; do
        actual="$(readlink -f "$proc_exe" 2>/dev/null || true)"
        actual="${actual% (deleted)}"
        [ "$actual" = "$target" ] || continue
        pid="${proc_exe#/proc/}"
        pid="${pid%/exe}"
        privileged kill "-$signal" "$pid" >/dev/null 2>&1 || true
    done
}

preflight_cleanup() {
    local deadline link namespace pids stale_links stale_namespaces upstream

    printf 'Cleaning up stale NAT-test resources...\n'
    "$adb" -s "$SERIAL_A" emu kill >/dev/null 2>&1 || true
    "$adb" -s "$SERIAL_B" emu kill >/dev/null 2>&1 || true
    privileged pkill -KILL -x netsimd >/dev/null 2>&1 || true
    deadline=$((SECONDS + 15))
    while "$adb" devices | grep -Eq '^emulator-(5554|5556)[[:space:]]'; do
        [ "$SECONDS" -lt "$deadline" ] || break
        sleep 1
    done

    kill_processes_using_binary "$relay_binary" TERM
    kill_processes_using_binary "$repo_root/target/release/envoix-rendezvous-server" TERM
    sleep 1
    kill_processes_using_binary "$relay_binary" KILL
    kill_processes_using_binary "$repo_root/target/release/envoix-rendezvous-server" KILL

    stale_namespaces="$(privileged ip netns list 2>/dev/null |
        awk '$1 ~ /^enx[0-9]+[ab]$/ { print $1 }')"
    for namespace in $stale_namespaces; do
        pids="$(privileged ip netns pids "$namespace" 2>/dev/null || true)"
        [ -z "$pids" ] || privileged kill -KILL $pids >/dev/null 2>&1 || true
        privileged ip netns delete "$namespace" >/dev/null 2>&1 || true
    done

    stale_links="$(privileged ip -o link show 2>/dev/null |
        awk -F ': ' '{ sub(/@.*/, "", $2); if ($2 ~ /^ex[0-9]+(ta|tb|la|lb|w|al|bl|aw|bw)$/) print $2 }')"
    upstream="$(ip -4 route get 1.1.1.1 2>/dev/null |
        awk '{for (i=1; i<=NF; i++) if ($i == "dev") {print $(i+1); exit}}')"
    for link in $stale_links; do
        if [[ "$link" =~ ^ex[0-9]+w$ ]] && [ -n "$upstream" ]; then
            while privileged iptables -C FORWARD -i "$link" -o "$link" -j ACCEPT \
                >/dev/null 2>&1; do
                privileged iptables -D FORWARD -i "$link" -o "$link" -j ACCEPT
            done
            while privileged iptables -C FORWARD -i "$link" -o "$upstream" -j ACCEPT \
                >/dev/null 2>&1; do
                privileged iptables -D FORWARD -i "$link" -o "$upstream" -j ACCEPT
            done
            while privileged iptables -C FORWARD -i "$upstream" -o "$link" \
                -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT >/dev/null 2>&1; do
                privileged iptables -D FORWARD -i "$upstream" -o "$link" \
                    -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
            done
        fi
        if [[ "$link" =~ ^ex[0-9]+t[ab]$ ]]; then
            privileged ip tuntap delete dev "$link" mode tap >/dev/null 2>&1 || true
        else
            privileged ip link delete "$link" >/dev/null 2>&1 || true
        fi
    done
    if [ -n "$upstream" ]; then
        while privileged iptables -t nat -C POSTROUTING -s 192.168.102.0/24 \
            -o "$upstream" -j MASQUERADE >/dev/null 2>&1; do
            privileged iptables -t nat -D POSTROUTING -s 192.168.102.0/24 \
                -o "$upstream" -j MASQUERADE
        done
        while privileged iptables -t nat -C POSTROUTING -s 198.18.0.0/24 \
            -o "$upstream" -j MASQUERADE >/dev/null 2>&1; do
            privileged iptables -t nat -D POSTROUTING -s 198.18.0.0/24 \
                -o "$upstream" -j MASQUERADE
        done
    fi
}

prepare_build() {
    printf 'Generating the NAT-test CA and relay certificate...\n'
    mkdir -p "$cert_dir" "$tool_root"
    rm -f "$cert_dir/ca.der" "$cert_dir/ca.key" "$cert_dir/ca.pem" \
        "$cert_dir/relay.csr" "$cert_dir/relay.key" "$cert_dir/relay.pem"
    umask 077
    openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
        -keyout "$cert_dir/ca.key" -out "$cert_dir/ca.pem" \
        -subj '/CN=Envoix NAT Test CA' >/dev/null 2>&1
    openssl req -new -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/relay.key" -out "$cert_dir/relay.csr" \
        -subj "/CN=$relay_host" -addext "subjectAltName=DNS:$relay_host" >/dev/null 2>&1
    openssl x509 -req -days 2 -in "$cert_dir/relay.csr" \
        -CA "$cert_dir/ca.pem" -CAkey "$cert_dir/ca.key" -set_serial 1 \
        -copy_extensions copy -out "$cert_dir/relay.pem" >/dev/null 2>&1
    openssl x509 -in "$cert_dir/ca.pem" -outform DER -out "$cert_dir/ca.der"

    if [ ! -x "$relay_binary" ]; then
        printf 'Building iroh-relay 1.0.0...\n'
        cargo install --locked --root "$tool_root" --version 1.0.0 \
            --features server iroh-relay
    fi

    printf 'Building the local broker...\n'
    cargo build --release -p envoix-rendezvous-server
    printf 'Building the CA-enabled x86_64 JNI library...\n'
    ENVOIX_NAT_TEST_CA_DER_PATH="$cert_dir/ca.der" \
        cargo ndk -t x86_64 --platform 26 build --release \
        -p envoix-android-jni
    mkdir -p "$(dirname "$staged_jni")"
    if [ -f "$staged_jni" ]; then
        cp "$staged_jni" "$jni_backup"
        jni_had_original=1
    fi
    cp "$repo_root/target/x86_64-linux-android/release/libenvoix_jni.so" \
        "$staged_jni"
    jni_replaced=1
    printf 'Building the debug APK...\n'
    (cd "$repo_root/android" && \
        ENVOIX_ANDROID_ABIS=x86_64 \
        ENVOIX_NAT_TEST_CA_DER_PATH="$cert_dir/ca.der" \
        ./gradlew assembleDebug --no-daemon --rerun-tasks)
    [ -f "$apk" ] || die "Gradle did not produce $apk"
}

write_relay_config() {
    cat >"$relay_config" <<EOF
enable_relay = true
enable_quic_addr_discovery = true
enable_metrics = false
http_bind_addr = "198.18.0.1:8080"

[tls]
https_bind_addr = "198.18.0.1:8444"
quic_bind_addr = "198.18.0.1:7842"
cert_mode = "Manual"
manual_cert_path = "$cert_dir/relay.pem"
manual_key_path = "$cert_dir/relay.key"
EOF
}

start_local_servers() {
    local deadline endpoint_id

    write_relay_config
    "$relay_binary" --config-path "$relay_config" >"$log_dir/relay.log" 2>&1 &
    relay_pid=$!
    deadline=$((SECONDS + 30))
    until curl --silent --show-error --output /dev/null \
        --cacert "$cert_dir/ca.pem" --resolve "$relay_host:8444:198.18.0.1" \
        "$relay_url/"; do
        kill -0 "$relay_pid" 2>/dev/null || die "local relay exited; see $log_dir/relay.log"
        [ "$SECONDS" -lt "$deadline" ] || die "local relay did not become ready"
        sleep 1
    done

    stdbuf -oL -eL "$repo_root/target/release/envoix-rendezvous-server" \
        --bind 198.18.0.1:8445 --secret-key "$log_dir/rendezvous-secret.key" \
        >"$log_dir/broker.log" 2>&1 &
    broker_pid=$!
    deadline=$((SECONDS + 30))
    endpoint_id=""
    while [ -z "$endpoint_id" ]; do
        endpoint_id="$(sed -n 's/^rendezvous endpoint id: //p' "$log_dir/broker.log" | head -n 1)"
        kill -0 "$broker_pid" 2>/dev/null || die "local broker exited; see $log_dir/broker.log"
        [ "$SECONDS" -lt "$deadline" ] || die "local broker did not become ready"
        [ -n "$endpoint_id" ] || sleep 1
    done
    broker_endpoint="$endpoint_id@198.18.0.1:8445"
    printf 'Local broker: %s\nLocal relay:  %s\n' "$broker_endpoint" "$relay_url"
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
    # Tolerate both `key=value` and `key = value` config.ini spacing (Android
    # Studio / avdmanager differ), so a compatible AVD isn't falsely rejected.
    grep -Eq '^abi\.type[[:space:]]*=[[:space:]]*x86_64[[:space:]]*$' "$config" ||
        die "AVD '$1' is not x86_64"
    ! grep -Eq '^PlayStore\.enabled[[:space:]]*=[[:space:]]*true[[:space:]]*$' "$config" ||
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

wait_for_wifi() {
    local serial="$1"
    local expected_subnet="$2"
    local deadline=$((SECONDS + 60))
    local next_report=$SECONDS
    local network_ids network_id connect_output

    "$adb" -s "$serial" shell cmd connectivity airplane-mode enable >/dev/null
    until "$adb" -s "$serial" shell dumpsys wifi 2>/dev/null |
        grep -q '^AirplaneModeOn true'; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            capture_startup_diagnostics "airplane-mode-timeout"
            die "$serial Wi-Fi stack did not enter airplane mode; see $log_dir/startup-*"
        fi
        sleep 1
    done
    "$adb" -s "$serial" shell svc wifi enable
    until "$adb" -s "$serial" shell cmd wifi status 2>/dev/null |
        grep -q '^Wifi is enabled'; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            capture_startup_diagnostics "wifi-enable-timeout"
            die "$serial Wi-Fi did not enable after airplane mode; see $log_dir/startup-*"
        fi
        sleep 1
    done
    network_ids="$("$adb" -s "$serial" shell cmd wifi list-networks 2>/dev/null |
        awk '$2 == "AndroidWifi" { print $1 }' | tr -d '\r')"
    for network_id in $network_ids; do
        "$adb" -s "$serial" shell cmd wifi forget-network "$network_id" >/dev/null 2>&1 || true
    done
    "$adb" -s "$serial" shell ip -4 address flush dev wlan0 scope global 2>/dev/null || true
    connect_output="$("$adb" -s "$serial" shell \
        cmd wifi connect-network AndroidWifi open -r persistent 2>&1 || true)"
    printf '%s\n' "$connect_output" >"$log_dir/$serial-wifi-connect.log"
    if [ "$verbose" -eq 1 ]; then
        printf '[wifi] %s connect-network: %s\n' "$serial" \
            "${connect_output//$'\n'/; }"
    fi
    until "$adb" -s "$serial" shell ip -4 -o address show dev wlan0 scope global 2>/dev/null |
        grep -q "inet $expected_subnet"; do
        if [ "$verbose" -eq 1 ] && [ "$SECONDS" -ge "$next_report" ]; then
            printf '[wifi] %s: %s; %s\n' "$serial" \
                "$("$adb" -s "$serial" shell ip -brief link show wlan0 2>/dev/null | tr -d '\r')" \
                "$("$adb" -s "$serial" shell ip -4 -brief address show wlan0 2>/dev/null | tr -d '\r')"
            next_report=$((SECONDS + 5))
        fi
        if [ "$SECONDS" -ge "$deadline" ]; then
            capture_startup_diagnostics "wifi-address-timeout"
            die "$serial did not obtain a TAP-backed Wi-Fi address in $expected_subnet; see $log_dir/startup-*"
        fi
        sleep 1
    done

    # eth0 uses the emulator's built-in user-mode NAT and would bypass the
    # profile under test. ADB remains available through the emulator bridge.
    "$adb" -s "$serial" shell ip link set eth0 down 2>/dev/null || true
}

wait_for_validation() {
    local serial="$1"
    local deadline=$((SECONDS + 60))

    until "$adb" -s "$serial" shell cmd wifi status 2>/dev/null |
        grep 'NetworkCapabilities:' | grep -q 'VALIDATED'; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            capture_startup_diagnostics "wifi-validation-timeout"
            die "$serial TAP-backed Wi-Fi did not gain Internet validation; see $log_dir/startup-*"
        fi
        sleep 2
    done
}

wait_for_wifi_route() {
    local serial="$1"
    local gateway="$2"
    local expected_subnet="$3"
    local deadline=$((SECONDS + 15))
    local retried=0

    until "$adb" -s "$serial" shell ip -4 route show table all 2>/dev/null |
        grep -q "^default via $gateway dev wlan0"; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            if [ "$retried" -eq 0 ]; then
                printf '[wifi] %s has no Wi-Fi default route; reconnecting once...\n' "$serial"
                wait_for_wifi "$serial" "$expected_subnet"
                retried=1
                deadline=$((SECONDS + 15))
            else
                capture_startup_diagnostics "wifi-route-timeout"
                die "$serial did not install its TAP-backed Wi-Fi default route; see $log_dir/startup-*"
            fi
        fi
        sleep 1
    done
}

configure_test_validation() {
    local serial="$1"

    # The broker and relay live on the isolated simulated WAN. Public Google
    # captive-portal probes are unrelated to the path under test and make
    # startup depend on the host's current Internet/DNS behavior. These AVDs
    # are disposable, so tell Android to trust the test Wi-Fi for this boot.
    "$adb" -s "$serial" shell settings put global captive_portal_mode 0
    "$adb" -s "$serial" shell settings put global private_dns_mode off
}

capture_startup_diagnostics() {
    local reason="$1"
    local serial role

    printf 'Capturing startup diagnostics (%s)...\n' "$reason" >&2
    for role in a b; do
        if [ "$role" = a ]; then serial="$SERIAL_A"; else serial="$SERIAL_B"; fi
        if "$adb" -s "$serial" get-state >/dev/null 2>&1; then
            "$adb" -s "$serial" shell \
                'echo "== addresses =="; ip -details -brief link; ip -brief address; echo "== routes =="; ip -4 rule; ip -4 route show table all; ip -6 route show table all; echo "== IPv6 sysctls =="; for key in disable_ipv6 accept_ra autoconf accept_ra_defrtr; do printf "%s=" "$key"; cat "/proc/sys/net/ipv6/conf/wlan0/$key"; done; echo "== Wi-Fi =="; cmd wifi status; dumpsys wifi' \
                >"$log_dir/startup-$role-network.log" 2>&1 || true
            "$adb" -s "$serial" logcat -b all -d \
                >"$log_dir/startup-$role-logcat.log" 2>&1 || true
        else
            printf '%s is unavailable to adb\n' "$serial" \
                >"$log_dir/startup-$role-network.log"
        fi
    done

    {
        printf 'reason=%s\n\n== host links ==\n' "$reason"
        privileged ip -details -brief link
        printf '\n== bridge ports ==\n'
        privileged bridge -details link show
        printf '\n== bridge forwarding databases ==\n'
        privileged bridge fdb show br "$lan_bridge_a"
        privileged bridge fdb show br "$lan_bridge_b"
        printf '\n== router A ==\n'
        in_namespace "$ns_a" ip -details -brief address
        in_namespace "$ns_a" ip -4 route show table all
        in_namespace "$ns_a" ip -6 route show table all
        printf '\n== router B ==\n'
        in_namespace "$ns_b" ip -details -brief address
        in_namespace "$ns_b" ip -4 route show table all
        in_namespace "$ns_b" ip -6 route show table all
        printf '\n== host forwarding ==\n'
        privileged iptables -S FORWARD
        privileged iptables -t nat -S POSTROUTING
    } >"$log_dir/startup-host-network.log" 2>&1 || true

    if [ "$verbose" -eq 1 ]; then
        printf '%s\n' '--- emulator A Wi-Fi snapshot ---'
        sed -n '1,120p' "$log_dir/startup-a-network.log"
        printf '%s\n' '--- emulator B Wi-Fi snapshot ---'
        sed -n '1,120p' "$log_dir/startup-b-network.log"
        printf '%s\n' '--- dnsmasq A ---'
        sed -n '1,120p' "$log_dir/dnsmasq-a.log"
        printf '%s\n' '--- dnsmasq B ---'
        sed -n '1,120p' "$log_dir/dnsmasq-b.log"
    fi
}

wifi_address() {
    "$adb" -s "$1" shell ip -4 -o address show dev wlan0 scope global |
        awk '{print $4}' | head -n 1 | cut -d/ -f1 | tr -d '\r'
}

wifi_ipv6_address() {
    "$adb" -s "$1" shell ip -6 -o address show dev wlan0 scope global |
        awk '{print $4}' | head -n 1 | cut -d/ -f1 | tr -d '\r'
}

set_device_ipv6() {
    local serial="$1"
    local enabled="$2"
    local expected_prefix="$3"
    local expected_ipv4_subnet="$4"
    local deadline=$((SECONDS + 30))
    local next_report=$SECONDS

    if [ "$enabled" -eq 0 ]; then
        "$adb" -s "$serial" shell \
            'echo 1 > /proc/sys/net/ipv6/conf/wlan0/disable_ipv6; ip -6 address flush dev wlan0 scope global'
        if [ -n "$(wifi_ipv6_address "$serial")" ]; then
            die "$serial retained a global IPv6 address after IPv6 was disabled"
        fi
        return
    fi

    "$adb" -s "$serial" shell \
        'echo 0 > /proc/sys/net/ipv6/conf/wlan0/disable_ipv6'

    # Android's existing IpClient session does not restart SLAAC merely because
    # disable_ipv6 changes. Reconnect Wi-Fi so IpClient processes the RAs again.
    wait_for_wifi "$serial" "$expected_ipv4_subnet"
    wait_for_wifi_route "$serial" "${expected_ipv4_subnet}1" "$expected_ipv4_subnet"
    wait_for_validation "$serial"
    deadline=$((SECONDS + 30))
    until "$adb" -s "$serial" shell \
        ip -6 -o address show dev wlan0 scope global 2>/dev/null |
        grep -q "inet6 $expected_prefix"; do
        if [ "$verbose" -eq 1 ] && [ "$SECONDS" -ge "$next_report" ]; then
            printf '[ipv6] %s: %s; routes: %s\n' "$serial" \
                "$("$adb" -s "$serial" shell ip -6 -brief address show wlan0 2>/dev/null | tr -d '\r')" \
                "$("$adb" -s "$serial" shell ip -6 route show dev wlan0 2>/dev/null | tr -d '\r\n')"
            next_report=$((SECONDS + 5))
        fi
        if [ "$SECONDS" -ge "$deadline" ]; then
            capture_startup_diagnostics "ipv6-address-timeout"
            die "$serial did not obtain an IPv6 address in $expected_prefix; see $log_dir/startup-*"
        fi
        sleep 1
    done
}

clear_bandwidth_limit() {
    "$adb" -s "$1" shell 'tc qdisc del dev wlan0 root 2>/dev/null || true'
}

limit_bandwidth() {
    "$adb" -s "$1" shell \
        "tc qdisc replace dev wlan0 root tbf rate ${TRANSFER_RATE_KBITS}kbit burst 64kb limit $TRANSFER_QUEUE_BYTES"
}

reset_app() {
    local serial="$1"

    "$adb" -s "$serial" shell pm clear "$PACKAGE" >/dev/null
    "$adb" -s "$serial" shell am start -W -n "$ACTIVITY" >/dev/null
    "$adb" -s "$serial" shell pm grant "$PACKAGE" android.permission.POST_NOTIFICATIONS 2>/dev/null || true
}

start_sender() {
    local serial="$1"
    local room="$2"
    local path="$3"
    local output

    output="$("$adb" -s "$serial" shell am broadcast \
        -n "$NAT_TEST_RECEIVER" -a "$ACTION_START_SENDER" \
        --es room "$room" --es path "$path" \
        --es broker "$broker_endpoint" --es relay "$relay_url" 2>&1)"
    if ! printf '%s\n' "$output" | grep -q 'result=-1, data="started"'; then
        die "failed to start sender Manifest V2 session on $serial: $output"
    fi
}

start_creator_receiver() {
    local output room

    output="$("$adb" -s "$SERIAL_B" shell am broadcast \
        -n "$NAT_TEST_RECEIVER" -a "$ACTION_CREATE_RECEIVER_INVITE" \
        --es broker "$broker_endpoint" --es relay "$relay_url" 2>&1)"
    room="$(printf '%s\n' "$output" |
        sed -n 's/.*result=-1, data="\([^"]*\)".*/\1/p' |
        tail -n 1)"
    if ! [[ "$room" =~ ^[0-9]{6}-[a-z0-9]{4}-[a-z0-9]{4}$ ]]; then
        die "failed to create receiver InviteV2 on $SERIAL_B: $output"
    fi
    printf '%s\n' "$room"
}

clear_receiver_outputs() {
    local output

    if ! output="$("$adb" -s "$SERIAL_B" shell \
        "content delete --user 0 --uri content://media/external/file --where \"_data LIKE '/storage/emulated/0/Download/Envoix/nat-test-input%'\"" \
        2>&1)"; then
        die "failed to clear prior NAT-test MediaStore rows on $SERIAL_B: $output"
    fi
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'nat-test-input*' -exec rm -f {} \\; 2>/dev/null || true"
}

wait_for_transfer_record() {
    local serial="$1"
    local role="$2"
    local profile="$3"
    local deadline=$((SECONDS + 15))

    while [ "$SECONDS" -lt "$deadline" ]; do
        if [ -n "$(record_state "$serial" "$role" || true)" ]; then
            return
        fi
        sleep 1
    done
    capture_diagnostics "$profile"
    die "$serial $role session did not expose transfer state; see $log_dir/$profile-$role-*"
}

device_hashes() {
    "$adb" -s "$SERIAL_B" shell \
        "find '$DEVICE_OUTPUT_DIR' -maxdepth 1 -type f -name 'nat-test-input*' -exec sha256sum {} \\; 2>/dev/null" |
        tr -d '\r'
}

query_transfer_field() {
    local serial="$1"
    local role="$2"
    local field="$3"
    local output

    output="$("$adb" -s "$serial" shell am broadcast \
        -n "$NAT_TEST_RECEIVER" -a "$ACTION_QUERY_TRANSFER" \
        --es direction "$role" --es field "$field" 2>/dev/null || true)"
    printf '%s\n' "$output" |
        sed -n 's/.*result=-1, data="\([^"]*\)".*/\1/p' |
        tail -n 1 | tr -d '\r'
}

record_state() {
    query_transfer_field "$1" "$2" state
}

record_peer() {
    query_transfer_field "$1" "$2" peer
}

record_peer_type() {
    record_peer "$1" "$2" | sed 's/[[:space:]].*$//'
}

print_peer_data() {
    local profile="$1"
    local sender receiver

    sender="$(record_peer "$SERIAL_A" sender || true)"
    receiver="$(record_peer "$SERIAL_B" receiver || true)"
    printf '[%s] Sender peer:   %s\n' "$profile" "${sender:-unavailable}"
    printf '[%s] Receiver peer: %s\n' "$profile" "${receiver:-unavailable}"
}

capture_diagnostics() {
    local profile="$1"
    local role serial ns

    for role in sender receiver; do
        if [ "$role" = sender ]; then
            serial="$SERIAL_A"
            ns="$ns_a"
        else
            serial="$SERIAL_B"
            ns="$ns_b"
        fi
        "$adb" -s "$serial" logcat -d >"$log_dir/$profile-$role.log" 2>&1 || true
        "$adb" -s "$serial" shell \
            "ip -brief address; ip route show table all; ip -6 route show table all; cmd wifi status; tc -s qdisc show dev wlan0" \
            >"$log_dir/$profile-$role-network.log" 2>&1 || true
        in_namespace "$ns" nft list ruleset \
            >"$log_dir/$profile-$role-router-nft.log" 2>&1 || true
        in_namespace "$ns" sh -c \
            'ip -4 route show table all; ip -6 route show table all' \
            >"$log_dir/$profile-$role-router-routes.log" 2>&1 || true
        rm -rf "$log_dir/$profile-$role-files"
        "$adb" -s "$serial" pull "/data/user/0/$PACKAGE/files" \
            "$log_dir/$profile-$role-files" >/dev/null 2>&1 || true
    done
    {
        privileged ip route show 192.168.102.0/24
        privileged iptables -S FORWARD
        privileged iptables -t nat -S POSTROUTING
    } >"$log_dir/$profile-host-routing.log" 2>&1 || true
}

configure_symmetric_nat() {
    local ns="$1"
    local lan_subnet="$2"
    local conntrack_zone="$3"

    # A distinct zone prevents a mapping from the preceding profile being
    # reused if the application happens to select the same local UDP port.
    in_namespace "$ns" nft delete table ip envoix_raw >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip envoix_raw
    in_namespace "$ns" nft add chain ip envoix_raw prerouting \
        '{ type filter hook prerouting priority raw; policy accept; }'
    in_namespace "$ns" nft add rule ip envoix_raw prerouting \
        iifname lan0 ct zone set "$conntrack_zone"
    in_namespace "$ns" nft add rule ip envoix_raw prerouting \
        iifname wan0 ct zone set "$conntrack_zone"
    in_namespace "$ns" nft delete table ip envoix >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip envoix
    in_namespace "$ns" nft add chain ip envoix forward \
        '{ type filter hook forward priority filter; policy drop; }'
    in_namespace "$ns" nft add rule ip envoix forward ct state established,related accept
    in_namespace "$ns" nft add rule ip envoix forward iifname lan0 oifname wan0 accept
    in_namespace "$ns" nft add chain ip envoix postrouting \
        '{ type nat hook postrouting priority srcnat; policy accept; }'
    # A random external port is selected for each conntrack tuple, whose key
    # includes the remote address and port. Return traffic is accepted only
    # when it belongs to that tuple.
    in_namespace "$ns" nft add rule ip envoix postrouting \
        oifname wan0 ip saddr "$lan_subnet" meta l4proto udp \
        masquerade to :40000-59999 fully-random
    in_namespace "$ns" nft add rule ip envoix postrouting \
        oifname wan0 ip saddr "$lan_subnet" meta l4proto != udp masquerade
}

configure_friendly_nat() {
    local ns="$1"
    local lan_subnet="$2"
    local device_ip="$3"
    local public_ip="$4"
    local conntrack_zone="$5"

    in_namespace "$ns" nft delete table ip envoix_raw >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip envoix_raw
    in_namespace "$ns" nft add set ip envoix_raw pinholes \
        '{ type ipv4_addr . inet_service . inet_service; flags timeout,dynamic; timeout 2m; }'
    in_namespace "$ns" nft add chain ip envoix_raw prerouting \
        '{ type filter hook prerouting priority raw; policy accept; }'
    in_namespace "$ns" nft add chain ip envoix_raw postrouting \
        '{ type filter hook postrouting priority mangle; policy accept; }'
    # Stateless one-to-one UDP translation guarantees endpoint-independent,
    # port-preserving mappings. The set retains port-restricted filtering.
    in_namespace "$ns" nft add rule ip envoix_raw prerouting \
        iifname lan0 ip saddr "$device_ip" ip daddr != "$lan_subnet" \
        meta l4proto udp update @pinholes \
        '{ ip daddr . udp dport . udp sport timeout 2m }' \
        counter notrack
    in_namespace "$ns" nft add rule ip envoix_raw prerouting \
        iifname wan0 ip daddr "$public_ip" meta l4proto udp \
        ip saddr . udp sport . udp dport @pinholes \
        counter ip daddr set "$device_ip" notrack
    in_namespace "$ns" nft add rule ip envoix_raw prerouting \
        meta l4proto != udp ct zone set "$conntrack_zone"
    # Rewriting the local source in prerouting makes Linux reject the packet
    # as arriving on lan0 with an address owned by wan0. Delay it until after
    # routing; notrack was already applied above.
    in_namespace "$ns" nft add rule ip envoix_raw postrouting \
        oifname wan0 ip saddr "$device_ip" meta l4proto udp \
        counter ip saddr set "$public_ip"

    in_namespace "$ns" nft delete table ip envoix >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip envoix
    in_namespace "$ns" nft add chain ip envoix forward \
        '{ type filter hook forward priority filter; policy drop; }'
    in_namespace "$ns" nft add rule ip envoix forward ct state established,related accept
    in_namespace "$ns" nft add rule ip envoix forward iifname lan0 oifname wan0 accept
    in_namespace "$ns" nft add rule ip envoix forward \
        iifname wan0 oifname lan0 ip daddr "$device_ip" meta l4proto udp accept
    in_namespace "$ns" nft add chain ip envoix postrouting \
        '{ type nat hook postrouting priority srcnat; policy accept; }'
    in_namespace "$ns" nft add rule ip envoix postrouting \
        oifname wan0 ip saddr "$lan_subnet" meta l4proto != udp masquerade
}

configure_routed() {
    local ns="$1"

    in_namespace "$ns" nft delete table ip envoix_raw >/dev/null 2>&1 || true
    in_namespace "$ns" nft delete table ip envoix >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip envoix
    in_namespace "$ns" nft add chain ip envoix forward \
        '{ type filter hook forward priority filter; policy accept; }'
}

set_routed_side_b() {
    local enabled="$1"

    while privileged iptables -t nat -C POSTROUTING -s 192.168.102.0/24 \
        -o "$host_upstream" -j MASQUERADE >/dev/null 2>&1; do
        privileged iptables -t nat -D POSTROUTING -s 192.168.102.0/24 \
            -o "$host_upstream" -j MASQUERADE
    done
    privileged ip route delete 192.168.102.0/24 via 198.18.0.3 \
        dev "$wan_bridge" >/dev/null 2>&1 || true
    while privileged iptables -C FORWARD -i "$wan_bridge" -o "$wan_bridge" \
        -j ACCEPT >/dev/null 2>&1; do
        privileged iptables -D FORWARD -i "$wan_bridge" -o "$wan_bridge" -j ACCEPT
    done
    if [ "$enabled" -eq 1 ]; then
        privileged ip route add 192.168.102.0/24 via 198.18.0.3 dev "$wan_bridge"
        privileged iptables -I FORWARD 1 -i "$wan_bridge" -o "$wan_bridge" -j ACCEPT
        # Preserve Internet validation without translating any traffic inside
        # the simulated WAN, including peer, broker, relay, and QAD traffic.
        privileged iptables -t nat -I POSTROUTING 1 -s 192.168.102.0/24 \
            -o "$host_upstream" -j MASQUERADE
    fi
}

configure_ipv6_forwarding() {
    local ns="$1"
    local enabled="$2"

    in_namespace "$ns" nft delete table ip6 envoix >/dev/null 2>&1 || true
    in_namespace "$ns" nft add table ip6 envoix
    in_namespace "$ns" nft add chain ip6 envoix forward \
        '{ type filter hook forward priority filter; policy drop; }'
    if [ "$enabled" -eq 1 ]; then
        in_namespace "$ns" nft add rule ip6 envoix forward \
            ct state established,related accept
        in_namespace "$ns" nft add rule ip6 envoix forward \
            iifname lan0 oifname wan0 accept
    fi
}

apply_profile() {
    local profile="$1"

    set_routed_side_b 0
    case "$profile" in
        symmetric-both-ipv4)
            configure_symmetric_nat "$ns_a" 192.168.101.0/24 101
            configure_symmetric_nat "$ns_b" 192.168.102.0/24 101
            configure_ipv6_forwarding "$ns_a" 0
            configure_ipv6_forwarding "$ns_b" 0
            ;;
        friendly-both-ipv4)
            if [ -n "$device_ip_a" ] && [ -n "$device_ip_b" ]; then
                configure_friendly_nat \
                    "$ns_a" 192.168.101.0/24 "$device_ip_a" 198.18.0.2 102
                configure_friendly_nat \
                    "$ns_b" 192.168.102.0/24 "$device_ip_b" 198.18.0.3 102
            else
                # Bootstrap Android's network before its DHCP address is known.
                configure_symmetric_nat "$ns_a" 192.168.101.0/24 102
                configure_symmetric_nat "$ns_b" 192.168.102.0/24 102
            fi
            configure_ipv6_forwarding "$ns_a" 0
            configure_ipv6_forwarding "$ns_b" 0
            ;;
        symmetric-one-side-ipv4)
            configure_symmetric_nat "$ns_a" 192.168.101.0/24 103
            configure_routed "$ns_b"
            set_routed_side_b 1
            configure_ipv6_forwarding "$ns_a" 0
            configure_ipv6_forwarding "$ns_b" 0
            ;;
        symmetric-both-ipv6)
            configure_symmetric_nat "$ns_a" 192.168.101.0/24 104
            configure_symmetric_nat "$ns_b" 192.168.102.0/24 104
            configure_ipv6_forwarding "$ns_a" 1
            configure_ipv6_forwarding "$ns_b" 1
            ;;
        *) die "unknown NAT profile: $profile" ;;
    esac
}

setup_router() {
    local ns="$1"
    local lan_peer="$2"
    local wan_peer="$3"
    local lan_address="$4"
    local wan_address="$5"
    local lan_ipv6_address="$6"
    local wan_ipv6_address="$7"
    local peer_ipv6_subnet="$8"
    local peer_ipv6_router="$9"

    privileged ip netns add "$ns"
    in_namespace "$ns" sysctl -q -w net.ipv6.conf.all.disable_ipv6=0
    in_namespace "$ns" sysctl -q -w net.ipv6.conf.default.disable_ipv6=0
    privileged ip link set "$lan_peer" netns "$ns"
    privileged ip link set "$wan_peer" netns "$ns"
    in_namespace "$ns" ip link set lo up
    in_namespace "$ns" ip link set "$lan_peer" name lan0
    in_namespace "$ns" ip link set "$wan_peer" name wan0
    in_namespace "$ns" ip address add "$lan_address" dev lan0
    in_namespace "$ns" ip address add "$wan_address" dev wan0
    in_namespace "$ns" ip -6 address add "$lan_ipv6_address" dev lan0
    in_namespace "$ns" ip -6 address add "$wan_ipv6_address" dev wan0
    in_namespace "$ns" ip link set lan0 up
    in_namespace "$ns" ip link set wan0 up
    in_namespace "$ns" ip route add default via 198.18.0.1
    in_namespace "$ns" ip -6 route add "$peer_ipv6_subnet" via "$peer_ipv6_router"
    in_namespace "$ns" sysctl -q -w net.ipv4.ip_forward=1
    in_namespace "$ns" sysctl -q -w net.ipv6.conf.all.forwarding=1
}

setup_network() {
    local owner_uid upstream

    owner_uid="$(id -u)"
    host_ip_forward="$(sysctl -n net.ipv4.ip_forward)"
    upstream="$(ip -4 route get 1.1.1.1 | awk '{for (i=1; i<=NF; i++) if ($i == "dev") {print $(i+1); exit}}')"
    [ -n "$upstream" ] || die "could not determine the host's Internet interface"

    privileged ip link add "$lan_bridge_a" type bridge
    privileged ip link add "$lan_bridge_b" type bridge
    privileged ip link add "$wan_bridge" type bridge
    privileged ip address add 198.18.0.1/24 dev "$wan_bridge"
    privileged ip link set "$lan_bridge_a" up
    privileged ip link set "$lan_bridge_b" up
    privileged ip link set "$wan_bridge" up

    privileged ip tuntap add dev "$tap_a" mode tap user "$owner_uid"
    privileged ip tuntap add dev "$tap_b" mode tap user "$owner_uid"
    privileged ip link set "$tap_a" master "$lan_bridge_a"
    privileged ip link set "$tap_b" master "$lan_bridge_b"
    privileged ip link set "$tap_a" up
    privileged ip link set "$tap_b" up

    privileged ip link add "$veth_lan_a" type veth peer name "${veth_lan_a}n"
    privileged ip link add "$veth_lan_b" type veth peer name "${veth_lan_b}n"
    privileged ip link add "$veth_wan_a" type veth peer name "${veth_wan_a}n"
    privileged ip link add "$veth_wan_b" type veth peer name "${veth_wan_b}n"
    privileged ip link set "$veth_lan_a" master "$lan_bridge_a"
    privileged ip link set "$veth_lan_b" master "$lan_bridge_b"
    privileged ip link set "$veth_wan_a" master "$wan_bridge"
    privileged ip link set "$veth_wan_b" master "$wan_bridge"
    privileged ip link set "$veth_lan_a" up
    privileged ip link set "$veth_lan_b" up
    privileged ip link set "$veth_wan_a" up
    privileged ip link set "$veth_wan_b" up

    setup_router "$ns_a" "${veth_lan_a}n" "${veth_wan_a}n" \
        192.168.101.1/24 198.18.0.2/24 \
        2001:db8:101::1/64 2001:db8:100::2/64 \
        2001:db8:102::/64 2001:db8:100::3
    setup_router "$ns_b" "${veth_lan_b}n" "${veth_wan_b}n" \
        192.168.102.1/24 198.18.0.3/24 \
        2001:db8:102::1/64 2001:db8:100::3/64 \
        2001:db8:101::/64 2001:db8:100::2

    host_upstream="$upstream"
    network_ready=1
    privileged sysctl -q -w net.ipv4.ip_forward=1
    privileged iptables -I FORWARD 1 -i "$wan_bridge" -o "$upstream" -j ACCEPT
    privileged iptables -I FORWARD 1 -i "$upstream" -o "$wan_bridge" \
        -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    privileged iptables -t nat -I POSTROUTING 1 -s 198.18.0.0/24 -o "$upstream" -j MASQUERADE
    in_namespace "$ns_a" dnsmasq --keep-in-foreground --bind-interfaces --interface=lan0 \
        --pid-file="$dnsmasq_a_pid_file" \
        --dhcp-leasefile="$dnsmasq_a_lease_file" \
        --log-dhcp --log-queries --log-facility=- \
        --dhcp-range=192.168.101.10,192.168.101.99,255.255.255.0,1h \
        --enable-ra --dhcp-range=2001:db8:101::,ra-only,64,2h \
        --dhcp-option=option:router,192.168.101.1 --dhcp-option=option:dns-server,192.168.101.1 \
        --address="/$relay_host/198.18.0.1" --no-resolv --server=8.8.8.8 \
        >"$log_dir/dnsmasq-a.log" 2>&1 &
    dnsmasq_a_pid=$!
    in_namespace "$ns_b" dnsmasq --keep-in-foreground --bind-interfaces --interface=lan0 \
        --pid-file="$dnsmasq_b_pid_file" \
        --dhcp-leasefile="$dnsmasq_b_lease_file" \
        --log-dhcp --log-queries --log-facility=- \
        --dhcp-range=192.168.102.10,192.168.102.99,255.255.255.0,1h \
        --enable-ra --dhcp-range=2001:db8:102::,ra-only,64,2h \
        --dhcp-option=option:router,192.168.102.1 --dhcp-option=option:dns-server,192.168.102.1 \
        --address="/$relay_host/198.18.0.1" --no-resolv --server=8.8.8.8 \
        >"$log_dir/dnsmasq-b.log" 2>&1 &
    dnsmasq_b_pid=$!
    sleep 1
    kill -0 "$dnsmasq_a_pid" 2>/dev/null ||
        die "router A dnsmasq exited; see $log_dir/dnsmasq-a.log"
    kill -0 "$dnsmasq_b_pid" 2>/dev/null ||
        die "router B dnsmasq exited; see $log_dir/dnsmasq-b.log"
}

run_test() {
    local profile="$1"
    local room deadline actual app_uid sender_state receiver_state
    local sender_peer_type receiver_peer_type failure_reason

    room=""
    actual=""
    sender_state=""
    receiver_state=""
    failure_reason=""
    printf '\n[%s] Preparing devices...\n' "$profile"
    device_ip_a="$(wifi_address "$SERIAL_A")"
    device_ip_b="$(wifi_address "$SERIAL_B")"
    [ -n "$device_ip_a" ] || die "$SERIAL_A has no global Wi-Fi IPv4 address"
    [ -n "$device_ip_b" ] || die "$SERIAL_B has no global Wi-Fi IPv4 address"
    apply_profile "$profile"
    if [ "$profile" = symmetric-both-ipv6 ]; then
        set_device_ipv6 "$SERIAL_A" 1 2001:db8:101: 192.168.101.
        set_device_ipv6 "$SERIAL_B" 1 2001:db8:102: 192.168.102.
        printf '[%s] IPv6 addresses: A=%s B=%s\n' "$profile" \
            "$(wifi_ipv6_address "$SERIAL_A")" "$(wifi_ipv6_address "$SERIAL_B")"
    else
        set_device_ipv6 "$SERIAL_A" 0 "" ""
        set_device_ipv6 "$SERIAL_B" 0 "" ""
    fi
    clear_bandwidth_limit "$SERIAL_A"
    reset_app "$SERIAL_A"
    reset_app "$SERIAL_B"
    "$adb" -s "$SERIAL_A" logcat -c
    "$adb" -s "$SERIAL_B" logcat -c
    clear_receiver_outputs
    "$adb" -s "$SERIAL_A" push "$test_file" /data/local/tmp/nat-test-input >/dev/null
    app_uid="$("$adb" -s "$SERIAL_A" shell stat -c %u "/data/user/0/$PACKAGE" | tr -d '\r')"
    "$adb" -s "$SERIAL_A" shell \
        "cp /data/local/tmp/nat-test-input '$DEVICE_INPUT'; chown $app_uid:$app_uid '$DEVICE_INPUT'"

    room="$(start_creator_receiver)"
    wait_for_transfer_record "$SERIAL_B" receiver "$profile"
    sleep 2
    printf '[%s] Limiting sender Wi-Fi to approximately 512 KiB/s...\n' "$profile"
    limit_bandwidth "$SERIAL_A"
    start_sender "$SERIAL_A" "$room" "$DEVICE_INPUT"
    wait_for_transfer_record "$SERIAL_A" sender "$profile"

    deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        actual="$(device_hashes || true)"
        sender_state="$(record_state "$SERIAL_A" sender || true)"
        receiver_state="$(record_state "$SERIAL_B" receiver || true)"
        if [ "$verbose" -eq 1 ]; then
            printf '[%s] sender=%s receiver=%s\r' "$profile" \
                "${sender_state:-unknown}" "${receiver_state:-unknown}"
        fi
        if printf '%s\n' "$actual" | grep -q "^$expected_hash " &&
            [ "$sender_state" = delivered ] && [ "$receiver_state" = delivered ]; then
            sender_peer_type="$(record_peer_type "$SERIAL_A" sender || true)"
            receiver_peer_type="$(record_peer_type "$SERIAL_B" receiver || true)"
            if [ "$profile" = friendly-both-ipv4 ] &&
                { [ "$sender_peer_type" != direct ] || [ "$receiver_peer_type" != direct ]; }; then
                failure_reason="completed through sender=${sender_peer_type:-unknown}, receiver=${receiver_peer_type:-unknown}; expected direct"
                break
            fi
            [ "$verbose" -eq 0 ] || printf '\n'
            printf '[%s] PASS: both peers completed; received SHA-256 matches %s\n' \
                "$profile" "$expected_hash"
            print_peer_data "$profile"
            passed=$((passed + 1))
            [ "$verbose" -eq 0 ] || capture_diagnostics "$profile"
            return
        fi
        if [ "$sender_state" = failed ] || [ "$receiver_state" = failed ]; then
            break
        fi
        sleep 2
    done

    [ "$verbose" -eq 0 ] || printf '\n'
    if [ -n "$failure_reason" ]; then
        printf '[%s] FAIL: %s\n' "$profile" "$failure_reason" >&2
    else
        printf '[%s] FAIL after %s seconds: sender=%s receiver=%s\n' \
            "$profile" "$timeout" "${sender_state:-unknown}" "${receiver_state:-unknown}" >&2
    fi
    [ -z "$actual" ] || printf '[%s] received candidates:\n%s\n' "$profile" "$actual" >&2
    print_peer_data "$profile"
    capture_diagnostics "$profile"
    failed=$((failed + 1))
}

cleanup() {
    local pids

    set +e
    clear_bandwidth_limit "$SERIAL_A" >/dev/null 2>&1
    if [ "$started" -eq 1 ]; then
        "$adb" -s "$SERIAL_A" emu kill >/dev/null 2>&1
        "$adb" -s "$SERIAL_B" emu kill >/dev/null 2>&1
    fi
    [ -z "$broker_pid" ] || kill "$broker_pid" >/dev/null 2>&1
    [ -z "$relay_pid" ] || kill "$relay_pid" >/dev/null 2>&1
    [ -z "$broker_pid" ] || wait "$broker_pid" >/dev/null 2>&1
    [ -z "$relay_pid" ] || wait "$relay_pid" >/dev/null 2>&1
    if [ "$jni_replaced" -eq 1 ]; then
        if [ "$jni_had_original" -eq 1 ]; then
            cp "$jni_backup" "$staged_jni"
        else
            rm -f "$staged_jni"
        fi
        rm -f "$jni_backup"
    fi
    if [ "$network_setup_started" -eq 1 ]; then
        [ -z "$dnsmasq_a_pid" ] || kill "$dnsmasq_a_pid" >/dev/null 2>&1
        [ -z "$dnsmasq_b_pid" ] || kill "$dnsmasq_b_pid" >/dev/null 2>&1
        [ -z "$dnsmasq_a_pid" ] || wait "$dnsmasq_a_pid" >/dev/null 2>&1
        [ -z "$dnsmasq_b_pid" ] || wait "$dnsmasq_b_pid" >/dev/null 2>&1
        pids="$(privileged ip netns pids "$ns_a" 2>/dev/null)"
        [ -z "$pids" ] || privileged kill $pids >/dev/null 2>&1
        pids="$(privileged ip netns pids "$ns_b" 2>/dev/null)"
        [ -z "$pids" ] || privileged kill $pids >/dev/null 2>&1
        if [ "$network_ready" -eq 1 ]; then
            privileged iptables -t nat -D POSTROUTING -s 192.168.102.0/24 \
                -o "$host_upstream" -j MASQUERADE >/dev/null 2>&1
            privileged iptables -D FORWARD -i "$wan_bridge" -o "$wan_bridge" \
                -j ACCEPT >/dev/null 2>&1
            privileged iptables -t nat -D POSTROUTING -s 198.18.0.0/24 \
                -o "$host_upstream" -j MASQUERADE >/dev/null 2>&1
            privileged iptables -D FORWARD -i "$host_upstream" -o "$wan_bridge" \
                -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT >/dev/null 2>&1
            privileged iptables -D FORWARD -i "$wan_bridge" -o "$host_upstream" \
                -j ACCEPT >/dev/null 2>&1
            privileged sysctl -q -w net.ipv4.ip_forward="$host_ip_forward" >/dev/null 2>&1
        fi
        privileged ip netns delete "$ns_a" >/dev/null 2>&1
        privileged ip netns delete "$ns_b" >/dev/null 2>&1
        privileged ip tuntap delete dev "$tap_a" mode tap >/dev/null 2>&1
        privileged ip tuntap delete dev "$tap_b" mode tap >/dev/null 2>&1
        privileged ip link delete "$lan_bridge_a" >/dev/null 2>&1
        privileged ip link delete "$lan_bridge_b" >/dev/null 2>&1
        privileged ip link delete "$wan_bridge" >/dev/null 2>&1
    fi
    privileged rm -f "${dnsmasq_a_pid_file:-}" "${dnsmasq_b_pid_file:-}" \
        "${dnsmasq_a_lease_file:-}" "${dnsmasq_b_lease_file:-}" 2>/dev/null || true
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --timeout)
            [ "$#" -ge 2 ] || die "--timeout requires a value"
            timeout="$2"
            shift 2
            ;;
        --run)
            [ "$#" -ge 2 ] || die "--run requires a value"
            run_spec="$2"
            shift 2
            ;;
        --list-tests) list_tests; exit 0 ;;
        --verbose) verbose=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*) die "unknown option: $1" ;;
        *) break ;;
    esac
done

select_tests "$run_spec"
[ "$#" -eq 3 ] || {
    usage
    exit 2
}
[ "$timeout" -gt 0 ] 2>/dev/null || die "--timeout must be a positive integer"
[ "$(uname -s)" = Linux ] || die "this test requires Linux network namespaces"

avd_a="$1"
avd_b="$2"
test_file="$3"
[ "$avd_a" != "$avd_b" ] || die "use two distinct AVDs"
[ -x "$emulator" ] || die "Android Emulator not found at $emulator"
[ -x "$adb" ] || die "adb not found at $adb"
[ -f "$test_file" ] || die "test file not found at $test_file"
for command in sha256sum ip bridge nft iptables dnsmasq sysctl openssl cargo curl stdbuf cp pkill readlink; do
    command -v "$command" >/dev/null || die "$command is required"
done
cargo ndk --version >/dev/null 2>&1 || die "cargo-ndk is required"
check_avd "$avd_a"
check_avd "$avd_b"

expected_hash="$(sha256sum "$test_file" | awk '{print $1}')"
log_dir="$repo_root/android/build/nat-test"
cert_dir="$log_dir/certs"
tool_root="$log_dir/tools"
relay_binary="$tool_root/bin/iroh-relay"
relay_config="$log_dir/relay.toml"
staged_jni="$repo_root/android/app/src/main/jniLibs/x86_64/libenvoix_jni.so"
jni_backup="$log_dir/libenvoix_jni.so.before-nat-test"
# dnsmasq is AppArmor-confined to standard paths, so its pidfile must live in
# /run as *dnsmasq*.pid (not the repo build dir) and its leasefile under
# /var/lib/misc as dnsmasq.*.leases. Root (via `privileged`) owns both.
dnsmasq_a_pid_file="/run/nat-test-$net_id-a-dnsmasq.pid"
dnsmasq_b_pid_file="/run/nat-test-$net_id-b-dnsmasq.pid"
dnsmasq_a_lease_file="/var/lib/misc/dnsmasq.nat-$net_id-a.leases"
dnsmasq_b_lease_file="/var/lib/misc/dnsmasq.nat-$net_id-b.leases"
mkdir -p "$log_dir"
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [ "$(id -u)" -ne 0 ]; then
    sudo -v || die "sudo access is required to create the test network"
fi
preflight_cleanup
if "$adb" devices | grep -Eq '^emulator-(5554|5556)[[:space:]]'; then
    die "emulator ports 5554/5556 remain in use after cleanup"
fi
prepare_build
printf 'Creating isolated Wi-Fi routers...\n'
network_setup_started=1
setup_network
apply_profile "${selected_tests[0]}"
start_local_servers
emulator_debug=()
if [ "$verbose" -eq 1 ]; then
    emulator_debug=(-debug wifi,socket)
fi
# -no-window + software GPU so the emulators run headless (no X/GL host deps).
# `swiftshader` is the mode name in emulator 36+ (the old `swiftshader_indirect`
# is rejected); the -gpu flag is authoritative over any hw.gpu.mode in config.ini.
"$emulator" "@$avd_a" -port 5554 -no-snapshot -no-window -gpu swiftshader \
    -feature -WiFiPacketStream -wifi-tap "$tap_a" \
    "${emulator_debug[@]}" \
    >"$log_dir/emulator-5554.log" 2>&1 &
"$emulator" "@$avd_b" -port 5556 -no-snapshot -no-window -gpu swiftshader \
    -feature -WiFiPacketStream -wifi-tap "$tap_b" \
    "${emulator_debug[@]}" \
    >"$log_dir/emulator-5556.log" 2>&1 &
started=1

printf 'Waiting for %s and %s to boot...\n' "$avd_a" "$avd_b"
wait_for_boot "$SERIAL_A"
wait_for_boot "$SERIAL_B"
root_device "$SERIAL_A"
root_device "$SERIAL_B"
"$adb" -s "$SERIAL_A" shell 'command -v tc >/dev/null' ||
    die "$SERIAL_A does not provide the tc traffic-control utility"
configure_test_validation "$SERIAL_A"
configure_test_validation "$SERIAL_B"
wait_for_wifi "$SERIAL_A" 192.168.101.
wait_for_wifi "$SERIAL_B" 192.168.102.
wait_for_wifi_route "$SERIAL_A" 192.168.101.1 192.168.101.
wait_for_wifi_route "$SERIAL_B" 192.168.102.1 192.168.102.
wait_for_validation "$SERIAL_A"
wait_for_validation "$SERIAL_B"
printf 'Wi-Fi addresses: A=%s B=%s\n' \
    "$(wifi_address "$SERIAL_A")" "$(wifi_address "$SERIAL_B")"

for serial in "$SERIAL_A" "$SERIAL_B"; do
    install_output="$("$adb" -s "$serial" install -r -t "$apk" 2>&1)" ||
        die "failed to install APK on $serial: $install_output"
done

for test_name in "${selected_tests[@]}"; do
    run_test "$test_name"
done

printf '\nResult: %d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ] || exit 1
