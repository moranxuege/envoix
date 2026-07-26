// BN3b cross-language conformance suite (Kotlin side), run by
// `native_harnesses_replay_the_conformance_vectors` in
// tests/cross_language_conformance.rs.
//
// Same two directions as the Dart suite. Kotlin's generated frames are data
// classes, so `decode(vector) == expected` is a real structural comparison; the
// bytes are NOT byte-identical to the Rust reference codec (org.json decides
// key order and escaping), so agreement in the encode direction is asserted as
// JSON-value equality instead.
//
// Compiled and run against the reference json.org jar; Android's org.json
// differs in duplicate-key handling and key order, neither of which this
// contract depends on.

package com.envoix.bindings

import java.io.File
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

var checks = 0
val failures = mutableListOf<String>()

fun expect(label: String, ok: Boolean) {
    checks++
    if (!ok) failures.add(label)
}

fun <T> expectEq(label: String, actual: T, expected: T) {
    checks++
    if (actual != expected) failures.add("$label: got <$actual>, want <$expected>")
}

fun expectThrows(label: String, kind: Any, context: String, body: () -> Unit) {
    checks++
    try {
        body()
        failures.add("$label: expected ($kind, $context), nothing was thrown")
    } catch (error: Exception) {
        val actualKind = when (error) {
            is CommandContractException -> error.kind.name to error.context
            is ReadContractException -> error.kind.name to error.context
            is ProbeContractException -> error.kind.name to error.context
            else -> null
        }
        if (actualKind != ("$kind" to context)) {
            failures.add("$label: expected ($kind, $context), got $error")
        }
    }
}

fun expectEncodes(label: String, body: () -> Unit) {
    checks++
    try {
        body()
    } catch (error: Exception) {
        failures.add("$label: unexpected $error")
    }
}

/// JSON-value equality: the wire contract is the decoded value, not the byte
/// order org.json happens to choose.
fun jsonEquals(left: Any?, right: Any?): Boolean = when {
    left is JSONObject && right is JSONObject ->
        left.keySet() == right.keySet() &&
            left.keySet().all { jsonEquals(left.get(it), right.get(it)) }
    left is JSONArray && right is JSONArray ->
        left.length() == right.length() &&
            (0 until left.length()).all { jsonEquals(left.get(it), right.get(it)) }
    left is Number && right is Number -> left.toLong() == right.toLong()
    else -> left == right
}

fun parse(text: String): Any = JSONTokener(text).nextValue()

fun submit(card: String, epoch: Long, id: String, command: CommandView): FrontendIntentView =
    FrontendIntentView.Command(SubmitView(card, epoch, id, command))

fun create(requestId: String, intent: CreateIntentView): FrontendIntentView =
    FrontendIntentView.Create(CreateView(intent, requestId))

fun createResult(outcome: CreateOutcomeView) =
    CommandFrame(
        body = CommandBody.CreateResult(
            CreateResultView(outcome, "11111111111111111111111111111111"),
        ),
    )

fun acceptance(value: AcceptanceView) =
    CommandFrame(
        body = CommandBody.Acceptance(
            CommandAcceptanceView("11111111111111111111111111111111", value),
        ),
    )

fun completion(value: CompletionView) =
    CommandFrame(
        body = CommandBody.Completion(
            CommandCompletionView("22222222222222222222222222222222", value),
        ),
    )

fun dispositions(): Map<String, DispositionView> = linkedMapOf(
    "preparing" to DispositionView.Preparing,
    "waiting" to DispositionView.Waiting,
    "connecting" to DispositionView.Connecting,
    "verifying" to DispositionView.Verifying,
    "transferring" to DispositionView.Transferring,
    "confirming" to DispositionView.Confirming,
    "paused_local" to DispositionView.Paused(PausedStateView(PauseCauseView.LOCAL)),
    "paused_peer" to DispositionView.Paused(PausedStateView(PauseCauseView.PEER)),
    "paused_lost" to DispositionView.Paused(PausedStateView(PauseCauseView.LOST)),
    "unconfirmed" to DispositionView.Unconfirmed,
    "completed" to DispositionView.Completed,
    "failed" to DispositionView.Failed,
    "cancelled" to DispositionView.Cancelled,
)

