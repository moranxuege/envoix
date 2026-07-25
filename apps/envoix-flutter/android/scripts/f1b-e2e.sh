#!/usr/bin/env bash
# flutter_attaches_and_survives_activity_death — F1b instrumentation.
# RUN BY THE RECONCILER ONLY (one emulator; legs never touch adb).
#
# Proves three things about the real packaged app:
#   1. the Flutter activity attaches to a host that was ALREADY running and
#      renders the card that host holds — the id asserted here is the one the
#      card TILE printed as it drew, not one the model merely decoded;
#   2. killing the activity leaves the host process untouched (same pid, same
#      service, same card) — the frontend owns no lifetime;
#   3. re-attaching opens a FRESH epoch, strictly greater than the one the dead
#      attachment saw.
set -euo pipefail
ADB="${ADB:-adb}"
APP_ID="${APP_ID:-app.envoix.host.dev}"
SVC="$APP_ID/app.envoix.host.EnvoixHostService"
ACTIVITY="$APP_ID/app.envoix.host.MainActivity"

# The debug service is exported behind a signature-level permission, which the
# shell UID does not hold; root bypasses permission checks on a userdebug image.
"$ADB" root >/dev/null 2>&1 || true
"$ADB" wait-for-device

# The line the card tile prints as it renders: `card=<16 hex> epoch=<n>`.
rendered_epoch() {
    "$ADB" logcat -d -s flutter |
        sed -n "s/.*envoix-f1b rendered card=$1 epoch=\([0-9]\{1,\}\).*/\1/p" |
        tail -1
}

echo "==> install"
"$ADB" install -r "$(dirname "$0")/../app/build/outputs/apk/dev/debug/app-dev-debug.apk"

echo "==> the host runs first, with no frontend anywhere"
"$ADB" logcat -c
"$ADB" shell am start-foreground-service -n "$SVC"
sleep 3
"$ADB" shell am start-foreground-service -n "$SVC" \
  -a "$APP_ID.action.e2e-create" --es name f1b-card.bin --el total 4096
sleep 3
CREATED=$("$ADB" logcat -d -s EnvoixE2e | sed -n 's/.*created=\([0-9a-f]\{16\}\).*/\1/p' | tail -1)
[ -n "$CREATED" ] && [ "$CREATED" != "0000000000000000" ] || {
  echo "FAIL: the host created no durable card"
  exit 1
}
HOST_PID=$("$ADB" shell pidof "$APP_ID" | tr -d '\r')
echo "    card $CREATED, host pid $HOST_PID"

echo "==> generation 1: attach the frontend"
"$ADB" shell am start -n "$ACTIVITY" >/dev/null
sleep 5
EPOCH1=$(rendered_epoch "$CREATED")
[ -n "$EPOCH1" ] || {
  echo "FAIL: the app never rendered card $CREATED"
  "$ADB" logcat -d -s flutter | tail -20
  exit 1
}
echo "    rendered card $CREATED at epoch $EPOCH1"

echo "==> the activity dies; the transfer must not notice"
"$ADB" logcat -c
"$ADB" shell input keyevent KEYCODE_BACK
sleep 3
if "$ADB" shell dumpsys activity activities | grep -q "ActivityRecord{.*$ACTIVITY"; then
  echo "FAIL: the activity is still there; killing it proves nothing"
  exit 1
fi
STILL_PID=$("$ADB" shell pidof "$APP_ID" | tr -d '\r')
[ "$STILL_PID" = "$HOST_PID" ] || {
  echo "FAIL: the host process changed ($HOST_PID -> $STILL_PID) when the activity died"
  exit 1
}
"$ADB" shell dumpsys activity services "$APP_ID" | grep -q EnvoixHostService || {
  echo "FAIL: the host service died with the activity"
  exit 1
}
echo "    host pid $STILL_PID still serving"

echo "==> generation 2: re-attach"
"$ADB" shell am start -n "$ACTIVITY" >/dev/null
sleep 5
EPOCH2=$(rendered_epoch "$CREATED")
[ -n "$EPOCH2" ] || {
  echo "FAIL: the re-attached app never rendered card $CREATED"
  "$ADB" logcat -d -s flutter | tail -20
  exit 1
}
[ "$EPOCH2" -gt "$EPOCH1" ] || {
  echo "FAIL: re-attaching must open a fresh epoch, got $EPOCH2 after $EPOCH1"
  exit 1
}
AGAIN_PID=$("$ADB" shell pidof "$APP_ID" | tr -d '\r')
[ "$AGAIN_PID" = "$HOST_PID" ] || {
  echo "FAIL: the host process restarted ($HOST_PID -> $AGAIN_PID)"
  exit 1
}
echo "PASS: flutter_attaches_and_survives_activity_death"
echo "      card $CREATED rendered at epoch $EPOCH1, re-attached at epoch $EPOCH2,"
echo "      host pid $HOST_PID throughout"
