package dev.envoix.app.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class TransferActivityPresentationTest {
    @Test
    fun `recorded destination remains stable after settings change`() {
        assertEquals(
            "Family archive",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = "Family archive",
                fallbackDestinationLabel = "New downloads folder",
            ),
        )
    }

    @Test
    fun `legacy activity without recorded destination uses current setting`() {
        assertEquals(
            "Current downloads folder",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = null,
                fallbackDestinationLabel = "Current downloads folder",
            ),
        )
        assertEquals(
            "Current downloads folder",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = " ",
                fallbackDestinationLabel = "Current downloads folder",
            ),
        )
    }

    @Test
    fun `delivered activity uses configured destination in both languages`() {
        assertEquals(
            "Saved to Family archive · tap to open",
            savedDestinationSubtitle("Family archive", AppText.ENGLISH),
        )
        assertEquals(
            "已保存到 家庭归档 · 点击打开",
            savedDestinationSubtitle("家庭归档", AppText.SIMPLIFIED_CHINESE),
        )
    }

    @Test
    fun `delivered activity falls back to Downloads for an empty destination`() {
        assertEquals(
            "Saved to Downloads · tap to open",
            savedDestinationSubtitle("  ", AppText.ENGLISH),
        )
        assertEquals(
            "Downloads",
            resolvedSavedDestinationLabel(
                recordedDestinationLabel = null,
                fallbackDestinationLabel = " ",
            ),
        )
    }
}
