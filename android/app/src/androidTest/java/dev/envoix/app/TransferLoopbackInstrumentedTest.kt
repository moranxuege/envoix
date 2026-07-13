package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.envoix.app.ffi.EnvoixSession
import dev.envoix.app.ffi.FfiTransferActivityRecord
import dev.envoix.app.ffi.FfiTransferEvent
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.TransferObserver
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class TransferLoopbackInstrumentedTest {
    @Test
    fun sendsSmallFileThroughUniffiInviteLoopback() =
        runBlocking {
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-loopback-test").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val sendFile = File(root, "android-loopback.txt")
            val receiveDir = File(root, "received").apply { mkdirs() }
            val payload = "envoix android loopback ${System.nanoTime()}\n".toByteArray()
            sendFile.writeBytes(payload)

            try {
                val receiverSession = EnvoixSession()
                val receiverObserver = RecordingObserver()
                receiverSession.receive(receiveDir.absolutePath, receiverObserver)

                val invite =
                    withTimeout(10_000) {
                        receiverObserver.invite.await()
                    }
                delay(300)

                val senderSession = EnvoixSession()
                val senderObserver = RecordingObserver()
                val sender =
                    async(Dispatchers.IO) {
                        senderSession.sendInvite(invite, sendFile.absolutePath, senderObserver)
                        senderObserver.awaitCompleted()
                    }
                val receiver =
                    async(Dispatchers.IO) {
                        receiverObserver.awaitCompleted()
                    }

                val (senderBytes, receiverBytes) =
                    withTimeout(90_000) {
                        sender.await() to receiver.await()
                    }
                assertTrue(
                    "sender completed with unexpected byte count: $senderBytes",
                    senderBytes >= payload.size.toULong(),
                )
                assertTrue(
                    "receiver completed with unexpected byte count: $receiverBytes",
                    receiverBytes >= payload.size.toULong(),
                )
                assertArrayEquals(payload, File(receiveDir, sendFile.name).readBytes())
            } finally {
                root.deleteRecursively()
            }
        }

    @Test
    fun nativeTransferWrapperUsesInviteModesForSmallFileLoopback() =
        runBlocking {
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root =
                File(context.cacheDir, "envoix-native-transfer-loopback-test").apply {
                    deleteRecursively()
                    mkdirs()
                }
            val sendFile = File(root, "android-native-transfer-loopback.txt")
            val receiveDir = File(root, "received").apply { mkdirs() }
            val payload = "envoix android native transfer loopback ${System.nanoTime()}\n".toByteArray()
            sendFile.writeBytes(payload)

            try {
                val receiverEvents = CliEventRecorder()
                val receiverJob =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
                                id = 91_001,
                                direction = "receive",
                                code = "invite-direct",
                                broker = Endpoints.BROKER,
                                relay = Endpoints.RELAY,
                                path = receiveDir.absolutePath,
                                configPath = "",
                                qrPayload = "pending-invite",
                                transferInvite = null,
                                internetAvailable = true,
                                useRoom = false,
                                useMdns = false,
                            ).collect { receiverEvents.record(it) }
                    }

                val invite =
                    withTimeout(10_000) {
                        receiverEvents.invite.await()
                    }
                delay(300)

                val senderEvents = CliEventRecorder()
                val senderJob =
                    launch(Dispatchers.IO) {
                        UniffiTransferRunner
                            .run(
                                id = 91_002,
                                direction = "send",
                                code = "invite-direct",
                                broker = Endpoints.BROKER,
                                relay = Endpoints.RELAY,
                                path = sendFile.absolutePath,
                                configPath = "",
                                qrPayload = null,
                                transferInvite = invite,
                                internetAvailable = true,
                                useRoom = false,
                                useMdns = false,
                            ).collect { senderEvents.record(it) }
                    }

                val (senderBytes, receiverBytes) =
                    withTimeout(90_000) {
                        senderEvents.awaitCompleted() to receiverEvents.awaitCompleted()
                    }
                senderJob.join()
                receiverJob.join()

                assertTrue(
                    "sender completed with unexpected byte count: $senderBytes",
                    senderBytes >= payload.size.toLong(),
                )
                assertTrue(
                    "receiver completed with unexpected byte count: $receiverBytes",
                    receiverBytes >= payload.size.toLong(),
                )
                assertArrayEquals(payload, File(receiveDir, sendFile.name).readBytes())
            } finally {
                root.deleteRecursively()
            }
        }

    private class RecordingObserver : TransferObserver {
        val invite = CompletableDeferred<String>()
        private val completed = CompletableDeferred<ULong>()
        private val failed = CompletableDeferred<String>()

        override fun onInviteReady(invite: String) {
            this.invite.complete(invite)
        }

        override fun onStarted(
            fileName: String,
            totalBytes: ULong,
        ) = Unit

        override fun onProgress(
            transferred: ULong,
            total: ULong,
        ) = Unit

        override fun onCompleted(bytes: ULong) {
            completed.complete(bytes)
        }

        override fun onTransferFailed(failure: FfiTransferFailure) {
            failed.complete(failure.diagnosticMessage.ifBlank { failure.userMessageKey })
        }

        override fun onFailed(reason: String) {
            failed.complete(reason)
        }

        override fun onTransferEvent(event: FfiTransferEvent) = Unit

        override fun onTransferActivity(record: FfiTransferActivityRecord) = Unit

        override fun onStatus(message: String) = Unit

        suspend fun awaitCompleted(): ULong {
            while (!completed.isCompleted && !failed.isCompleted) {
                delay(50)
            }
            if (failed.isCompleted) {
                error("transfer failed: ${failed.await()}")
            }
            return completed.await()
        }
    }

    private class CliEventRecorder {
        val invite = CompletableDeferred<String>()
        private val completed = CompletableDeferred<Long>()
        private val failed = CompletableDeferred<String>()

        fun record(event: CliEvent) {
            when (event) {
                is CliEvent.InviteReady -> invite.complete(event.invite)
                is CliEvent.Completed -> completed.complete(event.bytesTransferred)
                is CliEvent.Failed -> failed.complete(event.error)
                is CliEvent.Exit -> if (event.code != 0) failed.complete("exit ${event.code}")
                else -> Unit
            }
        }

        suspend fun awaitCompleted(): Long {
            while (!completed.isCompleted && !failed.isCompleted) {
                delay(50)
            }
            if (failed.isCompleted) {
                error("transfer failed: ${failed.await()}")
            }
            return completed.await()
        }
    }
}
