package dev.envoix.app.ui

object EnvoixTestTags {
    const val HOME_ROOT = "home_root"
    const val TRANSFER_TAB = "transfer_tab"
    const val ACTIVITY_TAB = "activity_tab"
    const val SETTINGS_TAB = "settings_tab"
    const val NEW_TRANSFER_BUTTON = "new_transfer_button"
    const val LOGS_BUTTON = "logs_button"
    const val ACTIVITY_LIST = "activity_list"
    const val SETTINGS_BUTTON = "settings_button"
    const val NEW_TRANSFER_SHEET = "new_transfer_sheet"
    const val SHOW_QR_TAB = "show_qr_tab"
    const val SCAN_QR_TAB = "scan_qr_tab"
    const val ROOM_CODE = "room_code"
    const val JOIN_CODE_FIELD = "join_code_field"
    const val ROLE_SEND = "role_send"
    const val ROLE_RECEIVE = "role_receive"
    const val SAVE_PATH_ROW = "save_path_row"
    const val FILE_PICKER_ROW = "file_picker_row"
    const val START_TRANSFER_BUTTON = "start_transfer_button"

    fun activityTitle(id: Long) = "activity_title_$id"

    fun activityAction(
        id: Long,
        action: String,
    ) = "activity_${action.lowercase()}_$id"
}
