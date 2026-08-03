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

readonly receiver_start_timeout_seconds=120
readonly rendezvous_invitation_count=6
readonly app_bundle_identifier='com.envoix.app.ios'
readonly invitation_sidecar_environment='ENVOIX_WIFI_AWARE_INVITATION_SIDECAR_FILENAME'
readonly default_pair_watchdog_seconds=360
readonly pair_watchdog_grace_seconds=60

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
patched_receiver_xctestrun="$receiver_products/.$run_label-receiver.xctestrun"
patched_sender_xctestrun="$sender_products/.$run_label-sender.xctestrun"
for artifact in \
  "$receiver_result" \
  "$sender_result" \
  "$receiver_log" \
  "$sender_log" \
  "$patched_receiver_xctestrun" \
  "$patched_sender_xctestrun"; do
  if [[ -e "$artifact" ]]; then
    echo "error: refusing to overwrite existing evidence: $artifact" >&2
    exit 2
  fi
done

receiver_pid=""
sender_pid=""
receiver_run_xctestrun="$receiver_xctestrun"
sender_run_xctestrun="$sender_xctestrun"
patched_receiver_created=0
patched_sender_created=0
invitation_sidechannel_directory=""
invitation_sidechannel_file=""
set_xctestrun_environment() {
  local file="$1"
  local key="$2"
  local value="$3"
  local scope
  for scope in EnvironmentVariables TestingEnvironmentVariables; do
    /usr/libexec/PlistBuddy \
      -c "Add :Envoix-iOSUITests:$scope:$key string $value" \
      "$file" >/dev/null 2>&1 \
      || /usr/libexec/PlistBuddy \
        -c "Set :Envoix-iOSUITests:$scope:$key $value" \
        "$file" >/dev/null
  done
}

append_xctestrun_argument() {
  local file="$1"
  local value="$2"
  /usr/libexec/PlistBuddy \
    -c "Add :Envoix-iOSUITests:CommandLineArguments: string $value" \
    "$file" >/dev/null
}

set_host_discovery_isolation() {
  local file="$1"
  append_xctestrun_argument "$file" --ui-testing-discovery-fixtures
  append_xctestrun_argument "$file" -envoix.nearby.visibility
  append_xctestrun_argument "$file" hidden
}

set_rendezvous_environment() {
  local file="$1"
  local role="$2"
  local local_peer_key="$3"
  local expected_peer_key="$4"
  local run_id="$5"
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_RENDEZVOUS_PHYSICAL 1
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_RENDEZVOUS_ROLE "$role"
  set_xctestrun_environment \
    "$file" \
    ENVOIX_WIFI_AWARE_RENDEZVOUS_LOCAL_PEER_KEY \
    "$local_peer_key"
  set_xctestrun_environment \
    "$file" \
    ENVOIX_WIFI_AWARE_RENDEZVOUS_EXPECTED_PEER_KEY \
    "$expected_peer_key"
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_RENDEZVOUS_RUN_ID "$run_id"
  set_host_discovery_isolation "$file"
}

set_physical_transfer_environment() {
  local file="$1"
  local role="$2"
  local peer_hint="$3"
  local run_id="$4"
  local pairing_token="$5"
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_PHYSICAL 1
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_ROLE "$role"
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_PEER_HINT "$peer_hint"
  set_xctestrun_environment "$file" ENVOIX_WIFI_AWARE_RUN_ID "$run_id"
  set_xctestrun_environment \
    "$file" \
    ENVOIX_WIFI_AWARE_PAIRING_TOKEN \
    "$pairing_token"
  set_host_discovery_isolation "$file"
}

invalid_host_environment() {
  local name="$1"
  local expected="$2"
  echo "error: $name must be $expected" >&2
  exit 2
}

