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
# against and the complete source/build/toolchain closure they came from, and
# rewrites every generated release record from them. Both the app's
# `verify<BuildType>JniLibs` task and the release gate fail on any closure edit
# until this script rebuilds and records the payload.
set -euo pipefail
cd "$(dirname "$0")/../../../.."
TOOLCHAIN_FILE="registry/android-native-toolchain.properties"

toolchain_property() {
    local key="$1"
    sed -n "s/^${key}=//p" "$TOOLCHAIN_FILE"
}

RUSTC_RELEASE="$(toolchain_property rustc_release)"
CARGO_RELEASE="$(toolchain_property cargo_release)"
ANDROID_NDK_REVISION="$(toolchain_property android_ndk_revision)"
[[ "$(rustc --version | awk '{print $2}')" == "$RUSTC_RELEASE" ]] || {
    echo "rustc does not match pinned release $RUSTC_RELEASE in $TOOLCHAIN_FILE" >&2
    exit 1
}
[[ "$(cargo --version | awk '{print $2}')" == "$CARGO_RELEASE" ]] || {
    echo "cargo does not match pinned release $CARGO_RELEASE in $TOOLCHAIN_FILE" >&2
    exit 1
}

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-/usr/local/android-sdk/ndk/$ANDROID_NDK_REVISION}"
OBSERVED_NDK_REVISION="$(
    sed -n 's/^Pkg\.Revision[[:space:]]*=[[:space:]]*//p' "$ANDROID_NDK_HOME/source.properties"
)"
[[ "$OBSERVED_NDK_REVISION" == "$ANDROID_NDK_REVISION" ]] || {
    echo "Android NDK at $ANDROID_NDK_HOME is $OBSERVED_NDK_REVISION, expected $ANDROID_NDK_REVISION" >&2
    exit 1
}
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
