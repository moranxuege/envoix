package app.envoix.host

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
 */
class MainActivity : FlutterActivity() {
    private var lane: FrontendLane? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        startForegroundService(Intent(this, EnvoixHostService::class.java))
        lane = FrontendLane(flutterEngine.dartExecutor.binaryMessenger)
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        lane?.dispose()
        lane = null
        super.cleanUpFlutterEngine(flutterEngine)
    }
}