validate_bounded_decimal_environment() {
  local name="$1"
  local value="$2"
  local minimum="$3"
  local maximum="$4"
  if [[ ! "$value" =~ ^[0-9]+$ ]] || (( ${#value} > 10 )); then
    invalid_host_environment "$name" "a decimal integer in [$minimum, $maximum]"
  fi
  local decimal_value=$((10#$value))
  if (( decimal_value < minimum || decimal_value > maximum )); then
    invalid_host_environment "$name" "a decimal integer in [$minimum, $maximum]"
  fi
}

validate_peer_id_environment() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[0-9A-Fa-f]{1,16}$ ]]; then
    invalid_host_environment "$name" "1 to 16 hexadecimal digits"
  fi
}

validate_optional_transfer_environment() {
  if [[ "${ENVOIX_WIFI_AWARE_PAYLOAD_MIB+x}" == x ]]; then
    validate_bounded_decimal_environment \
      ENVOIX_WIFI_AWARE_PAYLOAD_MIB \
      "$ENVOIX_WIFI_AWARE_PAYLOAD_MIB" \
      1 \
      1024
  fi
  if [[ "${ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS+x}" == x ]]; then
    validate_bounded_decimal_environment \
      ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS \
      "$ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS" \
      30 \
      7200
  fi
  if [[ "${ENVOIX_WIFI_AWARE_DROP_AT_PERCENT+x}" == x ]]; then
    validate_bounded_decimal_environment \
      ENVOIX_WIFI_AWARE_DROP_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_DROP_AT_PERCENT" \
      1 \
      90
  fi
  if [[ "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" == x ]]; then
    validate_bounded_decimal_environment \
      ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT" \
      0 \
      90
  fi
  if [[ "${ENVOIX_WIFI_AWARE_DROP_AT_PERCENT+x}" == x \
    && "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" == x ]]; then
    echo "error: DROP_AT_PERCENT and CANCEL_AT_PERCENT cannot be combined" >&2
    exit 2
  fi
  if [[ "${ENVOIX_WIFI_AWARE_RECEIVER_PEER_ID+x}" == x ]]; then
    validate_peer_id_environment \
      ENVOIX_WIFI_AWARE_RECEIVER_PEER_ID \
      "$ENVOIX_WIFI_AWARE_RECEIVER_PEER_ID"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_SENDER_PEER_ID+x}" == x ]]; then
    validate_peer_id_environment \
      ENVOIX_WIFI_AWARE_SENDER_PEER_ID \
      "$ENVOIX_WIFI_AWARE_SENDER_PEER_ID"
  fi
}

forward_optional_transfer_environment() {
  local receiver_file="$1"
  local sender_file="$2"
  if [[ "${ENVOIX_WIFI_AWARE_PAYLOAD_MIB+x}" == x ]]; then
    set_xctestrun_environment \
      "$receiver_file" \
      ENVOIX_WIFI_AWARE_PAYLOAD_MIB \
      "$ENVOIX_WIFI_AWARE_PAYLOAD_MIB"
    set_xctestrun_environment \
      "$sender_file" \
      ENVOIX_WIFI_AWARE_PAYLOAD_MIB \
      "$ENVOIX_WIFI_AWARE_PAYLOAD_MIB"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS+x}" == x ]]; then
    set_xctestrun_environment \
      "$receiver_file" \
      ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS \
      "$ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS"
    set_xctestrun_environment \
      "$sender_file" \
      ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS \
      "$ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_DROP_AT_PERCENT+x}" == x ]]; then
    set_xctestrun_environment \
      "$receiver_file" \
      ENVOIX_WIFI_AWARE_DROP_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_DROP_AT_PERCENT"
    set_xctestrun_environment \
      "$sender_file" \
      ENVOIX_WIFI_AWARE_DROP_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_DROP_AT_PERCENT"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" == x ]]; then
    set_xctestrun_environment \
      "$receiver_file" \
      ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT"
    set_xctestrun_environment \
      "$sender_file" \
      ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT \
      "$ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_RECEIVER_PEER_ID+x}" == x ]]; then
    set_xctestrun_environment \
      "$receiver_file" \
      ENVOIX_WIFI_AWARE_PEER_ID \
      "$ENVOIX_WIFI_AWARE_RECEIVER_PEER_ID"
  fi
  if [[ "${ENVOIX_WIFI_AWARE_SENDER_PEER_ID+x}" == x ]]; then
    set_xctestrun_environment \
      "$sender_file" \
      ENVOIX_WIFI_AWARE_PEER_ID \
      "$ENVOIX_WIFI_AWARE_SENDER_PEER_ID"
  fi
}

