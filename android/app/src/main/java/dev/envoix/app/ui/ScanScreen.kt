package dev.envoix.app.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Image
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors

/**
 * Full-screen QR scanner: a live CameraX preview behind a styled viewfinder, plus
 * a "Choose from photos" fallback that decodes a QR out of an image (handy when
 * the code is on another screen). [onResult] fires once with the decoded text.
 */
@Composable
fun ScanScreen(onResult: (String) -> Unit, onClose: () -> Unit) {
    val colors = Envoix.colors
    val context = LocalContext.current
    var handled by remember { mutableStateOf(false) }
    val deliver: (String) -> Unit = { if (!handled) { handled = true; onResult(it) } }

    var hasCamera by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { hasCamera = it }
    LaunchedEffect(Unit) { if (!hasCamera) permLauncher.launch(Manifest.permission.CAMERA) }

    var pickError by remember { mutableStateOf(false) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) decodeQrFromUri(context, uri)?.let(deliver) ?: run { pickError = true }
    }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        if (hasCamera) {
            CameraPreview(onQr = deliver, modifier = Modifier.fillMaxSize())
        } else {
            Column(
                Modifier.fillMaxSize(),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text("Camera access is off", color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                Spacer(Modifier.height(6.dp))
                Text("Choose a QR image below instead", color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp)
            }
        }

        ViewfinderOverlay(accent = colors.accent, dim = hasCamera)

        // top bar: close + title
        Row(
            Modifier.fillMaxWidth().padding(top = 18.dp, start = 12.dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.size(44.dp).clip(CircleShape).background(Color.Black.copy(alpha = 0.35f))
                    .clickable(onClick = onClose),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Default.Close, "Close", tint = Color.White) }
            Spacer(Modifier.width(12.dp))
            Text("Scan Envoix code", color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        }

        // prompt + gallery button
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(bottom = 46.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                if (pickError) "No QR code found in that image" else "Point at an Envoix QR code",
                color = if (pickError) Color(0xFFFF7676) else Color.White.copy(alpha = 0.9f),
                fontSize = 14.sp,
            )
            Spacer(Modifier.height(16.dp))
            Row(
                Modifier.clip(RoundedCornerShape(24.dp)).background(colors.accent)
                    .clickable { pickError = false; picker.launch("image/*") }
                    .padding(horizontal = 22.dp, vertical = 13.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Default.Image, null, tint = Color.White, modifier = Modifier.size(20.dp))
                Spacer(Modifier.width(9.dp))
                Text("Choose from photos", color = Color.White, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            }
        }
    }
}

@Composable
private fun CameraPreview(onQr: (String) -> Unit, modifier: Modifier = Modifier) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    DisposableEffect(Unit) { onDispose { executor.shutdown() } }
    AndroidView(
        modifier = modifier,
        factory = { ctx ->
            val previewView = PreviewView(ctx).apply { scaleType = PreviewView.ScaleType.FILL_CENTER }
            val future = ProcessCameraProvider.getInstance(ctx)
            future.addListener({
                val provider = future.get()
                val preview = Preview.Builder().build()
                    .also { it.setSurfaceProvider(previewView.surfaceProvider) }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                    .also { it.setAnalyzer(executor, QrAnalyzer(onQr)) }
                runCatching {
                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis,
                    )
                }
            }, ContextCompat.getMainExecutor(ctx))
            previewView
        },
    )
}

/** Decodes QR frames off the camera; calls [onResult] on the first hit. */
private class QrAnalyzer(val onResult: (String) -> Unit) : ImageAnalysis.Analyzer {
    private val reader = MultiFormatReader().apply {
        setHints(mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE)))
    }

    override fun analyze(image: ImageProxy) {
        try {
            val buffer = image.planes[0].buffer
            val bytes = ByteArray(buffer.remaining()).also { buffer.get(it) }
            val source = PlanarYUVLuminanceSource(
                bytes, image.width, image.height, 0, 0, image.width, image.height, false,
            )
            onResult(reader.decodeWithState(BinaryBitmap(HybridBinarizer(source))).text)
        } catch (_: Exception) {
            // no code in this frame
        } finally {
            reader.reset()
            image.close()
        }
    }
}

/** Decode a QR out of the image at [uri]; null if there isn't one. */
private fun decodeQrFromUri(context: Context, uri: Uri): String? {
    val bitmap = context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) }
        ?: return null
    val w = bitmap.width
    val h = bitmap.height
    val pixels = IntArray(w * h)
    bitmap.getPixels(pixels, 0, w, 0, 0, w, h)
    val source = RGBLuminanceSource(w, h, pixels)
    val reader = MultiFormatReader().apply {
        setHints(mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE)))
    }
    return runCatching { reader.decode(BinaryBitmap(HybridBinarizer(source))).text }
        .recoverCatching { reader.decode(BinaryBitmap(HybridBinarizer(source.invert()))).text }
        .getOrNull()
}

/** Dark scrim with a rounded cut-out window and accent corner brackets. */
@Composable
private fun ViewfinderOverlay(accent: Color, dim: Boolean) {
    Canvas(Modifier.fillMaxSize().graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)) {
        val box = size.minDimension * 0.66f
        val left = (size.width - box) / 2f
        val top = (size.height - box) / 2f
        val radius = 26.dp.toPx()
        if (dim) {
            drawRect(Color.Black.copy(alpha = 0.5f))
            drawRoundRect(
                color = Color.Black,
                topLeft = Offset(left, top),
                size = Size(box, box),
                cornerRadius = CornerRadius(radius, radius),
                blendMode = BlendMode.Clear,
            )
        }
        // accent corner brackets, starting just past the rounded corners
        val len = box * 0.16f
        val sw = 4.dp.toPx()
        val right = left + box
        val bottom = top + box
        fun l(a: Offset, b: Offset) = drawLine(accent, a, b, sw, StrokeCap.Round)
        // top-left
        l(Offset(left, top + radius), Offset(left, top + radius + len))
        l(Offset(left + radius, top), Offset(left + radius + len, top))
        // top-right
        l(Offset(right, top + radius), Offset(right, top + radius + len))
        l(Offset(right - radius, top), Offset(right - radius - len, top))
        // bottom-left
        l(Offset(left, bottom - radius), Offset(left, bottom - radius - len))
        l(Offset(left + radius, bottom), Offset(left + radius + len, bottom))
        // bottom-right
        l(Offset(right, bottom - radius), Offset(right, bottom - radius - len))
        l(Offset(right - radius, bottom), Offset(right - radius - len, bottom))
    }
}
