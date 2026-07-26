package app.envoix.host

import android.os.Handler
import android.os.Looper
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
 * The other direction is one method: [COMMAND_CHANNEL]'s [SUBMIT], which hands
 * a submit frame to the host and returns the encoded acceptance frame. It is
 * not a transfer verb — the host decides what the command does, and the
 * committed completion arrives later on the frame lane above.
 *
 * Kotlin never looks inside a frame. It moves `ByteArray`s between the JNI lane
 * and the message channels; the generated Dart codec is the only thing that
 * decodes them, and this class carries no contract type at all.
 */
class FrontendLane(
    messenger: BinaryMessenger,
) : EventChannel.StreamHandler {
    private val channel = EventChannel(messenger, CHANNEL)
    private val commands = MethodChannel(messenger, COMMAND_CHANNEL)
    private val main = Handler(Looper.getMainLooper())

    /**
     * Submits run off the platform thread: the host blocks on the runtime to
     * resolve an acceptance, and blocking the platform thread for that is an
     * ANR. One thread, so a burst of taps queues instead of spawning a thread
     * each; the reply is posted back where a `MethodChannel` result belongs.
     */
    private val submissions = Executors.newSingleThreadExecutor()

    /** The pump the current attachment owns; every earlier one is superseded. */
    private var current: Pump? = null

    init {
        channel.setStreamHandler(this)
        commands.setMethodCallHandler(::onCommand)
    }

    private fun onCommand(
        call: MethodCall,
        result: MethodChannel.Result,
    ) {
        if (call.method != SUBMIT) {
            result.notImplemented()
            return
        }
        val frame = call.arguments as? ByteArray
        if (frame == null) {
            result.error(NOT_A_FRAME, "submit takes the encoded command frame", null)
            return
        }
        submissions.execute {
            val acceptance = NativeHost.submit(frame)
            main.post {
                if (acceptance == null) {
                    result.error(HOST_UNAVAILABLE, "the transfer host is not running", null)
                } else {
                    result.success(acceptance)
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
        onCancel(null)
        channel.setStreamHandler(null)
        commands.setMethodCallHandler(null)
        submissions.shutdown()
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

        /** The one method that channel carries. */
        const val SUBMIT = "submit"

        /** No host to observe; the Dart side surfaces it and may re-listen. */
        const val HOST_UNAVAILABLE = "host-unavailable"

        /** The call carried something that is not an encoded command frame. */
        const val NOT_A_FRAME = "not-a-frame"

        const val POLL_MILLIS = 50L
    }
}
