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

/** Render [data] as a QR (ZXing -> Compose Canvas), dark modules on a white card. */
@Composable
fun QrCode(data: String, side: Dp) {
    val matrix = remember(data) {
        runCatching {
            QRCodeWriter().encode(data, BarcodeFormat.QR_CODE, 1, 1, mapOf(EncodeHintType.MARGIN to 1))
        }.getOrNull()
    }
    Box(Modifier.size(side).clip(RoundedCornerShape(14.dp)).background(Color.White).padding(10.dp)) {
        Canvas(Modifier.fillMaxSize()) {
            val m = matrix ?: return@Canvas
            val n = m.width
            val cell = size.width / n
            for (y in 0 until n) {
                for (x in 0 until n) {
                    if (m.get(x, y)) {
                        drawRect(
                            Color(0xFF101820),
                            topLeft = Offset(x * cell, y * cell),
                            size = Size(cell + 0.7f, cell + 0.7f),
                        )
                    }
                }
            }
        }
    }
}
