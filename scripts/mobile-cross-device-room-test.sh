#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
direction="${1:-both}"

usage() {
  cat <<'EOF'
Usage: scripts/mobile-cross-device-room-test.sh [android-to-ios|ios-to-android|both]

Compatibility entry point for the Android/iPhone subset of the scenario-driven
Manifest-v2 transfer matrix. New comprehensive runs should invoke
scripts/cross-device-transfer-matrix.sh directly.
EOF
}

case "$direction" in
  -h|--help)
    usage
    exit 0
    ;;
  android-to-ios)
    matrix_directions="android:ios"
    legacy_bytes="${ENVOIX_ANDROID_TO_IOS_BYTES:-}"
    ;;
  ios-to-android)
    matrix_directions="ios:android"
    legacy_bytes="${ENVOIX_IOS_TO_ANDROID_BYTES:-}"
    ;;
  both)
    matrix_directions="android:ios ios:android"
    android_bytes="${ENVOIX_ANDROID_TO_IOS_BYTES:-}"
    ios_bytes="${ENVOIX_IOS_TO_ANDROID_BYTES:-}"
    if [[ -n "$android_bytes" && -n "$ios_bytes" && "$android_bytes" != "$ios_bytes" ]]; then
      echo "error: the compatibility wrapper requires equal byte counts for a two-direction run" >&2
      exit 2
    fi
    legacy_bytes="${android_bytes:-$ios_bytes}"
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

scenario="${ENVOIX_CROSS_DEVICE_SCENARIO:-single_file}"
if [[ -n "$legacy_bytes" ]]; then
  scenario="large_file"
  export ENVOIX_MATRIX_LARGE_BYTES="$legacy_bytes"
fi

export ENVOIX_MATRIX_DIRECTIONS="$matrix_directions"
export ENVOIX_MATRIX_SCENARIOS="$scenario"
export ENVOIX_MATRIX_REPEAT="${ENVOIX_CROSS_DEVICE_REPEAT:-1}"
export ENVOIX_MATRIX_RUN_ID="${ENVOIX_CROSS_DEVICE_RUN_ID:-mobile-$(date +%Y%m%d-%H%M%S)-$$}"
export ENVOIX_MATRIX_READY_TIMEOUT_SECONDS="${ENVOIX_CROSS_DEVICE_READY_TIMEOUT_LONG:-${ENVOIX_CROSS_DEVICE_READY_TIMEOUT:-120}}"
export ENVOIX_MATRIX_TRANSFER_TIMEOUT_MS="${ENVOIX_CROSS_DEVICE_TIMEOUT_MS:-600000}"

exec "$script_dir/cross-device-transfer-matrix.sh"
