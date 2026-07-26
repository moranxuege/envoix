#!/usr/bin/env bash
# flutter_commands_a_card_and_keeps_its_truth — F2a instrumentation.
# RUN BY THE RECONCILER ONLY (one emulator; legs never touch adb).
#
# Extends `f1c-e2e.sh` rather than repeating it: that script leaves a real card
# in a host that outlived an activity death, with the frontend re-attached and
# the logs screen open. This one then proves the command path against it:
#
#   1. the card publishes an offer, and the app draws exactly that offer (the
#      line is printed by the BUTTON as it lays out, which is also how this
#      script knows where to tap — next to `Remove`, a guessed coordinate is
#      not a benign miss);
#   2. tapping `Pause` produces an accepted-then-settled command, and the two
#      are distinguishable on screen;
#   3. the card's DURABLE truth changed: the committed record ON DISK is paused
#      (read back through the debug probe, not through the projection that
#      would look the same for a change that never left memory), it comes back
#      Paused, and the offer the authority publishes moved with it;
#   4. the frontend restart — the activity dies and re-attaches, which is what
#      a restarted isolate does — keeps the card's truth and forgets the
#      command entirely, because the frontend keeps nothing.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ADB="${ADB:-adb}"
APP_ID="${APP_ID:-app.envoix.host.dev}"
ACTIVITY="$APP_ID/app.envoix.host.MainActivity"
SVC="$APP_ID/app.envoix.host.EnvoixHostService"

echo "==> F1c first: a card, an activity death, a re-attach, both screens"
F1C="$("$HERE/f1c-e2e.sh" | tee /dev/stderr)"
CARD=$(printf '%s\n' "$F1C" |
    sed -n 's/.*card \([0-9a-f]\{16\}\) is in the home list.*/\1/p' | tail -1)
[ -n "$CARD" ] || {
    echo "FAIL: could not tell which card F1c used"
    exit 1
}

card_line() {
    "$ADB" logcat -d -s flutter |
        sed -n "s/.*\(envoix-f2a card=$CARD .*\)/\1/p" | tail -1
}
affordance() {
    "$ADB" logcat -d -s flutter |
        sed -n "s/.*envoix-f2a affordance card=$CARD command=$1 x=\([0-9]\{1,\}\) y=\([0-9]\{1,\}\).*/\1 \2/p" |
        tail -1
}
command_line() {
    "$ADB" logcat -d -s flutter |
        sed -n "s/.*\(envoix-f2a command card=$CARD .*\)/\1/p" | tail -1
}

# F1c leaves the logs screen open; back returns to the transfers list.
echo "==> back to the transfers list"
"$ADB" logcat -c
"$ADB" shell input keyevent KEYCODE_BACK
sleep 3
LINE=$(card_line)
[ -n "$LINE" ] || {
    echo "FAIL: the transfers list never drew card $CARD"
    "$ADB" logcat -d -s flutter | tail -20
    exit 1
}
BEFORE_ACTIONS=$(printf '%s\n' "$LINE" | sed -n 's/.*actions=\([a-z_,]*\) state=.*/\1/p')
BEFORE_STATE=$(printf '%s\n' "$LINE" | sed -n 's/.*state=\(.*\)$/\1/p')
echo "    card $CARD is $BEFORE_STATE, and the authority offers: $BEFORE_ACTIONS"
case ",$BEFORE_ACTIONS," in
*,pause,*) ;;
*)
    echo "FAIL: the authority does not offer pause, so this test has nothing to tap"
    exit 1
    ;;
esac

echo "==> tap the affordance the app says it drew"
read -r TAP_X TAP_Y <<<"$(affordance pause)"
[ -n "${TAP_X:-}" ] && [ -n "${TAP_Y:-}" ] || {
    echo "FAIL: the app never reported where it drew the pause affordance"
    "$ADB" logcat -d -s flutter | tail -20
    exit 1
}
"$ADB" shell input tap "$TAP_X" "$TAP_Y"
sleep 5

