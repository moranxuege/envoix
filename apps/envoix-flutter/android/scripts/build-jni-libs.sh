#!/usr/bin/env bash
# Cross-compiles the Rust host into the app's per-build-type jniLibs, then
# RECORDS what it produced.
#
# The two payloads differ by ONE cargo feature: the debug library carries the
# `E2eBridge` instrumentation entry points, the release library was never
# compiled with them. That is why jniLibs is per build type rather than shared —
# a release APK cannot pick up the instrumented library by accident.
#
# Gradle never invokes cargo, so jniLibs is a hand-refreshed BUILD ARTIFACT.
# `xtask record-payload` is what stops that from being a hole: it hashes the
# libraries this run produced, the composed build manifest they were compiled
# against and the contract sources they came from, and rewrites every generated
# release record from them. Re-run this script whenever anything under crates/
# or hosts/ changes; both the app's `verify<BuildType>JniLibs` task and the
# release gate FAIL on a payload that no longer matches its record.
set -euo pipefail
cd "$(dirname "$0")/../../../.."
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-/usr/local/android-sdk/ndk/26.3.11579264}"
APP="apps/envoix-flutter/android/app/src"
SONAME="libenvoix_host_android.so"

build() {
    local out="$1"
    shift
    cargo ndk -t arm64-v8a -t x86_64 -o "$out" build -p envoix-host-android --release "$@"
    # cargo-ndk copies every .so it finds in the target directory. The app loads
    # exactly one library; dependency dylibs would just bloat the APK.
    find "$out" -name '*.so' ! -name "$SONAME" -delete
}

build "$APP/release/jniLibs"
build "$APP/debug/jniLibs" --features e2e-instrumentation

cargo run -q -p xtask -- record-payload
