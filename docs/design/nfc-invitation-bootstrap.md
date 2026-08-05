# NFC invitation bootstrap

NFC is an invitation carrier. It does not carry transfer data and it does not
define a second pairing, authentication, or transfer protocol.

## Verification status

The Android HCE protocol, both AID registrations, private-AID readers, bounded
role lifecycle, NDEF codec, and rejection rules have automated coverage. A
private-AID Android-to-iPhone read and room transfer passes on the attached
Xiaomi HyperOS 3 phone and iPhone 15 Pro Max. The 2026-08-05 regression run
completed private AID selection, NDEF-file selection, and bounded reads in
about 80--150 ms, and all 71 focused iPhone tests passed. Xiaomi still intercepts
the standard NDEF AID before Envoix receives it. The newer BLE-gated automatic
reader and Android-to-Android paths have automated coverage but await a
separate physical regression; this document does not claim that unrun gate.

## Interoperable NDEF contract

An Envoix NFC endpoint exposes exactly one NDEF message with exactly one NFC
Forum Well Known Type URI record. Its raw fields are:

- Type Name Format: NFC Well Known
- type: the single byte `0x55` (`U`)
- identifier: empty
- payload: URI Identifier Code `0x00`, followed by exact printable-ASCII bytes

Android host-card emulation and externally provisioned passive tags use this
versioned HTTPS carrier:

```text
https://ece4410j-nuub.github.io/nfc/v1/#<invitation>
```

`<invitation>` is the canonical, unpadded RFC 4648 base64url encoding of the
unchanged ASCII Envoix invitation. After strict decoding, it must:

- contain only bytes `0x21...0x7e`;
- be no larger than 8,211 bytes;
- contain at least one byte after the case-sensitive
  `envoix://invite/v2/` or `envoix://room/` prefix; and
- re-encode to the exact input token, rejecting padding, alternate alphabets,
  non-zero discarded bits, and other noncanonical representations.

The largest token is 10,948 characters. User info, ports, query parameters,
different schemes, different host casing, and other paths are not equivalent
carriers. Text, MIME, external-type, absolute-URI, Smart Poster, Android
Application Record, and multi-record messages are not Envoix NFC invitations.

Readers continue to accept a legacy direct canonical `envoix://invite/v2/` or
`envoix://room/` URI record when it reaches an Envoix decoder. Newly provisioned
carriers use the HTTPS form. The canonical invitation passed to Rust, QR, BLE,
and room control is unchanged.

## HCE application routes

Android registers two exact application identifiers on the same
`HostApduService`. Both expose the same process-only invitation snapshot and
the same capability-container and NDEF-file bytes. They differ only in how the
reader reaches that state machine.

### Standard NFC Forum NDEF route

The standard route uses NFC Forum Type 4 Tag NDEF application AID
`D2760000850101`. It is the interoperable path used by ordinary NDEF readers
and is the only Envoix HCE route that can participate in iOS background NFC
detection. Envoix keeps this AID registered for non-Xiaomi and other devices
whose system NFC stack dispatches the selected AID to the registered HCE
service.

### Private foreground fallback

The fallback uses the exact proprietary AID `F0454E564F495801`. Opening Connect
does not itself start Core NFC. An Android presenter first arms a 120-second HCE
lease and broadcasts a separate, secret-free BLE readiness UUID. One fresh
readiness generation may start one `NFCTagReaderSession` while Connect is
foreground and unobstructed. Apple first correlates that generation to exactly
one Envoix peer key recently advertised by the same Core Bluetooth peripheral;
an absent, expired, or ambiguous binding fails closed. That correlation is
routing metadata, not authentication. Automatic reads are limited to one per
Connect activation and no more than one per 60 seconds. A fresh offer may wait
for an in-app sheet or alert to close, but is discarded after its 30-second
lease. Apple's system NFC sheet is unavoidable and appears before private-AID
recognition. Cancellation, timeout, and failure never automatically reopen it.
The compact **Scan by NFC** action remains an explicit fallback without
requiring BLE. The Debug and Release Info plists allowlist only this private AID
under
`com.apple.developer.nfc.readersession.iso7816.select-identifiers`, so Core NFC
selects it before reporting the `NFCISO7816Tag`. Envoix verifies
`initialSelectedAID` exactly, selects file `E104`, reads the two-byte NLEN, and
then issues bounded `READ BINARY` commands of at most 255 bytes. Every command
must return `9000` with the exact expected response length. The completed bytes
are parsed as an `NFCNDEFMessage` and passed through the existing strict
invitation codec. This is a proprietary ISO-DEP route that reuses the Type 4
NDEF-file layout; it is not another standard NDEF application.

