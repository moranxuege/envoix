# End-to-end matrix contract

`cases.v1.json` is the source of truth for the product test matrix described by
issue #61. Every case is explicit. The runner must not generate an unreviewed
Cartesian product.

Validate the registry:

```bash
python3 scripts/matrix_contract.py validate-registry \
  tests/e2e/matrix/cases.v1.json
```

List its cases:

```bash
python3 scripts/matrix_contract.py list-cases \
  tests/e2e/matrix/cases.v1.json
```

Render the registry-only report:

```bash
python3 scripts/matrix_contract.py render-report \
  tests/e2e/matrix/cases.v1.json
```

Use the registry-backed runner:

```bash
scripts/cross-device-transfer-matrix.sh --list
scripts/cross-device-transfer-matrix.sh --validate
scripts/cross-device-transfer-matrix.sh --dry-run --case \
  l1.physical.room.ios-android.single-file
scripts/cross-device-transfer-matrix.sh --gate current-physical-harness \
  --commit "$(git rev-parse HEAD)" \
  --run-id physical-20260728 \
  --output-directory /path/to/test-owned-output \
  --android-device SERIAL \
  --ios-destination "platform=iOS,id=DEVICE_ID"
```

`--case` may be repeated. `--case`, `--gate`, and `--tag` are mutually
exclusive selection modes, and every selected row comes directly from the
registry. With no selector, the runner selects `current-physical-harness`.
Legacy scenario and direction environment variables are mapped to registered
rows for one migration period; combinations without a row are warned and
never synthesized.

Each run writes:

```text
matrix-plan.json
matrix-result.json
matrix-report.md
cases/<case-id>/r<repetition>/result.json
cases/<case-id>/r<repetition>/<evidence-enabled-role>.json
sanitized/cases/<case-id>/r<repetition>/*.log
private/
```

An Android endpoint writes its bounded result under the app's test-owned
`files/envoix-matrix/` directory. The runner retrieves and validates that JSON,
retains it under the matching sender or receiver artifact name, and removes
only the exact test-owned files. The current Android driver is labeled
`l1_native` / `direct_jni`; it does not satisfy the product-path L2 Activity
contract.

An iOS sender receives the receiver-generated Invite V2 payload through a
test-only sidecar after its hosted test reports readiness. The xctestrun file
contains only a bounded sidecar filename; the payload remains in the private
runner directory and the iOS app-data container, and both copies are removed
after loading. This out-of-band bootstrap is matrix infrastructure, not a
product discovery or transfer path.

`private/` is mode `0700`, is never an uploadable artifact, and can retain raw
logs only for local failure triage. Successful case logs are removed after
sanitized copies are created. The runner scans every retained public file for
Room Codes, Invite V2 URIs, device-serial canaries, private absolute paths, and
network addresses before the run can succeed.

Dry-runs build and execute nothing. They produce the same plan and report
shape as a physical run, but executable rows are recorded as `not_run`; a
dry-run can never produce `pass`.

Support status and execution status are separate. A planned, experimental,
hardware-blocked, or unsupported row never becomes a pass because it was
skipped or omitted.

The registry records the direct JNI/FFI physical coverage as experimental L1
evidence. The single-file and multi-file L2 baselines use each app's product
Activity and native file publication path and are experimental until their
mandatory physical repetitions are recorded. Multi-root L2 rows remain
planned until the Android driver provisions and verifies a real SAF folder.

`--build-variant release_equivalent` selects the Android `release` application
and release-targeted instrumentation APK, plus the Apple `Release` app and
hosted-test products. Debug and Release products use separate build-cache
directories. Reusing debug artifacts while labeling a run
`release_equivalent` is not supported.

Registry and report data must never contain Room Codes, invitation payloads,
tokens, credentials, stable device identifiers, or absolute private paths.
