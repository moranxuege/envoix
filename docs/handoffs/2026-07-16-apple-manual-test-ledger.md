# Apple product manual-test ledger — 2026-07-16

This ledger records the physical-device observations supplied during the live
test campaign. Entries separate reported facts from later interpretation. Do
not treat a non-terminal diagnostic snapshot as a transfer failure.

## Test 01 — iOS in-app Photos, one video, iOS to macOS

### Test input and route

- User action: open Photos from inside Envoix and select one video.
- Source: iOS Envoix main-app Photos picker.
- Direction: iOS send -> macOS receive.
- Pairing mode: Room.
- Selection shape: one regular file; no multi-file selection.
- File: `IMG_8362.mov`.
- Reported size: `371,926,650` bytes.
- User observation: transfer felt very fast and nearly comparable with AirDrop.
- Comparison observation: AirDrop displayed a wired connection; Envoix does not
  currently support a wired transport path.

### iOS sender report

- Record ID: `E9F8342A-327F-427E-A783-7887D4189DFE`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:38:23Z` (`21:38:23` local).
- Pairing joined: `13:38:23Z`.
- Peer matched: `13:38:29Z`.
- Keys exchanged: `13:38:30Z`.
- Connected and started: `13:38:31Z`.
- Completed: `13:38:43Z`.
- Final state: `completed`.
- Transfer ID:
  `transfer-407d87bf21a6d5ed2977a6f1ce741a4daaf86b5ab64e1481181b69c5229b7417`.
- Final bytes: `371926650/371926650`; resumed bytes: `0`.
- Initial reported direct path: `172.20.10.7:51407`.
- Later reported direct path:
  `[240a:42a3:fe00:17e8:142f:2ef3:8b2e:673c]:61832`.
- Payload interval at report precision: approximately 12 seconds.
- UI throughput samples began around `34 MB/s` and settled around
  `28 MB/s`; the final displayed value was `28.2 MB/s`.
- Final sender sequence: full progress -> `verifying`/`confirming` ->
  `completed`.

### macOS receiver report captured after sender completion

- Record ID: `A219CE38-D034-4BAA-ACEC-EB63DFE27E70`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:38:25Z` (`21:38:25` local).
- Pairing joined: `13:38:28Z`.
- Peer matched: `13:38:29Z`.
- Keys exchanged: `13:38:30Z`.
- Direct connection reported: `13:38:31Z`.
- Report generated: `13:38:49Z`.
- Snapshot `updated_at`: `13:38:31Z`.
- Snapshot state: `connecting`; direction: `receive`; bytes: `0/0`.
- Reported direct paths included `172.20.10.1:53737` and
  `[240a:42a3:fe00:17e8:820:a788:47e5:7d5b]:59512`.
- The report contained a `[failure]` section with default `unknown` metadata
  and `diagnostic_message=accept stream opened`, while the Activity itself was
  not in the terminal `failed` state.
- The supplied macOS snapshot therefore does not independently establish the
  receiver's terminal state, final byte count, or completed file path.

### Classification pending cross-run analysis

- Sender terminal result: completed.
- Receiver terminal result in supplied report: not captured.
- Data path reported by both sides: Direct.
- Multi-file/Manifest behavior: not exercised by this test input.

## Test 02 — repeat the same single video with the received file retained

### Test input and expected behavior

- User action: repeat the transfer without deleting the file received by Test
  01.
- Direction: iOS send -> macOS receive.
- Pairing mode requested initially: Room.
- Selection shape: one regular file.
- File: `IMG_8362.mov`.
- Reported size: `371,926,650` bytes.
- User expectation: an identical completed destination file should be detected
  and the payload should not be transferred again.
- User observation: the resulting lifecycle still contains a bug.

### macOS receiver report

- Record ID: `A95D2C71-994F-41C6-8617-3210B3FC0AC9`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:42:54Z` (`21:42:54` local).
- Pairing joined and peer matched: `13:42:57Z`.
- Keys exchanged, connected: `13:42:59Z`.
- Direct paths included `172.20.10.1:54375` and
  `[240a:429f:fe10:6906:8d0:68b:ff0d:8461]:60561`.
- Failed: `13:43:37Z`.
- Final state: `failed`; direction: `receive`; mode: `room`.
- Final bytes: `0/0`; no transfer ID or file name was projected into this
  receiver Activity.
- Failure: `internalError / internal / transferring / unknown`.
- Diagnostic: `I/O error during transfer: connection lost`.
- Retryable: `false`; recovery action: `none`.

### iOS sender report

- Record ID: `1D512ADD-B989-4ED1-BB36-C7882E626376`.
- Final attempt shown: `attempt-2`.
- Created: `2026-07-16T13:42:52Z` (`21:42:52` local).
- Initial Room pairing matched at `13:42:57Z`, exchanged keys and connected at
  `13:42:59Z`.
- Direct paths included `172.20.10.7:53117` and
  `[240a:42a3:fe00:17e8:7c91:4bfa:eb75:fab4]:64614`.
- `verifying` began at `13:43:04Z`.
- `verified` reported `371926650/371926650` at `13:43:09Z`.
- A `started` event followed at `13:43:09Z`, with the same file and total size,
  immediately followed by `confirming`; the supplied event stream contains no
  ordinary payload progress sequence for this attempt.
- `delivery unconfirmed` was reported at `13:43:29Z`.
- A new Room binding/join began at `13:43:40Z`.
- An mDNS binding was reported at `13:44:09Z`.
- Final snapshot state: `paused`; final mode field: `mdns`.
- Final bytes: `371926650/371926650`.
- Final resumed bytes: `371926650`, exactly equal to the file size.
- Transfer ID:
  `transfer-460c8100af2985fe5a6285eea1d86289e5a83fa0082059c7901342cd13afe467`.
- Final failure metadata: `userCanceled / user / transferring / local`, key
  `transfer.paused`, retryable with recovery action `resume`.
- Diagnostic: `transfer error: transfer paused by user`.

### Classification pending cross-run analysis

- The full file length was recorded as resumed rather than as a normal payload
  progress sequence.
- The run did not reach a mutually reported terminal completion: macOS reported
  connection loss, while iOS moved through delivery-unconfirmed and retry
  attempts before ending paused.
- Whether the destination file was byte-for-byte reused is not independently
  proven by the macOS Activity report because that report did not expose a file
  name, transfer ID, byte count, or terminal completion.
- The observed defect is retained as an open lifecycle/confirmation issue; its
  cause is intentionally deferred until cross-run analysis.

## Test 03 — iOS in-app Photos, two videos, iOS to macOS

### Test input and route

- User action: open Photos from inside the iOS Envoix app and select two
  videos.
- Direction: iOS send -> macOS receive.
- Pairing mode: Room.
- Selection shape: two regular files.
- Reported aggregate size: `800,127,969` bytes.
- Multi-file behavior was exercised in this test.

### macOS receiver report

- Record ID: `D54B0799-06D5-4CBD-A6C9-4DB20BFF0D6C`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:45:39Z` (`21:45:39` local).
- Pairing joined: `13:45:42Z`.
- Peer matched: `13:45:52Z`.
- Keys exchanged and connecting: `13:45:53Z`.
- Direct paths included `172.20.10.1:60835` and
  `[240a:42a3:fe00:17e8:cbd:42f8:19b3:5e70]:53248`.
- First zero-byte payload progress: `13:46:04Z`.
- Final payload progress and completion: `13:46:31Z`.
- Final state: `completed`; direction: `receive`; mode: `room`.
- Transfer ID: `B8419FAC-54C8-4992-AE13-3367B0ECC6F9`.
- Display name: `2 items`.
- Final bytes: `800127969/800127969`; resumed bytes: `0`.
- Completed path: `/Users/moranxuege/Downloads`.
- The report's `[transfer_events]` section was empty.

### iOS sender report

- Record ID and transfer ID: `B8419FAC-54C8-4992-AE13-3367B0ECC6F9`.
- Attempt: `attempt-1`.
- Created, preparing, and Room pairing joined:
  `2026-07-16T13:45:51Z` (`21:45:51` local).
- Peer matched: `13:45:52Z`.
- Keys exchanged and connecting: `13:45:53Z`.
- Direct paths included `172.20.10.7:59769` and
  `[240a:42a3:fe00:17e8:7c91:4bfa:eb75:fab4]:51636`.
- First zero-byte payload progress: `13:46:04Z`.
- Completion: `13:46:31Z`.
- Final state: `completed`; direction: `send`; mode: `room`.
- Display name: `2 items`.
- Final bytes: `800127969/800127969`; resumed bytes: `0`.
- The report's `[transfer_events]` section was empty.

### Timing facts retained for cross-run analysis

- iOS creation/pairing join -> peer match: approximately 1 second.
- Keys exchanged/connection -> first progress: approximately 11 seconds.
- First zero-byte progress -> completion: approximately 27 seconds.
- Aggregate payload divided by the report-level 27-second interval is about
  `29.6 MB/s` in decimal units; this is a coarse value because timestamps have
  one-second resolution.
- Both endpoints independently reported the same byte total and terminal
  completion time over Direct paths.