iOS background tag reading does not start an arbitrary Core NFC tag-reader
session and does not select app-defined AIDs. Therefore the private AID cannot
wake or launch an inactive iPhone. It always requires Envoix already in the
foreground and Apple's system NFC sheet; BLE gating does not remove those
platform restrictions.

The private AID is public routing metadata, not a secret, identity, proof of
proximity, or authentication mechanism. A different app or device can select,
copy, emulate, or relay it. Data obtained through either AID remains untrusted
until the unchanged Envoix invitation validation, confirmation, expiry, role,
SPAKE2, and channel-binding checks run.

## Public link association and privacy

The organization Pages repository
`ECE4410J-NUUB/ece4410j-nuub.github.io` serves:

- `/.well-known/apple-app-site-association`;
- `/apple-app-site-association`; and
- the no-analytics browser fallback at `/nfc/v1/`.

The association authorizes only `/nfc/v1/*` for Apple application identifier
`6638TTB2SF.com.envoix.app.ios`. The invitation is in the URL fragment, which
is not included in an HTTP request. The fallback removes it from browser
history, validates it locally, and requires an explicit **Open Envoix** action.
It uses no cookies, storage, analytics, third-party assets, or network API.

iOS normally obtains and caches the association from GitHub Pages when the app
is installed. The device needs internet access while obtaining or refreshing
that association, and an unassociated browser fallback needs internet access
to load the Pages site. Cache timing and Universal Link dispatch remain under
iOS control.

Once the association is cached, the standard background NFC notification can
hand the Universal Link to the installed app without loading the GitHub page.
The private foreground-AID reader also decodes the NDEF carrier locally. The
NFC read and local invitation decoding therefore do not themselves require
internet access, and the URL fragment is not sent in an HTTP request.

After the invitation enters the existing room flow, its rendezvous, broker,
relay, and transfer code has its own connectivity requirements. In particular,
a broker- or relay-dependent room normally requires internet access. NFC is
only the bootstrap carrier and does not provide a network transport.

## Trust and lifecycle

System-delivered NDEF and URL/deep-link records are untrusted bootstrap input.
After structural validation, Envoix shows a redacted confirmation. Only an
explicit Continue action passes those external candidates to the existing Rust
parsers and starts the normal room flow.

A successful in-app foreground private-AID read is a separate,
proximity-triggered route. Once Core NFC dismisses its system sheet, Envoix
shows a compact confirmation stating that a nearby invitation was found.
Only **Continue** passes the structurally validated canonical URI to the normal
room flow. The private AID is public and proximity is not peer authentication.

NFC never bypasses role checks, expiry checks, SPAKE2 authentication, or the
user's normal room and transfer decisions. A room URI is only a room-control
bootstrap whose peer is authenticated later; the NFC endpoint and its routing
metadata are not trusted. Selecting the private AID proves only that an NFC
reader addressed that application identifier; it does not prove which app,
person, or device selected it.

The URI contains a short-lived secret, just like the matching QR code. Anyone
close enough to read the endpoint before expiry can attempt to use it. Android
host-card emulation keeps its NDEF bytes only in process memory, clears them
after one contiguous full read, after 120 seconds, when Connect is left, when
the room ends, or when the active invitation is replaced. It invalidates an
in-progress ISO-DEP read when the generation changes. Hiding the QR only
changes on-screen disclosure. A passive tag provisioned outside Envoix retains
stale bytes until it is overwritten.

## Platform behavior

### Android

Phone-to-phone NFC hosting requires Android 15 / API 35 or newer. Opening
Connect does not create a room or arm HCE. On API 35+, Envoix suppresses polling
while Connect is visible so an idle receiver cannot wake an iPhone Wallet
surface. Settings, Activity, backgrounding, and leaving Connect restore Android
defaults. **Share via NFC** creates or reuses a room, enters listen-only mode,
arms HCE, and starts one 120-second presenter lease. **Show QR/code** does not
arm NFC. The service registers the standard NDEF AID and private Envoix AID and
exposes the same read-only Type 4 NDEF message through either route. Android 14
and older cannot provide the same idle suppression or HCE safety guarantee, so
phone presentation fails closed while QR/code and explicit reader fallback
remain available.

