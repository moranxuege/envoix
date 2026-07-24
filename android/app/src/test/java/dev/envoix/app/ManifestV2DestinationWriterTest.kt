package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

class ManifestV2DestinationWriterTest {
    @Test
    fun keepBothPreservesFileExtension() {
        assertEquals("report (1).txt", manifestV2KeepBothName("report.txt", 1, true))
        assertEquals(".gitignore (1)", manifestV2KeepBothName(".gitignore", 1, true))
    }

    @Test
    fun keepBothTreatsDotsInDirectoryNamesAsOrdinaryCharacters() {
        assertEquals("Folder.v1 (1)", manifestV2KeepBothName("Folder.v1", 1, false))
    }
}
