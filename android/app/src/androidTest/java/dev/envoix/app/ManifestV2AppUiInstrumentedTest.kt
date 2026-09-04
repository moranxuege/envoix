package dev.envoix.app

import android.app.Activity
import android.app.Instrumentation
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import dev.envoix.app.ui.AppLanguage
import dev.envoix.app.ui.RoomCloseReason
import dev.envoix.app.ui.RoomControlEndpoint
import dev.envoix.app.ui.RoomControlEvent
import dev.envoix.app.ui.RoomControlGateway
import dev.envoix.app.ui.RoomControlGatewayProvider
import dev.envoix.app.ui.RoomControlInvite
import dev.envoix.app.ui.RoomLifetimePolicy
import dev.envoix.app.ui.RoomLifetimeSnapshot
import dev.envoix.app.ui.RoomTransferOfferDraft
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
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
        val originalGateway = RoomControlGatewayProvider.gateway
        val gateway = ImmediatelyConnectedRoomGateway()
        SettingsStore.update { it.copy(language = AppLanguage.ENGLISH) }
        RoomControlGatewayProvider.gateway = gateway
        val device = UiDevice.getInstance(instrumentation)

        try {
            device.executeShellCommand("am start -W -n ${context.packageName}/.MainActivity")
            SystemClock.sleep(UI_TRANSITION_SETTLE_MS)
            exerciseConnectionFirstFlow(
                device = device,
                gateway = gateway,
                copy =
                    FlowCopy(
                        activity = "Activity",
                        settings = "Settings",
                        revealInvite = "Tap to reveal your room QR and code",
                        roomTitle = "ROOM",
                        roomStatus = "Authenticated for this room",
                        roomEnded = "ROOM ENDED",
                        roomClosed = "Room closed",
                        done = "Done",
                        addFiles = "Add files",
                        inventoryFiles = "ADD FILES",
                        inventoryFolder = "ADD FOLDER",
                    ),
            )

            SettingsStore.update { it.copy(language = AppLanguage.SIMPLIFIED_CHINESE) }
            exerciseConnectionFirstFlow(
                device = device,
                gateway = gateway,
                copy =
                    FlowCopy(
                        activity = "活动",
                        settings = "设置",
                        revealInvite = "轻触显示房间二维码和房间码",
                        roomTitle = "房间",
                        roomStatus = "已为此房间认证",
                        roomEnded = "房间已结束",
                        roomClosed = "房间已关闭",
                        done = "完成",
                        addFiles = "添加文件",
                        inventoryFiles = "添加文件",
                        inventoryFolder = "添加文件夹",
                    ),
            )
        } finally {
            device.setOrientationNatural()
            finishTargetActivities(instrumentation)
            RoomControlGatewayProvider.gateway = originalGateway
            SettingsStore.update { it.copy(language = originalLanguage) }
            device.pressHome()
        }
    }

    private fun exerciseConnectionFirstFlow(
        device: UiDevice,
        gateway: ImmediatelyConnectedRoomGateway,
        copy: FlowCopy,
    ) {
        resource(device, CONNECTION_HUB)
        text(device, "Envoix")

        clickResourceUntilText(device, HUB_ACTIVITY, copy.activity)
        device.pressBack()
        resource(device, CONNECTION_HUB)

        text(device, copy.revealInvite)
        clickResourceUntilText(device, HUB_ROOM_QR_TOGGLE, copy.roomTitle)

        text(device, copy.roomStatus)
        assertFalse(device.hasObject(By.res(TRANSFER_SHEET)))

        clickResourceUntilText(device, ROOM_ACTIVITY, copy.activity)
        device.pressBack()
        text(device, copy.roomTitle)
        clickResourceUntilText(device, ROOM_SETTINGS, copy.settings)
        device.pressBack()
        text(device, copy.roomTitle)

        assertRoomSurvivesRotation(device, copy)

        text(device, copy.addFiles)
        val sheet = clickResourceUntilResource(device, ROOM_ADD_FILES, TRANSFER_SHEET)
        textAfterScroll(sheet, copy.inventoryFiles)
        textAfterScroll(sheet, copy.inventoryFolder)
        resource(device, TRANSFER_START)

        dismissSheet(device)
        text(device, copy.roomTitle)
        text(device, copy.roomStatus)

        gateway.endRoom()
        text(device, copy.roomEnded)
        text(device, copy.roomClosed)
        assertFalse(resource(device, ROOM_ADD_FILES).isEnabled)
        text(device, copy.done).click()
        resource(device, CONNECTION_HUB)
        resource(device, HUB_ROOM_INVITE)
    }

    private fun assertRoomSurvivesRotation(
        device: UiDevice,
        copy: FlowCopy,
    ) {
        device.setOrientationLeft()
        SystemClock.sleep(ORIENTATION_SETTLE_MS)
        text(device, copy.roomTitle)
        text(device, copy.roomStatus)

        device.setOrientationNatural()
        SystemClock.sleep(ORIENTATION_SETTLE_MS)
        text(device, copy.roomTitle)
        text(device, copy.addFiles)
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

    private fun finishTargetActivities(instrumentation: Instrumentation) {
        instrumentation.runOnMainSync {
            Stage.entries
                .flatMap { stage ->
                    ActivityLifecycleMonitorRegistry
                        .getInstance()
                        .getActivitiesInStage(stage)
                }.filterIsInstance<MainActivity>()
                .distinct()
                .forEach(Activity::finish)
        }
        instrumentation.waitForIdleSync()
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
        const val CONNECTION_HUB = "connection_hub"
        const val HUB_ACTIVITY = "hub_activity"
        const val HUB_ROOM_INVITE = "hub_room_invite"
        const val HUB_ROOM_QR_TOGGLE = "hub_room_qr_toggle"
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
        val activity: String,
        val settings: String,
        val revealInvite: String,
        val roomTitle: String,
        val roomStatus: String,
        val roomEnded: String,
        val roomClosed: String,
        val done: String,
        val addFiles: String,
        val inventoryFiles: String,
        val inventoryFolder: String,
    )
}

