package dev.envoix.app

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Debug-build bridge used by scripts/nat-test.sh.
 *
 * The normal UI creates or parses an InviteV2 and prepares a canonical
 * Manifest-v2 job before starting a transfer. The test harness uses this
 * bridge to perform those same in-process steps and query the live transfer
 * card. Only the intentionally carried Room Code is returned to ADB.
 */
class NatTestReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent,
    ) {
        try {
            when (intent.action) {
                ACTION_CREATE_RECEIVER_INVITE -> createReceiverInvite(context, intent)
                ACTION_START_SENDER -> startSender(context, intent)
                ACTION_QUERY_TRANSFER -> queryTransfer(intent)
            }
        } catch (error: Throwable) {
            resultCode = Activity.RESULT_CANCELED
            resultData = error.message ?: "NAT test bridge failed"
        }
    }

    private fun createReceiverInvite(
        context: Context,
        intent: Intent,
    ) {
        val broker = intent.getStringExtra(EXTRA_BROKER).orEmpty()
        val relay = intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val invitation = JSONObject(Native.generateInvite("receive", broker, relay))
        invitation.throwNativeError()

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

    private fun startSender(
        context: Context,
        intent: Intent,
    ) {
        val room = intent.getStringExtra(EXTRA_ROOM).orEmpty()
        val source = File(intent.getStringExtra(EXTRA_PATH).orEmpty())
        require(room.isNotBlank()) { "Room Code is missing" }
        require(source.isFile) { "NAT test source is not a file" }

        val invitation = JSONObject(Native.parseInviteForRole(room, "send"))
        invitation.throwNativeError()
        val store = TransferService.jobStoreDirectory(context).absolutePath
        // Keep the NAT path active long enough to observe relay-to-direct
        // migration even when the supplied test fixture is highly compressible.
        val job = JSONObject(Native.createManifestV2Job(store, "never"))
        job.throwNativeError()
        val jobId = job.getString("job_id")
        val roots =
            JSONArray()
                .put(
                    JSONObject()
                        .put("path", source.absolutePath)
                        .put("requested_name", source.name)
                        .put("origin", "file_provider")
                        .put("issues", JSONArray()),
                )
        JSONObject(Native.prepareManifestV2Job(store, jobId, roots.toString())).throwNativeError()
        TransferService.startSend(
            context = context,
            room = invitation.getString("reference"),
            broker = intent.getStringExtra(EXTRA_BROKER).orEmpty(),
            relay = intent.getStringExtra(EXTRA_RELAY).orEmpty(),
            jobId = jobId,
            qrPayload = null,
        )
        resultCode = Activity.RESULT_OK
        resultData = "started"
    }

    private fun queryTransfer(intent: Intent) {
        val direction =
            when (intent.getStringExtra(EXTRA_DIRECTION)) {
                "sender" -> Direction.Send
                "receiver" -> Direction.Receive
                else -> error("direction must be sender or receiver")
            }
        val transfer =
            TransferRepository.transfers.value
                .filter { it.direction == direction }
                .maxByOrNull { it.id }
                ?: error("transfer not found")
        resultData =
            when (intent.getStringExtra(EXTRA_FIELD)) {
                "state" -> transfer.status.wire
                "peer" -> transfer.pathAddr.orEmpty()
                else -> error("field must be state or peer")
            }
        resultCode = Activity.RESULT_OK
    }

    private fun JSONObject.throwNativeError() {
        optString("error").takeIf(String::isNotEmpty)?.let(::error)
    }

    private companion object {
        const val ACTION_CREATE_RECEIVER_INVITE =
            "dev.envoix.app.NAT_TEST_CREATE_RECEIVER_INVITE"
        const val ACTION_START_SENDER = "dev.envoix.app.NAT_TEST_START_SENDER"
        const val ACTION_QUERY_TRANSFER = "dev.envoix.app.NAT_TEST_QUERY_TRANSFER"
        const val EXTRA_BROKER = "broker"
        const val EXTRA_RELAY = "relay"
        const val EXTRA_ROOM = "room"
        const val EXTRA_PATH = "path"
        const val EXTRA_DIRECTION = "direction"
        const val EXTRA_FIELD = "field"
    }
}
