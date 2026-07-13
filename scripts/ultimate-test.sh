#!/usr/bin/env bash
set -eu

readonly script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly mdns_test="$script_dir/mdns-test.sh"
readonly nat_test="$script_dir/nat-test.sh"

mdns_options=()
nat_options=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [options] <avd-a> <avd-b> <test-file> [apk]

Run the mDNS transfer suite followed by the NAT transfer suite. The optional
APK is used by the mDNS suite; the NAT suite builds its own CA-enabled APK.

Options:
  --timeout SECONDS   Per-transfer timeout for both suites
  --run TESTS         NAT tests to run: comma-separated names or "all"
  --list-tests        List the mDNS environments and NAT tests, then exit
  --verbose           Enable verbose diagnostics in both suites
  -h, --help          Show this help
EOF
}

list_tests() {
    printf '%s\n' 'mDNS environments (always run):' '  internet' '  lan-only'
    printf '%s\n' 'NAT tests (--run):'
    "$nat_test" --list-tests | sed 's/^/  /'
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --timeout)
            [ "$#" -ge 2 ] || { printf 'error: --timeout requires a value\n' >&2; exit 1; }
            mdns_options+=(--timeout "$2")
            nat_options+=(--timeout "$2")
            shift 2
            ;;
        --run)
            [ "$#" -ge 2 ] || { printf 'error: --run requires a value\n' >&2; exit 1; }
            nat_options+=(--run "$2")
            shift 2
            ;;
        --list-tests) list_tests; exit 0 ;;
        --verbose)
            mdns_options+=(--verbose)
            nat_options+=(--verbose)
            shift
            ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*) printf 'error: unknown option: %s\n' "$1" >&2; exit 1 ;;
        *) break ;;
    esac
done

[ "$#" -ge 3 ] && [ "$#" -le 4 ] || {
    usage
    exit 2
}

avd_a="$1"
avd_b="$2"
test_file="$3"
mdns_args=("${mdns_options[@]}" "$avd_a" "$avd_b" "$test_file")
nat_args=("${nat_options[@]}" "$avd_a" "$avd_b" "$test_file")
[ "$#" -lt 4 ] || mdns_args+=("$4")

failed=0
printf '%s\n' '=== mDNS test suite ==='
if "$mdns_test" "${mdns_args[@]}"; then
    printf '%s\n' '=== mDNS test suite passed ==='
else
    status=$?
    [ "$status" -lt 128 ] || exit "$status"
    printf '=== mDNS test suite failed (exit %d) ===\n' "$status" >&2
    failed=1
fi

printf '\n%s\n' '=== NAT test suite ==='
if "$nat_test" "${nat_args[@]}"; then
    printf '%s\n' '=== NAT test suite passed ==='
else
    status=$?
    [ "$status" -lt 128 ] || exit "$status"
    printf '=== NAT test suite failed (exit %d) ===\n' "$status" >&2
    failed=1
fi

exit "$failed"