log_contains_skipped_test() {
  local file="$1"
  LC_ALL=C grep -Eiq \
    'Test Case .* skipped|Executed [0-9]+ tests?, with [1-9][0-9]* tests? skipped|(^|[[:space:]])XCTSkip' \
    "$file"
}

require_fixed_marker() {
  local file="$1"
  local side="$2"
  local marker="$3"
  if ! LC_ALL=C grep -Fq -- "$marker" "$file"; then
    echo "error: $side did not emit completion marker: $marker" >&2
    semantic_failure=1
  fi
}

require_regex_marker() {
  local file="$1"
  local side="$2"
  local marker_pattern="$3"
  if ! LC_ALL=C grep -Eq -- "$marker_pattern" "$file"; then
    echo "error: $side did not emit its expected completion marker" >&2
    semantic_failure=1
  fi
}

device_id_from_destination() {
  local destination="$1"
  local suffix="${destination#*id=}"
  if [[ "$suffix" == "$destination" ]]; then
    echo "error: destination does not contain a device id" >&2
    return 1
  fi
  local device_id="${suffix%%,*}"
  if [[ ! "$device_id" =~ ^[A-Fa-f0-9-]{8,64}$ ]]; then
    echo "error: destination contains an invalid device id" >&2
    return 1
  fi
  printf '%s' "$device_id"
}

terminate_test_process() {
  local pid="$1"
  local signal="${2:-TERM}"
  local child
  while IFS= read -r child; do
    [[ -z "$child" ]] || kill -s "$signal" "$child" >/dev/null 2>&1 || true
  done < <(/usr/bin/pgrep -P "$pid" 2>/dev/null || true)
  kill -s "$signal" "$pid" >/dev/null 2>&1 || true
}

cleanup_invitation_sidechannel() {
  if [[ -n "$invitation_sidechannel_file" ]]; then
    rm -f "$invitation_sidechannel_file" >/dev/null 2>&1 || true
    invitation_sidechannel_file=""
  fi
  if [[ -n "$invitation_sidechannel_directory" ]]; then
    rmdir "$invitation_sidechannel_directory" >/dev/null 2>&1 || true
    invitation_sidechannel_directory=""
  fi
}

stop_children() {
  if [[ -n "$sender_pid" ]] && kill -0 "$sender_pid" >/dev/null 2>&1; then
    terminate_test_process "$sender_pid"
  fi
  if [[ -n "$receiver_pid" ]] && kill -0 "$receiver_pid" >/dev/null 2>&1; then
    terminate_test_process "$receiver_pid"
  fi
  [[ -z "$sender_pid" ]] || wait "$sender_pid" >/dev/null 2>&1 || true
  [[ -z "$receiver_pid" ]] || wait "$receiver_pid" >/dev/null 2>&1 || true
  if [[ "$patched_receiver_created" -eq 1 ]]; then
    rm -f "$patched_receiver_xctestrun"
  fi
  if [[ "$patched_sender_created" -eq 1 ]]; then
    rm -f "$patched_sender_xctestrun"
  fi
  cleanup_invitation_sidechannel
}
trap stop_children EXIT INT TERM

