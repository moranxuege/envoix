#!/usr/bin/env bash
# flutter_shows_the_card_and_its_evidence — F1c instrumentation.
# RUN BY THE RECONCILER ONLY (one emulator; legs never touch adb).
#
# Extends `f1b-e2e.sh` rather than repeating it: that script leaves a real card
# in a host that outlived an activity death, with the frontend re-attached and
# in the foreground. This one then proves the two read-only screens against it:
#
#   1. the home list is showing that card (the line is printed by the card TILE
#      as it draws, not by the model behind it);
#   2. the logs screen shows that card's session timeline — recorded by the
#      authority long before this attachment existed, with at least one entry
#      and its own claim about whether it is complete.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ADB="${ADB:-adb}"
APP_ID="${APP_ID:-app.envoix.host.dev}"

echo "==> F1b first: a card, an activity death, a re-attach"
F1B="$("$HERE/f1b-e2e.sh" | tee /dev/stderr)"
CARD=$(printf '%s\n' "$F1B" |
    sed -n 's/.*card \([0-9a-f]\{16\}\) rendered at epoch.*/\1/p' | tail -1)
[ -n "$CARD" ] || {
    echo "FAIL: could not tell which card F1b used"
    exit 1
}

echo "==> the home list is showing card $CARD"
"$ADB" logcat -d -s flutter | grep -q "envoix-f1b rendered card=$CARD" || {
    echo "FAIL: the re-attached home list never drew card $CARD"
    exit 1
}

# The Logs destination is the right-hand half of the bottom navigation bar,
# which sits above whatever the system's own navigation takes: 40dp up for the
# middle of the bar, plus 48dp for a three-button bar or 24dp for a gesture
# one. Density comes from the device rather than being assumed, and a miss is
# retried at the other heights before it is called a failure — a tap that lands
# nowhere must not read as an empty evidence screen.
read -r WIDTH HEIGHT <<<"$("$ADB" shell wm size | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1 \2/p' | tail -1)"
DENSITY=$("$ADB" shell wm density | sed -n 's/.*: \([0-9]*\).*/\1/p' | tail -1)
: "${DENSITY:=420}"
X=$((WIDTH * 3 / 4))

echo "==> open the logs destination (${WIDTH}x${HEIGHT} at ${DENSITY}dpi)"
timeline_line() {
    "$ADB" logcat -d -s flutter |
        sed -n "s/.*\(envoix-f1c timeline card=$CARD .*\)/\1/p" | tail -1
}
LINE=""
for DP in 88 64 40 112; do
    Y=$((HEIGHT - DP * DENSITY / 160))
    "$ADB" shell input tap "$X" "$Y"
    sleep 3
    LINE=$(timeline_line)
    [ -n "$LINE" ] && break
    echo "    no timeline yet after tapping ${DP}dp up from the bottom; trying again"
done
[ -n "$LINE" ] || {
    echo "FAIL: the logs screen never drew a timeline for card $CARD"
    "$ADB" logcat -d -s flutter | tail -20
    exit 1
}

ENTRIES=$(printf '%s\n' "$LINE" | sed -n 's/.*entries=\([0-9]\{1,\}\).*/\1/p')
DIAGNOSTICS=$(printf '%s\n' "$LINE" | sed -n 's/.*diagnostics=\([a-z:0-9]\{1,\}\).*/\1/p')
[ -n "$ENTRIES" ] && [ "$ENTRIES" -gt 0 ] || {
    echo "FAIL: the timeline on screen has no entries: $LINE"
    exit 1
}
echo "PASS: flutter_shows_the_card_and_its_evidence"
echo "      card $CARD is in the home list, and its timeline is on the logs"
echo "      screen with $ENTRIES entries, diagnostics=$DIAGNOSTICS"