Each touch has deliberately asymmetric roles. The presenter is listen-only
HCE. A receiver that observes one fresh readiness UUID may open one 12-second
`NFC_A` ReaderMode lease with the platform NDEF probe skipped, or the user may
select **Scan NFC** to open the same lease without BLE. Both readers select
only `F0454E564F495801`, parse the strict invitation, close immediately, and
show confirmation. A rotating or replayed BLE ID cannot keep reopening the
reader during one Connect activation. If both Android phones present, neither
scans; one must stop presenting.

BLE carries only a random 64-bit attempt ID in the service UUID
`d5f3a2d8-8f4a-4b34-<attempt-id>`. It carries no invitation, room code, name,
peer key, or authenticated identity. It is a nuisance-reduction doorbell, not
authorization. During the bounded Android ReaderMode lease, touching an iPhone
can still wake Wallet before Android identifies the peer; this residual
platform risk cannot be removed by an AID filter. Envoix never writes a passive
tag.

#### Xiaomi HyperOS limitation

On the inspected Xiaomi HyperOS 3 device, the privileged NFC implementation's
MiTouch handler consumes the standard `D2760000850101` SELECT and subsequent
Type 4 commands before Android dispatches them to Envoix. This occurs even
when Android resolves the AID to Envoix and Envoix is the preferred foreground
HCE service. Changing Envoix's NDEF parser cannot repair an APDU that never
reaches it.

The visible Xiaomi **Touch to share / 贴贴分享** switch controls Xiaomi Share
availability; it is not an HCE routing bypass. Turning it off does not make the
standard AID fall through to Envoix. The private AID is intended to avoid this
specific standard-AID collision by addressing Envoix without selecting the
intercepted AID, but routing on a particular firmware remains a physical test
gate. It does not restore inactive-iPhone background reading. A future Xiaomi
Tap Share SDK integration would be a separate vendor-specific route with its
own developer registration and client requirements.

### iOS

On iPhone XS and later, iOS supports background detection of a qualifying HTTPS
NDEF carrier while Envoix is not running. The display must be active and the
device must have been unlocked since restart. iOS shows a system NFC
notification; the user must tap it and unlock when requested. Subject to
Universal Link association and device policy, the link can then launch or
foreground Envoix and stage the normal confirmation. It is not a silent
transfer, and this standard route remains subject to the Android NFC stack
actually serving the standard NDEF AID.

Connect scans BLE but does not start Core NFC until it observes a fresh Android
presenter readiness generation bound to one recently seen Envoix BLE peer on
the same Core Bluetooth peripheral. Display names are never used for this
binding. iOS must be active on Connect, no room may be occupied, and no Envoix
sheet, scanner, alert, confirmation, or system pairing UI may be competing for
presentation. At most one automatic Apple sheet may appear per Connect
activation and per 60 seconds, even if an untrusted advertiser rotates IDs.
Cancellation, timeout, and failure do not reopen it. **Scan by NFC** starts one
manual pure-NFC attempt when BLE is unavailable or missed. Apple always
presents its system sheet before private-AID recognition. A successful read
still requires Envoix's redacted **Continue** confirmation; readiness never
auto-joins a room. Envoix does not write passive tags on iOS, and iOS does not
expose general third-party NDEF tag emulation. The phone-to-phone presenter
route is therefore one-way: Android may present an invitation to iPhone, but
iPhone cannot emulate the equivalent tag.

Presenting Apple's Core NFC system sheet may transition the SwiftUI scene from
`active` to `inactive` even though the application has not entered the
background. Envoix preserves the active `NFCTagReaderSession` across that
transition. Cancelling on `inactive` tears down ISO-DEP immediately after the
private AID is selected and produces a select-only Android trace. The session
is cancelled for an actual `background` transition or by its explicit terminal
paths. This lifecycle rule has a focused regression test.

### macOS

macOS does not expose NFC controls.

## Physical verification

The private-AID Android-to-iPhone route passed again on 2026-08-05 after fixing
the iOS scene-lifecycle regression. The Xiaomi trace completed application and
NDEF-file selection followed by two or three bounded reads and normal
deactivation, and the user confirmed that the room subsequently connected.
The complete handoff was still perceived as slow, so end-to-end latency remains
an open performance result rather than an NFC-read correctness failure. The
remaining standard-AID background and Android-to-Android routes still require
their separate physical gates. Record the device model, OS build, app build
marker, selected route, APDU trace shape, and result when executing those gates.

### Android listen-only and Wallet-safety gate

1. Use an Android 15 / API 35 or newer phone with NFC and HCE enabled. On Android
   14 or older, verify Envoix reports that phone-to-phone NFC requires Android
   15 and never arms an HCE invitation.
