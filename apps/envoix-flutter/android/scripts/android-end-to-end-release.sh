#!/usr/bin/env bash
# android_end_to_end_release — D2's device leg.
# RUN BY THE OWNER ONLY (one emulator; parallel legs never touch adb).
#
# WHAT IT NEEDS
#
#   * an emulator or device on `adb devices`, x86_64 (AVD_Phone) or arm64 —
#     the release APK ships both ABIs;
#   * a network route from THIS MACHINE to the live dev rendezvous
#     (rdz.dev.envoix.chkxwlyh.us:9645/udp). The device does not need one: see
#     "what it does NOT prove";
#   * the dev-RELEASE APK, signed with ~/envoix-release.jks:
#       apps/envoix-flutter/android/gradlew :app:assembleRelease
#     (`APK=` overrides the path);
#   * a cross-compiled payload and current release records:
#       bash apps/envoix-flutter/android/scripts/build-jni-libs.sh
#   * about four minutes: ~30 s for the live pairing probe, the rest on device.
#
# WHAT A PASS PROVES, in order:
#
#   1. the catalogue will deploy `dev`, so an app may be built for it at all;
#   2. the endpoint this build was compiled for is LIVE: two clients of this
#      codebase generate a fresh room code, join it through
#      `<node_id>@rdz.dev.envoix.chkxwlyh.us:9645`, complete SPAKE2 and exchange
#      sealed descriptors. The address is not typed here — the probe defaults to
#      `BUILD_TARGET`, the identity `deploy/environments.toml` derives;
#   3. a RELEASE-signed build installs and boots, its foreground Service owns the
#      Runtime, and Flutter attaches to it;
#   4. the build that app STATES — read out of a frame the host encoded, drawn on
#      the Logs screen — names that same deployment, endpoint for endpoint,
#      matching `registry/release-identity.toml`. A release artifact therefore
#      cannot be silently pointed somewhere else: the identity is part of the
#      build manifest the release gate pins to the shipped bytes;
#   5. in that release build the authority still creates durable truth: an
#      invite the CORE produced is pasted into the real sheet and becomes a card.
#
# WHAT IT DOES NOT PROVE, stated plainly rather than implied:
#
#   * that the APP PROCESS opens a socket to the rendezvous. It does not. No code
#     path in `envoix-host-android` dials one: `PreparedIrohExecutor` still parks
#     because no frontend flow prepares a launch (BN4's named deferral), and the
#     host crate has no rendezvous dependency at all. Step 2 proves the server is
#     live and pairs peers, using this codebase's client and this build's
#     address; step 4 proves the app is built for that address. The seam between
#     them — the app's own socket — is the largest thing still missing, and this
#     script deliberately does not paper over it.
#   * the QR SCAN path. CameraX binding, the runtime permission dialog and the
#     decode path have still never run on a device. Driving them needs a camera
#     the emulator fakes and a second screen showing a code; neither is covered
#     here, so it stays a KNOWN gap rather than an assumed pass.
#   * the send path past the system document picker — see `f2b-e2e.sh`, which
#     owns that boundary and explains why.
#   * a transfer. Nothing moves bytes; a joined card rests in `Connecting`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
ADB="${ADB:-adb}"
APP_ID="${APP_ID:-app.envoix.host.dev}"
ACTIVITY="$APP_ID/app.envoix.host.MainActivity"
APK="${APK:-$HERE/../app/build/outputs/apk/dev/release/app-dev-release.apk}"
IDENTITY="$REPO/registry/release-identity.toml"

