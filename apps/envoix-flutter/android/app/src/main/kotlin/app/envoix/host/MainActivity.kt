package app.envoix.host

import android.app.Activity
import android.content.Intent
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
        lane = FrontendLane(flutterEngine.dartExecutor.binaryMessenger, ::pickSource)
    }

    private fun pickSource() {
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
        startActivityForResult(intent, REQUEST_PICK_SOURCE)
    }

    override fun onActivityResult(
        requestCode: Int,
        resultCode: Int,
        data: Intent?,
    ) {
        if (requestCode != REQUEST_PICK_SOURCE) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }
        val uri = data?.data.takeIf { resultCode == Activity.RESULT_OK }
        val granted =
            uri?.let {
                // The persistable grant is taken FIRST: a source the OS may
                // revoke before the card exists is one the card could never
                // open.
                runCatching {
                    contentResolver.takePersistableUriPermission(
                        it,
                        Intent.FLAG_GRANT_READ_URI_PERMISSION,
                    )
                }
                SourcePicks.offer(this, it)
            }
        lane?.sourcePicked(granted)
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        lane?.dispose()
        lane = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    private companion object {
        /** Any document the provider will open; the product filters no types. */
        const val ANY_DOCUMENT = "*/*"

        /** This activity's own request code; nothing else here starts one. */
        const val REQUEST_PICK_SOURCE = 0x50_1c
    }
}