/// The command vocabulary by vector name: submitted, and named back by a
/// conflict.
val commandViews = linkedMapOf(
    "pause" to CommandView.PAUSE,
    "cancel" to CommandView.CANCEL,
    "resume" to CommandView.RESUME,
    "remove" to CommandView.REMOVE,
    "re_pick_source" to CommandView.RE_PICK_SOURCE,
)

/// Every body a frontend may originate, by vector name.
fun handBuiltSubmits(): Map<String, FrontendIntentView> {
    val card = "00000000000000ab"
    val id = "000102030405060708090a0b0c0d0e0f"
    val bodies = linkedMapOf<String, FrontendIntentView>()

    expectEq("every CommandView variant is swept", commandViews.size, CommandView.entries.size)
    commandViews.forEach { (name, command) -> bodies["submit_$name"] = submit(card, 7, id, command) }

    val epochs = linkedMapOf(
        "zero" to 0L,
        "one" to 1L,
        "two_pow_53" to 9007199254740992L,
        "u63_max" to Long.MAX_VALUE,
    )
    epochs.forEach { (name, epoch) ->
        bodies["submit_epoch_$name"] = submit(card, epoch, id, CommandView.PAUSE)
    }
    bodies["submit_ids_min"] = submit("0".repeat(16), 1, "0".repeat(32), CommandView.CANCEL)
    bodies["submit_ids_max"] = submit("f".repeat(16), 1, "f".repeat(32), CommandView.CANCEL)

    bodies["create_send_narrowest"] =
        create(id, CreateIntentView.Send(SendSourceView("", 0)))
    bodies["create_send_widest"] =
        create("f".repeat(32), CreateIntentView.Send(SendSourceView("世".repeat(84) + "x", Long.MAX_VALUE)))
    val invites = linkedMapOf(
        "empty" to "",
        "canonical" to "envoix://invite/v3/eyJ2ZXJzaW9uIjozfQ",
        "bidirectional" to "\u202eenvoix://invite",
        "at_bound" to "e".repeat(16384),
    )
    invites.forEach { (name, invite) ->
        bodies["create_join_$name"] = create(id, CreateIntentView.Join(JoinInviteView(CommandSecretString(invite))))
    }
    return bodies
}

/// Every frame on the contract, originable or not, for the decode direction.
fun handBuiltCommands(): Map<String, CommandFrame> {
    val frames = linkedMapOf<String, CommandFrame>()
    handBuiltSubmits().forEach { (name, body) ->
        frames[name] = CommandFrame(body = CommandBody.Intent(body))
    }
    frames["acceptance_accepted"] = acceptance(AcceptanceView.Accepted)
    val rejections = linkedMapOf(
        "unknown_card" to RejectionView.UNKNOWN_CARD,
        "stale_epoch" to RejectionView.STALE_EPOCH,
        "superseded" to RejectionView.SUPERSEDED,
        "at_capacity" to RejectionView.AT_CAPACITY,
        "runtime_stopped" to RejectionView.RUNTIME_STOPPED,
        "interrupted" to RejectionView.INTERRUPTED,
        "internal" to RejectionView.INTERNAL,
    )
    expectEq("every RejectionView variant is swept", rejections.size, RejectionView.entries.size)
    rejections.forEach { (name, rejection) ->
        frames["acceptance_rejected_$name"] = acceptance(AcceptanceView.Rejected(rejection))
    }
    commandViews.forEach { (name, command) ->
        frames["acceptance_conflict_$name"] = acceptance(AcceptanceView.Conflict(command))
    }
    dispositions().forEach { (name, disposition) ->
        frames["acceptance_duplicate_$name"] = acceptance(AcceptanceView.Duplicate(disposition))
        frames["completion_committed_$name"] = completion(CompletionView.Committed(disposition))
    }
    frames["completion_commit_failed_paused_lost"] = completion(
        CompletionView.CommitFailed(DispositionView.Paused(PausedStateView(PauseCauseView.LOST))),
    )
    frames["completion_interrupted"] = completion(CompletionView.Interrupted)
    frames["completion_internal"] = completion(CompletionView.Internal)
    val refusals = CreateRefusalView.entries
    refusals.forEachIndexed { index, refusal ->
        frames["create_refused_$index"] = createResult(CreateOutcomeView.Refused(refusal))
    }
    frames["create_created"] =
        createResult(CreateOutcomeView.Created(CardCreatedView("00000000000000ab")))
    return frames
}