identity() { sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$IDENTITY"; }
flutter_log() { "$ADB" logcat -d -s flutter; }
tap_at() { "$ADB" shell input tap $1; }

point() {
    flutter_log |
        sed -n "s/.*$1 control=$2 x=\(-\{0,1\}[0-9]\{1,\}\) y=\(-\{0,1\}[0-9]\{1,\}\).*/\1 \2/p" |
        tail -1
}
require_point() {
    local found
    found="$(point "$1" "$2")"
    [ -n "$found" ] || {
        echo "FAIL: the app never reported where it drew '$2'"
        flutter_log | tail -30
        exit 1
    }
    printf '%s\n' "$found"
}
# `input text` drops characters on a ~300-byte invite, and a partial paste is a
# DIFFERENT test (the authority refuses it as damaged, which is correct and not
# what is being asked here).
type_text() {
    local text="$1" i=0 n=${#1}
    while [ "$i" -lt "$n" ]; do
        "$ADB" shell input text "$(printf '%s' "${text:$i:32}")"
        i=$((i + 32))
        sleep 0.35
    done
}
create_answer() {
    flutter_log |
        sed -n 's/.*envoix-f2b create id=[0-9a-f]\{32\} kind=[a-z]\{1,\} answer=\([^ ]*\).*/\1/p' |
        tail -1
}
join_until_answered() {
    local before="$1" attempt=0 now
    while [ "$attempt" -lt 5 ]; do
        tap_at "$(require_point 'envoix-f2b sheet' join)"
        sleep 5
        now="$(create_answer)"
        if [ "$now" != "$before" ]; then
            printf '%s' "$now"
            return 0
        fi
        attempt=$((attempt + 1))
    done
    return 1
}

# ---- 1. the catalogue will deploy this environment -----------------------
echo "==> the catalogue admits the environment this build is for"
ENVIRONMENT="$(identity deployment_environment)"
RENDEZVOUS="$(identity deployment_rendezvous_endpoint)"
RELAY="$(identity deployment_relay_url)"
[ -n "$ENVIRONMENT" ] && [ -n "$RENDEZVOUS" ] && [ -n "$RELAY" ] || {
    echo "FAIL: $IDENTITY declares no deployment identity."
    echo "      Re-run scripts/build-jni-libs.sh, which records it."
    exit 1
}
(cd "$REPO" && cargo run -q -p xtask -- deploy-check "$ENVIRONMENT" >/dev/null)
echo "    $ENVIRONMENT: $RENDEZVOUS"

# ---- 2. that endpoint is live and pairs two peers ------------------------
# No address is typed here. The probe defaults to `BUILD_TARGET`, so the string
# it dials is the one the catalogue derived and the app was compiled with.
echo "==> two clients of this codebase pair through the LIVE rendezvous"
(cd "$REPO" && cargo run -q -p envoix-server --example pair_probe) || {
    echo "FAIL: $RENDEZVOUS did not pair two peers."
    echo "      Either the deployment is down, or this machine has no route to"
    echo "      its UDP port. That is a claim about the SERVER; the steps below"
    echo "      are about the artifact and can be run separately."
    exit 1
}

# ---- 3. a release-signed build boots ------------------------------------
echo "==> install the RELEASE apk over a clean slate"
"$ADB" wait-for-device
[ -f "$APK" ] || {
    echo "FAIL: $APK is missing; run ./gradlew :app:assembleRelease"
    exit 1
}
# The gate already judges the signer on the artifact; this is a cheap local
# re-check so a debug-signed APK fails before the emulator work. `apksigner`,
# not `keytool`: a v2/v3-only APK carries no JAR signature for keytool to read,
# and an unreadable answer must not look like a passing one.
APKSIGNER="${APKSIGNER:-$(ls -d "${ANDROID_HOME:-/usr/local/android-sdk}"/build-tools/*/apksigner 2>/dev/null | tail -1)}"
[ -x "$APKSIGNER" ] || {
    echo "FAIL: no apksigner under \$ANDROID_HOME/build-tools; set APKSIGNER="
    exit 1
}
SIGNER="$("$APKSIGNER" verify --print-certs "$APK" | sed -n 's/.*SHA-256 digest: *//p' | head -1)"
EXPECTED_SIGNER="$(sed -n 's/^signer_sha256 = "\(.*\)"$/\1/p' "$REPO/registry/release-ledger.toml")"
[ "$SIGNER" = "$EXPECTED_SIGNER" ] || {
    echo "FAIL: $APK is signed by '$SIGNER', not the release identity."
    exit 1
}
"$ADB" uninstall "$APP_ID" >/dev/null 2>&1 || true
"$ADB" install -r "$APK"
"$ADB" logcat -c
"$ADB" shell am start -n "$ACTIVITY" >/dev/null
sleep 8

"$ADB" shell dumpsys activity services "$APP_ID" | grep -q "EnvoixHostService" || {
    echo "FAIL: the foreground Service that owns the Runtime is not running"
    "$ADB" shell dumpsys activity services "$APP_ID" | head -20
    exit 1
}
require_point 'envoix-f2b sheet' new-transfer >/dev/null
echo "    the release build is up, the Service owns the Runtime"

# ---- 4. the build the app STATES names that deployment -------------------
echo "==> the app states which deployment it is for"
tap_at "$(require_point 'envoix-d2 destination' logs)"
sleep 4
BUILD_LINE="$(flutter_log | sed -n 's/.*\(envoix-d2 build .*\)/\1/p' | tail -1)"
[ -n "$BUILD_LINE" ] || {
    echo "FAIL: the Logs screen drew no build card, so the app states no"
    echo "      deployment at all. (Did the tap reach the Logs destination?)"
    flutter_log | tail -30
    exit 1
}
field() { printf '%s\n' "$BUILD_LINE" | sed -n "s/.*$1=\([^ ]*\).*/\1/p"; }
for name in environment:"$ENVIRONMENT" rendezvous:"$RENDEZVOUS" relay:"$RELAY"; do
    key="${name%%:*}"
    want="${name#*:}"
    got="$(field "$key")"
    [ "$got" = "$want" ] || {
        echo "FAIL: the app states $key=$got, but this build declares $want"
        echo "      $BUILD_LINE"
        exit 1
    }
done
echo "    $BUILD_LINE"

# ---- 5. the authority still creates durable truth in a release build -----
echo "==> ask the core for an invite"
(cd "$REPO" && cargo test -q -p envoix-host-android --test frontend_lane \
    flutter_creates_a_transfer_without_the_debug_bridge >/dev/null)
FIXTURES="$REPO/target/tmp/create-lane"
INVITE="$(cat "$FIXTURES/invite.txt")"
# The FINGERPRINT, not the code: a release build must never log the SPAKE2
# password, so the digest is what reaches the screen and what can be compared.
EXPECTED_FINGERPRINT="$(cat "$FIXTURES/fingerprint.txt")"
case "$INVITE" in
envoix://*) ;;
*)
    echo "FAIL: the headless test wrote no invite the core produced"
    exit 1
    ;;
esac

echo "==> a card comes into existence from the real UI"
"$ADB" shell input keyevent KEYCODE_BACK
sleep 3
tap_at "$(require_point 'envoix-f2b sheet' new-transfer)"
sleep 3
tap_at "$(require_point 'envoix-f2b sheet' invite)"
sleep 1
type_text "$INVITE"
"$ADB" shell input keyevent 4
sleep 2.5
ANSWER="$(join_until_answered "")"
CARD="$(printf '%s\n' "$ANSWER" | sed -n 's/^created:\([0-9a-f]\{16\}\)$/\1/p')"
[ -n "$CARD" ] || {
    echo "FAIL: the authority answered '$ANSWER' rather than creating a card"
    flutter_log | tail -30
    exit 1
}
"$ADB" shell input keyevent KEYCODE_BACK
sleep 4
FINGERPRINT="$(
    flutter_log |
        sed -n "s/.*envoix-f2b invite card=$CARD fingerprint=\([^ ]*\).*/\1/p" | tail -1
)"
[ "$FINGERPRINT" = "$EXPECTED_FINGERPRINT" ] || {
    echo "FAIL: card $CARD drew fingerprint '$FINGERPRINT', the core published"
    echo "      '$EXPECTED_FINGERPRINT' — the pasted invite is not the one created"
    exit 1
}

echo "PASS: android_end_to_end_release"
echo "      $RENDEZVOUS is live and paired two peers of this codebase;"
echo "      a release-signed build installed, booted its Service-owned Runtime,"
echo "      stated that same deployment ($ENVIRONMENT) on screen, and created"
echo "      card $CARD on the core's own channel ($FINGERPRINT) from its invite."
echo "      NOT covered: the app process opening that socket, the QR scan path,"
echo "      the send path past the picker, or any transfer (see this header)."
