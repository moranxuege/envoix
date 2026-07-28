package dev.envoix.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel

internal const val QR_QUIET_ZONE_MODULES = 4

internal data class QrRenderGeometry(
    val modulePixels: Int,
    val leftPixels: Int,
    val topPixels: Int,
)

internal fun encodeQrMatrix(data: String) =
    QRCodeWriter().encode(
        data,
        BarcodeFormat.QR_CODE,
        1,
        1,
        mapOf(
            EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,
            EncodeHintType.MARGIN to QR_QUIET_ZONE_MODULES,
        ),
    )

internal fun qrRenderGeometry(
    canvasWidthPixels: Int,
    canvasHeightPixels: Int,
    moduleCount: Int,
): QrRenderGeometry? {
    if (canvasWidthPixels <= 0 || canvasHeightPixels <= 0 || moduleCount <= 0) return null
    val modulePixels = minOf(canvasWidthPixels, canvasHeightPixels) / moduleCount
    if (modulePixels == 0) return null
    return QrRenderGeometry(
        modulePixels = modulePixels,
        leftPixels = (canvasWidthPixels - moduleCount * modulePixels) / 2,
        topPixels = (canvasHeightPixels - moduleCount * modulePixels) / 2,
    )
}

/** Render [data] as a QR (ZXing -> Compose Canvas), dark modules on a white card. */
@Composable
fun QrCode(
    data: String,
    side: Dp,
) {
    val matrix =
        remember(data) {
            runCatching {
                encodeQrMatrix(data)
            }.getOrNull()
        }
    Box(
        Modifier
            .size(side)
            .clip(RoundedCornerShape(14.dp))
            .background(Color.White)
            .padding(10.dp),
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val m = matrix ?: return@Canvas
            val n = m.width
            val geometry =
                qrRenderGeometry(
                    canvasWidthPixels = size.width.toInt(),
                    canvasHeightPixels = size.height.toInt(),
                    moduleCount = n,
                ) ?: return@Canvas
            val modulePixels = geometry.modulePixels.toFloat()
            for (y in 0 until n) {
                for (x in 0 until n) {
                    if (m.get(x, y)) {
                        drawRect(
                            Color(0xFF101820),
                            topLeft =
                                Offset(
                                    geometry.leftPixels + x * modulePixels,
                                    geometry.topPixels + y * modulePixels,
                                ),
                            size = Size(modulePixels, modulePixels),
                        )
                    }
                }
            }
        }
    }
}