completion_kind=""
case "$test_identifier" in
  *WifiAwarePhysicalRendezvousTests/testPhysicalWifiAwareRendezvousControlPlane | \
  *WifiAwarePhysicalRendezvousTests/testPhysicalWifiAwareSymmetricRendezvousControlPlane)
    completion_kind=rendezvous
    rendezvous_run_id="${run_label:0:48}"
    receiver_peer_key="$(
      printf 'receiver:%s' "$run_label" | /usr/bin/shasum -a 256 | cut -c 1-16
    )"
    sender_peer_key="$(
      printf 'sender:%s' "$run_label" | /usr/bin/shasum -a 256 | cut -c 1-16
    )"
    if [[ ! "$receiver_peer_key" =~ ^[0-9a-f]{16}$ ]] \
      || [[ ! "$sender_peer_key" =~ ^[0-9a-f]{16}$ ]] \
      || [[ "$receiver_peer_key" == "$sender_peer_key" ]]; then
      echo "error: failed to derive distinct strict peer keys from run label" >&2
      exit 2
    fi
    cp "$receiver_xctestrun" "$patched_receiver_xctestrun"
    patched_receiver_created=1
    cp "$sender_xctestrun" "$patched_sender_xctestrun"
    patched_sender_created=1
    set_rendezvous_environment \
      "$patched_receiver_xctestrun" \
      receiver \
      "$receiver_peer_key" \
      "$sender_peer_key" \
      "$rendezvous_run_id"
    set_rendezvous_environment \
      "$patched_sender_xctestrun" \
      sender \
      "$sender_peer_key" \
      "$receiver_peer_key" \
      "$rendezvous_run_id"
    receiver_run_xctestrun="$patched_receiver_xctestrun"
    sender_run_xctestrun="$patched_sender_xctestrun"
    ;;
  *WifiAwarePhysicalTransferTests/*)
    transfer_run_id="${run_label:0:48}"
    case "$test_identifier" in
      *WifiAwarePhysicalTransferTests/testRawUDPServicePath)
        completion_kind=raw_udp
        ;;
      *WifiAwarePhysicalTransferTests/testRawTransferServicePath)
        completion_kind=raw_transport
        ;;
      *WifiAwarePhysicalTransferTests/testNearbyHybridManifestV2Cancellation)
        completion_kind=manifest_cancellation
        ;;
      *WifiAwarePhysicalTransferTests/testManifestV2TransferServicePath | \
      *WifiAwarePhysicalTransferTests/testNearbyHybridManifestV2TransferServicePath | \
      *WifiAwarePhysicalTransferTests/testNearbyPreferredManifestV2TransferServicePath | \
      *WifiAwarePhysicalTransferTests/testNearbyPreferredAsymmetricBootstrapFailureFallsBackToIroh)
        completion_kind=manifest
        ;;
      *)
        echo "error: unsupported paired Wi-Fi Aware transfer test: $test_identifier" >&2
        exit 2
        ;;
    esac
    receiver_peer_hint="${ENVOIX_WIFI_AWARE_RECEIVER_PEER_HINT:-i}"
    sender_peer_hint="${ENVOIX_WIFI_AWARE_SENDER_PEER_HINT:-i}"
    if [[ -z "$receiver_peer_hint" || -z "$sender_peer_hint" ]]; then
      echo "error: Wi-Fi Aware peer hints must not be empty" >&2
      exit 2
    fi
    validate_optional_transfer_environment
    case "$test_identifier" in
      *WifiAwarePhysicalTransferTests/testNearbyHybridManifestV2Cancellation)
        if [[ "${ENVOIX_WIFI_AWARE_DROP_AT_PERCENT+x}" == x ]]; then
          echo "error: DROP_AT_PERCENT is not supported by the cancellation test" >&2
          exit 2
        fi
        if [[ "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" != x ]]; then
          echo "error: CANCEL_AT_PERCENT is required by the cancellation test" >&2
          exit 2
        fi
        ;;
      *WifiAwarePhysicalTransferTests/testNearbyHybridManifestV2TransferServicePath)
        if [[ "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" == x ]]; then
          echo "error: CANCEL_AT_PERCENT is supported only by the cancellation test" >&2
          exit 2
        fi
        ;;
      *)
        if [[ "${ENVOIX_WIFI_AWARE_DROP_AT_PERCENT+x}" == x ]]; then
          echo "error: DROP_AT_PERCENT is supported only by the Nearby Hybrid transfer test" >&2
          exit 2
        fi
        if [[ "${ENVOIX_WIFI_AWARE_CANCEL_AT_PERCENT+x}" == x ]]; then
          echo "error: CANCEL_AT_PERCENT is supported only by the cancellation test" >&2
          exit 2
        fi
        ;;
    esac
    pairing_token="$(
      printf 'pairing:%s' "$run_label" | /usr/bin/shasum -a 256 | cut -c 1-32
    )"
    cp "$receiver_xctestrun" "$patched_receiver_xctestrun"
    patched_receiver_created=1
    cp "$sender_xctestrun" "$patched_sender_xctestrun"
    patched_sender_created=1
    set_physical_transfer_environment \
      "$patched_receiver_xctestrun" \
      receive \
      "$receiver_peer_hint" \
      "$transfer_run_id" \
      "$pairing_token"
    set_physical_transfer_environment \
      "$patched_sender_xctestrun" \
      send \
      "$sender_peer_hint" \
      "$transfer_run_id" \
      "$pairing_token"
    forward_optional_transfer_environment \
      "$patched_receiver_xctestrun" \
      "$patched_sender_xctestrun"
    case "$test_identifier" in
      *testNearbyHybrid*|*testNearbyPreferred*)
        invitation_sidecar_filename="envoix-wfa-invite-$transfer_run_id"
        set_xctestrun_environment \
          "$patched_receiver_xctestrun" \
          "$invitation_sidecar_environment" \
          "$invitation_sidecar_filename"
        set_xctestrun_environment \
          "$patched_sender_xctestrun" \
          "$invitation_sidecar_environment" \
          "$invitation_sidecar_filename"
        ;;
    esac
    receiver_run_xctestrun="$patched_receiver_xctestrun"
    sender_run_xctestrun="$patched_sender_xctestrun"
    ;;
  *)
    echo "error: unsupported paired Wi-Fi Aware test: $test_identifier" >&2
    exit 2
    ;;
esac

/usr/bin/script -eqF "$receiver_log" xcodebuild \
  test-without-building \
  -xctestrun "$receiver_run_xctestrun" \
  -destination "$receiver_destination" \
  -derivedDataPath "$derived_data" \
  -parallel-testing-enabled NO \
  -collect-test-diagnostics never \
  -only-testing:"$test_identifier" \
  -resultBundlePath "$receiver_result" \
  >/dev/null 2>&1 &
receiver_pid="$!"

listener_deadline=$((SECONDS + receiver_start_timeout_seconds))
while ! grep -Eq \
  'receiver listener_state=(waiting|ready)|role=receiver listener_state=(waiting|ready)|role=receive .*receiver_waiting|udp receiver starting' \
  "$receiver_log" 2>/dev/null; do
  if ! kill -0 "$receiver_pid" >/dev/null 2>&1; then
    wait "$receiver_pid" || true
    echo "error: receiver exited before its listener became ready" >&2
    tail -n 80 "$receiver_log" >&2
    exit 1
  fi
  if (( SECONDS >= listener_deadline )); then
    echo "error: receiver listener did not start within ${receiver_start_timeout_seconds} seconds" >&2
    tail -n 80 "$receiver_log" >&2
    exit 1
  fi
  sleep 0.2
done

case "$test_identifier" in
  *testNearbyHybrid*|*testNearbyPreferred*)
    invitation_deadline=$((SECONDS + 30))
    while ! grep -Fq '[wifi-aware-physical] invitation_ready' "$receiver_log" 2>/dev/null; do
      if ! kill -0 "$receiver_pid" >/dev/null 2>&1; then
        wait "$receiver_pid" || true
        echo "error: receiver exited before exporting its InviteV2 sidecar" >&2
        tail -n 80 "$receiver_log" >&2
        exit 1
      fi
      if (( SECONDS >= invitation_deadline )); then
        echo "error: receiver did not export its InviteV2 sidecar within 30 seconds" >&2
        tail -n 80 "$receiver_log" >&2
        exit 1
      fi
      sleep 0.2
    done
    receiver_device_id="$(device_id_from_destination "$receiver_destination")"
    sender_device_id="$(device_id_from_destination "$sender_destination")"
    invitation_sidechannel_directory="$(
      mktemp -d /private/tmp/envoix-wfa-invite.XXXXXX
    )"
    chmod 700 "$invitation_sidechannel_directory"
    invitation_sidechannel_file="$invitation_sidechannel_directory/$invitation_sidecar_filename"
    xcrun devicectl device copy from \
      --quiet \
      --timeout 20 \
      --device "$receiver_device_id" \
      --domain-type appDataContainer \
      --domain-identifier "$app_bundle_identifier" \
      --source "Documents/$invitation_sidecar_filename" \
      --destination "$invitation_sidechannel_file"
    if [[ ! -f "$invitation_sidechannel_file" ]]; then
      echo "error: receiver InviteV2 sidecar was not copied" >&2
      exit 1
    fi
    chmod 600 "$invitation_sidechannel_file"
    invitation_sidechannel_size="$(wc -c < "$invitation_sidechannel_file")"
    if (( invitation_sidechannel_size < 1 || invitation_sidechannel_size > 16384 )) \
      || ! LC_ALL=C grep -Eq '^[A-Za-z0-9_-]+$' "$invitation_sidechannel_file"; then
      echo "error: receiver exported an invalid InviteV2 sidecar" >&2
      exit 1
    fi
    xcrun devicectl device copy to \
      --quiet \
      --timeout 20 \
      --device "$sender_device_id" \
      --domain-type appDataContainer \
      --domain-identifier "$app_bundle_identifier" \
      --source "$invitation_sidechannel_file" \
      --destination "Documents/$invitation_sidecar_filename"
    cleanup_invitation_sidechannel
    ;;
esac

/usr/bin/script -eqF "$sender_log" xcodebuild \
  test-without-building \
  -xctestrun "$sender_run_xctestrun" \
  -destination "$sender_destination" \
  -derivedDataPath "$derived_data" \
  -parallel-testing-enabled NO \
  -collect-test-diagnostics never \
  -only-testing:"$test_identifier" \
  -resultBundlePath "$sender_result" \
  >/dev/null 2>&1 &
sender_pid="$!"

pair_watchdog_seconds="$default_pair_watchdog_seconds"
if [[ "${ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS+x}" == x ]]; then
  pair_watchdog_seconds=$((
    10#$ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS + pair_watchdog_grace_seconds
  ))
fi
pair_deadline=$((SECONDS + pair_watchdog_seconds))
sender_finished=0
receiver_finished=0
pair_timed_out=0
set +e
while [[ "$sender_finished" -eq 0 || "$receiver_finished" -eq 0 ]]; do
  if [[ "$sender_finished" -eq 0 ]] && ! kill -0 "$sender_pid" >/dev/null 2>&1; then
    wait "$sender_pid"
    sender_status="$?"
    sender_finished=1
    if [[ "$sender_status" -ne 0 && "$receiver_finished" -eq 0 ]]; then
      terminate_test_process "$receiver_pid"
    fi
  fi
  if [[ "$receiver_finished" -eq 0 ]] && ! kill -0 "$receiver_pid" >/dev/null 2>&1; then
    wait "$receiver_pid"
    receiver_status="$?"
    receiver_finished=1
  fi
  if (( SECONDS >= pair_deadline )); then
    pair_timed_out=1
    [[ "$sender_finished" -eq 1 ]] || terminate_test_process "$sender_pid"
    [[ "$receiver_finished" -eq 1 ]] || terminate_test_process "$receiver_pid"
    sleep 1
    [[ "$sender_finished" -eq 1 ]] || terminate_test_process "$sender_pid" KILL
    [[ "$receiver_finished" -eq 1 ]] || terminate_test_process "$receiver_pid" KILL
    break
  fi
  sleep 0.2
done
if [[ "$sender_finished" -eq 0 ]]; then
  wait "$sender_pid"
  sender_status="$?"
fi
if [[ "$receiver_finished" -eq 0 ]]; then
  wait "$receiver_pid"
  receiver_status="$?"
fi
if [[ "$pair_timed_out" -eq 1 ]]; then
  sender_status=124
  receiver_status=124
  echo "error: paired test exceeded its ${pair_watchdog_seconds}-second wall-clock limit" >&2
fi
set -e
sender_pid=""
receiver_pid=""
if [[ "$patched_receiver_created" -eq 1 ]]; then
  rm -f "$patched_receiver_xctestrun"
fi
if [[ "$patched_sender_created" -eq 1 ]]; then
  rm -f "$patched_sender_xctestrun"
fi
trap - EXIT INT TERM

semantic_failure=0
if log_contains_skipped_test "$receiver_log"; then
  echo "error: receiver test was skipped" >&2
  semantic_failure=1
fi
if log_contains_skipped_test "$sender_log"; then
  echo "error: sender test was skipped" >&2
  semantic_failure=1
fi

case "$completion_kind" in
  rendezvous)
    require_fixed_marker \
      "$receiver_log" \
      receiver \
      "local_role=receiver phase=outbound invite_acknowledged sequence=$rendezvous_invitation_count count=$rendezvous_invitation_count run=$rendezvous_run_id"
    require_fixed_marker \
      "$receiver_log" \
      receiver \
      "local_role=receiver phase=inbound rendezvous_offer_received sequence=$rendezvous_invitation_count count=$rendezvous_invitation_count run=$rendezvous_run_id"
    require_fixed_marker \
      "$sender_log" \
      sender \
      "local_role=sender phase=outbound invite_acknowledged sequence=$rendezvous_invitation_count count=$rendezvous_invitation_count run=$rendezvous_run_id"
    require_fixed_marker \
      "$sender_log" \
      sender \
      "local_role=sender phase=inbound rendezvous_offer_received sequence=$rendezvous_invitation_count count=$rendezvous_invitation_count run=$rendezvous_run_id"
    ;;
  raw_udp)
    require_fixed_marker \
      "$receiver_log" \
      receiver \
      "[wifi-aware-physical] udp receiver completed run=$transfer_run_id bytes="
    require_fixed_marker \
      "$sender_log" \
      sender \
      "[wifi-aware-physical] udp sender completed run=$transfer_run_id bytes="
    ;;
  raw_transport)
    require_fixed_marker \
      "$receiver_log" \
      receiver \
      "[wifi-aware-physical] raw receiver completed run=$transfer_run_id bytes="
    require_fixed_marker \
      "$sender_log" \
      sender \
      "[wifi-aware-physical] raw sender completed run=$transfer_run_id bytes="
    ;;
  manifest)
    require_fixed_marker \
      "$receiver_log" \
      receiver \
      "[wifi-aware-physical] manifest receiver saved run=$transfer_run_id bytes="
    require_fixed_marker \
      "$sender_log" \
      sender \
      "[wifi-aware-physical] manifest sender completed run=$transfer_run_id bytes="
    ;;
  manifest_cancellation)
    require_regex_marker \
      "$receiver_log" \
      receiver \
      "\\[wifi-aware-benchmark\\] run=$transfer_run_id role=receive .* expected_cancellation_observed error="
    require_regex_marker \
      "$sender_log" \
      sender \
      "\\[wifi-aware-benchmark\\] run=$transfer_run_id role=send .* expected_cancellation_observed error="
    ;;
esac

if [[ "$receiver_status" -ne 0 || "$sender_status" -ne 0 ]] \
  || ! grep -Eq "Executed 1 test, with 0 failures" "$receiver_log" \
  || ! grep -Eq "Executed 1 test, with 0 failures" "$sender_log" \
  || [[ "$semantic_failure" -ne 0 ]]; then
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
