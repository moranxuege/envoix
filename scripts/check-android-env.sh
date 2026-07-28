#!/usr/bin/env bash
set -eu

readonly MIN_RUST_VERSION="1.91.0"
readonly MIN_BUILD_TOOLS_VERSION="34.0.0"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
missing=0

present() {
    printf '[present] %s\n' "$1"
}

missing() {
    printf '[missing] %s\n' "$1"
    missing=$((missing + 1))
}

version_at_least() {
    printf '%s\n%s\n' "$2" "$1" | sort -V -C
}

printf 'Checking Android build environment...\n\n'

if command -v java >/dev/null 2>&1; then
    java_version="$(java -version 2>&1 | sed -n '1s/.*version "\([^"]*\)".*/\1/p')"
    java_major="${java_version%%.*}"
    if [ -n "$java_major" ] && [ "$java_major" -ge 17 ] 2>/dev/null; then
        present "JDK $java_version (minimum 17)"
    else
        missing "JDK 17 or newer (active Java is ${java_version:-unknown}; set JAVA_HOME to a compatible JDK)"
    fi
else
    missing "JDK 17 (java is not on PATH)"
fi

if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
    rust_version="$(rustc --version | awk '{print $2}')"
    if version_at_least "$rust_version" "$MIN_RUST_VERSION"; then
        present "Rust $rust_version (minimum $MIN_RUST_VERSION)"
    else
        missing "Rust $MIN_RUST_VERSION or newer (found $rust_version)"
    fi
else
    missing "Rust toolchain (cargo and rustc)"
fi

if cargo ndk --version >/dev/null 2>&1; then
    cargo_ndk_version="$(cargo ndk --version | awk '{print $2}')"
    present "cargo-ndk ${cargo_ndk_version:-unknown}"
else
    missing "cargo-ndk (install with: cargo install cargo-ndk --locked)"
fi

installed_targets="$(rustup target list --installed 2>/dev/null || true)"
for target in aarch64-linux-android x86_64-linux-android; do
    if printf '%s\n' "$installed_targets" | grep -Fxq "$target"; then
        present "Rust target $target"
    else
        missing "Rust target $target (install with: rustup target add $target)"
    fi
done

android_sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
if [ -d "$android_sdk_root" ]; then
    present "Android SDK ($android_sdk_root)"
else
    missing "Android SDK ($android_sdk_root; set ANDROID_SDK_ROOT if installed elsewhere)"
fi

if [ -d "$android_sdk_root/platforms/android-34" ]; then
    present "Android SDK Platform 34"
else
    missing "Android SDK Platform 34"
fi

build_tools_version="$(
    for path in "$android_sdk_root"/build-tools/*; do
        [ -d "$path" ] || continue
        basename "$path"
    done | sort -V | tail -n 1
)"
if [ -n "$build_tools_version" ] && version_at_least "$build_tools_version" "$MIN_BUILD_TOOLS_VERSION"; then
    present "Android Build Tools $build_tools_version (minimum $MIN_BUILD_TOOLS_VERSION)"
else
    missing "Android Build Tools $MIN_BUILD_TOOLS_VERSION or newer"
fi

configured_ndk_home="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -n "$configured_ndk_home" ]; then
    android_ndk_home="$configured_ndk_home"
else
    android_ndk_home="$(
        for path in "$android_sdk_root"/ndk/*; do
            [ -d "$path" ] || continue
            printf '%s\n' "$path"
        done | sort -V | tail -n 1
    )"
fi
if [ -d "$android_ndk_home" ]; then
    present "Android NDK $(basename "$android_ndk_home") ($android_ndk_home)"
else
    missing "Android NDK (install any current NDK or set ANDROID_NDK_HOME)"
fi

printf '\n'
if [ "$missing" -ne 0 ]; then
    printf 'Environment is not ready: %d required item(s) missing.\n' "$missing"
    exit 1
fi

if [ "${ENVOIX_CHECK_ANDROID_ENV_BUILD:-0}" = "1" ]; then
    answer="yes"
else
    printf 'Environment is ready. Build and stage the JNI libraries now? [y/N] '
    read -r answer || answer=""
fi
case "$answer" in
    y|Y|yes|YES|Yes)
        if [ "${ENVOIX_BUILD_LEASE_HELD:-0}" = "1" ] \
            && [ "${ENVOIX_BUILD_LEASE_MODE:-writer}" = "reader" ]; then
            printf 'error: JNI build cannot run under a reader lease\n' >&2
            exit 3
        fi
        if [ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]; then
            exec "$repo_root/scripts/with-build-cache-guard.sh" \
                env ENVOIX_CHECK_ANDROID_ENV_BUILD=1 "$0" "$@"
        fi
        export ANDROID_NDK_HOME="$android_ndk_home"
        cd "$repo_root"
        cargo ndk -t arm64-v8a -t x86_64 --platform 26 \
            build --release -p envoix-android-jni
        mkdir -p \
            android/app/src/main/jniLibs/arm64-v8a \
            android/app/src/main/jniLibs/x86_64
        cp target/aarch64-linux-android/release/libenvoix_jni.so \
            android/app/src/main/jniLibs/arm64-v8a/
        cp target/x86_64-linux-android/release/libenvoix_jni.so \
            android/app/src/main/jniLibs/x86_64/
        printf 'JNI libraries built and staged under android/app/src/main/jniLibs/.\n'
        ;;
    *)
        printf 'Build skipped.\n'
        ;;
esac