fun frontendToBackend(vectors: Map<String, String>, originable: Set<String>) {
    val bodies = handBuiltSubmits()
    expectEq(
        "the Kotlin suite covers every originable command vector",
        bodies.keys.sorted().joinToString(","),
        originable.sorted().joinToString(","),
    )
    bodies.forEach { (name, body) ->
        val vector = vectors[name]
        if (vector == null) {
            failures.add("$name: no exported vector")
            return@forEach
        }
        val encoded = EnvoixCommandCodec.encode(body)
        expect(
            "$name encodes to the reference value (got $encoded)",
            jsonEquals(parse(encoded), parse(vector)),
        )
        // Data-class equality: the decoded frame IS the frame that was built.
        expectEq(
            "$name round-trips through its own bytes",
            EnvoixCommandCodec.decode(encoded),
            CommandFrame(CommandBody.Intent(body)),
        )
        val roundTripped = EnvoixCommandCodec.decode(vector).body
        expect("$name decodes to a frontend intent", roundTripped is CommandBody.Intent)
        expect(
            "$name re-encodes to the same value after a round trip",
            jsonEquals(
                parse(EnvoixCommandCodec.encode((roundTripped as CommandBody.Intent).value)),
                parse(vector),
            ),
        )
    }
}

fun backendToFrontend(command: Map<String, String>, read: Map<String, String>) {
    // The whole contract, decoded and compared structurally: every command,
    // acceptance, rejection, disposition, and completion arm.
    val expected = handBuiltCommands()
    expectEq(
        "the Kotlin suite covers every exported command vector",
        expected.keys.sorted().joinToString(","),
        command.keys.sorted().joinToString(","),
    )
    expected.forEach { (name, frame) ->
        expectEq("$name decodes to the intended value", EnvoixCommandCodec.decode(command[name]!!), frame)
    }

    fun epochOf(name: String): Long =
        (
            ((EnvoixCommandCodec.decode(command[name]!!).body) as CommandBody.Intent).value
                as FrontendIntentView.Command
        ).value.epoch
    expectEq("epoch 2^53 survives", epochOf("submit_epoch_two_pow_53"), 9007199254740992L)
    expectEq("epoch 2^63-1 survives", epochOf("submit_epoch_u63_max"), Long.MAX_VALUE)
    expectEq("epoch zero survives", epochOf("submit_epoch_zero"), 0L)

    val widest = EnvoixReadCodec.decode(read["read_card_update_widest"]!!).body
    expect("the widest read frame is a card update", widest is ReadBody.CardUpdate)
    val update = (widest as ReadBody.CardUpdate).value
    expectEq("read epoch at u63 max", update.epoch, Long.MAX_VALUE)
    val card = (update.kind as CardUpdateKindView.Snapshot).value
    expectEq("u32 max generation", card.generation, 4294967295L)
    expectEq("total at u63 max", card.total, Long.MAX_VALUE)
    expectEq("bytes at 2^53", card.bytes, 9007199254740992L)
    expectEq("bytes_resumed above 2^53", card.bytesResumed, 9007199254740993L)
    expectEq(
        "multi-byte name survives",
        card.offeredName,
        "\uD83D\uDC4D\uD83C\uDFFD \u0645\u0631\u062D\u0628\u0627 e\u0301 \uD83C\uDDFA\uD83C\uDDF3.pdf",
    )
    expectEq(
        "the nested pause origin decodes",
        (card.state as ProductStateView.Paused).value.origin,
        PauseOriginView.LOST,
    )
    val outcome = card.outcome!!
    expectEq(
        "text at exactly the 160-byte bound",
        outcome.display.toByteArray(Charsets.UTF_8).size,
        160,
    )
    expectEq("optional recovery present", outcome.recovery, RecoveryView.RECONNECT_PEER)

    val narrowest = EnvoixReadCodec.decode(read["read_card_update_narrowest"]!!).body
    val progress = ((narrowest as ReadBody.CardUpdate).value.kind as CardUpdateKindView.Progress).value
    expectEq("an empty string decodes as empty", progress.offeredName, "")
    expectEq("an explicit null optional decodes as absent", progress.outcome, null)

    val empty = (EnvoixReadCodec.decode(read["read_evidence_empty"]!!).body as ReadBody.Evidence).value
    expectEq("an empty list decodes as empty", empty.entries.size, 0)
    expectEq("u32 max session generation", empty.session.generation, 4294967295L)
    val entries = (EnvoixReadCodec.decode(read["read_evidence_entries"]!!).body as ReadBody.Evidence).value
    expectEq("list elements decode", entries.entries.size, 3)
    expectEq("a sequence above 2^53 survives", entries.entries[2].sequence, Long.MAX_VALUE)

    val manifest = (EnvoixReadCodec.decode(read["read_build_manifest"]!!).body as ReadBody.BuildManifest).value
    expectEq("u16 max wire version", manifest.protocol.dataWireVersion, 65535L)
    expectEq("variable-length hex decodes", manifest.protocol.dataMagic, "cafebabe")
    expectEq(
        "a 64-character fingerprint decodes",
        (manifest.trustRoot as TrustRootView.Sha256).value.fingerprint.length,
        64,
    )
    for (name in read.keys) {
        expectEncodes("$name decodes") { EnvoixReadCodec.decode(read[name]!!) }
    }
}

