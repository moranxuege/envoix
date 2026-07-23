package dev.envoix.app

import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import dev.envoix.app.ui.AppText
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ManifestV2AppUiInstrumentedTest {
    @Test(timeout = TEST_TIMEOUT_MS)
    fun sendAndReceiveExposeCanonicalInventoryControlsInBothLanguages() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val originalLanguage = SettingsStore.settings.value.language
        SettingsStore.update { it.copy(language = AppText.ENGLISH) }
        val device = UiDevice.getInstance(instrumentation)
        device.executeShellCommand("am start -W -n ${context.packageName}/.MainActivity")
        SystemClock.sleep(UI_TRANSITION_SETTLE_MS)

        try {
            var sheet = openSheet(device, HOME_SEND)

            textAfterScroll(device, sheet, "ADD FILES")
            textAfterScroll(device, sheet, "ADD FOLDER")
            resource(device, TRANSFER_START)

            dismissSheet(device)
            sheet = openSheet(device, HOME_RECEIVE)
            textAfterScroll(device, sheet, "SAVE TO")
            resource(device, TRANSFER_START)
            assertFalse(device.hasObject(By.textContains("SAF/MediaStore")))
            assertFalse(device.hasObject(By.textContains("Verify privately")))

            dismissSheet(device)
            SettingsStore.update { it.copy(language = AppText.SIMPLIFIED_CHINESE) }
            text(device, "传输文件")
            sheet = openSheet(device, HOME_SEND)
            textAfterScroll(device, sheet, "添加文件")
            textAfterScroll(device, sheet, "添加文件夹")
            resource(device, TRANSFER_START)

            dismissSheet(device)
            sheet = openSheet(device, HOME_RECEIVE)
            textAfterScroll(device, sheet, "保存到")
            resource(device, TRANSFER_START)
            assertFalse(device.hasObject(By.textContains("SAF/MediaStore")))
            assertFalse(device.hasObject(By.textContains("先在私有目录验证")))
        } finally {
            SettingsStore.update { it.copy(language = originalLanguage) }
            device.pressHome()
        }
    }

    private fun resource(
        device: UiDevice,
        id: String,
    ): UiObject2 = checkNotNull(device.wait(Until.findObject(By.res(id)), WAIT_TIMEOUT_MS)) { "Missing UI resource: $id" }

    private fun text(
        device: UiDevice,
        value: String,
    ): UiObject2 = checkNotNull(device.wait(Until.findObject(By.text(value)), WAIT_TIMEOUT_MS)) { "Missing UI text: $value" }

    private fun clickResource(
        device: UiDevice,
        id: String,
    ) {
        device.waitForIdle()
        resource(device, id).click()
    }

    private fun openSheet(
        device: UiDevice,
        homeAction: String,
    ): UiObject2 {
        repeat(OPEN_SHEET_ATTEMPTS) {
            clickResource(device, homeAction)
            device.wait(Until.findObject(By.res(TRANSFER_SHEET)), OPEN_SHEET_WAIT_MS)?.let { return it }
        }
        error("Transfer sheet did not open from: $homeAction")
    }

    private fun textAfterScroll(
        device: UiDevice,
        sheet: UiObject2,
        value: String,
    ): UiObject2 {
        device.wait(Until.findObject(By.text(value)), SHORT_WAIT_MS)?.let { return it }
        repeat(MAX_SCROLLS) {
            sheet.scroll(Direction.DOWN, SCROLL_PERCENT)
            device.wait(Until.findObject(By.text(value)), SHORT_WAIT_MS)?.let { return it }
        }
        error("Missing UI text after scrolling: $value")
    }

    private fun dismissSheet(device: UiDevice) {
        device.pressBack()
        check(device.wait(Until.gone(By.res(TRANSFER_SHEET)), WAIT_TIMEOUT_MS)) {
            "Transfer sheet did not close"
        }
        SystemClock.sleep(UI_TRANSITION_SETTLE_MS)
    }

    private companion object {
        const val HOME_SEND = "home_send"
        const val HOME_RECEIVE = "home_receive"
        const val TRANSFER_SHEET = "transfer_sheet"
        const val TRANSFER_START = "transfer_start"
        const val TEST_TIMEOUT_MS = 60_000L
        const val WAIT_TIMEOUT_MS = 15_000L
        const val SHORT_WAIT_MS = 500L
        const val UI_TRANSITION_SETTLE_MS = 750L
        const val OPEN_SHEET_WAIT_MS = 3_000L
        const val OPEN_SHEET_ATTEMPTS = 3
        const val MAX_SCROLLS = 5
        const val SCROLL_PERCENT = 0.8f
    }
}
