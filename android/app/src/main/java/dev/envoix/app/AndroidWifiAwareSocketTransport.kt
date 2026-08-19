package dev.envoix.app

import dev.envoix.app.ffi.FfiNativeDuplexTransport
import dev.envoix.app.ffi.FfiNativeTransportRead
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

/** Reliable TCP byte stream selected through an Android Wi-Fi Aware Network.
 * Read and write use separate locks because Rust pumps both directions
 * concurrently. */
internal class AndroidWifiAwareSocketTransport(
    private val socket: Socket,
) : FfiNativeDuplexTransport,
    Closeable {
    private val closed = AtomicBoolean(false)
    private val readLock = Any()
    private val writeLock = Any()

    override suspend fun send(bytes: ByteArray) {
        withContext(Dispatchers.IO) {
            check(!closed.get()) { "Wi-Fi Aware transport is closed" }
            synchronized(writeLock) {
                socket.getOutputStream().apply {
                    write(bytes)
                    flush()
                }
            }
        }
    }

    /** Reports EOF explicitly; a successful read always contains bytes. */
    override suspend fun receive(maxBytes: UInt): FfiNativeTransportRead =
        withContext(Dispatchers.IO) {
            require(maxBytes in 1u..Int.MAX_VALUE.toUInt()) { "maxBytes must fit the Android read range" }
            if (closed.get()) return@withContext FfiNativeTransportRead(ByteArray(0), true)
            synchronized(readLock) {
                val buffer = ByteArray(maxBytes.toInt())
                val count = socket.getInputStream().read(buffer)
                FfiNativeTransportRead(
                    bytes =
                        when {
                            count < 0 -> ByteArray(0)
                            count == buffer.size -> buffer
                            else -> buffer.copyOf(count)
                        },
                    endOfStream = count < 0,
                )
            }
        }

    override suspend fun shutdown() = withContext(Dispatchers.IO) { close() }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            socket.close()
        }
    }
}
