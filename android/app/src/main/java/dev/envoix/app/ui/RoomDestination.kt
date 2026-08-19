package dev.envoix.app.ui

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import dev.envoix.app.Settings

internal enum class RoomDestinationAvailability {
    Ready,
    RequiresFolder,
    Unavailable,
}

internal data class RoomDestinationPresentation(
    val availability: RoomDestinationAvailability,
    val label: String,
) {
    val ready: Boolean
        get() = availability == RoomDestinationAvailability.Ready
}

internal data class RoomCustomDestinationSnapshot(
    val hasPersistedWriteGrant: Boolean,
    val exists: Boolean,
    val isDirectory: Boolean,
    val canWrite: Boolean,
    val name: String?,
)

internal fun resolveRoomDestinationPresentation(
    directoryCount: Int,
    customDestination: RoomCustomDestinationSnapshot?,
    downloadsLabel: String,
    chooseFolderLabel: String,
    unavailableLabel: String,
    selectedFolderLabel: String,
): RoomDestinationPresentation {
    if (customDestination == null) {
        return if (directoryCount > 0) {
            RoomDestinationPresentation(
                availability = RoomDestinationAvailability.RequiresFolder,
                label = chooseFolderLabel,
            )
        } else {
            RoomDestinationPresentation(
                availability = RoomDestinationAvailability.Ready,
                label = downloadsLabel,
            )
        }
    }
    val ready =
        customDestination.hasPersistedWriteGrant &&
            customDestination.exists &&
            customDestination.isDirectory &&
            customDestination.canWrite
    return if (ready) {
        RoomDestinationPresentation(
            availability = RoomDestinationAvailability.Ready,
            label = customDestination.name?.trim()?.takeIf(String::isNotEmpty) ?: selectedFolderLabel,
        )
    } else {
        RoomDestinationPresentation(
            availability = RoomDestinationAvailability.Unavailable,
            label = unavailableLabel,
        )
    }
}

internal fun roomOfferDestinationPresentation(
    context: Context,
    settings: Settings,
    directoryCount: Int,
    downloadsRootLabel: String,
    chooseFolderLabel: String,
    unavailableLabel: String,
    selectedFolderLabel: String,
): RoomDestinationPresentation {
    val customUri = settings.saveTreeUri.trim()
    val customDestination =
        customUri.takeIf(String::isNotEmpty)?.let { value ->
            inspectCustomDestination(context, Uri.parse(value))
        }
    val saveFolder = settings.saveFolder.trim()
    val downloadsLabel =
        if (saveFolder.isEmpty()) {
            downloadsRootLabel
        } else {
            "$downloadsRootLabel / $saveFolder"
        }
    return resolveRoomDestinationPresentation(
        directoryCount = directoryCount,
        customDestination = customDestination,
        downloadsLabel = downloadsLabel,
        chooseFolderLabel = chooseFolderLabel,
        unavailableLabel = unavailableLabel,
        selectedFolderLabel = selectedFolderLabel,
    )
}

internal fun shouldResumeRoomOfferAfterDestinationRepair(
    requestedOfferId: String?,
    currentOfferId: String?,
    destinationReady: Boolean,
    alreadyResumedOfferId: String?,
): Boolean =
    requestedOfferId != null &&
        requestedOfferId == currentOfferId &&
        destinationReady &&
        requestedOfferId != alreadyResumedOfferId

private fun inspectCustomDestination(
    context: Context,
    uri: Uri,
): RoomCustomDestinationSnapshot {
    val hasWriteGrant =
        runCatching {
            context.contentResolver.persistedUriPermissions.any { permission ->
                permission.uri == uri && permission.isWritePermission
            }
        }.getOrDefault(false)
    val document =
        runCatching {
            DocumentFile.fromTreeUri(context, uri)
        }.getOrNull()
    return RoomCustomDestinationSnapshot(
        hasPersistedWriteGrant = hasWriteGrant,
        exists = runCatching { document?.exists() == true }.getOrDefault(false),
        isDirectory = runCatching { document?.isDirectory == true }.getOrDefault(false),
        canWrite = runCatching { document?.canWrite() == true }.getOrDefault(false),
        name = runCatching { document?.name }.getOrNull(),
    )
}