## Test 04 — repeat the same two videos with both destination files retained

### Test input and expected behavior

- User action: repeat Test 03 without deleting either previously received
  destination file.
- Direction: iOS send -> macOS receive.
- Pairing mode: Room.
- Selection shape: the same two regular files.
- Previously reported aggregate size: `800,127,969` bytes.
- Expected behavior: recognize both existing identical files and avoid sending
  their payload again.

### Supplied-report duplication

- Two report blocks were supplied, but both are the same macOS report.
- Both blocks have `app=envoix-macos`, record ID
  `8B37952C-EA92-4651-82D9-354BB83894D8`, generation time
  `2026-07-16T13:53:14Z`, and identical contents throughout.
- Their identical `1202`-character counts therefore do not independently
  indicate a transfer or file-copy defect.
- No iOS sender report was supplied for this test at the time of recording.
- It remains undetermined whether the duplicate arose from repeating the same
  copy action or from the UI selecting the same Activity/report twice.

### macOS receiver report

- Record ID: `8B37952C-EA92-4651-82D9-354BB83894D8`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:51:28Z` (`21:51:28` local).
- Pairing joined: `13:51:31Z`.
- Peer matched: `13:51:42Z`.
- Keys exchanged and connecting: `13:51:43Z`.
- Direct IPv6 path first reported at `13:51:43Z`, then changed at
  `13:51:44Z`; final detail was
  `[240a:42a3:fe00:17e8:cbd:42f8:19b3:5e70]:58276`.
- A zero-byte progress event and completion were both logged at
  `13:52:06Z`.
- `started_at` and `completed_at` are both `13:52:06Z`.
- Final state: `completed`; direction: `receive`; mode: `room`.
- Transfer ID: `239CA7FC-9292-4A52-965C-5F1B33E01BD5`.
- Display name: `2 items`.
- Final logical bytes: `800127969/800127969`.
- Reported resumed bytes: `0`.
- Completed path: `/Users/moranxuege/Downloads`.
- The report's `[transfer_events]` section was empty.

### Classification pending cross-run analysis

- The supplied receiver timeline contains no non-zero payload progress and
  completes at the same timestamp as its first zero-byte progress event.
- The aggregate Activity nevertheless reports all logical bytes completed and
  reports zero resumed bytes.
- The macOS receiver reached terminal completion.
- The logical-byte/resumed-byte presentation is retained as an open accounting
  question for cross-run analysis.

### iOS sender record recovered directly from the physical device

The user retained the Activity after the in-app copy action failed. The latest
persisted Manifest JSON was read without modifying the phone from the Envoix
app data container. The source record remained on the phone; the temporary
local read-only copy was `/private/tmp/envoix-test04-ios-record.json`.

- Persisted record file:
  `manifest-record-6356642396141090283.json`.
- External Activity ID, Manifest ID, and transfer ID:
  `239CA7FC-9292-4A52-965C-5F1B33E01BD5`.
- Protocol: `manifest_v1`.
- Attempt: `1`.
- Direction: `Send`.
- Created: `2026-07-16T13:51:41.502Z`.
- Updated: `2026-07-16T13:52:06.165Z`.
- Final state: `completed`.
- Final logical bytes: `800127969/800127969`.
- Recorded resumed bytes: `0`.
- Completed files: `2`; current entry: none.
- Final Direct path:
  `[240a:42a3:fe00:17e8:7c91:4bfa:eb75:fab4]:53555`.
- Session facts included `proof_delivered=false` and
  `receipt_mismatch=false`.
- Entry 0: `ScreenRecording_07-05-2026 12-49-14_1.mp4`,
  `428,201,319` bytes, result `skipped_identical`.
- Entry 1: `IMG_8362.mov`, `371,926,650` bytes, result
  `skipped_identical`.

The recovered sender record therefore confirms that both entries were
classified as identical and skipped. The aggregate `bytes` field represents
logical completion in this case; the record does not count skipped-identical
bytes in `bytes_resumed`.

### User-visible multi-file preparation observation

- After Test 04, the user reported that sending multiple files still has a
  long preparation delay.
- Test 03 independently records an approximately 11-second interval from key
  exchange/connection to the first zero-byte payload progress event.
- Test 04 records an approximately 23-second interval from key
  exchange/connection to skip-identical completion, despite sending no file
  payload.
- These report-visible intervals do not include any Photos provider export,
  staging, or initial Manifest preparation that occurred before the durable
  sender record was created.
- The preparation delay is retained as a distinct multi-file performance issue,
  including the existing-file/skip-identical path.

## Test 05 — two retained videos plus one new video

### Test input and route

- User action: select the two videos already present in Downloads and add one
  new video in the iOS Envoix main-app Photos picker.
- Direction: iOS send -> macOS receive.
- Pairing mode: Room.
- Selection shape: three regular files.
- Aggregate logical size: `1,290,110,310` bytes.
- User observations: speed presentation was ambiguous; preparation remained
  abnormally long; other visible behavior appeared normal.

### iOS sender report

- Record ID and transfer ID: `1755CA96-95D7-4067-BB4E-91F11E62E4A2`.
- Attempt: `attempt-1`.
- Created and Room pairing joined: `2026-07-16T13:59:20Z`
  (`21:59:20` local).
- Peer matched: `13:59:20Z`.
- Keys exchanged and connecting: `13:59:21Z`.
- Direct path landmarks occurred at `13:59:21Z`, `13:59:22Z`,
  `13:59:35Z`, and `13:59:45Z`; the final report detail was
  `[240a:42a3:fe00:17e8:142f:2ef3:8b2e:673c]:64528`.
- First progress events: `13:59:50Z`, first zero and then approximately
  `800.2 MB` within the same report second.
- Completion: `14:00:08Z`.
- Final state: `completed`; direction: `send`; mode: `room`.
- Final logical bytes: `1290110310/1290110310`; resumed bytes: `0`.
- The report's `[transfer_events]` section was empty.

### macOS receiver report

- Record ID: `B9D0628C-772D-4FD8-B11F-6373A08E3232`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T13:59:04Z` (`21:59:04` local).
- Pairing joined: `13:59:07Z`.
- Peer matched: `13:59:20Z`.
- Keys exchanged and connecting: `13:59:21Z`.
- Matching Direct path landmarks occurred at `13:59:21Z`, `13:59:35Z`,
  and `13:59:45Z`; final report detail was
  `[240a:42a3:fe00:17e8:820:a788:47e5:7d5b]:57702`.
- First progress events: `13:59:50Z`, first zero and then approximately
  `800.1 MB` within the same report second.
- Completion: `14:00:08Z`.
- Final state: `completed`; direction: `receive`; mode: `room`.
- Final logical bytes: `1290110310/1290110310`; resumed bytes: `0`.
- Completed path: `/Users/moranxuege/Downloads`.
- The report's `[transfer_events]` section was empty.

### iOS sender record recovered directly from the physical device

The persisted Manifest record was read without modifying the phone and copied
temporarily to `/private/tmp/envoix-test05-ios-record.json`.

- Persisted record file:
  `manifest-record-5251073700528190464.json`.
- Protocol: `manifest_v1`; attempt: `1`; final state: `completed`.
- Created: `2026-07-16T13:59:20.023Z`.
- Updated: `2026-07-16T14:00:08.958Z`.
- Entry 0: `IMG_8362.mov`, `371,926,650` bytes,
  `skipped_identical`.
- Entry 1: `ScreenRecording_07-05-2026 12-49-14_1.mp4`,
  `428,201,319` bytes, `skipped_identical`.
- Entry 2: `studio_video_1781963291579352.mp4`, `489,982,341` bytes,
  `completed`.
- Completed files: `3`; current entry: none.
- Session facts included `proof_delivered=false` and
  `receipt_mismatch=false`.

### Timing and accounting facts retained for cross-run analysis

- The two skipped-identical files total exactly `800,127,969` bytes, matching
  the immediate approximately 800.1/800.2 MB logical-progress jump.
- Only `489,982,341` bytes belonged to the newly transferred file.
- Keys exchanged/connection -> first progress: approximately 29 seconds.
- First logical progress -> completion: approximately 18 seconds.
- New payload divided by that coarse 18-second interval is approximately
  `27.2 MB/s` in decimal units.
- Dividing the full logical total by the same interval would yield an invalid
  network-throughput estimate of approximately `71.7 MB/s` because it includes
  the two skipped files.
- `bytes_resumed=0` does not expose the `800,127,969` skipped-identical bytes.
  The progress/speed presentation therefore combines logical completion with
  physical payload accounting and is retained as a confirmed reporting issue.

## Test 06 — macOS to iOS, one small image

### Test input and route

- Direction: macOS send -> iOS receive.
- Pairing mode: Room.
- Selection shape: one regular file.
- File: `扫描全能王 2026-07-16 19.35_1.jpg`.
- Size: `913,274` bytes.
- This test exercised the single-file path rather than a multi-file Manifest
  send path.

### macOS sender report

- Record ID: `2BD136CB-34E8-47DE-83BA-F6BEBB3F013D`.
- Attempt: `attempt-1`.
- Created and Room pairing joined: `2026-07-16T14:01:52Z`
  (`22:01:52` local).
