package dev.envoix.app

import android.Manifest
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import dev.envoix.app.ui.EnvoixTestTags
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class EnvoixSmokeTest {
    @get:Rule
    val permissions: GrantPermissionRule = GrantPermissionRule.grant(
        Manifest.permission.CAMERA,
        Manifest.permission.POST_NOTIFICATIONS,
    )

    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    @Test
    fun opensNewTransferSheetWithStableControls() {
        composeRule.onNodeWithTag(EnvoixTestTags.HOME_ROOT).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.NEW_TRANSFER_BUTTON).performClick()

        composeRule.onNodeWithTag(EnvoixTestTags.NEW_TRANSFER_SHEET).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.SHOW_QR_TAB).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.SCAN_QR_TAB).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.ROOM_CODE).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.JOIN_CODE_FIELD).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.ROLE_RECEIVE).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.SAVE_PATH_ROW).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.START_TRANSFER_BUTTON).assertIsEnabled()

        composeRule.onNodeWithTag(EnvoixTestTags.ROLE_SEND).performClick()
        composeRule.onNodeWithTag(EnvoixTestTags.FILE_PICKER_ROW).assertIsDisplayed()
        composeRule.onNodeWithTag(EnvoixTestTags.START_TRANSFER_BUTTON).assertIsNotEnabled()
    }
}
