#!/usr/bin/env bash
# packaged_process_death_preserves_cards — BN4 instrumentation.
# RUN BY THE RECONCILER ONLY (one emulator; legs never touch adb).
#
# Proves: a durable card created in process generation 1 is RESTORED by
# generation 2 after an uncooperative process death. The assertion reads the
# LIVE runtime — the debug probe reports the cards the host actually brought
# back and attached — not a file count that survives a force-stop by
# definition.
set -euo pipefail
ADB="${ADB:-adb}"
APP_ID="${APP_ID:-app.envoix.host.dev}"
SVC="$APP_ID/app.envoix.host.EnvoixHostService"
TAG="EnvoixE2e"

# The debug service is exported behind a signature-level permission, which the
# shell UID does not hold; root bypasses permission checks on a userdebug image.
"$ADB" root >/dev/null 2>&1 || true
"$ADB" wait-for-device

echo "==> install"
"$ADB" install -r "$(dirname "$0")/../app/build/outputs/apk/dev/debug/app-dev-debug.apk"

echo "==> generation 1: start the host and create a durable card"
"$ADB" logcat -c
"$ADB" shell am start-foreground-service -n "$SVC"
sleep 3
"$ADB" shell am start-foreground-service -n "$SVC" \
  -a "$APP_ID.action.e2e-create" --es name e2e-card.bin --el total 4096
sleep 3
CREATED=$("$ADB" logcat -d -s "$TAG" | sed -n 's/.*created=\([0-9a-f]\{16\}\).*/\1/p' | tail -1)
[ -n "$CREATED" ] && [ "$CREATED" != "0000000000000000" ] || {
  echo "FAIL: generation 1 created no durable card"
  exit 1
}
echo "    created card: $CREATED"

echo "==> uncooperative process death"
"$ADB" shell am force-stop "$APP_ID"
sleep 2

echo "==> generation 2: relaunch and probe the RESTORED runtime"
"$ADB" logcat -c
"$ADB" shell am start-foreground-service -n "$SVC"
sleep 4
"$ADB" shell am start-foreground-service -n "$SVC" -a "$APP_ID.action.e2e-probe"
sleep 2
RESTORED=$("$ADB" logcat -d -s "$TAG" | sed -n 's/.*restored=\([0-9a-f,]*\).*/\1/p' | tail -1)
echo "    restored cards: ${RESTORED:-<none>}"
case ",$RESTORED," in
  *",$CREATED,"*) ;;
  *)
    echo "FAIL: card $CREATED did not come back from durable truth"
    exit 1
    ;;
esac
echo "PASS: packaged_process_death_preserves_cards (card $CREATED restored in generation 2)"
