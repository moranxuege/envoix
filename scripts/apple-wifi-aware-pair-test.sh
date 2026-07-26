#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/apple-wifi-aware-pair-test.sh \
    <receiver.xctestrun> <receiver-destination> \
    <sender.xctestrun> <sender-destination> \
    <test-identifier> <evidence-directory> <run-label>

Run both sides of one already-built Apple Wi-Fi Aware physical test. The
receiver is started first; the sender starts as soon as the receiver listener
has entered a running state. A waiting listener is sufficient because the
subscriber may make the Wi-Fi Aware network viable.
EOF
}

if [[ "$#" -ne 7 ]]; then
  usage >&2
  exit 2
fi

receiver_xctestrun="$1"
receiver_destination="$2"
sender_xctestrun="$3"
sender_destination="$4"
test_identifier="$5"
evidence_directory="$6"
run_label="$7"

for xctestrun in "$receiver_xctestrun" "$sender_xctestrun"; do
  if [[ ! -f "$xctestrun" ]]; then
    echo "error: missing xctestrun: $xctestrun" >&2
    exit 2
  fi
done
if [[ ! "$run_label" =~ ^[A-Za-z0-9_-]{1,64}$ ]]; then
  echo "error: run label must contain only letters, digits, underscores, or hyphens" >&2
  exit 2
fi

receiver_products="$(cd "$(dirname "$receiver_xctestrun")" && pwd)"
sender_products="$(cd "$(dirname "$sender_xctestrun")" && pwd)"
if [[ "$receiver_products" != "$sender_products" ]]; then
  echo "error: both xctestrun files must use the same build products directory" >&2
  exit 2
fi
derived_data="$(cd "$receiver_products/../.." && pwd)"

mkdir -p "$evidence_directory"
receiver_result="$evidence_directory/$run_label-receiver.xcresult"
sender_result="$evidence_directory/$run_label-sender.xcresult"
receiver_log="$evidence_directory/$run_label-receiver.log"
sender_log="$evidence_directory/$run_label-sender.log"
for artifact in "$receiver_result" "$sender_result" "$receiver_log" "$sender_log"; do
  if [[ -e "$artifact" ]]; then
    echo "error: refusing to overwrite existing evidence: $artifact" >&2
    exit 2
  fi
done

receiver_pid=""
sender_pid=""
stop_children() {
  if [[ -n "$sender_pid" ]] && kill -0 "$sender_pid" >/dev/null 2>&1; then
    kill "$sender_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$receiver_pid" ]] && kill -0 "$receiver_pid" >/dev/null 2>&1; then
    kill "$receiver_pid" >/dev/null 2>&1 || true
  fi
  [[ -z "$sender_pid" ]] || wait "$sender_pid" >/dev/null 2>&1 || true
  [[ -z "$receiver_pid" ]] || wait "$receiver_pid" >/dev/null 2>&1 || true
}
trap stop_children EXIT INT TERM

xcodebuild \
  test-without-building \
  -xctestrun "$receiver_xctestrun" \
  -destination "$receiver_destination" \
  -derivedDataPath "$derived_data" \
  -parallel-testing-enabled NO \
  -only-testing:"$test_identifier" \
  -resultBundlePath "$receiver_result" \
  >"$receiver_log" 2>&1 &
receiver_pid="$!"

listener_deadline=$((SECONDS + 60))
while ! grep -Eq \
  'receiver listener_state=(waiting|ready)|role=receiver listener_state=(waiting|ready)' \
  "$receiver_log" 2>/dev/null; do
  if ! kill -0 "$receiver_pid" >/dev/null 2>&1; then
    wait "$receiver_pid" || true
    echo "error: receiver exited before its listener became ready" >&2
    tail -n 80 "$receiver_log" >&2
    exit 1
  fi
  if (( SECONDS >= listener_deadline )); then
    echo "error: receiver listener did not start within 60 seconds" >&2
    tail -n 80 "$receiver_log" >&2
    exit 1
  fi
  sleep 0.2
done

xcodebuild \
  test-without-building \
  -xctestrun "$sender_xctestrun" \
  -destination "$sender_destination" \
  -derivedDataPath "$derived_data" \
  -parallel-testing-enabled NO \
  -only-testing:"$test_identifier" \
  -resultBundlePath "$sender_result" \
  >"$sender_log" 2>&1 &
sender_pid="$!"

set +e
wait "$sender_pid"
sender_status="$?"
if [[ "$sender_status" -ne 0 ]] && kill -0 "$receiver_pid" >/dev/null 2>&1; then
  kill "$receiver_pid" >/dev/null 2>&1 || true
fi
wait "$receiver_pid"
receiver_status="$?"
set -e
sender_pid=""
receiver_pid=""
trap - EXIT INT TERM

if [[ "$receiver_status" -ne 0 || "$sender_status" -ne 0 ]] \
  || ! grep -Eq "Executed 1 test, with 0 failures" "$receiver_log" \
  || ! grep -Eq "Executed 1 test, with 0 failures" "$sender_log"; then
  echo "error: paired Apple Wi-Fi Aware test failed" >&2
  echo "receiver status: $receiver_status" >&2
  tail -n 100 "$receiver_log" >&2
  echo "sender status: $sender_status" >&2
  tail -n 100 "$sender_log" >&2
  exit 1
fi

echo "Apple Wi-Fi Aware paired test passed: $run_label"
grep -E 'wifi-aware-(physical|benchmark)|Executed 1 test' "$receiver_log" || true
grep -E 'wifi-aware-(physical|benchmark)|Executed 1 test' "$sender_log" || true
