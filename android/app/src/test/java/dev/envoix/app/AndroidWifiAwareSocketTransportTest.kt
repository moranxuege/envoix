package dev.envoix.app

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

class AndroidWifiAwareSocketTransportTest {
    @Test
    fun preservesReadBoundsAndReportsEof() {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { listener ->
            Socket(InetAddress.getLoopbackAddress(), listener.localPort).use { client ->
                listener.accept().use { server ->
                    val transport = AndroidWifiAwareSocketTransport(client)
                    server.getOutputStream().apply {
                        write(byteArrayOf(1, 2, 3, 4, 5))
                        flush()
                    }
                    val first = runBlocking { transport.receive(3u) }
                    assertArrayEquals(byteArrayOf(1, 2, 3), first.bytes)
                    assertFalse(first.endOfStream)
                    val second = runBlocking { transport.receive(3u) }
                    assertArrayEquals(byteArrayOf(4, 5), second.bytes)
                    assertFalse(second.endOfStream)
                    server.shutdownOutput()
                    val eof = runBlocking { transport.receive(3u) }
                    assertTrue(eof.bytes.isEmpty())
                    assertTrue(eof.endOfStream)
                    runBlocking { transport.shutdown() }
                }
            }
        }
    }

    @Test
    fun rejectsInvalidReadBound() {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { listener ->
            Socket(InetAddress.getLoopbackAddress(), listener.localPort).use { client ->
                listener.accept().use {
                    val transport = AndroidWifiAwareSocketTransport(client)
                    assertThrows(IllegalArgumentException::class.java) {
                        runBlocking { transport.receive(0u) }
                    }
                    transport.close()
                }
            }
        }
    }
}
