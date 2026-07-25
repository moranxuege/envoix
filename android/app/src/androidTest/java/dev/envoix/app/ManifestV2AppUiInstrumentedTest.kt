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
    fun connectionFirstRoomExposesCanonicalInventoryControlsInBothLanguages() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val originalLanguage = SettingsStore.settings.value.language
        SettingsStore.update { it.copy(language = AppText.ENGLISH) }
        val device = UiDevice.getInstance(instrumentation)
        device.executeShellCommand("am start -W -n ${context.packageName}/.MainActivity")
        SystemClock.sleep(UI_TRANSITION_SETTLE_MS)

        try {
            exerciseConnectionFirstFlow(
                device = device,
                copy =
                    FlowCopy(
                        connectTitle = "Connect to a device",
                        activity = "Activity",
                        settings = "Settings",
                        showQr = "Show QR",
                        openRoom = "Open room",
                        roomSubtitle = "One-time room",
                        noActiveTransfer = "Ready · unverified",
                        reviewInvite = "Review invite",
                        addFiles = "Add files",
                        inventoryFiles = "ADD FILES",
                        inventoryFolder = "ADD FOLDER",
                        saveTo = "SAVE TO",
                    ),
            )

            SettingsStore.update { it.copy(language = AppText.SIMPLIFIED_CHINESE) }
            exerciseConnectionFirstFlow(
                device = device,
                copy =
                    FlowCopy(
                        connectTitle = "连接设备",
                        activity = "活动",
                        settings = "设置",
                        showQr = "显示二维码",
                        openRoom = "打开房间",
                        roomSubtitle = "一次性房间",
                        noActiveTransfer = "已就绪 · 未验证",
                        reviewInvite = "查看邀请",
                        addFiles = "添加文件",
                        inventoryFiles = "添加文件",
                        inventoryFolder = "添加文件夹",
                        saveTo = "保存到",
                    ),
            )
        } finally {
            device.setOrientationNatural()
            SettingsStore.update { it.copy(language = originalLanguage) }
            device.pressHome()
        }
    }

    private fun exerciseConnectionFirstFlow(
        device: UiDevice,
        copy: FlowCopy,
    ) {
        text(device, copy.connectTitle)

        clickResourceUntilText(device, HUB_ACTIVITY, copy.activity)
        device.pressBack()
        text(device, copy.connectTitle)

        text(device, copy.showQr)
        clickResourceUntilText(device, HUB_SHOW_QR, copy.openRoom)
        clickResourceUntilText(device, HUB_OPEN_ROOM, copy.roomSubtitle)

        text(device, copy.noActiveTransfer)
        assertFalse(device.hasObject(By.res(TRANSFER_SHEET)))

        clickResourceUntilText(device, ROOM_ACTIVITY, copy.activity)
        device.pressBack()
        text(device, copy.roomSubtitle)
        clickResourceUntilText(device, ROOM_SETTINGS, copy.settings)
        device.pressBack()
        text(device, copy.roomSubtitle)

        assertRoomSurvivesRotation(device, copy)

        text(device, copy.reviewInvite)
        var sheet = clickResourceUntilResource(device, ROOM_REVIEW_INVITE, TRANSFER_SHEET)
        textAfterScroll(sheet, copy.saveTo)
        resource(device, TRANSFER_START)
        assertFalse(device.hasObject(By.textContains("SAF/MediaStore")))
        assertFalse(device.hasObject(By.textContains("Verify privately")))
        assertFalse(device.hasObject(By.textContains("先在私有目录验证")))

        dismissSheet(device)
        text(device, copy.roomSubtitle)
        text(device, copy.noActiveTransfer)

        text(device, copy.addFiles)
        sheet = clickResourceUntilResource(device, ROOM_ADD_FILES, TRANSFER_SHEET)
        textAfterScroll(sheet, copy.inventoryFiles)
        textAfterScroll(sheet, copy.inventoryFolder)
        resource(device, TRANSFER_START)

        dismissSheet(device)
        device.pressBack()
        text(device, copy.connectTitle)
    }

    private fun assertRoomSurvivesRotation(
        device: UiDevice,
        copy: FlowCopy,
    ) {
        device.setOrientationLeft()
        SystemClock.sleep(ORIENTATION_SETTLE_MS)
        text(device, copy.roomSubtitle)
        text(device, copy.noActiveTransfer)

        device.setOrientationNatural()
        SystemClock.sleep(ORIENTATION_SETTLE_MS)
        text(device, copy.roomSubtitle)
        text(device, copy.reviewInvite)
    }

    private fun resource(
        device: UiDevice,
        id: String,
    ): UiObject2 = checkNotNull(device.wait(Until.findObject(By.res(id)), WAIT_TIMEOUT_MS)) { "Missing UI resource: $id" }

    private fun text(
        device: UiDevice,
        value: String,
    ): UiObject2 = checkNotNull(device.wait(Until.findObject(By.text(value)), WAIT_TIMEOUT_MS)) { "Missing UI text: $value" }

    private fun clickResourceUntilText(
        device: UiDevice,
        id: String,
        targetText: String,
    ): UiObject2 {
        repeat(CLICK_ATTEMPTS) {
            device.waitForIdle()
            resource(device, id).click()
            device.wait(Until.findObject(By.text(targetText)), CLICK_WAIT_MS)?.let { return it }
        }
        error("Clicking $id did not reveal text: $targetText")
    }

    private fun clickResourceUntilResource(
        device: UiDevice,
        id: String,
        targetId: String,
    ): UiObject2 {
        repeat(CLICK_ATTEMPTS) {
            device.waitForIdle()
            resource(device, id).click()
            device.wait(Until.findObject(By.res(targetId)), CLICK_WAIT_MS)?.let { return it }
        }
        error("Clicking $id did not reveal resource: $targetId")
    }

    private fun textAfterScroll(
        sheet: UiObject2,
        value: String,
    ): UiObject2 {
        sheet.findObject(By.text(value))?.let { return it }
        repeat(MAX_SCROLLS) {
            sheet.scroll(Direction.DOWN, SCROLL_PERCENT)
            SystemClock.sleep(SHORT_WAIT_MS)
            sheet.findObject(By.text(value))?.let { return it }
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
        const val TRANSFER_SHEET = "transfer_sheet"
        const val TRANSFER_START = "transfer_start"
        const val HUB_ACTIVITY = "hub_activity"
        const val HUB_SHOW_QR = "hub_show_qr"
        const val HUB_OPEN_ROOM = "hub_open_room"
        const val ROOM_REVIEW_INVITE = "room_review_invite"
        const val ROOM_ADD_FILES = "room_add_files"
        const val ROOM_ACTIVITY = "room_activity"
        const val ROOM_SETTINGS = "room_settings"
        const val TEST_TIMEOUT_MS = 90_000L
        const val WAIT_TIMEOUT_MS = 15_000L
        const val SHORT_WAIT_MS = 500L
        const val CLICK_WAIT_MS = 3_000L
        const val UI_TRANSITION_SETTLE_MS = 750L
        const val ORIENTATION_SETTLE_MS = 1_250L
        const val CLICK_ATTEMPTS = 3
        const val MAX_SCROLLS = 5
        const val SCROLL_PERCENT = 0.8f
    }

    private data class FlowCopy(
        val connectTitle: String,
        val activity: String,
        val settings: String,
        val showQr: String,
        val openRoom: String,
        val roomSubtitle: String,
        val noActiveTransfer: String,
        val reviewInvite: String,
        val addFiles: String,
        val inventoryFiles: String,
        val inventoryFolder: String,
        val saveTo: String,
    )
}
