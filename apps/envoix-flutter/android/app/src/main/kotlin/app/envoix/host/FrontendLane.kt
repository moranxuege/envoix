package app.envoix.host

import android.os.Handler
import android.os.Looper
import com.envoix.bindings.capability.CapabilityBody
import com.envoix.bindings.capability.CapabilityExchangeView
import com.envoix.bindings.capability.CapabilitySecretString
import com.envoix.bindings.capability.DeclinedReasonView
import com.envoix.bindings.capability.DeclinedView
import com.envoix.bindings.capability.EnvoixCapabilityCodec
import com.envoix.bindings.capability.PickSourceExchangeView
import com.envoix.bindings.capability.PickSourceFailureReasonView
import com.envoix.bindings.capability.PickSourceFailureView
import com.envoix.bindings.capability.PickSourceStepView
import com.envoix.bindings.capability.PickedSourceView
import com.envoix.bindings.capability.ScanInviteExchangeView
import com.envoix.bindings.capability.ScanInviteStepView
import com.envoix.bindings.capability.ScannedTextView
import com.envoix.bindings.capability.SourceAcquisitionKeyView
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.Executors
import kotlin.concurrent.thread

/**
 * The Dart lane onto the running host: encoded contract frames, and nothing
 * else.
 *
 * The stream subscription IS the attachment. Listening opens a fresh
 * attachment (every known card's stream restarts at a new epoch, opening with
 * the snapshot the contract promises); cancelling — or the engine dying — only
 * stops this pump. There is no detach call, so a frontend going away is not
 * something a transfer can observe (Pillar 7).
 *
 * Every attachment carries its own token and its own [Pump], because Flutter
 * re-subscribes by calling `onCancel` and `onListen` microseconds apart on one
 * thread: a stop flag shared by every pump the instance ever started cannot say
 * WHICH pump was meant. The host settles it regardless — a superseded token
 * consumes nothing — so the flag here only ends the thread promptly.
 *
 * The other direction is two methods on [COMMAND_CHANNEL]. [INTENT] hands an
 * intent frame to the host and returns the host's encoded answer; it is not a
 * transfer verb — the host decides what the intent does, and a committed
 * completion arrives later on the frame lane above. [CAPABILITY] carries a
 * capability exchange between the frontend and THIS adapter, which the Rust
 * authority never sees.
 *
 * The document picker used to be a third method with an untyped map for its
 * answer (`displayName`/`sizeBytes`, absent collapsing to `""` and `0`). It is
 * a capability arm now, so the pick names the acquisition it is for and an
 * unknown size stays unknown. The URI itself is still never part of any reply:
 * it lives in [SourcePicks] under that acquisition, so a frontend can describe
 * a chosen file without ever holding one.
 *
 * Apart from that one contract, Kotlin never looks inside a frame: it moves
 * `ByteArray`s between the JNI lane and the message channels, and the generated
 * Dart codec is the only thing that decodes them.
 */