- Peer matched: `14:01:57Z`.
- Keys exchanged, connected, started, and completed: `14:01:59Z`.
- Direct path:
  `[240a:42a3:fe00:17e8:820:a788:47e5:7d5b]:60840`.
- Transfer ID:
  `transfer-f1daf661c0d74cfd7de7443e137325aa0458d41f17afabc2c014ac0717f1acc0`.
- Final state: `completed`; final bytes: `913274/913274`; resumed
  bytes: `0`.
- The event stream includes `started`, progress from `65,536` to `913,274`,
  `confirming`, and `completed`, all within report timestamp second
  `14:01:59Z`.

### iOS receiver report

- Record ID: `0D00216A-D801-48AC-A0C8-394AF6078743`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T14:01:36Z` (`22:01:36` local).
- Pairing joined and peer matched: `14:01:57Z`.
- Keys exchanged and connecting: `14:01:59Z`.
- Direct path landmarks occurred at `14:01:59Z` and `14:02:00Z`; final
  detail was `[240a:42a3:fe00:17e8:142f:2ef3:8b2e:673c]:57529`.
- Completed: `14:02:00Z`.
- Final state: `completed`; final bytes: `913274/913274`; resumed
  bytes: `0`.
- Transfer ID matches the macOS sender.
- Completed path:
  `/var/mobile/Containers/Data/Application/60CC60E0-B270-4F21-AFED-9E2321945ED2/Documents/Downloads`.
- The receiver report has `started_at=0` despite terminal completion and an
  exact completed byte count.
- The report's `[transfer_events]` section was empty.

### Sender-scanner role handling observation

- During this test, the user deliberately opened the scanner from the Send
  side/context.
- On encountering the incompatible peer role, the app displayed a one-line
  prompt instead of adapting or transitioning the workflow to Receive.
- The exact prompt text and the encoded QR payload were not captured, so this
  ledger does not infer which concrete role-mismatch branch produced it.
- User expectation/design suggestion: the scanner should understand the peer's
  role and provide a smoother transition to the compatible local role instead
  of stopping at a passive text hint.
- This is retained as a direction/role UX optimization issue, separate from
  the successful payload transfer.

### Classification pending cross-run analysis

- Both endpoints independently reported terminal completion with the same
  transfer ID, file name, and byte total over Direct paths.
- The payload was small enough to start and complete within one-second report
  precision.
- Receiver `started_at=0` is retained as a diagnostic Activity projection
  inconsistency.

## Test 07 — macOS multi-file send to iOS

### Test input and route

- User action: import/select multiple files on macOS and send them to iOS.
- Direction: macOS send -> iOS receive.
- Pairing mode: Room.
- Selection shape: two items.
- Aggregate size: `861,908,991` bytes.
- User observation: the macOS multi-file import/preparation phase remained
  noticeably long.

### iOS receiver report

- Record ID: `F97FCC5C-5DA0-487F-B051-67987D1AAC67`.
- Attempt: `attempt-1`.
- Created: `2026-07-16T14:11:04Z` (`22:11:04` local).
- Pairing joined: `14:11:05Z`.
- Peer matched: `14:11:34Z`.
- Keys exchanged and connecting: `14:11:35Z`.
- Direct connection: `14:11:38Z`, final detail
  `[240a:42a3:fe00:17e8:7c91:4bfa:eb75:fab4]:51374`.
- First zero-byte payload progress: `14:11:50Z`.
- Completion: `14:12:17Z`.
- Final state: `completed`; direction: `receive`; mode: `room`.
- Transfer ID: `CB2D0019-A3EE-4F4A-BF1C-39207013DE23`.
- Display name: `2 items`.
- Final bytes: `861908991/861908991`; resumed bytes: `0`.
- Completed path:
  `/var/mobile/Containers/Data/Application/60CC60E0-B270-4F21-AFED-9E2321945ED2/Documents/Downloads`.
- The report's `[transfer_events]` section was empty.

### macOS sender report

- Record ID, Activity ID, and transfer ID:
  `CB2D0019-A3EE-4F4A-BF1C-39207013DE23`.
- Attempt: `attempt-1`.
- Created, preparing, and Room pairing joined:
  `2026-07-16T14:11:33Z` (`22:11:33` local).
- Peer matched: `14:11:34Z`.
- Keys exchanged, connecting, and first Direct path: `14:11:35Z`.
- Direct path changed at `14:11:38Z`; final detail was
  `[240a:42a3:fe00:17e8:820:a788:47e5:7d5b]:63694`.
- First zero-byte payload progress: `14:11:50Z`.
- Completion: `14:12:17Z`.
- Final state: `completed`; direction: `send`; mode: `room`.
- Display name: `2 items`.
- Final bytes: `861908991/861908991`; resumed bytes: `0`.
- The report's `[transfer_events]` section was empty.

### Timing facts retained for cross-run analysis

- iOS receiver pairing join -> peer match: approximately 29 seconds. This
  overlaps the period before the macOS durable sender Activity was created,
  but the reports cannot isolate user interaction from import/Manifest work.
- macOS sender Activity creation -> peer match: approximately 1 second.
- Keys exchanged/initial Direct connection -> first progress: approximately
  15 seconds; final Direct path change -> first progress: approximately 12
  seconds.
- First progress -> completion: approximately 27 seconds.
- Aggregate payload divided by the coarse 27-second interval is approximately
  `31.9 MB/s` in decimal units.
- Both endpoints reported the same total, transfer ID, completion time, and
  Direct transport path.
- Because this delay occurred on the macOS ordinary multi-file selection path,
  it is retained as evidence that multi-file preparation cost is not exclusive
  to the iOS Photos/App Group staging path.

## Post-test macOS setup and action observations

- The user observed that the macOS Transfer page continues to present the most
  recent terminal Activity instead of returning to a clean transfer setup.
- The existing “Send Again” and “Receive Again” actions were judged ambiguous:
  the UI does not explain whether they resume the same transfer, create a new
  Activity, retain the previous Room, reuse source items, or reuse the previous
  destination.
- Source inspection confirms that terminal `transferActivity` remains bound to
  each `TransferViewModel`; the setup page treats every non-idle phase as a
  reason to expose recent Activity, and the two labels are selected generically
  for completed, canceled, and failed states.
- These observations are tracked as setup/history ownership and action-semantics
  issues, not as transfer payload failures.

## Remediation implementation note — 2026-07-16

This note records later implementation status without changing the original
physical observations above. In the current remediation worktree based on
`f74852e`:

- the false `[failure]` section, platform report header, weak build identity,
  untrustworthy pasteboard-success toast, and empty Manifest diagnostic-event
  stream have code fixes and hosted-test coverage;
- the connection-time redundant full source BLAKE3 pass has been removed;
  source identity is still enforced by cheap preflight facts and the
  authoritative stream-time hash;
- negotiated single-file receive lifecycle and `started_at` projection have
  code fixes, and the completed file is no longer reopened solely to synthesize
  its compatibility Manifest;
- retained single-file completion now has dual-ALPN and Room regressions that
  pass on the current source, but Test 02's physical failure has not yet been
  reproduced or closed by a device rerun;
- the FFI change is additive: existing V1 Manifest observer functions remain,
  Apple opts into V2 diagnostic events, and the Android Debug build succeeds.

The following observations remain open: destination permission timing, initial
Photos/App Group and multi-file preparation, skipped-versus-payload speed
accounting, terminal Activity leakage into Transfer setup, ambiguous repeat
actions, QR role switching, and wired-path investigation.

The next physical reports should show `core_ffi_api=3`,
`manifest_diagnostic_events_v1`, an endpoint-specific executable fingerprint,
non-empty Manifest transfer events, and no diagnostic-only `[failure]` block.
These fields are the installation/synchronization gate before comparing timing
against Tests 01–07.

## Test 08 — post-fix retained single video, iOS to macOS

### Build identity and input

- Both reports expose `core_ffi_api=3` and capability
  `manifest_diagnostic_events_v1`.
- macOS executable fingerprint: `1572af7135c3fb4f90133505`.
- iOS executable fingerprint: `af28dfc57df85df2c984cf0b`.
- Input: `IMG_8362.mov`, `371,926,650` bytes.
- The existing destination file was retained in macOS Downloads.
- Direction: iOS send -> macOS receive; requested pairing mode: Room.

### Shared terminal evidence

- Sender Activity: `2445B13F-8CCB-4C06-8540-ECDDE14648C5`.
- Receiver Activity: `32BCC3EA-C4A8-4086-B31B-368558E6DAD6`.
- Both sides reported transfer ID
  `transfer-1161aca0935a4189f31e58ea143795b586f37a0bb12bc71d47135b18ff3aaf29`.
- Both sides reached `completed` over a Direct path.
- The iOS sender reported
  `bytes=371926650/371926650` and
  `resumed_bytes=371926650`, proving that no ordinary payload was required.
- The macOS receiver reported the exact completed byte total and Downloads as
  its completed root, but retained `resumed_bytes=0`.
- Neither report contains a false `[failure]` section, and both reports contain
  populated structured `[transfer_events]`.
- The receiver now has a non-zero `started_at`, file name, transfer ID, byte
  total, and terminal state. This physically revalidates the negotiated
  single-file lifecycle repair.

### Timing and remaining work

- Pairing completed and both endpoints connected at approximately `23:38:22`.
- The receiver verified the existing destination from approximately
  `23:38:24` to `23:38:29`.
- The sender verified the fully reused source/prefix from approximately
  `23:38:29` to `23:38:34`.
- Completion followed immediately after sender verification; there was no
  delivery-unconfirmed fallback or connection-loss failure.
- Test 02's failure is therefore not reproduced on the fixed build. Physical
  gate G2 passes for this run.
- The two approximately five-second verification passes remain observable work
  and should be addressed by the trusted completed-hash/cache design in M2.
- Receiver-side reused-byte accounting remains asymmetric and belongs to the
  explicit byte-domain work in M3.

### Pre-test macOS Activity-control observation

- A preceding receive Activity, external ID
  `FD40C247-6129-4BA3-87B5-2D3582BBCD95`, used Room as its primary source with
  mDNS fallback and was persisted as `cancelled` at approximately `23:37:00`.
- The visible card labelled the mode `mDNS` and briefly continued to expose
  Pause/Cancel. A later action produced “This action is no longer available.”
- The durable terminal record proves that Cancel reached the core at least
  once. The confirmed defect is stale action availability/feedback after the
  terminal snapshot, plus ambiguous presentation of a fallback route as the
  Activity mode. The exact order of the user's Pause/Cancel clicks was not
  captured, so this is not classified as a core cancellation failure.

## Test 09 — post-fix three-video Manifest, iOS to macOS

This is the user's second post-fix device run ("test2"). The global ledger
number remains Test 09.

### Input and user-visible preparation

- Direction: iOS send -> macOS receive; requested pairing mode: Room.
- Selection: three regular video files, reported aggregate size
  `1,290,110,310` bytes.
- Files: `IMG_8362.mov` (`371,926,650` bytes),
  `ScreenRecording_07-05-2026 12-49-14_1.mp4` (`428,201,319` bytes), and
  `studio_video_1781963291579352.mp4` (`489,982,341` bytes).
- The user observed that iOS preparation before the transfer Activity appeared
  was still long, while the wait after transfer startup was substantially
  shorter.
- The reports begin only after that preparation work: the iOS Activity was
  created at `15:45:36Z` (`23:45:36` Asia/Shanghai). They therefore cannot measure the full picker,
  provider-materialization, staging, or initial Manifest-hash interval.
- Whether all three exact destination files were retained immediately before
  this attempt was not recorded. Duplicate-detection behavior must not be
  inferred from this run without that fact.

### Shared build and terminal evidence

- iOS Activity: `B3B14454-D324-4EDE-80A4-005056543C97`; executable fingerprint
  `af28dfc57df85df2c984cf0b`.
- macOS Activity: `E0E17647-EA41-4A75-8893-1B0F07E432E3`; executable
  fingerprint `1572af7135c3fb4f90133505`.
- Both reports expose `core_ffi_api=3` and
  `manifest_diagnostic_events_v1`.
- Both sides reported transfer ID
  `B3B14454-D324-4EDE-80A4-005056543C97`, Direct transport, the same aggregate
  byte total, `resumed_bytes=0`, and terminal `completed` at `15:46:15Z`
  (`23:46:15` local).
- Both sides have non-zero `started_at=15:45:38Z` (`23:45:38` local), populated per-entry
  Manifest events, and no false `[failure]` section.

### Timing and payload facts

- The macOS receiver was created at `15:45:17Z` (`23:45:17` local), joined
  Room at `15:45:20Z` (`23:45:20` local), and matched only when the iOS sender
  Activity appeared at `15:45:36Z` (`23:45:36` local). The
  approximately 16-second join-to-match interval is receiver-side waiting for
  a sender and overlaps the user-reported iOS preparation; it is not evidence
  of slow broker matching once both peers had joined.
- iOS joined and matched at `15:45:36Z` (`23:45:36` local), exchanged keys at
  `15:45:37Z` (`23:45:37` local), and connected at `15:45:38Z`
  (`23:45:38` local).
- Immediately after connection, all three lightweight source checks, Manifest
  planning, aggregate Started, first-entry Started, and first payload progress
  occurred at `15:45:38Z` (`23:45:38` local). The earlier connection-time full-source preflight
  delay did not recur.
- The three receiver entry intervals in local time were approximately
  `23:45:38–48`, `23:45:48–23:46:00`, and `23:46:00–15`.
- The payload interval was approximately 37 seconds. Aggregate bytes divided
  by that coarse interval is approximately `34.9 MB/s` decimal (`33.3 MiB/s`).
- Both event streams contain continuous per-entry chunk progress. The macOS
  durable record classifies every entry as `completed`, not
  `skipped_identical`, and all three destination modification times match the
  corresponding completion seconds. This was a full-payload/publication run,
  not merely a logical-byte jump.

### Classification

- Post-connect liveness and startup gate: pass. No avoidable post-connect
  hashing stall, connection loss, or delivery-unconfirmed fallback occurred.
- Fresh/full Manifest payload path: pass, with healthy Direct throughput.
- Initial iOS preparation: still open under M2 and still invisible to the
  current Activity report.
- Duplicate/conflict behavior: not classified because pre-run destination
  retention was not recorded. A controlled immediate resend without deleting
  these three files is the appropriate follow-up.

### Additional macOS navigation observation

- After the completed Activity, the macOS sidebar displayed a red pending
  count badge on `Transfer`.
- The count describes pending Activity state and belongs on `Activity`; the
  `Transfer` entry should remain an unbadged setup destination.
- This is a presentation-placement bug, separate from transfer correctness.

## Test 10 — preparation timing baseline after APFS clone-first staging

This entry records user-observed wall-clock timing rather than an Activity
report. Values are approximate and should be compared using the same three
source videos in the next build.

### Envoix main-app Photos picker

- Input: three videos totaling approximately 1 GB.
- User-observed interval from selection to transfer-session availability:
  approximately 17 seconds.
- This path does not traverse the Share Extension handoff, so the remaining
  interval is not attributable solely to App Group persistence.

### Photos share sheet -> Envoix Share Extension

- Input: the same three videos.
- Photos took approximately 42 seconds to reach roughly one quarter of the
  visible preparation indicator; shortly afterwards the indicator completed
  almost instantaneously.
- The visible Photos preparation indicator is therefore not a linear byte or
  time measurement. It includes system-owned Photos/provider export before
  Envoix can materialize the representations.
- The fact that WeChat also exhibits a long Photos preparation interval is
  consistent with a shared system export cost; it does not make Envoix's own
  post-provider work acceptable or unmeasurable.
- The user additionally observed that WeChat sometimes accepts two selected
  videos and sometimes only one, while a three-video selection currently does
  not expose a supported export path. This is comparison evidence about the
  Photos host and WeChat extension activation/runtime boundary, not evidence
  that iOS globally limits multi-video sharing to a fixed count.
- Envoix's own Share Extension declares movie, image, file, and attachment
  activation counts of 10,000, so the same three-video selection is expected
  to expose Envoix. That declaration is only an activation boundary; it does
  not guarantee that Photos export or extension staging can finish within a
  fixed time or memory budget.

### Current cause boundary and next A/B gate

- `scripts/build-apple-core.sh` still defaulted to a Debug Rust core for this
  run. Test 08 measured about five seconds to hash 371,926,650 bytes on the
  same device, which scales to approximately 17 seconds for this selection.
- `ManifestSendRequest::from_paths` hashes the regular files sequentially
  before the durable sender Activity is created. The observed 17 seconds is
  therefore consistent with unoptimized BLAKE3 work, but this remains an
  inference until a Release-core build is tested with the same inputs.
- Next gate: build and install an otherwise equivalent app with the Release
  Rust core, then repeat only the Envoix main-app three-video selection. Hash
  parallelism or caching should be considered only if a material delay remains.

## Test 11 — Release-core three-video A/B and lifecycle observations

### Build identity and preparation result

- Input: the same three videos as Tests 09–10, totaling `1,290,110,310`
  bytes.
- The iOS executable fingerprint changed from `af28dfc57df85df2c984cf0b` to
  `ec49788526654089c8ce0a88`, confirming installation of the Release Rust
  core. The Xcode host app remained a Debug build.
- The macOS executable fingerprint remained
  `1572af7135c3fb4f90133505`; this is therefore a Release-core iOS sender to
  the preceding macOS build, not a symmetric Release-to-Release comparison.
- The user observed that the Envoix main-app selection reached Activity in
  under two seconds. The iOS report independently records
  `created_at=16:54:36Z` and `started_at=16:54:38Z`, a two-second interval.
- Compared with the approximately 17-second Debug-core baseline, this is a
  greater-than-eightfold wall-clock improvement and closes the initial
  main-app preparation bottleneck for these inputs.
- Debug and Release produce the same Manifest and BLAKE3 values. Both local
  profile builds select BLAKE3 ARM NEON; the difference is whole-core
  optimization (`opt-level=0` versus the observed Release `opt-level=3`), so
  this A/B identifies the unoptimized Rust preparation path but does not
  isolate BLAKE3 from its file-read and Manifest-building callers.

### Shared transfer evidence

- iOS Activity and transfer ID:
  `2073BEDF-4CEB-4B15-95D9-4B238CCBB518`.
- macOS Activity: `DB3D67F5-6CDA-4D3E-87EE-B2DD669C2527`.
- Both endpoints reported Direct transport, the same aggregate byte total,
  `resumed_bytes=0`, and terminal `completed` at `16:55:44Z` (`00:55:44`
  Asia/Shanghai on the following day).
- Payload ran for approximately 66 seconds, or about `19.5 MB/s` by aggregate
  wall time. This is slower than Test 09 and is not a preparation regression.
- There were two visible low-progress intervals: approximately six seconds
  around `16:54:43Z–16:54:49Z`, and approximately sixteen seconds around
  `16:55:05Z–16:55:21Z`. Several Direct-path changes occurred during the run.
  Network/path stability should be evaluated separately after both endpoints
  run the same Release-core generation.

### Photos share and app-lifecycle observations

- Photos -> Envoix for the same three videos remained system-export bound:
  roughly 43 seconds elapsed before one quarter of the indicator, followed by
  rapid acceleration around 45 seconds and completion within roughly 50
  seconds. This remains distinct from the now-fast in-app preparation path.
- In separate attempts, opening Envoix while a non-home sheet was active did
  not reliably present Sender; after force quit and reopen Sender appeared but
  the send failed, and a later reopen no longer displayed it. The Settings
  cache display was approximately 509 KB at the time.
- No paired Activity report identifies those failed lifecycle attempts. A
  later device snapshot showing no `ShareDrafts` directory cannot establish
  when or why the earlier draft disappeared. Likewise, a Manifest record
  captured while Test 11 was in progress belonged to this ultimately
  successful transfer and is not evidence of deletion during transfer.
- A simulator regression with Activity already presented successfully swaps
  to Sender, so sheet replacement alone has not reproduced the physical
  failure. A separate hosted regression did reproduce a concrete ownership
  bug: releasing `ShareDraftLease` deleted its durable draft in `deinit`.
  Remediation now preserves drafts across view recreation, acknowledges them
  only after binding to a durable Activity, and explicitly removes them on a
  non-retryable terminal state or Activity deletion.

## Test 12 — Photos-share payload starts, then the Direct path is lost

### Build and input

- iOS runtime code fingerprint: `db235c178b672dedb843d9b7`.
- macOS runtime code fingerprint: `b44e5007a5cd892f6c82f313`.
- Photos share supplied three videos totaling `1,481,943,969` bytes. The first
  exported representation was `IMG_8362.MOV` at `563,760,309` bytes, rather
  than the `371,926,650`-byte `IMG_8362.mov` selected by the in-app path. This
  attempt therefore did not use the exact same exported payload as Test 11.

### Evidence and classification

- iOS Activity/Manifest ID:
  `020E25B9-4535-44EB-8C4A-2AE49BA99B33`.
- All three sender-side source checks passed, the Manifest was accepted, and
  entry 0 eventually started. iOS reported `12,124,160` bytes sent and macOS
  reported `7,929,856` bytes received. The source was therefore readable when
  transfer began; this run does not support a pre-send source-deletion cause.
- There was an abnormal approximately 113-second interval between aggregate
  Manifest Started (`19:30:45Z`) and entry 0 Started (`19:32:38Z`). There was
  no `verifying`/`verified` event in that interval. The current
  `manifest source check` event is metadata-only, so the interval must not be
  labelled BLAKE3 time from this report.
- The Direct route changed repeatedly. macOS ultimately reported a remote
  `2409:...` IPv6 path, then detected `connection lost` at `19:33:23Z`; iOS
  detected the same loss at `19:34:15Z`.
- The macOS card was subsequently resumed alone as attempt 2 and later paused
  by the user. Its top-level `[failure]` consequently describes that latest
  user pause, while the retained attempt-1 transfer event contains the actual
  network loss. A retry initiated on only one endpoint did not re-establish
  the paired transfer.

## Test 13 — In-app control reproduces zero-byte path loss

### Build and input

- The same iOS and macOS runtime code fingerprints as Test 12 were present.
- The in-app picker selected the established three-video set totaling
  `1,290,110,310` bytes. This removes the Photos Share Extension and App Group
  handoff from the failing path.
- iOS Activity/Manifest ID:
  `F080F480-57FD-43E8-AA37-B457673A722F`; macOS Activity:
  `4029F567-7C29-425F-8E48-EEBAB45201A9`.

### Evidence and classification

- Pairing, key exchange, all three source checks, Manifest planning, and
  aggregate Started completed by `19:37:43Z`. No Manifest entry reached
  Started and both sides remained at zero payload bytes.
- macOS first reported a Direct peer inside the hotspot's `240a:42a3:fe00:17e8::/64`
  prefix, then one second later migrated to
  `[2409:811f:63e4:19e4:8886:14c:8ae3:5f0c]`. It reported connection loss 60
  seconds later. iOS reported the same loss after its longer detection delay.
- A contemporaneous macOS route snapshot maps `172.20.10.7` plus the report's
  `240a:...:1960...` address to the Mac hotspot interface. The selected
  `2409:...` peer is outside that on-link prefix and is routed through the
  iPhone gateway. This is the material environmental difference from the
  earlier successful runs, whose selected peer paths stayed inside
  `172.20.10.0/28` or the shared `240a:42a3:fe00:17e8::/64` prefix.
- Because the in-app control fails before the first entry in the same way, the
  current regression is classified as Direct-path selection/migration or the
  entry-handshake traffic stalled on that path, not ShareDraft cleanup and not
  Photos export. A CIDR-controlled repeat that advertises only
  `172.20.10.0/28` is the shortest discriminating test.
- Routing through the iPhone gateway means this run should not be described as
  proven LAN-only traffic. The reports and route table identify the route but
  do not by themselves prove carrier billing; that requires interface-byte or
  carrier accounting evidence.

## Test 14 — small single-file control fails on the same off-link route

- Input: one `63539.jpeg` file totaling `353,224` bytes, sent from iOS to
  macOS through the compatible single-file protocol rather than Manifest.
- iOS Activity: `EB4FC43A-50C5-4B2A-ABBB-C812A5A2CA39`; macOS Activity:
  `621EC96F-CF35-4442-9A7A-C3F03789ACFB`.
- iOS sent all `353,224` bytes in the same second and entered Confirming.
  macOS had accepted and started the file but reported zero payload bytes.
  This excludes large-file hashing, Manifest planning, multi-file handling,
  Photos export, and ShareDraft lifetime as necessary causes of the current
  failure.
- macOS initially connected to the iPhone at `172.20.10.1`, then migrated to
  `[240a:429f:fe10:6906:8d0:68b:ff0d:8461]`, which is outside the Mac's
  on-link `240a:42a3:fe00:17e8::/64` prefix. It later reported connection
  loss. The iOS sender's selected peer remained the Mac's on-link
  `240a:42a3:fe00:17e8:1960:ed5:4080:87c` address and ended as delivery
  unconfirmed because no completion receipt returned.
- Failure normalization is inconsistent for the same transport loss: macOS
  maps `I/O error during transfer: connection lost` to non-retryable
  `internalError`, while the sender maps the missing receipt to retryable
  `networkLost`. This is a separate structured-error bug and prevents a
  coherent recovery UI.

## macOS installation cleanup after Test 14

- The running reports' main executable hash `aac583...` and runtime-code hash
  `b44e500...` matched the stable latest build product exactly. The failed
  tests were therefore already running the new macOS code, not the stale
  installed copy.
- A stale `/Applications/Envoix.app` from 30 June used an old monolithic
  executable (`60beee...`, Xcode 16.2). It was atomically replaced with the
  current Xcode 17.6 build and removed after verification.
- `/Applications/Envoix.app` now hashes to `aac583...` plus runtime code
  `b44e500...`; the live process path was verified as
  `/Applications/Envoix.app/Contents/MacOS/Envoix`.
- The stable build product remains in the guarded Envoix build cache as a
  build artifact, but it was unregistered from LaunchServices. No second
  installed Envoix App remains in Applications, user Applications, Desktop,
  Downloads, or the staging area.

## Test 15 — same small file succeeds while the path remains on-link

- Input: the same `63539.jpeg` file totaling `353,224` bytes, sent iOS to
  macOS through Room and the compatible single-file protocol.
- iOS Activity: `68FB4BD8-0981-4958-A80D-D50FB2EC7015`; macOS Activity:
  `87A29A51-09C9-4695-AB59-8263FDA1316E`.
- Runtime fingerprints are unchanged from the failed Tests 12–14: iOS
  `db235c178b672dedb843d9b7`, macOS
  `b44e5007a5cd892f6c82f313`. This is a same-code control, not evidence that a
  different binary fixed the transfer.
- Both endpoints selected peers inside the shared hotspot IPv6 prefix
  `240a:42a3:fe00:17e8::/64`: macOS used the iPhone address ending
  `:2531:91e:2770:5e33`, while iOS used the Mac address ending
  `:1960:ed5:4080:87c`.
- Neither event stream contains a path migration. Started, full progress,
  confirmation, and Completed all occurred at `19:52:05Z`; both reports agree
  on transfer ID, byte count, and terminal completion.
- The direct A/B is now: Test 14 migrated the Mac's peer path from local IPv4
  to an off-link IPv6 address and failed; Test 15 kept both directions on-link
  and completed immediately. This materially strengthens Direct candidate
  selection/migration as the current root-cause area. Reinstall/relaunch may
  have reset endpoint discovery, but the unchanged runtime hash means stale
  application code was not the causal variable.

## Test 16 — 1.29 GB completes after two hotspot-IPv6 stalls and IPv4 fallback

- Input: the established three-video in-app selection totaling
  `1,290,110,310` bytes.
- iOS Activity/Manifest ID:
  `DC571178-69FB-43B5-A1EE-80200E8CEB3E`; macOS Activity:
  `6EC74978-B41E-40A9-9EA4-799B35BAB101`.
- Runtime fingerprints remain iOS `db235c178b672dedb843d9b7` and macOS
  `b44e5007a5cd892f6c82f313`. No candidate allow/deny preference was present
  on macOS, so automatic multi-candidate selection remained enabled.
- Pairing, all source checks, Manifest planning, entry 0 Started, and its first
  64 KiB occurred by `19:55:37Z`. This confirms the Release preparation path
  remained fast and the later delay was inside active payload transport.
- At `19:55:38Z`, the macOS receiver changed the iPhone peer from the on-link
  IPv6 address ending `:2531:91e:2770:5e33` to another on-link address ending
  `:1431:662a:c92c:9df6`. No further payload progress was reported for about
  55 seconds. A live route snapshot maps both addresses to the same iPhone
  neighbor on the hotspot interface.
- Payload then advanced to roughly 294 MB by `19:56:42Z` before another
  approximately 15-second low-progress interval. macOS changed back to the
  first IPv6 at `19:56:57Z`; at `19:56:59Z`, both sides converged on hotspot
  IPv4 (`172.20.10.1` and `172.20.10.7`).
- After IPv4 convergence, approximately 981 MB remained and completed in
  roughly 13 seconds by coarse event timestamps. The active stable-path rate
  was therefore around 75 MB/s, while the full wall-clock average was only
  about 13.6 MB/s because of the two path stalls.
- All three entries and the aggregate Manifest completed successfully with no
  integrity or publication failure. The sample proves that off-link public
  IPv6 is not required for the bug: switching among multiple on-link hotspot
  IPv6/privacy addresses can also black-hole progress. The practical
  mitigation to test is still to advertise only `172.20.10.0/28`; the product
  fix must address hotspot path preference/stickiness, not merely reject one
  public IPv6 prefix.

## Test 17 — controlled hotspot-IPv4 allow-list removes all stalls

- Both Apple endpoints were configured with candidate allow
  `172.20.10.0/28`, candidate deny empty, then fully restarted without
  changing the hotspot or test files.
- Input: the same three-video in-app selection totaling
  `1,290,110,310` bytes.
- iOS Activity/Manifest ID:
  `FBDF7F52-2276-453C-8C3B-DAF05E67A842`; macOS Activity:
  `C89F446D-C84B-438D-852B-089C0FB28926`.
- The macOS accept diagnostic reports `direct=1 relay=1`, confirming that the
  configured filter reduced the four Direct candidates to one while retaining
  the Room relay fallback. Both endpoints selected only hotspot IPv4:
  `172.20.10.1` from macOS and `172.20.10.7` from iOS.
- There are no IPv6 paths and no `pathChanged` events. All source checks,
  Manifest planning, first entry start, and first progress occurred at
  `20:06:57Z`; all three entries and the aggregate completed by `20:07:14Z`.
- The 1.29 GB payload therefore completed in approximately 17 seconds, about
  `75.9 MB/s` decimal (`72.4 MiB/s`) by coarse wall-clock timestamps. Progress
  is continuous across all three entries with no startup, mid-file, or
  confirmation stall.
- Test 16 used the same code, files, devices, hotspot, direction, and Room
  protocol with automatic four-candidate selection. It took about 95 seconds
  because of approximately 55- and 15-second path stalls. The controlled
  IPv4-only run is about 5.6 times faster end-to-end and matches the stable
  post-fallback rate observed at the end of Test 16.
- This A/B establishes automatic candidate selection/path migration as the
  causal subsystem for the observed regression in this environment. The
  hard-coded CIDR is a diagnostic workaround for the current iPhone hotspot,
  not a general product default; the product remediation must derive a safe
  local-path preference and retain WAN/IPv6/relay compatibility.

## Test 18 — identical IPv4-only control repeats successfully

- Without changing the Test 17 candidate settings or test procedure, the same
  transfer was repeated once more.
- The tester reports that the run was again stable and fast. No exact timing or
  paired Activity report was supplied for this run, so no more precise metric
  is inferred here.
- This is a second successful run of the IPv4-only mitigation and reduces the
  likelihood that Test 17 was a one-off favorable path selection. It validates
  repeatability of the workaround; it does not by itself reproduce or prove the
  root cause of the unfiltered multi-candidate failure.

## Test 19 — orphan completion receipt causes a false successful receive

- Input: `63539.jpeg`, `353,224` bytes, sent iOS to macOS through Room while
  the hotspot IPv4-only candidate control remained active.
- iOS Activity: `C368E2BF-87D0-4EB0-B7AB-831DE2F63F8B`; macOS Activity:
  `E1096753-F97B-41F7-A522-044053C2402D`.
- Both endpoints report Completed with transfer ID
  `transfer-055fc350a55bf8d4324ac81b973b4c8fd81f1874a99581f1d323ec0ea33a150e`.
  The selected path stayed on hotspot IPv4 and pairing completed normally.
- The sender reports `resumed_bytes=353224`, Verifying, Verified, Started, and
  Completed in the same second. The receiver emits neither Started nor
  Progress and jumps directly from Connected to Completed. This proves that no
  payload bytes were written during this attempt.
- Post-run disk inspection found
  `/Users/moranxuege/Downloads/.envoix-receipt.63539.jpeg.json` but no visible
  `/Users/moranxuege/Downloads/63539.jpeg`. The receipt was left by the earlier
  successful transfer and remained after the visible file was deleted or moved.
- Root cause: the compatible single-file receive path accepted a matching
  durable completion receipt even though a direct destination no longer
  contained the completed file. Receipt-only completion is valid for private
  staging followed by native publication, but not for direct-to-folder output.
- Remediation: before a direct receive starts, remove only valid Envoix receipt
  sidecars whose referenced file is absent. Preserve receipts when the file is
  still present, and preserve receipt-only semantics for iOS custom-folder
  native publication. A hosted regression test covers both the orphan-removal
  and existing-file-preservation branches.

## Test 20 — old installed build repeats receipt-only completion with delayed UI state

- Input: the same `63539.jpeg`, `353,224` bytes, sent iOS to macOS through
  Room after the orphan-receipt code fix had been built but before that build
  was installed into `/Applications`.
- iOS Activity: `9FD6EBFA-6D94-4405-901B-6367A02B323E`; macOS Activity:
  `A6E7B4D7-C534-4BDD-8027-73558F1BA11E`.
- The macOS report still identifies executable `aac583e6e4c26a456fd97de8`
  and runtime dylib `b44e5007a5cd892f6c82f313`. These are the pre-fix
  `/Applications/Envoix.app` fingerprints, not the newly built executable
  `6283d2b1eacea73c4263a447` and dylib `a8751e28af2183f1f1a98a6`.
- The sender again reports the full `353,224` bytes as resumed, and the receiver
  again has no Started or Progress event before Completed. This is a second
  receipt-only false completion, not a payload transfer and not a validation of
  the orphan-receipt fix.
- Both core event streams reach Completed at `01:07:34Z`, but the macOS durable
  Activity fields and user-facing activity log do not record completion until
  `01:07:45Z`. The approximately 11-second discrepancy is receiver state/UI
  propagation latency on the old receipt-only path; it must not be interpreted
  as file-transfer time.
- Post-test process inspection confirmed that the running application was the
  stale `/Applications/Envoix.app`. It was subsequently replaced with the
  hash-verified new build and relaunched from the same canonical path; the old
  temporary backup was removed.

## Test 21 — orphan receipt fix rematerializes the missing direct-destination file

- Precondition: the stale
  `/Users/moranxuege/Downloads/.envoix-receipt.63539.jpeg.json` remained in
  place while the referenced visible file was absent. No manual cleanup was
  performed before starting the receive.
- Input: the same `63539.jpeg`, `353,224` bytes, sent iOS to macOS through
  Room. iOS Activity: `520D0189-68FE-4203-9B8F-48A8F8EB8B25`; macOS
  Activity: `25B3C8C5-8C85-4ADB-A6A2-2C32C293DB06`.
- The macOS report identifies the fixed installed build: executable
  `6283d2b1eacea73c4263a447` and runtime dylib
  `a8751e28af2183f1f1a98a6b`.
- Both endpoints report `resumed_bytes=0`. The sender emits Started and full
  Progress; the receiver emits Started, zero/full Progress, and Completed.
  This is an actual payload transfer, unlike Tests 19 and 20.
- The receiver reached its core Completed event at `01:20:16Z` and durable
  Activity completion at `01:20:17Z`, reducing the previous receipt-only
  approximately 11-second state delay to the normal one-second coarse-log
  boundary. The sender completed at `01:20:15Z` after confirmation.
- Post-run disk verification found the visible Downloads file at exactly
  `353,224` bytes, modified at `09:20:16+0800`, with SHA-256
  `11cb43b671bfe891ec9e24560a6de4dec87e53ac79716de698e1edeb2c80d0aa`.
  The completion receipt was rewritten with this run's transfer ID
  `transfer-782f23de0a25787f5a202a0bdb361bb4ce4dc1dcbd6f2a8dea78c0af7ac7a2b4`.
- Result: the direct-output orphan-receipt remediation is validated on the
  real macOS/iOS pair. The remaining control is an immediate repeat without
  deleting the visible file, which must preserve the file and use receipt/file
  deduplication rather than retransmitting the payload.

## Test 22 — existing direct-destination file is reused but presented as a transfer

- Precondition: Test 21's visible `63539.jpeg` and its completion receipt were
  both retained in Downloads. The same source was immediately sent again.
- iOS Activity: `E002750F-2C58-4E68-8398-BA80A6B51F1A`; macOS Activity:
  `20E9AB78-CD6F-4A45-BDF7-1FD40F30DB6E`. The fixed macOS runtime fingerprints
  remain `6283d2b1eacea73c4263a447` and `a8751e28af2183f1f1a98a6b`.
- The iOS sender reports `resumed_bytes=353224`. The macOS receiver performs
  full-file Verifying and Verified, then completes without any payload Progress
  event. Post-run inspection confirms the destination inode, modification time
  (`09:20:16+0800`), size, and SHA-256 are unchanged. No file data was resent
  or rewritten.
- The protocol behavior is correct, but the presentation is ambiguous: the
  receiver report still says `resumed_bytes=0`, and both activity cards render
  an ordinary Completed result rather than explaining that the destination
  already had the identical file.
- Remediation: project receive-side verified existing bytes through the
  existing `bytes_resumed` FFI field, without changing FFI API version or enum
  layout. A fully resumed terminal activity can then explicitly render zero
  bytes transferred this attempt plus “file already exists” / “receiver already
  has this file” in the card, details, status, and diagnostic log.

## Test 23 — retained single-file accounting and presentation are correct on both peers

- Precondition: the visible `63539.jpeg`, `353,224` bytes, and its completion
  receipt remained in Downloads. The same iOS source was sent again through
  Room after installing the receive-side Manifest accounting fix.
- iOS Activity: `7E86C9B7-59DE-4B51-A3B4-19E0ADE674D8`; macOS Activity:
  `BA0082D1-B5D1-4824-9E21-0EDD95F948B5`. Both reports share transfer ID
  `transfer-47d4d43cf07c09fbf53b5ccbcb5699b6c7d241c36f65237579b8f1bcf7f2fc64`.
- The installed-build fingerprints match the acceptance build. macOS reports
  executable `1572af7135c3fb4f90133505` and runtime dylib
  `322eece0a3eedfd887229f3d0`; iOS reports executable
  `92a264d0388462f22f9625a4` and runtime dylib
  `c2da2e8f9e9d3e1c0ea5ad01`.
- Both canonical Activities reached Completed and now report
  `bytes=353224/353224` with `resumed_bytes=353224`. The receiver emitted
  Verifying and Verified with `resumed=353224`, then Completed without a
  payload Progress event.
- The user-facing logs distinguish the no-payload result: macOS reports
  `completed · already present · 353 KB`, while iOS reports
  `completed · already at receiver · 353 KB`.
- Pairing completed at `08:24:30Z`; the sender completed in that same second
  and the receiver durable Activity completed at `08:24:31Z`. Neither peer
  entered delivery-unconfirmed, retry, pause, or connection-loss fallback.
- Result: the retained single-file protocol, receive-side resumed-byte
  projection, and terminal presentation acceptance gate all pass. This closes
  the Test 22 accounting/presentation defect.

## Test 24 — one retained and one new file expose the remaining Manifest byte domains

- Input: two iOS-selected images sent to macOS through Room. The retained
  `63539.jpeg` is `353,224` bytes and the new `IMG_8368.jpeg` is `442,612`
  bytes, for a logical Manifest total of `795,836` bytes.
- iOS Activity: `59C7A6D8-573D-45A2-B6D2-A5FF50991129`; macOS Activity:
  `55E07EDC-95A4-4FEC-88D2-0920E549B924`. Both endpoints use transfer ID
  `59C7A6D8-573D-45A2-B6D2-A5FF50991129` and the same accepted build
  fingerprints as Test 23.
- Receiver planning classified entry 0, `63539.jpeg`, as
  `skipped_identical`. It emitted no Started or payload Progress for that
  entry. Entry 1, `IMG_8368.jpeg`, emitted Started, zero/full per-entry
  Progress, and Completed.
- Sender aggregate Progress first reported `418,760/795,836`, which is the
  `353,224` logically skipped bytes plus the first `65,536` payload bytes of
  the new file. Both Activities then completed at the logical
  `795,836/795,836` total with `resumed_bytes=0`.
- Pairing and authentication completed at `08:25:04Z`; payload began at
  `08:25:05Z`, and both endpoints completed in that same second. No liveness,
  confirmation, or publication failure was reported.
- Result: mixed retained/new Manifest behavior is correct, and
  `resumed_bytes=0` is semantically appropriate because a whole-file
  `skipped_identical` result is not a resumed prefix. The report still cannot
  state that only `442,612` physical payload bytes were transferred; aggregate
  progress and the completion log present the `795,836` logical total. This is
  the small deterministic acceptance fixture for separate skipped, resumed,
  logical, and physical-payload counters.

## Test 25 — all-skipped Manifest completes instantly after a 30-second receiver pre-pairing wait

- Precondition: both Test 24 destination images remained in Downloads. The
  same two-file, `795,836`-byte logical Manifest was sent again through Room.
- iOS Activity: `BB33DE40-39FF-402F-9C0E-5F2ADD4DF6E5`; macOS Activity:
  `8B5C6304-12EE-493C-8B7C-C83B62E84A91`. Both endpoints use transfer ID
  `BB33DE40-39FF-402F-9C0E-5F2ADD4DF6E5` and the accepted Test 23/24 build
  fingerprints.
- Manifest behavior passes: the macOS receiver classified both
  `63539.jpeg` and `IMG_8368.jpeg` as `skipped_identical`; neither peer emitted
  per-entry Started or payload Progress, and both reached Completed at
  `08:40:17Z`.
- The user-visible delay happened before Room pairing. iOS created its Activity
  and emitted `pairing: joining room` at `08:39:44Z`. macOS created its receive
  Activity at `08:39:45Z` but did not emit `pairing: joining room` until
  `08:40:15Z`, approximately 30 seconds later. It matched the already waiting
  sender in that same second, exchanged keys one second later, connected, and
  completed all skip planning in the next second.
- Source inspection identifies the cause boundary: Room receivers call
  `ready_endpoint_addr(config.data_relay().is_some())` before joining the
  rendezvous room. Auto mode configures the default data relay, so the receiver
  waits for a relay home for up to `ENDPOINT_ADDR_WAIT_TIMEOUT` (30 seconds)
  even when local Direct candidates are already available. The later accept
  diagnostic reported `direct=4 relay=1`, and the payload path was Direct.
- Result: the all-skipped zero-payload Manifest acceptance gate passes. The
  approximately 33-second wall time is a separate endpoint-readiness/Room-join
  latency defect, not hashing, conflict planning, payload, or completion time.
  A fix must preserve relay fallback rather than silently making Auto
  direct-only.

## Test 26 — unrelated fresh single-file control

- The user explicitly identified this run as unrelated to the planned Share
  Extension lifecycle test. The reports do not establish the selection entry
  point, an app termination/relaunch, or draft recovery, so this run must not
  be counted as evidence for those behaviors.
- Input visible in the reports: one new `IMG_8378.jpeg`, `199,517` bytes, sent
  iOS to macOS through Room. iOS Activity:
  `CB003635-4601-4BCE-AF51-D78CFE42EC92`; macOS Activity:
  `B71C95ED-E6F2-492D-B043-F247B92C64FF`. Both share transfer ID
  `transfer-ba718e2375207427a80733c56fe9cf45ba6dadc0aead808c2c2da237ccd9c420`.
- Both accepted builds completed with `resumed_bytes=0` and full payload
  Progress. macOS joined Room approximately three seconds after Activity
  creation, matched the sender two seconds later, and completed at
  `09:18:16Z`. The initial payload path was Direct IPv4; iOS reported a Direct
  IPv6 path change after completion.
- Result: ordinary fresh single-file transfer remains healthy and the
  30-second relay-readiness delay from Test 25 did not recur. The Share
  Extension lifecycle acceptance gate remains untested.

## Test 27 — Share Extension draft survives relaunch and transfers after two wait intervals

- User-confirmed entry path: two images were sent from Photos through the
  Envoix Share Extension. Envoix automatically presented Sender and showed the
  correct two-item selection.
- The app was force-quit before sending and then reopened. The same two items
  remained available, and both ultimately appeared in the macOS Downloads
  folder. These manual observations close the activation, item-count, durable
  draft recovery, and final-publication acceptance gates that Activity reports
  cannot establish on their own.
- Input visible in the reports: `IMG_8373.jpeg`, `4,231,762` bytes, plus
  `63539.JPG`, `353,224` bytes, for a total of `4,584,986` bytes. iOS
  Activity/Manifest ID and transfer ID:
  `38CE0C9B-EECB-488D-9091-6A1A508F19AB`; macOS Activity:
  `8FB6E905-8474-40F9-9E94-057CDB16F3DC`.
- Both endpoints reached Completed at `09:37:15Z`, agreed on the logical byte
  total, and reported `resumed_bytes=0`. Both files emitted per-entry Started
  and completed as ordinary new payload, with no skip, pause, confirmation, or
  integrity failure.
- The first wait is before pairing: macOS created its receive Activity at
  `09:36:17Z` but did not emit `pairing: joining room` until `09:36:43Z`, about
  26 seconds later. The iOS sender had already joined at `09:36:24Z` and matched
  as soon as macOS joined. This is another occurrence of the receiver
  endpoint-readiness/relay-registration delay classified in Test 25.
- The second wait is after Manifest construction: by `09:36:45Z`, iOS had
  emitted both source checks, Manifest Planned, and aggregate Manifest Started;
  macOS had also emitted Manifest Planned and Started. The first entry did not
  start until `09:37:15Z`, exactly 30 seconds later. This interval is therefore
  not pre-Activity Photos export and not sender BLAKE3/Manifest construction.
- The receiver connected through one Direct hotspot IPv6 address at
  `09:36:44Z`, changed to another at `09:36:45Z`, then reported no payload
  progress during the 30-second interval. At `09:37:15Z` it changed again and
  immediately completed all `4,584,986` bytes at the report's one-second
  precision. In light of Tests 12–18, this strongly classifies the second wait
  as an entry-handshake/data-stream stall on a selected hotspot IPv6 path,
  followed by Iroh path recovery; the Activity report alone does not expose the
  lower-level path probe that selected the candidates.
- Result: the Share Extension lifecycle and transfer correctness pass. Both
  already-recorded liveness defects nevertheless recur in the same run:
  delayed Room publication before pairing and a post-connect Direct IPv6 black
  hole. Those latency defects are independent of ShareDraft persistence.

## Test 28 — single-file pause and resume preserves 142.6 MB and completes

- Input: one new `IMG_8202.mov`, `370,520,202` bytes, sent iOS to macOS through
  Room. iOS Activity: `55560172-DC54-48F4-A802-46A1F08DAD47`; macOS Activity:
  `A94277A2-9B27-4EBA-80A6-14922B55ED6A`. Both terminal records show
  `attempt-2` and transfer ID
  `transfer-119d522579182b893f93b60d8a8811a210e28b23b94a9b67c15f0eb0d1d93064`.
- Attempt 1 transferred for roughly three seconds. The iOS event stream then
  reports `transfer paused by user` and the macOS event stream reports
  `transfer paused by peer`. This structured origin evidence identifies iOS as
  the pause initiator unless the native command was routed to the wrong
  session; manual confirmation of which device's button was pressed remains
  required.
- Attempt 2 verified and reused exactly `142,606,336` bytes on both endpoints,
  then continued from that offset rather than restarting at zero. Both reports
  expose the same value in `resumed_bytes` and reached Completed by
  `13:58:13Z`. The receiver reports Downloads as the completed path.
- The resume sequence did not create a confirmation or integrity failure.
  Approximately `227.9` MB remained after the verified prefix and completed in
  about four seconds at coarse timestamp precision.
- The initial Room wait recurred independently of pause/resume. macOS created
  the receive Activity and emitted Binding at `13:57:03Z`, but did not emit
  `pairing: joining room` until `13:57:33Z`, exactly 30 seconds later. The iOS
  sender had joined at `13:57:05Z` and matched one second after macOS finally
  joined. This is another occurrence of the relay-readiness gate from Tests 25
  and 27, not file preparation or payload throughput.
- A separate user observation says a hidden video could not be shared. The
  successful transfer report does not identify that failed selection or its
  entry path. Current source applies no Envoix-specific hidden-asset filter:
  the in-app path uses the system Photos picker and the Share Extension consumes
  system-provided item providers. The observation remains unclassified until
  the tester states whether the hidden video was absent from the in-app picker,
  Envoix was absent from the Photos share sheet, or Envoix displayed an import
  error after selection.
- Result: core pause/resume and partial-byte reuse pass. macOS-button semantics,
  final playback, and the hidden-video entry-point behavior remain pending
  manual observations.

### Post-Test 28 Activity deletion observation

- The user reports that deleting the Activity failed again. The exact visible
  symptom and whether the record returns only after relaunch are not yet
  captured, so this is recorded as an open defect rather than assigned to the
  SwiftUI layer alone.
- Source inspection shows a cross-layer acknowledgement gap. Swift
  `removeActivity` discards frontend-owned resources, adds the ID to a local
  suppression set, and removes the card immediately. It ignores the Boolean
  returned by both durable session `remove()` calls. The Rust/FFI `remove()`
  result in turn confirms only that a discard command was queued; durable
  record and sidecar deletion happen asynchronously and have no completion or
  failure acknowledgement back to Apple UI.
- Result: classify as **Activity removal can fail or be presented as complete
  without durable confirmation**. Keep it in the M4 action-semantics scope,
  but investigate Swift state, session ownership, FFI command acceptance, and
  durable cleanup together before calling it a frontend-only bug.

## Test 29 — some Photos videos remain in an abnormal preparation stage

- The user reports that selecting certain videos enters an abnormal preparation
  stage and does not become shareable, although ordinary videos and the earlier
  optimized preparation path can complete normally. No Activity report exists
  for the failed selection yet, so the observation is upstream of, or at least
  not proven to have reached, durable Activity/Manifest construction.
- The in-app picker already requests
  `preferredAssetRepresentationMode = .current`, which asks Photos to avoid a
  conversion when possible but cannot guarantee that every asset is locally
  available or immediately exportable. Both the in-app picker and Share
  Extension then call `NSItemProvider.loadFileRepresentation`.
- That API returns a `Progress`, but the current Apple clients retain it only
  for cancellation. They do not observe its byte/fraction progress, record its
  duration, or distinguish Photos retrieval/export from the following App
  Group materialization. The UI therefore renders all of this work as a generic
  preparation spinner.
- After the provider callback, Envoix persists the temporary representation to
  App Group storage with APFS clone requested and ordinary copy fallback. The
  current staging call discards whether materialization was cloned or copied
  and records no duration. A large fallback copy is consequently also
  indistinguishable from a Photos-provider wait.
- This is not yet evidence that the Release-core BLAKE3/Manifest optimization
  regressed: provider export/download and durable App Group materialization
  occur before the Manifest preparation that optimization addressed.
- Result: classify as **Photos video preparation has an unobservable provider
  or durable-materialization stall**. Capture the entry path, provider progress,
  materialization method/duration, and whether the same asset becomes fast after
  it is fully played/downloaded in Photos before changing representation or
  integrity behavior.

## M4 state/UI repair implementation — 2026-07-17

- Terminal completed, failed, or canceled operations now release the Transfer
  presentation slot after their diagnostics are snapshotted. Their durable
  records remain in Activity, while reopening Transfer returns to fresh setup.
- Generic `Send Again` and `Receive Again` labels were removed. Fresh setup now
  says `Send`, `Start Receiving`, or the Invite-specific
  `Create Link and Wait`; same-Activity recovery remains owned by Activity.
- A sender QR scanned on Send switches to Receive, and a receiver QR scanned on
  Receive switches to Send. An unfinished send selection is preserved for a
  later switch back instead of being discarded.
- Activity removal no longer optimistically hides the card. The card displays
  `Removing…` while durable disappearance is being checked; a timeout or
  persisted record restores the delete action and reports that removal should
  be retried. Owned resources are released only after acknowledgement.
- Focused automated evidence passed on 2026-07-17: one hosted terminal-slot
  regression and five UI regressions covering wrong-role QR switching,
  canonical Pause/Cancel availability, cancel acknowledgement timeout, and
  stalled durable removal, all with zero failures. The shared Swift changes
  also pass a macOS Debug build.
- Physical acceptance remains required for four user-visible checks: Transfer
  returns to idle after a terminal operation, no ambiguous repeat label remains,
  wrong-role QR switching retains selected items, and deletion stays removed
  after relaunch without flicker. The intermittent Photos-video preparation
  case, Room readiness delay, path selection, speed/ETA, Wi-Fi Aware, and
  Android transport are explicitly outside this M4 repair.
