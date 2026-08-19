package dev.envoix.app

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.runBlocking
import java.io.File

/**
 * Debug-build bridge used by scripts/nat-test.sh and
 * scripts/remembered-device-test.sh.
 *
 * The normal UI creates or parses an InviteV2 and prepares a canonical
 * Manifest-v2 job before starting a transfer. The test harness uses this
 * bridge to perform those same in-process steps and query test-visible transfer
 * state. It is compiled only into debug builds.
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
                ACTION_START_REMEMBERED_RECEIVER -> startRememberedReceiver(context, intent)
                ACTION_QUERY_REMEMBERED -> queryRemembered(context, intent)
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
        val invitation =
            requireNotNull(InviteCodec.generate("receive", broker, relay)) {
                "Could not create receiver invitation"
            }

        TransferService.startReceive(
            context = context,
            room = invitation.reference,
            broker = broker,
            relay = relay,
            qrPayload = null,
            destinationCopyApproved = true,
            rememberLabel = intent.getStringExtra(EXTRA_REMEMBER_LABEL),
            rememberedRelationshipId = null,
            invitationCreator = true,
        )
        resultCode = Activity.RESULT_OK
        resultData = invitation.payload
    }

    private fun startSender(
        context: Context,
        intent: Intent,
    ) {
        val source = File(intent.getStringExtra(EXTRA_PATH).orEmpty())
        require(source.isFile) { "NAT test source is not a file" }
        val remembered =
            intent
                .getStringExtra(EXTRA_REMEMBERED_LABEL)
                ?.takeIf(String::isNotBlank)
                ?.let { rememberedPeer(context, it) }
        val invitation =
            if (remembered == null) {
                val invite = intent.getStringExtra(EXTRA_INVITATION).orEmpty()
                require(invite.isNotBlank()) { "Complete InviteV2 URI is missing" }
                requireNotNull(InviteCodec.parseForRole(invite, "send")) {
                    "Complete InviteV2 URI is invalid for a sender"
                }
            } else {
                null
            }

        val store = TransferService.jobStoreDirectory(context).absolutePath
        // Keep the NAT path active long enough to observe relay-to-direct
        // migration even when the supplied test fixture is highly compressible.
        val jobId =
            runBlocking {
                val created = ManifestV2JobGateway.shared.create(store, "never")
                ManifestV2JobGateway.shared.addStagedProviderRoot(
                    store,
                    created.jobId,
                    ManifestV2StagedProviderRoot(
                        path = source.absolutePath,
                        requestedName = source.name,
                        origin = ManifestV2SourceOrigin.FileProvider,
                        issues = emptyList(),
                    ),
                )
                created.jobId
            }
        if (remembered == null) {
            val invitationReference = requireNotNull(invitation?.reference)
            TransferService.startSend(
                context = context,
                room = invitationReference,
                broker = intent.getStringExtra(EXTRA_BROKER).orEmpty(),
                relay = intent.getStringExtra(EXTRA_RELAY).orEmpty(),
                jobId = jobId,
                qrPayload = null,
                rememberLabel = intent.getStringExtra(EXTRA_REMEMBER_LABEL),
                rememberedRelationshipId = null,
            )
        } else {
            TransferService.startSend(
                context = context,
                room = remembered.label,
                broker = remembered.broker,
                relay = remembered.relay,
                jobId = jobId,
                qrPayload = null,
                rememberLabel = null,
                rememberedRelationshipId = remembered.relationshipId,
            )
        }
        resultCode = Activity.RESULT_OK
        resultData = "started"
    }

    private fun startRememberedReceiver(
        context: Context,
        intent: Intent,
    ) {
        val peer = rememberedPeer(context, intent.getStringExtra(EXTRA_REMEMBERED_LABEL).orEmpty())
        TransferService.startReceive(
            context = context,
            room = peer.label,
            broker = peer.broker,
            relay = peer.relay,
            qrPayload = null,
            destinationCopyApproved = true,
            rememberLabel = null,
            rememberedRelationshipId = peer.relationshipId,
        )
        resultCode = Activity.RESULT_OK
        resultData = "started"
    }

    private fun queryRemembered(
        context: Context,
        intent: Intent,
    ) {
        val peer = rememberedPeer(context, intent.getStringExtra(EXTRA_REMEMBERED_LABEL).orEmpty())
        resultCode = Activity.RESULT_OK
        resultData =
            "${peer.relationshipId}:${peer.generation}:${peer.previousGeneration ?: -1}"
    }

    private fun rememberedPeer(
        context: Context,
        label: String,
    ): RememberedPeerSummary {
        require(label.isNotBlank()) { "Remembered-device label is missing" }
        val matches = RememberedPeerStore.get(context).peers().filter { it.label == label }
        require(matches.size == 1) { "Expected one remembered device labeled '$label'" }
        return matches.single()
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
                "error" -> transfer.error.orEmpty()
                else -> error("field must be state, peer, or error")
            }
        resultCode = Activity.RESULT_OK
    }

    private companion object {
        const val ACTION_CREATE_RECEIVER_INVITE =
            "dev.envoix.app.NAT_TEST_CREATE_RECEIVER_INVITE"
        const val ACTION_START_SENDER = "dev.envoix.app.NAT_TEST_START_SENDER"
        const val ACTION_START_REMEMBERED_RECEIVER =
            "dev.envoix.app.NAT_TEST_START_REMEMBERED_RECEIVER"
        const val ACTION_QUERY_REMEMBERED = "dev.envoix.app.NAT_TEST_QUERY_REMEMBERED"
        const val ACTION_QUERY_TRANSFER = "dev.envoix.app.NAT_TEST_QUERY_TRANSFER"
        const val EXTRA_BROKER = "broker"
        const val EXTRA_RELAY = "relay"
        const val EXTRA_INVITATION = "invitation"
        const val EXTRA_PATH = "path"
        const val EXTRA_DIRECTION = "direction"
        const val EXTRA_FIELD = "field"
        const val EXTRA_REMEMBER_LABEL = "remember_label"
        const val EXTRA_REMEMBERED_LABEL = "remembered_label"
    }
}
