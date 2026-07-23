package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import dev.envoix.app.ui.AppText
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

        try {
            resource(device, HOME_NEW_TRANSFER).click()
            var sheet = resource(device, TRANSFER_SHEET)

            resource(device, TRANSFER_ROLE_SEND).click()
            textAfterScroll(device, sheet, "ADD FILES")
            textAfterScroll(device, sheet, "ADD FOLDER")

            device.pressBack()
            resource(device, HOME_NEW_TRANSFER).click()
            sheet = resource(device, TRANSFER_SHEET)
            resource(device, TRANSFER_ROLE_RECEIVE).click()
            textAfterScroll(device, sheet, "SAVE TO")
            textAfterScroll(device, sheet, "Verify privately, then save")

            device.pressBack()
            SettingsStore.update { it.copy(language = AppText.SIMPLIFIED_CHINESE) }
            text(device, "新建传输")
            resource(device, HOME_NEW_TRANSFER).click()
            sheet = resource(device, TRANSFER_SHEET)
            resource(device, TRANSFER_ROLE_SEND).click()
            textAfterScroll(device, sheet, "添加文件")
            textAfterScroll(device, sheet, "添加文件夹")

            device.pressBack()
            resource(device, HOME_NEW_TRANSFER).click()
            sheet = resource(device, TRANSFER_SHEET)
            resource(device, TRANSFER_ROLE_RECEIVE).click()
            textAfterScroll(device, sheet, "保存到")
            textAfterScroll(device, sheet, "先在私有目录验证，再保存")
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

    private companion object {
        const val HOME_NEW_TRANSFER = "home_new_transfer"
        const val TRANSFER_SHEET = "transfer_sheet"
        const val TRANSFER_ROLE_SEND = "transfer_role_send"
        const val TRANSFER_ROLE_RECEIVE = "transfer_role_receive"
        const val TEST_TIMEOUT_MS = 60_000L
        const val WAIT_TIMEOUT_MS = 15_000L
        const val SHORT_WAIT_MS = 500L
        const val MAX_SCROLLS = 5
        const val SCROLL_PERCENT = 0.8f
    }
}
