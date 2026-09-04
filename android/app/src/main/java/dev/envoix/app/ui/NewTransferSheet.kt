package dev.envoix.app.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.InviteCodec
import dev.envoix.app.R
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import java.util.UUID

internal typealias PrepareReceiveBeforeDecision = (
    code: String,
    broker: String,
    relay: String,
    qrPayload: String?,
    copyApproved: Boolean,
    completion: (id: Long, error: String?) -> Unit,
) -> Unit

internal typealias QueuePreparedSend = (
    jobId: String,
    rootNames: List<String>,
    itemCount: Int,
    directoryCount: Int,
    totalBytes: Long,
    completion: (String?) -> Unit,
) -> Unit

internal data class TransferSetupPreferences(
    val broker: String,
    val relay: String,
    val defaultRole: String,
    val compressionPolicy: String,
    val saveLocationLabel: String,
    val savePickerInitialUri: Uri,
)

/**
 * Role-specific transfer setup. Scanning an invite may switch to the opposite
 * role; the change is explained inline instead of interrupting with a dialog.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun NewTransferSheet(
    draftId: String = "standalone",
    preparationState: TransferDraftPreparationState? = null,
    onReceive: (
        code: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        copyApproved: Boolean,
        rememberLabel: String?,
        rememberedRelationshipId: String?,
    ) -> Unit,
    onSend: (
        code: String,
        broker: String,
        relay: String,
        jobId: String,
        qrPayload: String?,
        rememberLabel: String?,
        rememberedRelationshipId: String?,
    ) -> Unit,
    preferences: TransferSetupPreferences,
    sourcePreparationIntents: TransferSourcePreparationIntents,
    onSaveTreePicked: (Uri) -> Unit,
    nearbySelection: NearbyPairingSelection? = null,
    nearbyDeliveryAvailable: Boolean = true,
    initialPairingInput: String? = null,
    initialSources: List<android.net.Uri> = emptyList(),
    initialRole: String? = null,
    roomMode: Boolean = false,
    connectedRoom: Boolean = false,
    roomEndpoint: RoomControlEndpoint? = null,
    showQrInitially: Boolean = false,
    onExternalActivityChanged: (Boolean) -> Unit = {},
    onOfferInvite: ((offer: RoomTransferOfferDraft, completion: (error: String?) -> Unit) -> Unit)? = null,
    onBeforeStart: ((completion: (error: String?) -> Unit) -> Unit)? = null,
    onPrepareReceiveBeforeDecision: PrepareReceiveBeforeDecision? = null,
    onCancelPreparedReceive: (Long) -> Unit = {},
    onPreparedReceiveCommitted: (String) -> Unit = {},
    onQueuePreparedSend: QueuePreparedSend? = null,
) {
    val colors = Envoix.colors
    val clipboard = LocalClipboardManager.current
    val switchedToSendNotice = appString(R.string.switched_to_send_notice)
    val switchedToReceiveNotice = appString(R.string.switched_to_receive_notice)
    val invalidInvitationError = appString(R.string.transfer_setup_complete_invitation_required)
    val clipboardEmptyError = appString(R.string.hub_clipboard_empty)
    val invalidFlowError = appString(R.string.transfer_setup_invalid_invitation_flow)
    val receiveSetupClosedError = appString(R.string.transfer_setup_receive_closed)
    val sourcePreparationMessages =
        TransferSourcePreparationMessages(
            prepareFailed = appString(R.string.transfer_setup_prepare_source_failed),
            removeFailed = appString(R.string.transfer_setup_remove_source_failed),
            selectionChanged = appString(R.string.transfer_setup_selection_changed),
            authorizationFailed = appString(R.string.transfer_setup_authorize_folder_failed),
        )
    val broker = roomEndpoint?.broker ?: preferences.broker
    val relay = roomEndpoint?.relay ?: preferences.relay

    val fallbackPreparation =
        remember(draftId) {
            TransferDraftPreparationState(
                initialRole = initialRole ?: preferences.defaultRole,
                showQrInitially = showQrInitially,
            )
        }
    val preparation = preparationState ?: fallbackPreparation
    var role by preparation.role
    var typed by preparation.typedCode
    var invitationInput by preparation.invitationInput
    var generated by preparation.generatedInvite
    var generatedRole by preparation.generatedInviteRole
    var scannedBroker by preparation.scannedBroker
    var scannedRelay by preparation.scannedRelay
    val preparedSources = preparation.preparedSources
    var preparedJobId by preparation.preparedJobId
    var preparationSummary by preparation.summary
    var preparingCount by preparation.preparingCount
    var preparationError by preparation.error
    var sourceAwaitingReauthorization by preparation.sourceAwaitingReauthorization
    var roleChangeNotice by preparation.roleChangeNotice
    var topMode by preparation.topMode // "closed" | "show" | "scan"
    var rendezvousBusy by preparation.rendezvousBusy
    var rendezvousError by preparation.rendezvousError
    var initialPairingInputApplied by preparation.initialPairingInputApplied
    var startSubmitted by preparation.startSubmitted
    var rememberAfterPairing by remember { mutableStateOf(false) }
    var rememberLabel by remember { mutableStateOf("") }
    var pairingInputError by remember(draftId) { mutableStateOf<String?>(null) }

    DisposableEffect(preparation, preparationState) {
        onDispose {
            if (preparationState == null) preparation.discard()
        }
    }

    val filePicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            onExternalActivityChanged(false)
            sourcePreparationIntents.addSources(
                preparation,
                uris,
                false,
                preferences.compressionPolicy,
                sourcePreparationMessages,
            )
        }
    val sourceFolderPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            uri?.let {
                sourcePreparationIntents.addSources(
                    preparation,
                    listOf(it),
                    true,
                    preferences.compressionPolicy,
                    sourcePreparationMessages,
                )
            }
        }
    val sourceReauthorizationPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            val previous = sourceAwaitingReauthorization
            sourceAwaitingReauthorization = null
            if (uri != null && previous != null) {
                sourcePreparationIntents.reauthorizeSource(
                    preparation,
                    previous,
                    uri,
                    sourcePreparationMessages,
                )
            }
        }
    val saveFolderPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            if (uri != null) onSaveTreePicked(uri)
        }
    val joining = invitationInput != null || typed.isNotBlank()

    LaunchedEffect(draftId, initialSources, initialRole) {
        if (initialSources.isNotEmpty()) {
            role = "send"
            sourcePreparationIntents.addSources(
                preparation,
                initialSources,
                false,
                preferences.compressionPolicy,
                sourcePreparationMessages,
            )
        }
    }

    // Generate once per role. The draft owns this value so rotation cannot
    // silently replace a QR that the other device is already scanning.
    LaunchedEffect(draftId, role, joining) {
        if (!joining && (generated == null || generatedRole != role)) {
            generated = InviteCodec.generate(role, broker, relay)
            generatedRole = role
        }
    }

    fun applyScanned(scanned: String): Boolean {
        val inv = InviteCodec.parseForRouting(scanned) ?: return false
        pairingInputError = null
        invitationInput = scanned
        typed = ""
        scannedBroker = inv.broker
        scannedRelay = inv.relay
        val scannedRole = inv.joinerRole
        if (scannedRole != role) {
            role = scannedRole
            roleChangeNotice =
                if (scannedRole == "send") {
                    switchedToSendNotice
                } else {
                    switchedToReceiveNotice
                }
        }
        topMode = "closed" // stop the camera; the code is filled in now
        return true
    }

    fun applyPairingInput(input: String): Boolean {
        pairingInputError = null
        if (input.startsWith("envoix:") && applyScanned(input)) {
            return true
        }
        typed = input
        invitationInput = null
        scannedBroker = null
        scannedRelay = null
        return false
    }

    LaunchedEffect(draftId, initialPairingInput) {
        if (!initialPairingInputApplied) {
            initialPairingInput?.takeIf(String::isNotBlank)?.let {
                if (!applyPairingInput(it)) pairingInputError = invalidInvitationError
            }
            initialPairingInputApplied = true
        }
    }

    val requiresNearbyDelivery =
        nearbySelection?.sources?.contains(DiscoverySource.Bluetooth) == true &&
            initialPairingInput.isNullOrBlank()
    val ready =
        !startSubmitted &&
            !rendezvousBusy &&
            (!requiresNearbyDelivery || nearbyDeliveryAvailable) &&
            (
                onQueuePreparedSend != null ||
                    if (joining) invitationInput != null else generated != null
            ) &&
            (!rememberAfterPairing || rememberLabel.trim().isNotEmpty()) &&
            when (role) {
                "send" ->
                    preparedSources.isNotEmpty() &&
                        preparingCount == 0 &&
                        preparedJobId != null &&
                        preparedSources.all { it.issueCount == 0 || it.partialApproved }
                else -> true
            }
    val roomConnectionReady =
        roomMode &&
            (
                connectedRoom ||
                    !initialPairingInput.isNullOrBlank() ||
                    nearbySelection?.sources?.contains(DiscoverySource.Bluetooth) == true
            )

    Column(
        Modifier
            .semantics { testTagsAsResourceId = true }
            .testTag("transfer_sheet")
            .fillMaxWidth()
            .fillMaxHeight(0.94f),
    ) {
        Text(
            if (roomMode && role == "send") {
                appString(R.string.transfer_setup_offer_files)
            } else if (roomMode) {
                appString(R.string.transfer_setup_receive_files)
            } else if (role == "send") {
                appString(R.string.send_action_title)
            } else {
                appString(R.string.receive_action_title)
            },
            color = colors.text,
            fontSize = 24.sp,
            fontWeight = FontWeight.ExtraBold,
            modifier = Modifier.padding(horizontal = 20.dp),
        )
        Text(
            if (roomMode && role == "send") {
                appString(R.string.transfer_setup_offer_files_explanation)
            } else if (roomMode) {
                appString(R.string.transfer_setup_receive_files_explanation)
            } else if (role == "send") {
                appString(R.string.send_setup_subtitle)
            } else {
                appString(R.string.receive_setup_subtitle)
            },
            color = colors.muted,
            fontSize = 13.sp,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
        )

        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp)
                .padding(top = 12.dp, bottom = 12.dp),
        ) {
            nearbySelection?.let { selection ->
                NearbyPairingContext(selection, nearbyDeliveryAvailable)
                Spacer(Modifier.height(14.dp))
            }

            rendezvousError?.let { error ->
                Text(error, color = colors.danger, fontSize = 12.sp, lineHeight = 17.sp)
                Spacer(Modifier.height(10.dp))
            }

            roleChangeNotice?.let { notice ->
                Text(
                    notice,
                    color = colors.accentStrong,
                    fontSize = 12.sp,
                    lineHeight = 17.sp,
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .background(colors.accentSoft)
                            .padding(10.dp),
                )
                Spacer(Modifier.height(10.dp))
            }

            // ---- canonical roots or receive destination ----
            if (role == "send") {
                PathRow(
                    appString(R.string.add_files),
                    appString(R.string.add_files_hint),
                    placeholder = preparedSources.isEmpty(),
                    onClick =
                        if (startSubmitted) {
                            null
                        } else {
                            {
                                onExternalActivityChanged(true)
                                filePicker.launch(arrayOf("*/*"))
                            }
                        },
                )
                Spacer(Modifier.height(8.dp))
                PathRow(
                    appString(R.string.add_folder),
                    appString(R.string.add_folder_hint),
                    placeholder = false,
                    onClick =
                        if (startSubmitted) {
                            null
                        } else {
                            {
                                onExternalActivityChanged(true)
                                sourceFolderPicker.launch(null)
                            }
                        },
                )
                preparedSources.forEach { prepared ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 8.dp)
                            .clip(RoundedCornerShape(12.dp))
                            .background(colors.surface)
                            .border(1.dp, colors.line, RoundedCornerShape(12.dp))
                            .padding(10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(
                            if (prepared.source.directory) {
                                Icons.Default.Folder
                            } else {
                                Icons.AutoMirrored.Filled.InsertDriveFile
                            },
                            contentDescription = null,
                            tint = colors.accent,
                            modifier = Modifier.size(24.dp),
                        )
                        Spacer(Modifier.width(10.dp))
                        Column(Modifier.weight(1f)) {
                            Text(
                                prepared.source.displayName,
                                color = colors.text,
                                fontSize = 14.sp,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(
                                when {
                                    prepared.partialApproved ->
                                        appString(R.string.transfer_setup_folder_accessible_only)
                                    prepared.issueCount > 0 ->
                                        appString(R.string.transfer_setup_folder_decision_required)
                                    prepared.source.directory -> appString(R.string.activity_folder_label)
                                    else -> appString(R.string.activity_file_label)
                                },
                                color = if (prepared.issueCount > 0 && !prepared.partialApproved) colors.warning else colors.muted,
                                fontSize = 11.sp,
                            )
                        }
                        if (prepared.issueCount > 0 && !prepared.partialApproved) {
                            Text(
                                appString(R.string.transfer_setup_authorize_again),
                                color = colors.accent,
                                fontSize = 11.sp,
                                fontWeight = FontWeight.Bold,
                                modifier =
                                    Modifier
                                        .clip(RoundedCornerShape(7.dp))
                                        .clickable(enabled = !startSubmitted) {
                                            sourceAwaitingReauthorization = prepared
                                            onExternalActivityChanged(true)
                                            sourceReauthorizationPicker.launch(null)
                                        }.padding(horizontal = 7.dp, vertical = 5.dp),
                            )
                            if (prepared.canApprovePartial) {
                                Text(
                                    appString(R.string.transfer_setup_send_accessible),
                                    color = colors.accent,
                                    fontSize = 11.sp,
                                    fontWeight = FontWeight.Bold,
                                    modifier =
                                        Modifier
                                            .clip(RoundedCornerShape(7.dp))
                                            .clickable(enabled = !startSubmitted) {
                                                sourcePreparationIntents.approvePartial(preparation, prepared)
                                            }.padding(horizontal = 7.dp, vertical = 5.dp),
                                )
                            }
                        }
                        Icon(
                            Icons.Default.Close,
                            contentDescription =
                                appString(
                                    R.string.transfer_setup_remove_source,
                                    prepared.source.displayName,
                                ),
                            tint = colors.muted,
                            modifier =
                                Modifier
                                    .clip(CircleShape)
                                    .clickable(enabled = !startSubmitted) {
                                        sourcePreparationIntents.removeSource(
                                            preparation,
                                            prepared,
                                            sourcePreparationMessages,
                                        )
                                    }.padding(7.dp)
                                    .size(17.dp),
                        )
                    }
                }
                if (preparingCount > 0) {
                    Text(
                        appQuantityString(
                            R.plurals.transfer_setup_preparing_sources,
                            preparingCount,
                            preparingCount,
                        ),
                        color = colors.accent,
                        fontSize = 12.sp,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
                preparationSummary?.takeIf { preparedSources.isNotEmpty() }?.let { summary ->
                    val files = summary.inventory.fileCount
                    val directories = summary.inventory.directoryCount
                    val size = dev.envoix.app.humanBytes(summary.inventory.totalBytes)
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 10.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(colors.accentSoft)
                            .padding(horizontal = 12.dp, vertical = 9.dp),
                    ) {
                        Text(
                            appQuantityString(
                                R.plurals.transfer_setup_selected_roots,
                                preparedSources.size,
                                preparedSources.size,
                            ),
                            color = colors.accentStrong,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Bold,
                        )
                        Text(
                            appString(
                                R.string.room_offer_summary_format,
                                appQuantityString(R.plurals.room_file_count, files, files),
                                appQuantityString(R.plurals.room_folder_count, directories, directories),
                                size,
                            ),
                            color = colors.muted,
                            fontSize = 12.sp,
                        )
                    }
                }
                preparationError?.let {
                    Text(
                        it,
                        color = colors.danger,
                        fontSize = 12.sp,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            } else {
                PathRow(appString(R.string.save_to), preferences.saveLocationLabel, placeholder = false) {
                    onExternalActivityChanged(true)
                    saveFolderPicker.launch(preferences.savePickerInitialUri)
                }
            }

            if (roomConnectionReady) {
                Spacer(Modifier.height(18.dp))
                RoomConnectionSummary(
                    initialPairingInput = initialPairingInput,
                    nearbySelection = nearbySelection,
                )
            } else {
                Spacer(Modifier.height(18.dp))
                Text(
                    appString(R.string.connect_section_title),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 1.sp,
                )
                Spacer(Modifier.height(6.dp))

                // ---- top pane: show my QR vs scan one ----
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(12.dp))
                        .background(colors.bg)
                        .border(1.dp, colors.line, RoundedCornerShape(12.dp))
                        .padding(3.dp),
                ) {
                    SegTab(appString(R.string.show_qr), topMode == "show", Modifier.weight(1f)) {
                        topMode = "show"
                    }
                    SegTab(appString(R.string.scan_qr), topMode == "scan", Modifier.weight(1f)) {
                        topMode = "scan"
                    }
                }

                if (topMode != "closed") {
                    Spacer(Modifier.height(12.dp))
                    Box(Modifier.fillMaxWidth().heightIn(min = 210.dp), contentAlignment = Alignment.Center) {
                        if (topMode == "scan") {
                            InlineScanner(
                                onScanned = {
                                    if (!applyScanned(it)) {
                                        pairingInputError = invalidInvitationError
                                    }
                                },
                                modifier = Modifier.fillMaxWidth(),
                            )
                        } else if (joining) {
                            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                Text(
                                    appString(R.string.transfer_setup_joining_preview),
                                    color = colors.muted,
                                    fontSize = 13.sp,
                                )
                                Spacer(Modifier.height(6.dp))
                                Text(
                                    if (invitationInput == null) {
                                        typed
                                    } else {
                                        appString(R.string.transfer_setup_invitation_ready)
                                    },
                                    color = colors.accent,
                                    fontSize = 18.sp,
                                    fontWeight = FontWeight.Bold,
                                    fontFamily = FontFamily.Monospace,
                                )
                                Spacer(Modifier.height(6.dp))
                                Text(
                                    appString(R.string.transfer_setup_clear_invitation_hint),
                                    color = colors.muted,
                                    fontSize = 11.sp,
                                )
                            }
                        } else {
                            generated?.let { invite ->
                                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                    QrCode(invite.payload, side = 168.dp)
                                    Spacer(Modifier.height(12.dp))
                                    Row(
                                        modifier =
                                            Modifier
                                                .clip(RoundedCornerShape(8.dp))
                                                .clickable {
                                                    clipboard.setText(AnnotatedString(invite.payload))
                                                }.padding(horizontal = 8.dp, vertical = 6.dp),
                                        verticalAlignment = Alignment.CenterVertically,
                                    ) {
                                        Text(
                                            appString(R.string.activity_copy_invite_link),
                                            color = colors.muted,
                                            fontSize = 13.sp,
                                        )
                                        Spacer(Modifier.width(6.dp))
                                        Icon(
                                            Icons.Default.ContentCopy,
                                            contentDescription = null,
                                            tint = colors.muted,
                                            modifier =
                                                Modifier
                                                    .size(18.dp),
                                        )
                                    }
                                }
                            }
                        }
                    }
                }

                // ---- invitation field (paste a complete invite to join) ----
                Spacer(Modifier.height(16.dp))
                OutlinedTextField(
                    value = typed,
                    onValueChange = { applyPairingInput(it) },
                    placeholder = { Text(appString(R.string.enter_pairing_code_hint)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                TextButton(
                    onClick = {
                        val pasted =
                            clipboard
                                .getText()
                                ?.text
                                ?.trim()
                                .orEmpty()
                        if (pasted.isEmpty()) {
                            pairingInputError = clipboardEmptyError
                        } else if (!applyPairingInput(pasted)) {
                            pairingInputError = invalidInvitationError
                        }
                    },
                    modifier =
                        Modifier
                            .align(Alignment.End)
                            .testTag("transfer_code_paste"),
                ) {
                    Text(appString(R.string.common_paste))
                }
                pairingInputError?.let { error ->
                    Text(
                        error,
                        color = colors.danger,
                        fontSize = 12.sp,
                        modifier =
                            Modifier
                                .fillMaxWidth()
                                .testTag("transfer_code_error"),
                    )
                }

                Spacer(Modifier.height(12.dp))
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable { rememberAfterPairing = !rememberAfterPairing },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Checkbox(
                        checked = rememberAfterPairing,
                        onCheckedChange = { rememberAfterPairing = it },
                    )
                    Text(appString(R.string.transfer_setup_remember_device), color = colors.text)
                }
                if (rememberAfterPairing) {
                    OutlinedTextField(
                        value = rememberLabel,
                        onValueChange = { rememberLabel = it },
                        placeholder = { Text(appString(R.string.transfer_setup_device_label)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }

        // ---- start ----
        Box(
            Modifier
                .padding(horizontal = 20.dp, vertical = 12.dp)
                .testTag("transfer_start")
                .fillMaxWidth()
                .height(52.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(colors.accent.copy(alpha = if (ready) 1f else 0.4f))
                .clickable(enabled = ready) {
                    startSubmitted = true
                    if (role == "send" && onQueuePreparedSend != null) {
                        val jobId = preparedJobId
                        val summary = preparationSummary
                        if (jobId == null || summary == null || !preparation.transferOwnership()) {
                            startSubmitted = false
                            return@clickable
                        }
                        rendezvousBusy = true
                        rendezvousError = null
                        onQueuePreparedSend(
                            jobId,
                            preparedSources.map { it.source.displayName }.take(3),
                            summary.inventory.fileCount + summary.inventory.directoryCount,
                            summary.inventory.directoryCount,
                            summary.inventory.totalBytes,
                        ) { error ->
                            rendezvousBusy = false
                            if (error != null) {
                                preparation.rollbackTransferredOwnership()
                                rendezvousError = error
                                startSubmitted = false
                            }
                        }
                        return@clickable
                    }

                    val prepared =
                        if (joining) {
                            InviteCodec.parseForRole(invitationInput ?: typed, role)
                                ?: run {
                                    rendezvousError = invalidFlowError
                                    startSubmitted = false
                                    return@clickable
                                }
                        } else {
                            null
                        }
                    val c = prepared?.reference ?: generated?.reference ?: return@clickable
                    val useBroker =
                        prepared?.broker?.takeIf(String::isNotBlank)
                            ?: generated?.broker
                            ?: scannedBroker
                            ?: broker
                    val useRelay =
                        prepared?.relay
                            ?: generated?.relay
                            ?: scannedRelay
                            ?: relay
                    val qr = if (joining) null else generated?.payload
                    val startLocal = {
                        if (role == "send") {
                            if (preparation.transferOwnership()) {
                                onSend(
                                    c,
                                    useBroker,
                                    useRelay,
                                    preparedJobId!!,
                                    qr,
                                    rememberLabel.trim().takeIf { rememberAfterPairing },
                                    null,
                                )
                            }
                        } else if (preparation.transferOwnership()) {
                            onReceive(
                                c,
                                useBroker,
                                useRelay,
                                qr,
                                true,
                                rememberLabel.trim().takeIf { rememberAfterPairing },
                                null,
                            )
                        }
                    }
                    val continueAfterRoomDecision = {
                        val offer = onOfferInvite
                        if (!joining && offer != null) {
                            val payload = generated?.payload
                            if (payload == null) {
                                startSubmitted = false
                            } else {
                                rendezvousBusy = true
                                rendezvousError = null
                                val summary = preparationSummary
                                offer(
                                    RoomTransferOfferDraft(
                                        id = UUID.randomUUID().toString(),
                                        transferInvite = payload,
                                        rootNames =
                                            preparedSources
                                                .map { it.source.displayName }
                                                .take(3),
                                        itemCount =
                                            (summary?.inventory?.fileCount ?: 0) +
                                                (summary?.inventory?.directoryCount ?: 0),
                                        directoryCount =
                                            summary?.inventory?.directoryCount ?: 0,
                                        totalBytes = summary?.inventory?.totalBytes ?: 0L,
                                    ),
                                ) { error ->
                                    rendezvousBusy = false
                                    if (error == null) {
                                        startLocal()
                                    } else {
                                        rendezvousError = error
                                        startSubmitted = false
                                    }
                                }
                            }
                        } else {
                            startLocal()
                        }
                    }
                    val beforeStart = onBeforeStart
                    val prepareReceive = onPrepareReceiveBeforeDecision
                    if (beforeStart != null && role == "receive" && prepareReceive != null) {
                        rendezvousBusy = true
                        rendezvousError = null
                        prepareReceive(c, useBroker, useRelay, qr, true) { receiveId, startError ->
                            if (startError != null) {
                                onCancelPreparedReceive(receiveId)
                                rendezvousBusy = false
                                rendezvousError = startError
                                startSubmitted = false
                            } else if (!preparation.transferOwnership()) {
                                onCancelPreparedReceive(receiveId)
                                rendezvousBusy = false
                                rendezvousError = receiveSetupClosedError
                                startSubmitted = false
                            } else {
                                beforeStart { decisionError ->
                                    rendezvousBusy = false
                                    if (decisionError == null) {
                                        onPreparedReceiveCommitted(c)
                                    } else {
                                        onCancelPreparedReceive(receiveId)
                                        preparation.rollbackTransferredOwnership()
                                        rendezvousError = decisionError
                                        startSubmitted = false
                                    }
                                }
                            }
                        }
                    } else if (beforeStart != null) {
                        rendezvousBusy = true
                        rendezvousError = null
                        beforeStart { error ->
                            rendezvousBusy = false
                            if (error == null) {
                                continueAfterRoomDecision()
                            } else {
                                rendezvousError = error
                                startSubmitted = false
                            }
                        }
                    } else {
                        continueAfterRoomDecision()
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            Text(
                when {
                    rendezvousBusy && role == "receive" ->
                        appString(R.string.room_preparing_receiver)
                    rendezvousBusy && onQueuePreparedSend != null ->
                        appString(R.string.transfer_setup_queueing_files)
                    rendezvousBusy -> appString(R.string.transfer_setup_delivering_invite)
                    onQueuePreparedSend != null -> appString(R.string.transfer_setup_queue_files)
                    roomMode && role == "send" -> appString(R.string.transfer_setup_offer_files)
                    roomMode -> appString(R.string.receive_action_title)
                    role == "send" -> appString(R.string.send_action_title)
                    else -> appString(R.string.receive_action_title)
                },
                color = Color.White,
                fontWeight = FontWeight.Bold,
                fontSize = 16.sp,
            )
        }
    }
}

@Composable
private fun RoomConnectionSummary(
    initialPairingInput: String?,
    nearbySelection: NearbyPairingSelection?,
) {
    val colors = Envoix.colors
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(colors.accentSoft)
            .padding(14.dp),
    ) {
        Text(
            appString(R.string.transfer_setup_ready_section),
            color = colors.accentStrong,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.8.sp,
        )
        Spacer(Modifier.height(5.dp))
        Text(
            when {
                nearbySelection != null ->
                    appString(R.string.transfer_setup_nearby_invite_delivery)
                !initialPairingInput.isNullOrBlank() ->
                    appString(R.string.transfer_setup_shared_invite)
                else -> appString(R.string.transfer_setup_connection_ready)
            },
            color = colors.muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
    }
}

@Composable
private fun NearbyPairingContext(
    selection: NearbyPairingSelection,
    nearbyDeliveryAvailable: Boolean,
) {
    val colors = Envoix.colors
    val sourceText =
        selection.sources
            .sortedBy(DiscoverySource::ordinal)
            .joinToString(" + ") { source ->
                when (source) {
                    DiscoverySource.Bluetooth -> "BLE"
                    DiscoverySource.Mdns -> "mDNS"
                    DiscoverySource.WifiAware -> "Wi-Fi Aware"
                }
            }
    val secureLocalDelivery =
        DiscoverySource.Mdns in selection.sources &&
            selection.nearbyInviteRoute != null
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(colors.accentSoft)
            .padding(14.dp),
    ) {
        Text(
            selection.displayName ?: appString(R.string.nearby_envoix_device),
            color = colors.text,
            fontWeight = FontWeight.Bold,
            fontSize = 16.sp,
        )
        if (sourceText.isNotEmpty()) {
            Text(
                appString(R.string.transfer_setup_found_over, sourceText),
                color = colors.muted,
                fontSize = 12.sp,
            )
        }
        Spacer(Modifier.height(6.dp))
        Text(
            if (!nearbyDeliveryAvailable) {
                appString(R.string.transfer_setup_nearby_not_visible)
            } else if (secureLocalDelivery) {
                appString(R.string.transfer_setup_secure_local_delivery)
            } else if (DiscoverySource.Bluetooth in selection.sources) {
                appString(R.string.transfer_setup_insecure_ble_warning)
            } else {
                appString(R.string.transfer_setup_ble_unreachable)
            },
            color = colors.muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
    }
}

@Composable
private fun SegTab(
    text: String,
    selected: Boolean,
    modifier: Modifier,
    onClick: () -> Unit,
) {
    val colors = Envoix.colors
    Box(
        modifier
            .clip(RoundedCornerShape(9.dp))
            .background(if (selected) colors.accent else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            color = if (selected) Color.White else colors.muted,
            fontWeight = FontWeight.Bold,
            fontSize = 14.sp,
        )
    }
}

/** A labelled path row: a tappable file/folder picker (onClick != null) or a
 *  read-only value. */
@Composable
private fun PathRow(
    label: String,
    value: String,
    placeholder: Boolean,
    onClick: (() -> Unit)?,
) {
    val colors = Envoix.colors
    Text(label, color = colors.muted, fontSize = 11.sp, fontWeight = FontWeight.Bold, letterSpacing = 1.sp)
    Spacer(Modifier.height(6.dp))
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .border(1.dp, colors.line, RoundedCornerShape(12.dp))
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 14.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            value,
            color = if (placeholder) colors.muted else colors.text,
            fontSize = 14.sp,
            fontFamily = FontFamily.Monospace,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        if (onClick != null) {
            Icon(Icons.Default.ChevronRight, null, tint = colors.muted, modifier = Modifier.size(20.dp))
        }
    }
}
