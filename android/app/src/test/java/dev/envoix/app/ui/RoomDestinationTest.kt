package dev.envoix.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RoomDestinationTest {
    @Test
    fun `downloads is ready only for file-only offers without a custom folder`() {
        val fileOnly =
            destination(
                directoryCount = 0,
                customDestination = null,
            )
        val withFolder =
            destination(
                directoryCount = 1,
                customDestination = null,
            )

        assertTrue(fileOnly.ready)
        assertEquals(RoomDestinationAvailability.Ready, fileOnly.availability)
        assertEquals("Downloads / Envoix", fileOnly.label)
        assertFalse(withFolder.ready)
        assertEquals(RoomDestinationAvailability.RequiresFolder, withFolder.availability)
        assertEquals("Choose a folder", withFolder.label)
    }

    @Test
    fun `every custom destination validity signal is required`() {
        val valid =
            RoomCustomDestinationSnapshot(
                hasPersistedWriteGrant = true,
                exists = true,
                isDirectory = true,
                canWrite = true,
                name = "Shared files",
            )
        val invalidDestinations =
            listOf(
                valid.copy(hasPersistedWriteGrant = false),
                valid.copy(exists = false),
                valid.copy(isDirectory = false),
                valid.copy(canWrite = false),
            )

        invalidDestinations.forEach { customDestination ->
            val result =
                destination(
                    directoryCount = 0,
                    customDestination = customDestination,
                )
            assertFalse(result.ready)
            assertEquals(RoomDestinationAvailability.Unavailable, result.availability)
            assertEquals("Unavailable folder", result.label)
        }
    }

    @Test
    fun `valid custom destination uses its name without falling back to Downloads`() {
        val result =
            destination(
                directoryCount = 2,
                customDestination =
                    RoomCustomDestinationSnapshot(
                        hasPersistedWriteGrant = true,
                        exists = true,
                        isDirectory = true,
                        canWrite = true,
                        name = "Shared files",
                    ),
            )

        assertTrue(result.ready)
        assertEquals("Shared files", result.label)
        assertEquals(
            "Selected folder",
            destination(
                directoryCount = 2,
                customDestination =
                    RoomCustomDestinationSnapshot(
                        hasPersistedWriteGrant = true,
                        exists = true,
                        isDirectory = true,
                        canWrite = true,
                        name = " ",
                    ),
            ).label,
        )
    }

    @Test
    fun `destination repair resumes only the same ready offer once`() {
        assertTrue(
            shouldResumeRoomOfferAfterDestinationRepair(
                requestedOfferId = "offer-1",
                currentOfferId = "offer-1",
                destinationReady = true,
                alreadyResumedOfferId = null,
            ),
        )
        assertFalse(
            shouldResumeRoomOfferAfterDestinationRepair(
                requestedOfferId = "offer-1",
                currentOfferId = "offer-2",
                destinationReady = true,
                alreadyResumedOfferId = null,
            ),
        )
        assertFalse(
            shouldResumeRoomOfferAfterDestinationRepair(
                requestedOfferId = null,
                currentOfferId = "offer-1",
                destinationReady = true,
                alreadyResumedOfferId = null,
            ),
        )
        assertFalse(
            shouldResumeRoomOfferAfterDestinationRepair(
                requestedOfferId = "offer-1",
                currentOfferId = "offer-1",
                destinationReady = false,
                alreadyResumedOfferId = null,
            ),
        )
        assertFalse(
            shouldResumeRoomOfferAfterDestinationRepair(
                requestedOfferId = "offer-1",
                currentOfferId = "offer-1",
                destinationReady = true,
                alreadyResumedOfferId = "offer-1",
            ),
        )
    }

    private fun destination(
        directoryCount: Int,
        customDestination: RoomCustomDestinationSnapshot?,
    ) = resolveRoomDestinationPresentation(
        directoryCount = directoryCount,
        customDestination = customDestination,
        downloadsLabel = "Downloads / Envoix",
        chooseFolderLabel = "Choose a folder",
        unavailableLabel = "Unavailable folder",
        selectedFolderLabel = "Selected folder",
    )
}