private class ImmediatelyConnectedRoomGateway : RoomControlGateway {
    private val mutableEvents = MutableSharedFlow<RoomControlEvent>(extraBufferCapacity = 8)
    override val available = true
    override val events: Flow<RoomControlEvent> = mutableEvents

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        val endpoint = RoomControlEndpoint(broker, relay)
        mutableEvents.emit(
            RoomControlEvent.Hosting(
                RoomControlInvite(
                    code = "123456-a1b2-c3d4",
                    payload = "envoix://room/123456-a1b2-c3d4",
                    endpoint = endpoint,
                    expiresAtEpochMs = System.currentTimeMillis() + ROOM_LIFETIME_MS,
                ),
            ),
        )
        mutableEvents.emit(
            RoomControlEvent.Connected(
                peerName = "Test device",
                creator = true,
                endpoint = endpoint,
                lifetime =
                    RoomLifetimeSnapshot(
                        revision = 1,
                        policy = RoomLifetimePolicy.Idle15Minutes,
                        idleDeadlineEpochMs = System.currentTimeMillis() + ROOM_LIFETIME_MS,
                    ),
            ),
        )
    }

    fun endRoom() {
        check(mutableEvents.tryEmit(RoomControlEvent.Closed(RoomCloseReason.UserEnded)))
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) = Unit

    override suspend fun refreshInvite() = Unit

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) = Unit

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) = Unit

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) = Unit

    override suspend fun updateTransferActive(active: Boolean) = Unit

    override suspend fun close(reason: RoomCloseReason) {
        mutableEvents.emit(RoomControlEvent.Closed(reason))
    }

    private companion object {
        const val ROOM_LIFETIME_MS = 15 * 60 * 1_000L
    }
}
