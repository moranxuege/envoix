package app.envoix.host

import android.app.Activity
import android.content.Intent
import com.envoix.bindings.capability.PickSourceFailureView
import com.envoix.bindings.capability.SourceAcquisitionKeyView
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

/**
 * The Flutter attachment: an observer with no lifetime of its own.
 *
 * It makes sure the host service is running — a cold launch has to start
 * something, and this is the only entry point a user has — and it can never
 * stop it: there is no `stopService` here and no native verb that would end a
 * transfer. It holds no runtime handle; everything it sees arrives as frames on
 * [FrontendLane]. Killing this activity leaves the service, the runtime and
 * every transfer exactly where they were (Pillar 7).
 *
 * It also owns the document picker, because SAF needs an Activity to launch
 * one. That is a platform capability, not a transfer verb: the picked URI goes
 * to [SourcePicks] and never to Dart, and picking a document creates nothing —
 * the authority does that, later, when the frontend asks it to.
 *
 * `FlutterActivity` extends the framework `Activity` rather than androidx's
 * `ComponentActivity`, so the picker uses the classic request/result pair. That
 * keeps the app's dependency list — which is also its release trust surface —
 * exactly where F1 left it.
 */
class MainActivity : FlutterActivity() {
    private var lane: FrontendLane? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        startForegroundService(Intent(this, EnvoixHostService::class.java))
        lane =
            FrontendLane(
                flutterEngine.dartExecutor.binaryMessenger,
                ::pickSource,
                ::scanInvite,
            )
    }

    /** Which acquisition the open pick belongs to, so its answer can name it. */
    private var pickingFor: SourceAcquisitionKeyView? = null

    private fun pickSource(acquisition: SourceAcquisitionKeyView) {
        pickingFor = acquisition
        val intent =
            Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = ANY_DOCUMENT
                // Asked for so the grant survives the pick; whether the
                // provider honours it is its own answer, and the duty reports
                // what is actually readable rather than what was requested.
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                addFlags(Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
            }
        // A device with no document provider at all answers the exchange rather
        // than throwing: "there is no picker here" is a first-class answer, and
        // it is what a desktop or CLI adapter says unconditionally.
        if (intent.resolveActivity(packageManager) == null) {
            pickingFor = null
            lane?.sourcePickFailed(PickSourceFailureView.PICKER_UNAVAILABLE)
            return
        }
        // Launching can still throw — a provider disabled between the resolve
        // and the start, a restricted profile. The exchange answers either way.
        runCatching { startActivityForResult(intent, REQUEST_PICK_SOURCE) }
            .onFailure {
                pickingFor = null
                lane?.sourcePickFailed(PickSourceFailureView.PICKER_UNAVAILABLE)
            }
    }

    /**
     * Opens the invite scanner. Like the picker, it is a platform capability
     * this Activity owns rather than a transfer verb: what it reads comes back
     * as TEXT, and a card is created only if the frontend then asks for one.
     */
    private fun scanInvite() {
        startActivityForResult(Intent(this, ScanActivity::class.java), REQUEST_SCAN_INVITE)
    }

    override fun onActivityResult(
        requestCode: Int,
        resultCode: Int,
        data: Intent?,
    ) {
        if (requestCode == REQUEST_SCAN_INVITE) {
            lane?.scanned(
                text = data?.getStringExtra(ScanActivity.EXTRA_TEXT),
                declined = data?.getStringExtra(ScanActivity.EXTRA_DECLINED),
            )
            return
        }
        if (requestCode != REQUEST_PICK_SOURCE) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }
        val acquisition = pickingFor
        pickingFor = null
        if (acquisition == null) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }
        val uri = data?.data.takeIf { resultCode == Activity.RESULT_OK }
        if (uri == null) {
            lane?.sourcePicked(null)
            return
        }
        // A pick owns no durable capability. The grant is taken only when a
        // committed card claims this URI through its source duty, which gives
        // it a lifecycle owner — and it is recorded under the ACQUISITION that
        // asked, so a later generation cannot inherit it.
        //
        // Every failure here answers the EXCHANGE. An uncaught throw would
        // leave the frontend's method result unanswered, which the channel
        // treats as a leak and a person experiences as a picker that did
        // nothing.
        val granted =
            runCatching { SourcePicks.offer(this, acquisition, uri) }
                .getOrElse {
                    lane?.sourcePickFailed(PickSourceFailureView.INTERNAL)
                    return
                }
        if (granted == null) {
            // Either the provider would not describe the document, or the
            // acquisition is already bound to a different one. Both are the
            // platform failing to answer, not a person declining.
            lane?.sourcePickFailed(PickSourceFailureView.METADATA_UNAVAILABLE)
            return
        }
        lane?.sourcePicked(listOf(granted))
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        lane?.dispose()
        lane = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    private companion object {
        /** Any document the provider will open; the product filters no types. */
        const val ANY_DOCUMENT = "*/*"

        /** This activity's own request codes; nothing else here starts one. */
        const val REQUEST_PICK_SOURCE = 0x50_1c
        const val REQUEST_SCAN_INVITE = 0x5c_a4
    }
}