class FrontendLane(
    messenger: BinaryMessenger,
    private val pickSource: (SourceAcquisitionKeyView) -> Unit,
    private val scanInvite: () -> Unit,
) : EventChannel.StreamHandler {
    private val channel = EventChannel(messenger, CHANNEL)
    private val commands = MethodChannel(messenger, COMMAND_CHANNEL)
    private val main = Handler(Looper.getMainLooper())

    /**
     * Intents run off the platform thread: the host blocks on the runtime to
     * resolve an answer, and blocking the platform thread for that is an ANR.
     * One thread, so a burst of taps queues instead of spawning a thread each;
     * the reply is posted back where a `MethodChannel` result belongs.
     */
    private val intents = Executors.newSingleThreadExecutor()

    /** The pump the current attachment owns; every earlier one is superseded. */
    private var current: Pump? = null

    init {
        channel.setStreamHandler(this)
        commands.setMethodCallHandler(::onCommand)
    }

    /** The pick in flight, answered when the Activity reports its result. */
    private var picking: MethodChannel.Result? = null

    /** The capability exchange in flight, answered by the Activity's result. */
    private var scanning: MethodChannel.Result? = null

    private fun onCommand(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        when (call.method) {
            INTENT -> onIntent(call, result)
            CAPABILITY -> onCapability(call, result)
            else -> result.notImplemented()
        }
    }

    /** Which acquisition the open pick is for, so its answer can name it. */
    private var pickingFor: SourceAcquisitionKeyView? = null

    /**
     * The capability seam, and the ONE place this class decodes a frame.
     *
     * Everything else here moves opaque `ByteArray`s, because the host owns
     * those contracts. This one it must speak: a frontend and its platform
     * adapter are the contract's two peers, and this side is the adapter. It is
     * the same generated codec a Swift adapter would use, which is exactly why
     * an Apple frontend owes this file nothing.
     */
    private fun onCapability(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        val frame = call.arguments as? ByteArray
        if (frame == null) {
            result.error(NOT_A_FRAME, "capability takes an encoded capability frame", null)
            return
        }
        val request =
            try {
                EnvoixCapabilityCodec.decode(String(frame, Charsets.UTF_8))
            } catch (malformed: Exception) {
                result.error(NOT_A_FRAME, malformed.message ?: "not a capability frame", null)
                return
            }
        when (val exchange = (request.body as CapabilityBody.Exchange).value) {
            is CapabilityExchangeView.ScanInvite -> {
                if (exchange.value.step !is ScanInviteStepView.Requested) {
                    // The adapter's own half echoed back is not a question.
                    result.error(NOT_A_FRAME, "a capability request was expected", null)
                    return
                }
                if (scanning != null) {
                    result.error(SCAN_IN_FLIGHT, "a scan is already open", null)
                    return
                }
                scanning = result
                scanInvite()
            }
            is CapabilityExchangeView.PickSource -> {
                if (exchange.value.step !is PickSourceStepView.Requested) {
                    result.error(NOT_A_FRAME, "a capability request was expected", null)
                    return
                }
                // One at a time: a second request while one is open would leave
                // the first `Result` unanswered, which a `MethodChannel` treats
                // as a leak. The refusal is an ANSWER on the contract rather
                // than a channel error — a frontend that got `unavailable` here
                // would read a busy picker as a missing adapter.
                if (picking != null) {
                    result.success(
                        EnvoixCapabilityCodec
                            .encode(
                                CapabilityExchangeView.PickSource(
                                    PickSourceExchangeView(
                                        acquisition = exchange.value.acquisition,
                                        step =
                                            PickSourceStepView.Failed(
                                                PickSourceFailureReasonView(
                                                    PickSourceFailureView.INTERNAL,
                                                ),
                                            ),
                                    ),
                                ),
                            ).toByteArray(Charsets.UTF_8),
                    )
                    return
                }
                picking = result
                pickingFor = exchange.value.acquisition
                pickSource(exchange.value.acquisition)
            }
        }
    }

    /**
     * The Activity's answer for the pick in flight: the sanitized metadata for
     * the chosen document, or which decline it was. Never a URI, and always
     * naming the acquisition the request named — a frontend that receives an
     * answer for a different one refuses it before building an offer.
     */
    fun sourcePicked(granted: SourcePicks.Granted?) {
        val result = picking ?: return
        val acquisition = pickingFor ?: return
        picking = null
        pickingFor = null
        val step =
            when (granted) {
                null ->
                    PickSourceStepView.Declined(DeclinedReasonView(DeclinedView.CANCELLED))
                else ->
                    PickSourceStepView.Provided(
                        PickedSourceView(
                            displayName = granted.displayName,
                            reportedSize = granted.sizeBytes,
                        ),
                    )
            }
        val answer =
            EnvoixCapabilityCodec.encode(
                CapabilityExchangeView.PickSource(
                    PickSourceExchangeView(acquisition = acquisition, step = step),
                ),
            )
        result.success(answer.toByteArray(Charsets.UTF_8))
    }

    /** The picker itself could not run — not a person declining it. */
    fun sourcePickFailed(reason: PickSourceFailureView) {
        val result = picking ?: return
        val acquisition = pickingFor ?: return
        picking = null
        pickingFor = null
        val answer =
            EnvoixCapabilityCodec.encode(
                CapabilityExchangeView.PickSource(
                    PickSourceExchangeView(
                        acquisition = acquisition,
                        step = PickSourceStepView.Failed(PickSourceFailureReasonView(reason)),
                    ),
                ),
            )
        result.success(answer.toByteArray(Charsets.UTF_8))
    }

    /**
     * The Activity's answer: the text a camera read, or which decline it was.
     * Both are ANSWERS on the contract; neither is a platform error.
     */
    fun scanned(
        text: String?,
        declined: String?,
    ) {
        val result = scanning ?: return
        scanning = null
        val step =
            when {
                text != null ->
                    ScanInviteStepView.Provided(ScannedTextView(CapabilitySecretString(text)))
                else ->
                    ScanInviteStepView.Declined(
                        DeclinedReasonView(
                            when (declined) {
                                ScanActivity.DECLINED_REFUSED -> DeclinedView.REFUSED
                                ScanActivity.DECLINED_UNSUPPORTED -> DeclinedView.UNSUPPORTED
                                else -> DeclinedView.CANCELLED
                            },
                        ),
                    )
            }
        val answer =
            EnvoixCapabilityCodec.encode(
                CapabilityExchangeView.ScanInvite(ScanInviteExchangeView(step = step)),
            )
        result.success(answer.toByteArray(Charsets.UTF_8))
    }

    private fun onIntent(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        val frame = call.arguments as? ByteArray
        if (frame == null) {
            result.error(NOT_A_FRAME, "intent takes the encoded intent frame", null)
            return
        }
        intents.execute {
            val answer =
                try {
                    NativeHost.intent(frame)
                } catch (rejected: RejectedIntent) {
                    main.post {
                        result.error(
                            HOST_REJECTED,
                            rejected.message ?: "the authority refused the intent",
                            null,
                        )
                    }
                    return@execute
                }
            main.post {
                if (answer == null) {
                    result.error(HOST_UNAVAILABLE, "the transfer host is not running", null)
                } else {
                    result.success(answer)
                }
            }
        }
    }

    override fun onListen(
        arguments: Any?,
        events: EventChannel.EventSink,
    ) {
        current?.stop()
        val token = NativeHost.attach()
        if (token == NativeHost.NO_ATTACHMENT) {
            current = null
            events.error(HOST_UNAVAILABLE, "the transfer host is not running", null)
            return
        }
        current = Pump(token, events).also(Pump::start)
    }

    override fun onCancel(arguments: Any?) {
        current?.stop()
        current = null
    }

    /** Releases the channels when the engine that owns them goes away. */
    fun dispose() {
        picking?.error(HOST_UNAVAILABLE, "the frontend went away", null)
        picking = null
        scanning?.error(HOST_UNAVAILABLE, "the frontend went away", null)
        scanning = null
        onCancel(null)
        channel.setStreamHandler(null)
        commands.setMethodCallHandler(null)
        intents.shutdown()
    }

    /** One attachment's pump: its own token, its own thread, its own stop. */
    private inner class Pump(
        private val token: Long,
        private val events: EventChannel.EventSink,
    ) {
        @Volatile
        private var running = true

        fun start() {
            thread(name = "envoix-frame-pump-$token", isDaemon = true) { drain() }
        }

        fun stop() {
            running = false
        }

        private fun drain() {
            while (running) {
                val frame =
                    try {
                        NativeHost.pollFrame(token)
                    } catch (superseded: SupersededAttachment) {
                        return
                    }
                if (frame == null) {
                    Thread.sleep(POLL_MILLIS)
                    continue
                }
                main.post { if (running) events.success(frame) }
            }
        }
    }

    companion object {
        /** Mirrors the catalogued `android.frontend_lane_channel`. */
        const val CHANNEL = "app.envoix.host/frontend-lane"

        /** Mirrors the catalogued `android.frontend_command_channel`. */
        const val COMMAND_CHANNEL = "app.envoix.host/frontend-commands"

        /** The intent method that channel carries. */
        const val INTENT = "intent"

        /**
         * The generated-capability method. Mirrors the catalogued
         * `android.frontend_capability_method`; it carries a capability frame
         * in and one out.
         *
         * It is now the ONLY platform-capability method. The document picker
         * had its own method and an untyped map for its answer; that map had no
         * acquisition key and collapsed an unknown size to zero, so a pick could
         * satisfy whichever ask happened to be outstanding and "unknown" and
         * "empty file" were the same value.
         */
        const val CAPABILITY = "capability"

        /** A scan was requested while one was already open. */
        const val SCAN_IN_FLIGHT = "scan-in-flight"

        /** No host to observe; the Dart side surfaces it and may re-listen. */
        const val HOST_UNAVAILABLE = "host-unavailable"

        /** The host received the frame and refused it before running an intent. */
        const val HOST_REJECTED = "host-rejected"

        /** The call carried something that is not an encoded intent frame. */
        const val NOT_A_FRAME = "not-a-frame"

        const val POLL_MILLIS = 50L
    }
}