CMD=$(command_line)
[ -n "$CMD" ] || {
    echo "FAIL: tapping pause produced no command at all"
    "$ADB" logcat -d -s flutter | tail -20
    exit 1
}
COMMAND_ID=$(printf '%s\n' "$CMD" | sed -n 's/.*id=\([0-9a-f]\{32\}\).*/\1/p')
PHASE=$(printf '%s\n' "$CMD" | sed -n 's/.*phase=\([a-z]\{1,\}\).*/\1/p')
echo "    command $COMMAND_ID reached phase=$PHASE"
[ "$PHASE" = "settled" ] || {
    echo "FAIL: the command never settled (phase=$PHASE); acceptance is not completion"
    "$ADB" logcat -d -s flutter | grep envoix-f2a | tail -20
    exit 1
}
# Acceptance and completion are distinct states, and both were on screen.
"$ADB" logcat -d -s flutter | grep -q "envoix-f2a command card=$CARD id=$COMMAND_ID .*phase=accepted" || {
    echo "FAIL: the command went straight to settled; the in-flight state was never shown"
    exit 1
}
# `settled` is equally the phase of a refusal, so require the answer itself.
printf '%s\n' "$CMD" | grep -q "answer=committed:paused" || {
    echo "FAIL: the command settled without a committed pause: $CMD"
    exit 1
}

# The projection would read the same for a change that never left memory, so
# ask the store what it actually committed.
"$ADB" shell am start-foreground-service -n "$SVC" \
    -a "$APP_ID.action.e2e-durable" >/dev/null
sleep 2
"$ADB" logcat -d -s EnvoixE2e | grep -q "durable=.*$CARD:paused" || {
    echo "FAIL: the command settled but the on-disk record is not paused"
    "$ADB" logcat -d -s EnvoixE2e | tail -10
    exit 1
}
echo "    the committed record on disk is paused"

AFTER=$(card_line)
AFTER_ACTIONS=$(printf '%s\n' "$AFTER" | sed -n 's/.*actions=\([a-z_,]*\) state=.*/\1/p')
AFTER_STATE=$(printf '%s\n' "$AFTER" | sed -n 's/.*state=\(.*\)$/\1/p')
echo "    the card is now $AFTER_STATE, offering: $AFTER_ACTIONS"
[ "$AFTER_STATE" != "$BEFORE_STATE" ] || {
    echo "FAIL: the command settled but the card's state never changed"
    exit 1
}
case ",$AFTER_ACTIONS," in
*,resume,*) ;;
*)
    echo "FAIL: a paused card must be offered resume, got: $AFTER_ACTIONS"
    exit 1
    ;;
esac

echo "==> the frontend restarts; the card's truth does not"
HOST_PID=$("$ADB" shell pidof "$APP_ID" | tr -d '\r')
"$ADB" logcat -c
"$ADB" shell input keyevent KEYCODE_BACK
sleep 3
"$ADB" shell am start -n "$ACTIVITY" >/dev/null
sleep 5
AGAIN_PID=$("$ADB" shell pidof "$APP_ID" | tr -d '\r')
[ "$AGAIN_PID" = "$HOST_PID" ] || {
    echo "FAIL: the host process restarted ($HOST_PID -> $AGAIN_PID); this proves nothing"
    exit 1
}
RESTARTED=$(card_line)
[ -n "$RESTARTED" ] || {
    echo "FAIL: the restarted app never drew card $CARD"
    "$ADB" logcat -d -s flutter | tail -20
    exit 1
}
RESTARTED_STATE=$(printf '%s\n' "$RESTARTED" | sed -n 's/.*state=\(.*\)$/\1/p')
[ "$RESTARTED_STATE" = "$AFTER_STATE" ] || {
    echo "FAIL: the card came back as '$RESTARTED_STATE', not '$AFTER_STATE'"
    exit 1
}
# The frontend keeps nothing: a new attachment has no intent to draw, and the
# completions the old one was owed were discarded with it.
if [ -n "$(command_line)" ]; then
    echo "FAIL: the restarted frontend still shows a command it never issued"
    "$ADB" logcat -d -s flutter | grep envoix-f2a | tail -20
    exit 1
fi
echo "PASS: flutter_commands_a_card_and_keeps_its_truth"
echo "      card $CARD went $BEFORE_STATE -> $AFTER_STATE (paused on disk) by"
echo "      command $COMMAND_ID,"
echo "      the offer went ($BEFORE_ACTIONS) -> ($AFTER_ACTIONS), and after a"
echo "      frontend restart the card is still $RESTARTED_STATE with no command"
echo "      state carried across (host pid $HOST_PID throughout)"
