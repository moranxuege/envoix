package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

class ManifestSessionIdFloorTest {
    @Test
    fun startsAtOneWithoutPersistedState() {
        assertEquals(1L, nextManifestSessionIdFloor(emptyList(), emptyList()))
    }

    @Test
    fun retainedReceiverWorkspacePreventsIdReuseAfterTerminalSpecRemoval() {
        assertEquals(
            8L,
            nextManifestSessionIdFloor(
                persistedIds = emptyList(),
                retainedWorkspaceNames = listOf("1", "7", "not-a-session"),
            ),
        )
    }

    @Test
    fun usesHighestValidIdAcrossSpecsAndWorkspaces() {
        assertEquals(
            12L,
            nextManifestSessionIdFloor(
                persistedIds = listOf(4L, 11L),
                retainedWorkspaceNames = listOf("3", "9", "0", "-1"),
            ),
        )
    }

    @Test
    fun retainedSenderLogPreventsCrossSessionLogContamination() {
        assertEquals(
            43L,
            nextManifestSessionIdFloor(
                persistedIds = emptyList(),
                retainedWorkspaceNames = emptyList(),
                retainedLogNames =
                    listOf(
                        "transfer-42.raw.log",
                        "transfer-42.timeline.log",
                        "not-a-transfer.log",
                    ),
            ),
        )
    }
}
