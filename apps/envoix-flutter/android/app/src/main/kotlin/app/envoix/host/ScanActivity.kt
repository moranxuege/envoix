package app.envoix.host

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import android.util.Size
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The Android half of the `scan_invite` capability, and NOTHING more.
 *
 * It produces TEXT. It does not parse an invite, does not validate one, does not
 * decide a role and cannot create a card: what it reads goes back to the
 * frontend, which hands it to the same create-join call the paste field fills,
 * and the invite grammar in Rust judges it identically. A scanner that
 * understood invites would be a second, weaker copy of that grammar — on every
 * platform that ever ships one.
 *
 * The three ways this can end without text are three ANSWERS, not three
 * failures, and each leaves by its own door: the user backed out
 * ([DECLINED_CANCELLED]), the user refused the camera ([DECLINED_REFUSED]), or
 * this device has nothing to scan with ([DECLINED_UNSUPPORTED]). The frontend
 * tells them apart because it must — offer the scanner again, send the user to
 * settings, or stop offering it at all are different next steps.
 *
 * A separate Activity, launched with `startActivityForResult`, because that is
 * how [MainActivity] already borrows the document picker: a platform capability
 * the Activity owns, answered once, with no lifetime of its own. `FlutterActivity`
 * extends the framework `Activity`, so this does too and supplies CameraX's
 * `LifecycleOwner` from a plain [LifecycleRegistry] — which keeps androidx
 * `appcompat` and `activity` out of the app's dependency list, and out of its
 * release trust surface, entirely.
 */
