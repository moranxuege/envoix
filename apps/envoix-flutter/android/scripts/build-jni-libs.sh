#!/usr/bin/env bash
# Cross-compiles the Rust host into the app's jniLibs.
#
# Gradle never invokes cargo, so jniLibs is a hand-refreshed BUILD ARTIFACT: an
# APK assembled after a Rust change silently packages the PREVIOUS .so unless
# this script runs first. Re-run it whenever anything under crates/ or hosts/
# changes; the app's `verifyJniLibs` task warns when the packaged library is
# older than the Rust sources and fails on any stray library.
set -euo pipefail
cd "$(dirname "$0")/../../../.."
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-/usr/local/android-sdk/ndk/26.3.11579264}"
OUT="apps/envoix-flutter/android/app/src/main/jniLibs"
SONAME="libenvoix_host_android.so"
cargo ndk -t arm64-v8a -t x86_64 -o "$OUT" build -p envoix-host-android --release
# cargo-ndk copies every .so it finds in the target directory. The app loads
# exactly one library; dependency dylibs would just bloat the APK.
find "$OUT" -name '*.so' ! -name "$SONAME" -delete
