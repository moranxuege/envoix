package dev.envoix.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
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
                    assertArrayEquals(byteArrayOf(1, 2, 3), transport.receive(3))
                    assertArrayEquals(byteArrayOf(4, 5), transport.receive(3))
                    server.shutdownOutput()
                    assertNull(transport.receive(3))
                    transport.close()
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
                        transport.receive(0)
                    }
                    transport.close()
                }
            }
        }
    }
}