2. Record `adb shell dumpsys nfc` before opening Connect. On API 35+, verify
   Connect suppresses polling without arming an Envoix invitation. Leave
   Connect for Settings or Activity and verify Android defaults are restored.
3. Select **Share via NFC**. After the hosted invitation becomes available, in
   the NFC event log verify Envoix's foreground
   `discovery_technology_update` records `poll_tech: 0` while `listen_tech`
   remains present. Independently inspect NFC service/native logs for
   `NFA_ChangeDiscoveryTech` with `pollTech=0` and retained listen technology.
   Do not use the dump's top-level `pollTech`/`listenTech` values for this
   assertion: AOSP prints the saved defaults there, not the foreground override.
4. End waiting or otherwise clear the current invitation without pausing
   Envoix. Verify no discovery reset or nonzero poll update occurs and an iPhone
   can no longer read an invitation. This confirms the process-only HCE store
   cleared without a transient polling restart.
5. Host a fresh room, touch the iPhone's top edge to the Android NFC antenna,
   and verify Wallet, a payment chooser, or another contactless UI never appears
   on either phone as an Android-initiated side effect.
6. During the same touch, verify the intended iPhone reader still selects the
   standard or private Envoix AID and reads the HCE invitation successfully.
   Polling disabled must not disable Android listen/HCE.
7. Pause or destroy the Android Activity and inspect `adb shell dumpsys nfc`
   and the NFC service/native logs again. Verify a discovery reset restores
   Android's default technologies and the Envoix HCE invitation is no longer
   readable.
8. If the API 35 discovery call is absent, rejected, or only partially applied
   on an OEM build, verify Envoix reports listen-only hosting as unavailable,
   attempts a discovery reset, and leaves the HCE invitation unarmed.

### Standard AID: Android phone to inactive iPhone

1. Install the associated-domain iOS build while online, and install the HCE
   Android build on an Android 15+ device that passes the listen-only gate above
   and does not intercept the standard NDEF AID.
2. On Android, host a room. Leave its QR hidden and confirm that no
   tag-programming prompt appears.
3. Leave Envoix on iPhone or terminate it, keep the iPhone display active, and
   touch the iPhone's top edge to the Android NFC antenna.
4. Tap the iOS NFC notification. Confirm that Envoix opens to the untrusted
   invitation confirmation and does not join before **Continue**.
5. Background Envoix on Android or end the room and verify another tap no
   longer exposes the invitation. Bring the still-hosting activity to the
   foreground, select **Share via NFC** again, and verify only that fresh
   user action publishes another bounded lease.
6. After the association has been cached, disable internet access and repeat
   the NFC launch separately from any broker-dependent room-transfer test.

### Private AID: foreground iPhone fallback

1. Install the builds that register and select `F0454E564F495801`.
2. Host a room on Android and keep Envoix resumed.
3. Open Envoix to Connect on iPhone. Confirm no Apple sheet appears until the
   Android presenter advertises readiness after its normal Envoix BLE identity
   was observed. Present another Envoix sheet or alert during readiness and
   verify NFC waits only while that offer remains fresh. Cancel once and verify
   that the sheet does not reopen automatically; **Scan by NFC** starts one
   manual attempt.
4. Touch the iPhone's top edge to the Android NFC antenna. Verify the trace
   reports only the private application selection, NDEF-file selection,
   bounded-read shapes, and status words; it must not log APDU bodies,
   offsets, lengths, or invitation bytes.
5. Confirm that a successful private-AID read closes Apple's sheet and shows
   one Envoix **Continue** card before the room flow starts.
6. End the Android room or background the Android app, repeat the explicit
   scan, and verify that no invitation is returned.
7. Repeat the local NFC read with internet disabled, then separately test any
   broker- or relay-dependent room flow with its required network available.
8. With the iPhone app terminated or no foreground scan active, verify that the
   private AID does not produce a background launch or notification.
9. Separately open the HTTPS carrier or an `envoix://` deep link and verify that
   it retains the persistent **Continue** confirmation.

### Passive tags and rejection cases

Provision a passive NDEF test tag externally with the HTTPS wrapper. Base64url
expands the canonical invitation by about one third, so the tag must be large
enough to contain the complete record.

Verify Android handles the tag only when Android delivers a matching system NDEF
intent; Envoix itself must not start ReaderMode to poll for it. Verify iOS routes
a qualifying background scan through the associated Universal Link. Both
platforms must reject a non-Envoix URL, a wrong carrier origin or path, malformed
base64url, a prefix-only or oversized invitation, a text record, multiple
records, and an expired or role-conflicting canonical invitation. Envoix must
never modify the passive tag.
