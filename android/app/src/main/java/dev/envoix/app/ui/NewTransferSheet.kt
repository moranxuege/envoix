package dev.envoix.app.ui

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
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
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
import dev.envoix.app.ManifestV2Source
import dev.envoix.app.ManifestV2SourceStager
import dev.envoix.app.ManifestV2StageResult
import dev.envoix.app.Native
import dev.envoix.app.PreparedManifestV2Source
import dev.envoix.app.R
import dev.envoix.app.SettingsStore
import dev.envoix.app.TransferService
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.util.UUID

internal typealias PrepareReceiveBeforeDecision = (
    code: String,
    broker: String,
    relay: String,
    qrPayload: String?,
    copyApproved: Boolean,
    completion: (id: Long, error: String?) -> Unit,
) -> Unit

/**
 * Role-specific transfer setup. Scanning an invite may switch to the opposite
 * role; the change is explained inline instead of interrupting with a dialog.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
internal fun NewTransferSheet(
    draftId: String = "standalone",
    preparationState: TransferDraftPreparationState? = null,
    onReceive: (code: String, broker: String, relay: String, qrPayload: String?, copyApproved: Boolean) -> Unit,
    onSend: (code: String, broker: String, relay: String, jobId: String, qrPayload: String?) -> Unit,
    nearbySelection: NearbyPairingSelection? = null,
    nearbyDeliveryAvailable: Boolean = true,
    initialPairingInput: String? = null,
    initialSources: List<android.net.Uri> = emptyList(),
    initialRole: String? = null,
    initialHostedCode: String? = null,
    initialHostedPayload: String? = null,
    roomMode: Boolean = false,
    connectedRoom: Boolean = false,
    showQrInitially: Boolean = false,
    onExternalActivityChanged: (Boolean) -> Unit = {},
    onOfferInvite: ((offer: RoomTransferOfferDraft, completion: (error: String?) -> Unit) -> Unit)? = null,
    onBeforeStart: ((completion: (error: String?) -> Unit) -> Unit)? = null,
    onPrepareReceiveBeforeDecision: PrepareReceiveBeforeDecision? = null,
    onCancelPreparedReceive: (Long) -> Unit = {},
    onPreparedReceiveCommitted: (String) -> Unit = {},
) {
    val colors = Envoix.colors
    val context = LocalContext.current
    val settings by SettingsStore.settings.collectAsState()
    val language = LocalAppLanguage.current

    fun text(
        english: String,
        simplifiedChinese: String,
    ) = AppText.value(english, simplifiedChinese, language)
    val switchedToSendNotice = appString(R.string.switched_to_send_notice)
    val switchedToReceiveNotice = appString(R.string.switched_to_receive_notice)
    val broker = settings.broker
    val relay = settings.relay

    val fallbackPreparation =
        remember(draftId) {
            TransferDraftPreparationState(
                initialRole = initialRole ?: settings.defaultRole,
                showQrInitially = showQrInitially,
            )
        }
    val preparation = preparationState ?: fallbackPreparation
    var role by preparation.role
    var typed by preparation.typedCode
    var generated by preparation.generatedInvite
    var generatedRole by preparation.generatedInviteRole
    var scannedBroker by preparation.scannedBroker
    var scannedRelay by preparation.scannedRelay
    val preparedSources = preparation.preparedSources
    var preparedJobId by preparation.preparedJobId
    var jobStoreDirectory by preparation.jobStoreDirectory
    var stagingRootDirectory by preparation.stagingRootDirectory
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

    val preparationScope = rememberCoroutineScope()
    val preparationMutex = preparation.mutex

    DisposableEffect(preparation, preparationState) {
        onDispose {
            if (preparationState == null) preparation.discard()
        }
    }

    fun addSources(sources: List<ManifestV2Source>) {
        if (sources.isEmpty()) return
        preparationScope.launch {
            preparationMutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                val fresh =
                    sources.filter { candidate ->
                        preparedSources.none { it.source.uri == candidate.uri }
                    }
                if (fresh.isEmpty()) return@withLock
                preparingCount += fresh.size
                preparationError = null
                try {
                    val store = TransferService.jobStoreDirectory(context).absolutePath
                    jobStoreDirectory = store
                    val jobId =
                        preparedJobId ?: withContext(Dispatchers.IO) {
                            val response =
                                JSONObject(
                                    Native.createManifestV2Job(store, settings.compressionPolicy),
                                )
                            response.optString("error").takeIf(String::isNotEmpty)?.let(::error)
                            response.getString("job_id")
                        }.also {
                            preparedJobId = it
                            stagingRootDirectory =
                                java.io.File(context.filesDir, "manifest-v2/source-staging/$it").absolutePath
                        }
                    for (source in fresh) {
                        val staged = ManifestV2SourceStager.stage(context, jobId, source)
                        var attached = false
                        try {
                            val response =
                                withContext(Dispatchers.IO) {
                                    Native.prepareManifestV2Job(
                                        store,
                                        jobId,
                                        ManifestV2SourceStager.rootsJson(source, staged),
                                    )
                                }
                            val parsed = JSONObject(response)
                            parsed.optString("error").takeIf(String::isNotEmpty)?.let(::error)
                            attached = true
                            preparationSummary = parsed
                            preparedSources +=
                                ManifestV2SourceStager.parsePreparedSnapshot(source, staged.root, response)
                        } catch (error: Throwable) {
                            if (!attached) staged.root.parentFile?.deleteRecursively()
                            throw error
                        }
                    }
                } catch (error: Throwable) {
                    preparationError =
                        error.message ?: text(
                            "Could not prepare the selected source",
                            "无法准备所选内容",
                        )
                } finally {
                    preparingCount -= fresh.size
                }
            }
        }
    }

    fun removeSource(source: PreparedManifestV2Source) {
        val jobId = preparedJobId ?: return
        preparationScope.launch {
            preparationMutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                preparingCount += 1
                try {
                    val store = TransferService.jobStoreDirectory(context).absolutePath
                    val response =
                        withContext(Dispatchers.IO) {
                            Native.resolveManifestV2Source(
                                store,
                                jobId,
                                source.rootItemId,
                                "remove_selection",
                                "",
                            )
                        }
                    val error = JSONObject(response).optString("error")
                    if (error.isEmpty()) {
                        preparedSources.remove(source)
                        source.localRoot.parentFile?.deleteRecursively()
                        preparationSummary = JSONObject(response)
                    } else {
                        preparationError = error
                    }
                } catch (error: Throwable) {
                    preparationError =
                        error.message ?: text(
                            "Could not remove the selected source",
                            "无法移除所选来源",
                        )
                } finally {
                    preparingCount -= 1
                }
            }
        }
    }

    fun approvePartial(source: PreparedManifestV2Source) {
        val jobId = preparedJobId ?: return
        preparationScope.launch {
            preparationMutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                val response =
                    withContext(Dispatchers.IO) {
                        Native.resolveManifestV2Source(
                            TransferService.jobStoreDirectory(context).absolutePath,
                            jobId,
                            source.rootItemId,
                            "approve_partial",
                            "",
                        )
                    }
                val parsed = JSONObject(response)
                val error = parsed.optString("error")
                if (error.isEmpty()) {
                    val index = preparedSources.indexOf(source)
                    if (index >= 0) preparedSources[index] = source.copy(partialApproved = true)
                    preparationSummary = parsed
                    preparationError = null
                } else {
                    preparationError = error
                }
            }
        }
    }

    fun reauthorizeSource(
        previous: PreparedManifestV2Source,
        uri: android.net.Uri,
    ) {
        val jobId = preparedJobId ?: return
        preparationScope.launch {
            preparationMutex.withLock {
                if (!preparation.acceptsPreparationChanges()) return@withLock
                preparingCount += 1
                preparationError = null
                val source = ManifestV2SourceStager.sourceFromUri(context, uri, true)
                var staged: ManifestV2StageResult? = null
                var committed = false
                try {
                    val stagedResult = ManifestV2SourceStager.stage(context, jobId, source)
                    staged = stagedResult
                    val response =
                        withContext(Dispatchers.IO) {
                            Native.reauthorizeManifestV2ProviderSource(
                                TransferService.jobStoreDirectory(context).absolutePath,
                                jobId,
                                previous.rootItemId,
                                ManifestV2SourceStager.rootsJson(source, stagedResult),
                            )
                        }
                    val parsed = JSONObject(response)
                    parsed.optString("error").takeIf(String::isNotEmpty)?.let(::error)
                    committed = true
                    val replacement =
                        ManifestV2SourceStager.parsePreparedSnapshot(
                            source,
                            stagedResult.root,
                            response,
                            previous.rootItemId,
                        )
                    val index = preparedSources.indexOf(previous)
                    check(index >= 0) {
                        text("Source selection changed while authorizing", "授权期间所选内容发生了变化")
                    }
                    preparedSources[index] = replacement
                    previous.localRoot.parentFile?.deleteRecursively()
                    preparationSummary = parsed
                } catch (error: Throwable) {
                    if (!committed) staged?.root?.parentFile?.deleteRecursively()
                    preparationError =
                        error.message ?: text(
                            "Could not authorize the selected folder",
                            "无法授权所选文件夹",
                        )
                } finally {
                    preparingCount -= 1
                }
            }
        }
    }

    val filePicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            onExternalActivityChanged(false)
            addSources(uris.map { ManifestV2SourceStager.sourceFromUri(context, it, false) })
        }
    val sourceFolderPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            uri?.let {
                addSources(listOf(ManifestV2SourceStager.sourceFromUri(context, it, true)))
            }
        }
    val sourceReauthorizationPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            val previous = sourceAwaitingReauthorization
            sourceAwaitingReauthorization = null
            if (uri != null && previous != null) reauthorizeSource(previous, uri)
        }
    val saveFolderPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
            onExternalActivityChanged(false)
            if (uri != null) SettingsStore.setSaveTree(context, uri)
        }
    val joining = typed.isNotBlank()

    LaunchedEffect(draftId, initialSources, initialRole) {
        if (initialSources.isNotEmpty()) {
            role = "send"
            addSources(initialSources.map { ManifestV2SourceStager.sourceFromUri(context, it, false) })
        }
    }

    // Generate once per role. The draft owns this value so rotation cannot
    // silently replace a QR that the other device is already scanning.
    LaunchedEffect(draftId, role, joining, initialHostedCode, initialHostedPayload) {
        if (!joining && (generated == null || generatedRole != role)) {
            val hosted =
                if (role == initialRole &&
                    !initialHostedCode.isNullOrBlank() &&
                    !initialHostedPayload.isNullOrBlank()
                ) {
                    initialHostedCode to initialHostedPayload
                } else {
                    null
                }
            generated = hosted ?: InviteCodec.generate(role, broker, relay)
            generatedRole = role
        }
    }

    fun applyScanned(scanned: String) {
        val inv = InviteCodec.parse(scanned) ?: return
        typed = inv.code
        scannedBroker = inv.broker
        scannedRelay = inv.relay
        InviteCodec.oppositeRole(inv.role)?.let { scannedRole ->
            if (scannedRole != role) {
                role = scannedRole
                roleChangeNotice =
                    if (scannedRole == "send") {
                        switchedToSendNotice
                    } else {
                        switchedToReceiveNotice
                    }
            }
        }
        topMode = "closed" // stop the camera; the code is filled in now
    }

    LaunchedEffect(draftId, initialPairingInput) {
        if (!initialPairingInputApplied) {
            initialPairingInput?.takeIf(String::isNotBlank)?.let(::applyScanned)
            initialPairingInputApplied = true
        }
    }

    val code = if (joining) typed.trim() else generated?.first
    val useBroker = scannedBroker ?: broker
    val useRelay = scannedRelay ?: relay
    val requiresNearbyDelivery =
        nearbySelection?.sources?.contains(DiscoverySource.Bluetooth) == true &&
            initialPairingInput.isNullOrBlank() &&
            initialHostedPayload.isNullOrBlank()
    val ready =
        !startSubmitted &&
            !rendezvousBusy &&
            (!requiresNearbyDelivery || nearbyDeliveryAvailable) &&
            !code.isNullOrBlank() &&
            code.contains("-") &&
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
                    !initialHostedPayload.isNullOrBlank() ||
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
                text("Offer files", "发送文件")
            } else if (roomMode) {
                text("Receive files", "接收文件")
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
                text("Choose what to share with this device.", "选择要发送给此设备的内容。")
            } else if (roomMode) {
                text("Confirm where incoming files will be saved.", "确认接收文件的保存位置。")
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
                ) {
                    onExternalActivityChanged(true)
                    filePicker.launch(arrayOf("*/*"))
                }
                Spacer(Modifier.height(8.dp))
                PathRow(
                    appString(R.string.add_folder),
                    appString(R.string.add_folder_hint),
                    placeholder = false,
                ) {
                    onExternalActivityChanged(true)
                    sourceFolderPicker.launch(null)
                }
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
                                        text("Folder · accessible content only", "文件夹 · 仅发送可访问内容")
                                    prepared.issueCount > 0 ->
                                        text("Folder · source decision required", "文件夹 · 需要处理来源访问问题")
                                    prepared.source.directory -> text("Folder", "文件夹")
                                    else -> text("File", "文件")
                                },
                                color = if (prepared.issueCount > 0 && !prepared.partialApproved) colors.warning else colors.muted,
                                fontSize = 11.sp,
                            )
                        }
                        if (prepared.issueCount > 0 && !prepared.partialApproved) {
                            Text(
                                text("Authorize again", "重新授权"),
                                color = colors.accent,
                                fontSize = 11.sp,
                                fontWeight = FontWeight.Bold,
                                modifier =
                                    Modifier
                                        .clip(RoundedCornerShape(7.dp))
                                        .clickable {
                                            sourceAwaitingReauthorization = prepared
                                            onExternalActivityChanged(true)
                                            sourceReauthorizationPicker.launch(null)
                                        }.padding(horizontal = 7.dp, vertical = 5.dp),
                            )
                            if (prepared.canApprovePartial) {
                                Text(
                                    text("Send accessible", "发送可访问内容"),
                                    color = colors.accent,
                                    fontSize = 11.sp,
                                    fontWeight = FontWeight.Bold,
                                    modifier =
                                        Modifier
                                            .clip(RoundedCornerShape(7.dp))
                                            .clickable { approvePartial(prepared) }
                                            .padding(horizontal = 7.dp, vertical = 5.dp),
                                )
                            }
                        }
                        Icon(
                            Icons.Default.Close,
                            contentDescription =
                                text(
                                    "Remove ${prepared.source.displayName}",
                                    "移除 ${prepared.source.displayName}",
                                ),
                            tint = colors.muted,
                            modifier =
                                Modifier
                                    .clip(CircleShape)
                                    .clickable { removeSource(prepared) }
                                    .padding(7.dp)
                                    .size(17.dp),
                        )
                    }
                }
                if (preparingCount > 0) {
                    Text(
                        text(
                            "Preparing $preparingCount selected source(s)…",
                            "正在准备 $preparingCount 个所选来源…",
                        ),
                        color = colors.accent,
                        fontSize = 12.sp,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
                preparationSummary?.takeIf { preparedSources.isNotEmpty() }?.let { summary ->
                    val files = summary.optInt("file_count")
                    val directories = summary.optInt("directory_count")
                    val size = dev.envoix.app.humanBytes(summary.optLong("total"))
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 10.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(colors.accentSoft)
                            .padding(horizontal = 12.dp, vertical = 9.dp),
                    ) {
                        Text(
                            text(
                                "${preparedSources.size} selected root(s)",
                                "已选择 ${preparedSources.size} 个根项目",
                            ),
                            color = colors.accentStrong,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Bold,
                        )
                        Text(
                            text(
                                "$files files · $directories folders · $size",
                                "$files 个文件 · $directories 个文件夹 · $size",
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
                PathRow(appString(R.string.save_to), SettingsStore.saveLabel(context), placeholder = false) {
                    onExternalActivityChanged(true)
                    saveFolderPicker.launch(SettingsStore.savePickerInitialUri())
                }
            }

            if (roomConnectionReady) {
                Spacer(Modifier.height(18.dp))
                RoomConnectionSummary(
                    initialPairingInput = initialPairingInput,
                    initialHostedCode = initialHostedCode,
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
                            InlineScanner(onScanned = ::applyScanned, modifier = Modifier.fillMaxWidth())
                        } else if (joining) {
                            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                Text(text("You'll join", "即将加入"), color = colors.muted, fontSize = 13.sp)
                                Spacer(Modifier.height(6.dp))
                                Text(
                                    typed,
                                    color = colors.accent,
                                    fontSize = 18.sp,
                                    fontWeight = FontWeight.Bold,
                                    fontFamily = FontFamily.Monospace,
                                )
                                Spacer(Modifier.height(6.dp))
                                Text(
                                    text("clear the code below to show your own", "清空下方配对码即可显示自己的二维码"),
                                    color = colors.muted,
                                    fontSize = 11.sp,
                                )
                            }
                        } else if (generated != null) {
                            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                QrCode(generated!!.second, side = 168.dp)
                                Spacer(Modifier.height(12.dp))
                                val clip = LocalClipboardManager.current
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text(
                                        generated!!.first,
                                        color = colors.text,
                                        fontSize = 15.sp,
                                        fontWeight = FontWeight.SemiBold,
                                        fontFamily = FontFamily.Monospace,
                                    )
                                    Spacer(Modifier.width(8.dp))
                                    Icon(
                                        Icons.Default.ContentCopy,
                                        text("Copy code", "复制配对码"),
                                        tint = colors.muted,
                                        modifier =
                                            Modifier
                                                .clip(CircleShape)
                                                .clickable { clip.setText(AnnotatedString(generated!!.first)) }
                                                .padding(6.dp)
                                                .size(18.dp),
                                    )
                                }
                            }
                        }
                    }
                }

                // ---- code field (type/paste a code to join) ----
                Spacer(Modifier.height(16.dp))
                OutlinedTextField(
                    value = typed,
                    onValueChange = {
                        typed = it.trim()
                        scannedBroker = null
                        scannedRelay = null
                    },
                    placeholder = { Text(appString(R.string.enter_pairing_code_hint)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
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
                    val c = code ?: return@clickable
                    val qr = if (joining) null else generated?.second
                    startSubmitted = true
                    val startLocal = {
                        if (preparation.transferOwnership()) {
                            if (role == "send") {
                                onSend(c, useBroker, useRelay, preparedJobId!!, qr)
                            } else {
                                onReceive(c, useBroker, useRelay, qr, true)
                            }
                        }
                    }
                    val continueAfterRoomDecision = {
                        val offer = onOfferInvite
                        if (!joining && offer != null) {
                            val payload = generated?.second
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
                                            (summary?.optInt("file_count") ?: 0) +
                                                (summary?.optInt("directory_count") ?: 0),
                                        totalBytes = summary?.optLong("total") ?: 0L,
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
                                rendezvousError =
                                    text(
                                        "The receive setup was already closed",
                                        "接收设置已关闭",
                                    )
                                startSubmitted = false
                            } else {
                                beforeStart { decisionError ->
                                    rendezvousBusy = false
                                    if (decisionError == null) {
                                        onPreparedReceiveCommitted(c)
                                    } else {
                                        onCancelPreparedReceive(receiveId)
                                        preparation.rollbackTransferredOwnership()
                                        rendezvousError =
                                            decisionError
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
                        text("Preparing receiver…", "正在准备接收…")
                    rendezvousBusy -> text("Delivering invite…", "正在发送邀请…")
                    roomMode && role == "send" -> text("Offer files", "发送文件")
                    roomMode -> text("Start waiting", "开始等待")
                    role == "send" -> text("Send", "发送")
                    else -> text("Receive", "接收")
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
    initialHostedCode: String?,
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
            appText("TRANSFER SETUP READY", "传输设置已就绪"),
            color = colors.accentStrong,
            fontSize = 11.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.8.sp,
        )
        Spacer(Modifier.height(5.dp))
        Text(
            when {
                nearbySelection != null ->
                    appText(
                        "The invite will be delivered to the nearby device when you start.",
                        "开始后，邀请会发送到附近设备。",
                    )
                !initialHostedCode.isNullOrBlank() ->
                    appText(
                        "Shared invite · $initialHostedCode",
                        "已分享邀请 · $initialHostedCode",
                    )
                !initialPairingInput.isNullOrBlank() ->
                    appText(
                        "The shared invite will be used for this transfer.",
                        "此传输将使用已分享的邀请。",
                    )
                else -> appText("Connection details are ready.", "连接信息已就绪。")
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
    val language = LocalAppLanguage.current

    fun text(
        english: String,
        simplifiedChinese: String,
    ) = AppText.value(english, simplifiedChinese, language)
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
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(colors.accentSoft)
            .padding(14.dp),
    ) {
        Text(
            selection.displayName ?: text("Nearby Envoix device", "附近的 Envoix 设备"),
            color = colors.text,
            fontWeight = FontWeight.Bold,
            fontSize = 16.sp,
        )
        if (sourceText.isNotEmpty()) {
            Text(text("Found over $sourceText", "通过 $sourceText 发现"), color = colors.muted, fontSize = 12.sp)
        }
        Spacer(Modifier.height(6.dp))
        Text(
            if (DiscoverySource.Bluetooth in selection.sources && !nearbyDeliveryAvailable) {
                text(
                    "This device is not visible right now. Keep this sheet open; offering files becomes available after it reappears.",
                    "当前无法发现此设备。请保持此页面打开；设备重新出现后即可发送文件。",
                )
            } else if (DiscoverySource.Bluetooth in selection.sources) {
                text(
                    "Experimental insecure BLE pairing: the invitation is sent without peer authentication. A nearby attacker may impersonate or relay this device.",
                    "实验性非安全 BLE 配对：邀请未经对端身份认证。附近的攻击者可能冒充或中继此设备。",
                )
            } else {
                text(
                    "This device is not currently reachable over BLE. Use QR or a typed Envoix code to continue.",
                    "当前无法通过 BLE 连接此设备。请使用二维码或输入 Envoix 配对码继续。",
                )
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