fun encoderHonesty() {
    val card = "00000000000000ab"
    val id = "000102030405060708090a0b0c0d0e0f"
    expectThrows("a negative epoch is rejected", CommandErrorKind.RANGE, "SubmitView.epoch") {
        EnvoixCommandCodec.encode(submit(card, -1, id, CommandView.PAUSE))
    }
    // Long.MAX_VALUE IS the u63 bound, so 2^63 is unrepresentable rather than
    // merely checked: the type carries the contract.
    expectEq("the u63 bound is Long.MAX_VALUE", Long.MAX_VALUE, 9223372036854775807L)

    val badCards = linkedMapOf(
        "uppercase" to "00000000000000AB",
        "too short" to "00000000000000a",
        "too long" to "00000000000000abc",
        "empty" to "",
        "non-hex" to "00000000000000ag",
        "unpaired surrogate" to "00000000000000\uD800",
    )
    badCards.forEach { (label, value) ->
        expectThrows("a card that is $label is rejected", CommandErrorKind.BOUND, "SubmitView.card") {
            EnvoixCommandCodec.encode(submit(value, 1, id, CommandView.PAUSE))
        }
    }
    val badIds = linkedMapOf(
        "too short" to "000102030405060708090a0b0c0d0e0",
        "too long" to "000102030405060708090a0b0c0d0e0ff",
        "uppercase" to "000102030405060708090A0B0C0D0E0F",
        "empty" to "",
    )
    badIds.forEach { (label, value) ->
        expectThrows("a command id that is $label is rejected", CommandErrorKind.BOUND, "SubmitView.command_id") {
            EnvoixCommandCodec.encode(submit(card, 1, value, CommandView.PAUSE))
        }
    }
    val stamped = parse(EnvoixCommandCodec.encode(submit(card, 1, id, CommandView.PAUSE))) as JSONObject
    expectEq("the encoder stamps the schema envelope", stamped.getString("schema"), COMMAND_SCHEMA_ID)
    expectEq("the encoder stamps the intent arm", stamped.getJSONObject("body").getString("kind"), "intent")
}

