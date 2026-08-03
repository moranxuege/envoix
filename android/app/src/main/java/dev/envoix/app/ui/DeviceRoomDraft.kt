package dev.envoix.app.ui

import dev.envoix.app.discovery.NearbyPairingSelection
import java.util.UUID

/**
 * UI bootstrap for the connection-first experiment.
 *
 * Direction is retained only as an adapter for the current one-transfer core;
 * it is not presented as a top-level product choice.
 */
internal data class DeviceRoomDraft(
    val id: String = UUID.randomUUID().toString(),
    val displayName: String,
    val pairingInput: String? = null,
    val directionAdapter: String = "send",
    val nearbySelection: NearbyPairingSelection? = null,
    val pendingRoleAdapter: String? = null,
    val transferCodes: Set<String> = emptySet(),
    val controlSession: Boolean = false,
    val controlEndpoint: RoomControlEndpoint? = null,
)

internal data class RoomTransferDraft(
    val id: String = UUID.randomUUID().toString(),
    val roleAdapter: String,
    val usesPendingAction: Boolean,
    val showQrInitially: Boolean = false,
    val preparation: TransferDraftPreparationState =
        TransferDraftPreparationState(
            initialRole = roleAdapter,
            showQrInitially = showQrInitially,
        ),
)