class ScanActivity :
    Activity(),
    LifecycleOwner {
    private val registry = LifecycleRegistry(this)
    private val analysis = Executors.newSingleThreadExecutor()

    /** The first decode wins; everything after it is ignored. */
    private val answered = AtomicBoolean(false)

    private var provider: ProcessCameraProvider? = null

    override val lifecycle: Lifecycle get() = registry

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        registry.currentState = Lifecycle.State.CREATED

        // A device with no camera at all cannot be asked for permission, so
        // this answer comes first and is never dressed as a refusal.
        if (!packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            answer(DECLINED_UNSUPPORTED)
            return
        }

        val preview =
            PreviewView(this).apply {
                layoutParams =
                    FrameLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT,
                    )
                setBackgroundColor(Color.BLACK)
            }
        setContentView(preview)
        this.preview = preview

        if (checkSelfPermission(CAMERA) == PackageManager.PERMISSION_GRANTED) {
            start()
        } else {
            requestPermissions(arrayOf(CAMERA), REQUEST_CAMERA)
        }
    }

    private var preview: PreviewView? = null

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        if (requestCode != REQUEST_CAMERA) {
            super.onRequestPermissionsResult(requestCode, permissions, grantResults)
            return
        }
        // Refusing is an answer the user gave. It is reported as itself, so the
        // frontend can say what permission would buy rather than showing a
        // scanner that silently does nothing.
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            start()
        } else {
            answer(DECLINED_REFUSED)
        }
    }

    override fun onStart() {
        super.onStart()
        registry.currentState = Lifecycle.State.STARTED
    }

    override fun onResume() {
        super.onResume()
        registry.currentState = Lifecycle.State.RESUMED
    }

    override fun onPause() {
        registry.currentState = Lifecycle.State.STARTED
        super.onPause()
    }

    override fun onStop() {
        registry.currentState = Lifecycle.State.CREATED
        super.onStop()
    }

    override fun onDestroy() {
        registry.currentState = Lifecycle.State.DESTROYED
        provider?.unbindAll()
        analysis.shutdown()
        super.onDestroy()
    }

    /**
     * The last door, and the one every other exit falls through.
     *
     * Backing out, a gesture, a system finish — all of them reach here, so a
     * scan that ends any way nobody enumerated still answers `cancelled` rather
     * than leaving the frontend waiting forever. Overriding `onBackPressed` as
     * well would be a second, deprecated spelling of this same guarantee.
     */
    override fun finish() {
        if (answered.compareAndSet(false, true)) {
            setResult(RESULT_CANCELED, Intent().putExtra(EXTRA_DECLINED, DECLINED_CANCELLED))
        }
        super.finish()
    }

    private fun start() {
        val future = ProcessCameraProvider.getInstance(this)
        future.addListener({
            val provider =
                runCatching { future.get() }.getOrNull() ?: return@addListener answer(
                    DECLINED_UNSUPPORTED,
                )
            this.provider = provider
            val selector =
                if (provider.hasCamera(CameraSelector.DEFAULT_BACK_CAMERA)) {
                    CameraSelector.DEFAULT_BACK_CAMERA
                } else if (provider.hasCamera(CameraSelector.DEFAULT_FRONT_CAMERA)) {
                    CameraSelector.DEFAULT_FRONT_CAMERA
                } else {
                    // The feature flag said a camera exists but none binds.
                    return@addListener answer(DECLINED_UNSUPPORTED)
                }
            val viewfinder =
                Preview.Builder().build().also {
                    it.setSurfaceProvider(preview?.surfaceProvider)
                }
            val frames =
                ImageAnalysis
                    .Builder()
                    .setResolutionSelector(ANALYSIS_RESOLUTION)
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                    .also { it.setAnalyzer(analysis, ::read) }
            runCatching {
                provider.unbindAll()
                provider.bindToLifecycle(this, selector, viewfinder, frames)
            }.onFailure { answer(DECLINED_UNSUPPORTED) }
        }, mainExecutor)
    }

    /**
     * Decodes one frame, reading the luminance plane directly.
     *
     * ZXing is asked for QR alone: the invite grammar spells a QR, so accepting
     * another symbology would be this side deciding what an invite may be.
     */
    private fun read(image: ImageProxy) {
        image.use {
            if (answered.get()) {
                return
            }
            val plane = it.planes.firstOrNull() ?: return
            val buffer = plane.buffer
            val bytes = ByteArray(buffer.remaining())
            buffer.get(bytes)
            val source =
                PlanarYUVLuminanceSource(
                    bytes,
                    plane.rowStride,
                    it.height,
                    0,
                    0,
                    minOf(it.width, plane.rowStride),
                    it.height,
                    false,
                )
            val text =
                runCatching {
                    READER.decodeWithState(BinaryBitmap(HybridBinarizer(source)))?.text
                }.getOrNull()
            READER.reset()
            if (text != null) {
                runOnUiThread { provide(text) }
            }
        }
    }

    private fun provide(text: String) {
        if (!answered.compareAndSet(false, true)) {
            return
        }
        setResult(RESULT_OK, Intent().putExtra(EXTRA_TEXT, text))
        super.finish()
    }

    private fun answer(declined: String) {
        if (!answered.compareAndSet(false, true)) {
            return
        }
        setResult(RESULT_CANCELED, Intent().putExtra(EXTRA_DECLINED, declined))
        super.finish()
    }

    companion object {
        /** Named here rather than imported, so this file states what it needs. */
        const val CAMERA = "android.permission.CAMERA"

        private const val REQUEST_CAMERA = 0x5ca4

        /** The scanned text, when there is any. */
        const val EXTRA_TEXT = "app.envoix.host.SCANNED_TEXT"

        /** Which decline, when there is no text. */
        const val EXTRA_DECLINED = "app.envoix.host.SCAN_DECLINED"

        /**
         * The three declines, spelled as the generated contract spells them so
         * the value crosses to Dart unchanged.
         */
        const val DECLINED_CANCELLED = "cancelled"
        const val DECLINED_REFUSED = "refused"
        const val DECLINED_UNSUPPORTED = "unsupported"

        /**
         * Enough to read a dense invite without asking the camera for frames
         * nobody looks at. A QR at version 40 is 177 modules a side, and the
         * default analysis resolution (640x480) leaves too few pixels per
         * module for one; this is the nearest larger size the device offers.
         */
        private val ANALYSIS_STRATEGY =
            ResolutionStrategy(
                Size(1280, 720),
                ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER,
            )

        private val ANALYSIS_RESOLUTION =
            ResolutionSelector.Builder().setResolutionStrategy(ANALYSIS_STRATEGY).build()

        private val READER =
            MultiFormatReader().apply {
                setHints(
                    mapOf(
                        DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
                        DecodeHintType.TRY_HARDER to true,
                    ),
                )
            }
    }
}
