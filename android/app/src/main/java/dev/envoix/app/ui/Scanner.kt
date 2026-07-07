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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
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
 * Inline QR scanner: a live camera preview in a rounded, bracketed viewfinder,
 * plus a "Choose from photos" fallback that decodes a QR from an image. Meant to
 * sit *inside* another surface (the New-transfer sheet), not as a separate screen.
 * [onScanned] fires once with the decoded text.
 */
@Composable
fun InlineScanner(onScanned: (String) -> Unit, modifier: Modifier = Modifier) {
    val colors = Envoix.colors
    val context = LocalContext.current
    var handled by remember { mutableStateOf(false) }
    val deliver: (String) -> Unit = { if (!handled) { handled = true; onScanned(it) } }

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

    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.fillMaxWidth().height(210.dp).clip(RoundedCornerShape(16.dp)).background(Color.Black),
            contentAlignment = Alignment.Center,
        ) {
            if (hasCamera) {
                CameraPreview(onQr = deliver, modifier = Modifier.fillMaxSize())
                CornerBrackets(colors.accent, Modifier.fillMaxSize())
            } else {
                Text(
                    "Camera access is off —\nchoose a QR image below",
                    color = Color.White.copy(alpha = 0.75f), fontSize = 13.sp,
                    modifier = Modifier.padding(16.dp),
                )
            }
        }
        Spacer(Modifier.height(10.dp))
        Text(
            if (pickError) "No QR code found in that image" else "Point at an Envoix QR code",
            color = if (pickError) Color(0xFFE05B5B) else colors.muted, fontSize = 12.sp,
        )
        Spacer(Modifier.height(8.dp))
        Row(
            Modifier.clip(RoundedCornerShape(20.dp)).background(colors.accentSoft)
                .clickable { pickError = false; picker.launch("image/*") }
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Image, null, tint = colors.accent, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(7.dp))
            Text("Choose from photos", color = colors.accent, fontWeight = FontWeight.Bold, fontSize = 13.sp)
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

/** Accent corner brackets hugging the viewfinder box. */
@Composable
private fun CornerBrackets(accent: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val inset = 14.dp.toPx()
        val len = size.minDimension * 0.14f
        val sw = 3.5.dp.toPx()
        val l = inset
        val t = inset
        val r = size.width - inset
        val b = size.height - inset
        fun line(a: Offset, c: Offset) = drawLine(accent, a, c, sw, StrokeCap.Round)
        line(Offset(l, t), Offset(l + len, t)); line(Offset(l, t), Offset(l, t + len))
        line(Offset(r, t), Offset(r - len, t)); line(Offset(r, t), Offset(r, t + len))
        line(Offset(l, b), Offset(l + len, b)); line(Offset(l, b), Offset(l, b - len))
        line(Offset(r, b), Offset(r - len, b)); line(Offset(r, b), Offset(r, b - len))
    }
}