/// 2-, 3-, and 4-byte characters: 9 bytes, 4 UTF-16 units per group.
const val WIDE_GROUP = "\u00E9\u4E2D\uD83D\uDE00"

/// U+0085 is two UTF-8 bytes canonically and six once org.json escapes it.
const val INFLATED = "\u0085"

fun probeScalars(
    small: Long = 0,
    medium: Long = 0,
    large: Long = 0,
    shortId: String = "0".repeat(16),
    longId: String = "0".repeat(32),
    digest: String = "0".repeat(64),
    blobby: String = "ab",
    text: String = "",
    label: String = "",
    maybe: ProbeTone? = null,
    maybeText: String? = null,
    leaves: List<ProbeLeaf> = emptyList(),
    choice: ProbeChoice = ProbeChoice.Nothing,
) = ProbeScalars(
    small, medium, large, shortId, longId, digest, blobby, text, label, maybe, maybeText, leaves,
    choice,
)

fun decodedScalars(text: String): ProbeScalars =
    (EnvoixProbeCodec.decode(text).body as ProbeBody.Scalars).value

fun probeSurface() {
    expectEncodes("a minimal probe frame encodes") { EnvoixProbeCodec.encode(probeScalars()) }

    // Every field is in bounds and the frame is still over the cap: it fails
    // typed instead of leaving the process oversized. The text fields are
    // multi-byte, so the frame clears the cap in UTF-16 units and breaks it in
    // UTF-8 bytes — counting the wrong unit accepts this frame.
    val over = probeScalars(
        small = 65535,
        medium = 4294967295L,
        large = Long.MAX_VALUE,
        shortId = "f".repeat(16),
        longId = "f".repeat(32),
        digest = "f".repeat(64),
        blobby = "aabbccdd",
        text = WIDE_GROUP.repeat(5),
        label = "y".repeat(8),
        maybe = ProbeTone.LOUD,
        maybeText = WIDE_GROUP.repeat(5),
        leaves = listOf(
            ProbeLeaf(ProbeTone.CALM),
            ProbeLeaf(ProbeTone.LOUD),
            ProbeLeaf(ProbeTone.CALM),
            ProbeLeaf(ProbeTone.LOUD),
        ),
        choice = ProbeChoice.Leaf(ProbeLeaf(ProbeTone.LOUD)),
    )
    expectThrows(
        "an over-cap multi-byte frame is rejected",
        ProbeErrorKind.FRAME_TOO_LARGE,
        "ProbeFrame",
    ) { EnvoixProbeCodec.encode(over) }

    // Recorded divergence, stated in the artifact header: the frame cap is
    // defined over the canonical serde serialization, and org.json escapes
    // U+0080..U+009F and U+2000..U+20FF as \uXXXX. A frame the contract permits
    // can therefore exceed the cap once this artifact renders it — Kotlin is
    // strictly tighter, never looser, so it is an interop limit and not a
    // smuggling path.
    expect(
        "org.json escapes U+0085 threefold",
        JSONObject().put("k", INFLATED).toString().length ==
            JSONObject().put("k", "x").toString().length + 5,
    )
    val inflating = INFLATED.repeat(22) + "x"
    expectEq("the inflating text is 45 canonical bytes", inflating.toByteArray(Charsets.UTF_8).size, 45)
    expectThrows(
        "org.json escaping makes this artifact's cap strictly tighter",
        ProbeErrorKind.FRAME_TOO_LARGE,
        "ProbeFrame",
    ) { EnvoixProbeCodec.encode(probeScalars(text = inflating, maybeText = inflating)) }

    val rich = probeScalars(
        small = 65535,
        medium = 4294967295L,
        large = Long.MAX_VALUE,
        blobby = "aabbccdd",
        text = "\uD83D\uDE00".repeat(4),
        label = "y".repeat(8),
        maybe = ProbeTone.LOUD,
        leaves = listOf(ProbeLeaf(ProbeTone.CALM)),
        choice = ProbeChoice.Leaf(ProbeLeaf(ProbeTone.LOUD)),
    )
    val encoded = EnvoixProbeCodec.encode(rich)
    expectEq("a rich probe frame round-trips", decodedScalars(encoded), rich)
    expect(
        "a rich probe frame re-encodes to the same value",
        jsonEquals(parse(EnvoixProbeCodec.encode(decodedScalars(encoded))), parse(encoded)),
    )

    expectEncodes("u16 max encodes") { EnvoixProbeCodec.encode(probeScalars(small = 65535)) }
    expectThrows("u16 overflow is rejected", ProbeErrorKind.RANGE, "ProbeScalars.small") {
        EnvoixProbeCodec.encode(probeScalars(small = 65536))
    }
    expectEncodes("u32 max encodes") { EnvoixProbeCodec.encode(probeScalars(medium = 4294967295L)) }
    expectThrows("u32 overflow is rejected", ProbeErrorKind.RANGE, "ProbeScalars.medium") {
        EnvoixProbeCodec.encode(probeScalars(medium = 4294967296L))
    }
    expectThrows("a negative u16 is rejected", ProbeErrorKind.RANGE, "ProbeScalars.small") {
        EnvoixProbeCodec.encode(probeScalars(small = -1))
    }
    expectThrows("a negative u63 is rejected", ProbeErrorKind.RANGE, "ProbeScalars.large") {
        EnvoixProbeCodec.encode(probeScalars(large = -1))
    }

    expectThrows("an uppercase digest is rejected", ProbeErrorKind.BOUND, "ProbeScalars.digest") {
        EnvoixProbeCodec.encode(probeScalars(digest = "A".repeat(64)))
    }
    expectThrows("a short digest is rejected", ProbeErrorKind.BOUND, "ProbeScalars.digest") {
        EnvoixProbeCodec.encode(probeScalars(digest = "a".repeat(63)))
    }

    expectEncodes("minimal variable hex encodes") { EnvoixProbeCodec.encode(probeScalars(blobby = "ab")) }
    listOf(
        "" to "empty",
        "abc" to "odd-length",
        "aabbccddee" to "over-long",
        "AABB" to "uppercase",
        "zzzz" to "non-hex",
    )
        .forEach { (value, label) ->
            expectThrows("$label variable hex is rejected", ProbeErrorKind.BOUND, "ProbeScalars.blobby") {
                EnvoixProbeCodec.encode(probeScalars(blobby = value))
            }
        }

    expectEncodes("an empty string encodes") { EnvoixProbeCodec.encode(probeScalars(text = "")) }
    expectEncodes("45 ascii bytes at the bound encode") {
        EnvoixProbeCodec.encode(probeScalars(text = "x".repeat(45)))
    }
    expectEncodes("45 multi-byte bytes at the bound encode") {
        EnvoixProbeCodec.encode(probeScalars(text = WIDE_GROUP.repeat(5)))
    }
    expectThrows("one byte over the bound is rejected", ProbeErrorKind.BOUND, "ProbeScalars.text") {
        EnvoixProbeCodec.encode(probeScalars(text = "x".repeat(46)))
    }
    expectThrows(
        "one multi-byte character over the bound is rejected",
        ProbeErrorKind.BOUND,
        "ProbeScalars.text",
    ) { EnvoixProbeCodec.encode(probeScalars(text = WIDE_GROUP.repeat(5) + "x")) }
    expectEncodes("right-to-left text encodes") {
        EnvoixProbeCodec.encode(probeScalars(text = "\u0645\u0631\u062D\u0628\u0627"))
    }
    expectEncodes("a combining mark encodes") {
        EnvoixProbeCodec.encode(probeScalars(text = "e\u0301"))
    }
    expectThrows("a lone high surrogate is rejected", ProbeErrorKind.SHAPE, "ProbeScalars.text") {
        EnvoixProbeCodec.encode(probeScalars(text = "a\uD800"))
    }
    expectThrows("a lone low surrogate is rejected", ProbeErrorKind.SHAPE, "ProbeScalars.text") {
        EnvoixProbeCodec.encode(probeScalars(text = "a\uDC00"))
    }
    expectThrows("a reversed surrogate pair is rejected", ProbeErrorKind.SHAPE, "ProbeScalars.text") {
        EnvoixProbeCodec.encode(probeScalars(text = "\uDC00\uD800"))
    }
    expectThrows(
        "a lone surrogate in an optional string is rejected",
        ProbeErrorKind.SHAPE,
        "ProbeScalars.maybe_text",
    ) {
        EnvoixProbeCodec.encode(probeScalars(maybeText = "\uD800"))
    }

    expectEncodes("empty ascii encodes") { EnvoixProbeCodec.encode(probeScalars(label = "")) }
    expectEncodes("ascii at the bound encodes") { EnvoixProbeCodec.encode(probeScalars(label = "y".repeat(8))) }
    expectThrows("over-long ascii is rejected", ProbeErrorKind.BOUND, "ProbeScalars.label") {
        EnvoixProbeCodec.encode(probeScalars(label = "y".repeat(9)))
    }
    expectThrows("a control character is rejected", ProbeErrorKind.BOUND, "ProbeScalars.label") {
        EnvoixProbeCodec.encode(probeScalars(label = "a"))
    }
    expectThrows("delete is rejected", ProbeErrorKind.BOUND, "ProbeScalars.label") {
        EnvoixProbeCodec.encode(probeScalars(label = "a"))
    }
    expectThrows("non-ascii text in an ascii field is rejected", ProbeErrorKind.BOUND, "ProbeScalars.label") {
        EnvoixProbeCodec.encode(probeScalars(label = "\u00E9"))
    }

    expectEncodes("a list at its cap encodes") {
        EnvoixProbeCodec.encode(probeScalars(leaves = List(4) { ProbeLeaf(ProbeTone.CALM) }))
    }
    expectThrows("one element over the cap is rejected", ProbeErrorKind.BOUND, "ProbeScalars.leaves") {
        EnvoixProbeCodec.encode(probeScalars(leaves = List(5) { ProbeLeaf(ProbeTone.CALM) }))
    }

    val base = parse(EnvoixProbeCodec.encode(probeScalars())) as JSONObject
    fun scalarsOf(map: JSONObject): JSONObject = map.getJSONObject("body").getJSONObject("value")

    // The cap is enforced on the way in on the same unit: bytes the encoder
    // refuses are refused again when they arrive, though they are under the cap
    // in UTF-16 units.
    expectThrows(
        "an over-cap multi-byte frame is rejected on decode",
        ProbeErrorKind.FRAME_TOO_LARGE,
        "ProbeFrame",
    ) {
        val map = parse(base.toString()) as JSONObject
        val scalars = scalarsOf(map)
        scalars.put("small", 65535)
        scalars.put("medium", 4294967295L)
        scalars.put("large", Long.MAX_VALUE)
        scalars.put("short_id", "f".repeat(16))
        scalars.put("long_id", "f".repeat(32))
        scalars.put("digest", "f".repeat(64))
        scalars.put("blobby", "aabbccdd")
        scalars.put("text", WIDE_GROUP.repeat(5))
        scalars.put("label", "y".repeat(8))
        scalars.put("maybe", "loud")
        scalars.put("maybe_text", WIDE_GROUP.repeat(5))
        scalars.put(
            "leaves",
            JSONArray(List(4) { JSONObject().put("tone", "calm") }),
        )
        scalars.put(
            "choice",
            JSONObject().put("kind", "leaf").put("value", JSONObject().put("tone", "loud")),
        )
        val text = map.toString()
        expect(
            "the over-cap frame is under the cap in UTF-16 units",
            text.length <= PROBE_MAX_FRAME_BYTES &&
                text.toByteArray(Charsets.UTF_8).size > PROBE_MAX_FRAME_BYTES,
        )
        EnvoixProbeCodec.decode(text)
    }

    expectThrows("an unknown field is rejected on decode", ProbeErrorKind.UNKNOWN_FIELD, "ProbeBody.scalars") {
        val map = parse(base.toString()) as JSONObject
        scalarsOf(map).put("smuggled", 1)
        EnvoixProbeCodec.decode(map.toString())
    }
    expectThrows("an unknown union arm is rejected on decode", ProbeErrorKind.UNKNOWN_VARIANT, "ProbeFrame.body") {
        val map = parse(base.toString()) as JSONObject
        map.getJSONObject("body").put("kind", "shell")
        EnvoixProbeCodec.decode(map.toString())
    }
    expectThrows("a wrong schema envelope is rejected on decode", ProbeErrorKind.UNKNOWN_SCHEMA, "ProbeFrame") {
        val map = parse(base.toString()) as JSONObject
        map.put("schema", "envoix/binding/probe/2")
        EnvoixProbeCodec.decode(map.toString())
    }
    // Absent is not the same as explicitly null: an optional key must be
    // present and may be null, so a frame that simply omits it is malformed
    // rather than silently defaulted.
    expectThrows("an absent optional key is rejected on decode", ProbeErrorKind.SHAPE, "ProbeScalars.maybe") {
        val map = parse(base.toString()) as JSONObject
        scalarsOf(map).remove("maybe")
        EnvoixProbeCodec.decode(map.toString())
    }
    // Long.MAX_VALUE is the u63 bound, so the encoder cannot produce 2^63, but
    // a decoder can be handed one: org.json widens it past Long.
    expectThrows("a raw integer above u63 is rejected on decode", ProbeErrorKind.SHAPE, "ProbeScalars.large") {
        EnvoixProbeCodec.decode(base.toString().replaceFirst("\"large\":0", "\"large\":9223372036854775808"))
    }
}

fun main(args: Array<String>) {
    val bundle = JSONObject(File(args[0]).readText())
    val commandArray = bundle.getJSONArray("command")
    val command = (0 until commandArray.length()).associate { index ->
        val entry = commandArray.getJSONObject(index)
        entry.getString("name") to entry.getString("frame")
    }
    val originable = (0 until commandArray.length())
        .map { commandArray.getJSONObject(it) }
        .filter { it.getBoolean("originable") }
        .map { it.getString("name") }
        .toSet()
    val readArray = bundle.getJSONArray("read")
    val read = (0 until readArray.length()).associate { index ->
        val entry = readArray.getJSONObject(index)
        entry.getString("name") to entry.getString("frame")
    }

    frontendToBackend(command, originable)
    backendToFrontend(command, read)
    encoderHonesty()
    probeSurface()

    println("command vectors: ${command.size}")
    println("originable vectors: ${originable.size}")
    println("read vectors: ${read.size}")
    println("checks: $checks")
    if (failures.isEmpty()) {
        println("RESULT: all checks passed")
        return
    }
    failures.forEach { println("FAIL: $it") }
    println("RESULT: ${failures.size} failed")
    kotlin.system.exitProcess(1)
}
