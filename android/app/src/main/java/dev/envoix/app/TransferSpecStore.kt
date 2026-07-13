package dev.envoix.app

import android.content.Context
import org.json.JSONObject

/** Android-only facts needed around the durable cross-platform transfer record. */
internal data class TransferSpec(
    val direction: String,
    val room: String,
    val path: String,
    val broker: String,
    val relay: String,
    val config: String,
    val qrPayload: String?,
    val transferInvite: String?,
    val internetAvailable: Boolean,
    val useRoom: Boolean,
    val useMdns: Boolean,
    val saveTreeUri: String,
    val saveFolder: String,
) {
    fun dir(): Direction = if (direction == "send") Direction.Send else Direction.Receive
}

internal object TransferSpecStore {
    private const val PREFERENCES = "envoix.transfer-specs.v1"

    fun save(
        context: Context,
        id: Long,
        spec: TransferSpec,
    ) {
        preferences(context).edit().putString(id.toString(), spec.toJson().toString()).apply()
    }

    fun load(
        context: Context,
        id: Long,
    ): TransferSpec? =
        preferences(context).getString(id.toString(), null)?.let { value ->
            runCatching { JSONObject(value).toSpec() }.getOrNull()
        }

    fun remove(
        context: Context,
        id: Long,
    ) {
        preferences(context).edit().remove(id.toString()).apply()
    }

    private fun preferences(context: Context) = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun TransferSpec.toJson() =
        JSONObject()
            .put("direction", direction)
            .put("room", room)
            .put("path", path)
            .put("broker", broker)
            .put("relay", relay)
            .put("config", config)
            .put("qr_payload", qrPayload)
            .put("transfer_invite", transferInvite)
            .put("internet_available", internetAvailable)
            .put("use_room", useRoom)
            .put("use_mdns", useMdns)
            .put("save_tree_uri", saveTreeUri)
            .put("save_folder", saveFolder)

    private fun JSONObject.toSpec() =
        TransferSpec(
            direction = getString("direction"),
            room = getString("room"),
            path = getString("path"),
            broker = getString("broker"),
            relay = getString("relay"),
            config = getString("config"),
            qrPayload = optString("qr_payload").ifBlank { null },
            transferInvite = optString("transfer_invite").ifBlank { null },
            internetAvailable = getBoolean("internet_available"),
            useRoom = getBoolean("use_room"),
            useMdns = getBoolean("use_mdns"),
            saveTreeUri = optString("save_tree_uri"),
            saveFolder = optString("save_folder", "Envoix"),
        )
}
