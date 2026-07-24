package dev.envoix.app.ui

import dev.envoix.app.discovery.NearbyPairingSelection

/**
 * UI bootstrap for the connection-first experiment.
 *
 * Direction is retained only as an adapter for the current one-transfer core;
 * it is not presented as a top-level product choice.
 */
internal data class DeviceRoomDraft(
    val displayName: String,
    val pairingInput: String? = null,
    val directionAdapter: String = "send",
    val nearbySelection: NearbyPairingSelection? = null,
    val hostedCode: String? = null,
    val hostedPayload: String? = null,
)
