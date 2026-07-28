package dev.envoix.app

import java.io.Closeable
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

/** Reliable TCP byte stream selected through an Android Wi-Fi Aware Network.
 * Read and write use separate locks because Rust pumps both directions
 * concurrently. */
internal class AndroidWifiAwareSocketTransport(
    private val socket: Socket,
) : NativeDuplexTransport,
    Closeable {
    private val closed = AtomicBoolean(false)
    private val readLock = Any()
    private val writeLock = Any()

    override fun send(bytes: ByteArray) {
        check(!closed.get()) { "Wi-Fi Aware transport is closed" }
        synchronized(writeLock) {
            socket.getOutputStream().apply {
                write(bytes)
                flush()
            }
        }
    }

    /** Returns null only for EOF; a successful read always contains bytes. */
    override fun receive(maxBytes: Int): ByteArray? {
        require(maxBytes > 0) { "maxBytes must be positive" }
        if (closed.get()) return null
        return synchronized(readLock) {
            val buffer = ByteArray(maxBytes)
            val count = socket.getInputStream().read(buffer)
            when {
                count < 0 -> null
                count == buffer.size -> buffer
                else -> buffer.copyOf(count)
            }
        }
    }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            socket.close()
        }
    }
}
