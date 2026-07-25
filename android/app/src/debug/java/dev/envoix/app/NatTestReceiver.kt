package dev.envoix.app

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.json.JSONObject

/**
 * Debug-build bridge used by scripts/nat-test.sh.
 *
 * The normal UI creates an InviteV2 before starting a creator transfer. ADB
 * starts the service directly, so the test harness needs the same in-process
 * setup step. Only the intentionally carried Room Code is returned to ADB.
 */
class NatTestReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent,
    ) {
        if (intent.action != ACTION_CREATE_RECEIVER_INVITE) {
            return
        }

        val broker = intent.getStringExtra(EXTRA_BROKER).orEmpty()
        val relay = intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val invitation = JSONObject(Native.generateInvite("receive", broker, relay))
        val error = invitation.optString("error")
        if (error.isNotEmpty()) {
            resultCode = Activity.RESULT_CANCELED
            resultData = error
            return
        }

        val roomCode = invitation.getString("code")
        TransferService.startReceive(
            context = context,
            room = invitation.getString("reference"),
            broker = broker,
            relay = relay,
            qrPayload = null,
            destinationCopyApproved = true,
        )
        resultCode = Activity.RESULT_OK
        resultData = roomCode
    }

    private companion object {
        const val ACTION_CREATE_RECEIVER_INVITE =
            "dev.envoix.app.NAT_TEST_CREATE_RECEIVER_INVITE"
        const val EXTRA_BROKER = "broker"
        const val EXTRA_RELAY = "relay"
    }
}
