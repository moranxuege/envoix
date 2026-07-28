package dev.envoix.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class QrCodeTest {
    @Test
    fun encodedMatrixHasStandardQuietZone() {
        val matrix = encodeQrMatrix("envoix:interop-test")
        val darkCoordinates =
            buildList {
                for (y in 0 until matrix.height) {
                    for (x in 0 until matrix.width) {
                        if (matrix[x, y]) add(x to y)
                    }
                }
            }

        assertEquals(matrix.width, matrix.height)
        assertEquals(QR_QUIET_ZONE_MODULES, darkCoordinates.minOf { it.first })
        assertEquals(QR_QUIET_ZONE_MODULES, darkCoordinates.minOf { it.second })
        assertEquals(matrix.width - QR_QUIET_ZONE_MODULES - 1, darkCoordinates.maxOf { it.first })
        assertEquals(matrix.height - QR_QUIET_ZONE_MODULES - 1, darkCoordinates.maxOf { it.second })
    }

    @Test
    fun geometryUsesWholePixelsAndCentersMatrix() {
        val geometry = requireNotNull(qrRenderGeometry(503, 501, 41))

        assertEquals(12, geometry.modulePixels)
        assertEquals(5, geometry.leftPixels)
        assertEquals(4, geometry.topPixels)
    }

    @Test
    fun geometryRejectsCanvasSmallerThanOneModulePerPixel() {
        assertNull(qrRenderGeometry(40, 40, 41))
    }
}
