package dev.envoix.app

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.core.content.IntentCompat
import dev.envoix.app.discovery.DiscoveryMode
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.ui.AppText
import dev.envoix.app.ui.ConnectionHubScreen
import dev.envoix.app.ui.ConnectionWorkflowViewModel
import dev.envoix.app.ui.DeviceRoomScreen
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.LocalAppLanguage
import dev.envoix.app.ui.SettingsScreen
import dev.envoix.app.ui.TransferActivityScreen
import dev.envoix.app.ui.WorkflowScreen

class MainActivity : ComponentActivity() {
    private val vm: TransferViewModel by viewModels()
    private val discoveryVm: DiscoveryViewModel by viewModels()
    private val workflowVm: ConnectionWorkflowViewModel by viewModels()

    private val requestNotif =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestNotif.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        TransferService.restoreAll(this)
        captureSharedUris(intent)
        captureInvite(intent)
        setContent {
            val settings by SettingsStore.settings.collectAsState()
            CompositionLocalProvider(LocalAppLanguage provides settings.language) {
                EnvoixTheme {
                    val transfers by vm.transfers.collectAsState()
                    val workflow by workflowVm.uiState.collectAsState()
                    val selectedPeerKey = workflow.room?.nearbySelection?.discoveryPeerKey
                    val controlRoom = workflow.room?.controlSession == true
                    val activeRoomTransferCount =
                        workflow.room
                            ?.transferCodes
                            ?.let { roomCodes ->
                                transfers.count { transfer ->
                                    transfer.room in roomCodes &&
                                        !transfer.status.isTerminal
                                }
                            } ?: 0

                    LaunchedEffect(workflow.screen, selectedPeerKey, controlRoom) {
                        when {
                            workflow.screen == WorkflowScreen.Hub ->
                                discoveryVm.setMode(DiscoveryMode.BrowseNearby)
                            workflow.screen == WorkflowScreen.Room &&
                                selectedPeerKey != null &&
                                !controlRoom ->
                                discoveryVm.setMode(DiscoveryMode.SelectedPeer, selectedPeerKey)
                            else -> discoveryVm.setMode(DiscoveryMode.Off)
                        }
                    }
                    // Keep room idleness correct even while Activity, Settings,
                    // or the Hub is covering the room screen.
                    LaunchedEffect(activeRoomTransferCount, workflow.room?.id) {
                        workflowVm.updateRoomTransferActivity(activeRoomTransferCount)
                    }

                    if (workflow.screen != WorkflowScreen.Hub) {
                        BackHandler { workflowVm.navigateBack() }
                    }
                    when (workflow.screen) {
                        WorkflowScreen.Hub ->
                            ConnectionHubScreen(
                                control = workflow.control,
                                onRevealInvite = workflowVm::revealRoomInvite,
                                onHideInvite = workflowVm::hideRoomInvite,
                                onRefreshInvite = workflowVm::refreshRoomInvite,
                                onEndWaitingRoom = workflowVm::endWaitingRoom,
                                onJoinInvite = { workflowVm.joinRoom(it) },
                                onNearbyRoom = workflowVm::startNearbyRoom,
                                onReturnToRoom = workflowVm::returnToCurrentRoom,
                                onActivity = workflowVm::openActivity,
                                onSettings = workflowVm::openSettings,
                                onAcceptIncomingOffer = workflowVm::acceptIncomingOffer,
                                onCancelReplacement = workflowVm::cancelReplacement,
                                onConfirmReplacement = workflowVm::confirmReplacement,
                                onExternalActivityChanged = workflowVm::setExternalActivityActive,
                                pendingShareCount = workflow.pendingShares.size,
                                discoveryViewModel = discoveryVm,
                            )
                        WorkflowScreen.Room -> {
                            val draft = workflow.room
                            if (draft == null) {
                                ConnectionHubScreen(
                                    control = workflow.control,
                                    onRevealInvite = workflowVm::revealRoomInvite,
                                    onHideInvite = workflowVm::hideRoomInvite,
                                    onRefreshInvite = workflowVm::refreshRoomInvite,
                                    onEndWaitingRoom = workflowVm::endWaitingRoom,
                                    onJoinInvite = { workflowVm.joinRoom(it) },
                                    onNearbyRoom = workflowVm::startNearbyRoom,
                                    onReturnToRoom = workflowVm::returnToCurrentRoom,
                                    onActivity = workflowVm::openActivity,
                                    onSettings = workflowVm::openSettings,
                                    onAcceptIncomingOffer = workflowVm::acceptIncomingOffer,
                                    onCancelReplacement = workflowVm::cancelReplacement,
                                    onConfirmReplacement = workflowVm::confirmReplacement,
                                    onExternalActivityChanged = workflowVm::setExternalActivityActive,
                                    pendingShareCount = workflow.pendingShares.size,
                                    discoveryViewModel = discoveryVm,
                                )
                            } else {
                                DeviceRoomScreen(
                                    draft = draft,
                                    control = workflow.control,
                                    transferDraft = workflow.transferDraft,
                                    transfers = transfers,
                                    onBack = workflowVm::returnToHub,
                                    onActivity = workflowVm::openActivity,
                                    onSettings = workflowVm::openSettings,
                                    initialSources = workflow.pendingShares,
                                    onBeginTransfer = workflowVm::beginTransfer,
                                    onShowRoomQr = workflowVm::showRoomQr,
                                    onDismissTransfer = workflowVm::dismissTransferDraft,
                                    onTransferStarted = workflowVm::completeTransferDraft,
                                    onOfferRoomTransfer = workflowVm::offerRoomTransfer,
                                    incomingOfferBusy = workflow.incomingOfferBusy,
                                    incomingOfferError = workflow.incomingOfferError,
                                    onAcceptIncomingRoomOffer = {
                                        workflowVm.acceptIncomingRoomOffer(
                                            parseInvitation = {
                                                InviteCodec.parseForRole(it, "receive")
                                            },
                                            onPrepareReceive = {
                                                c,
                                                b,
                                                r,
                                                qr,
                                                copyApproved,
                                                completion,
                                                ->
                                                vm.startReceiveWhenReady(
                                                    c,
                                                    b,
                                                    r,
                                                    qr,
                                                    copyApproved,
                                                    completion,
                                                )
                                            },
                                            onCancelReceive = vm::cancel,
                                        )
                                    },
                                    onRejectIncomingRoomOffer = workflowVm::rejectIncomingRoomOffer,
                                    onKeepOpen = workflowVm::setKeepOpen,
                                    onEndRoom = { workflowVm.endRoom() },
                                    onDismissEndedRoom = workflowVm::dismissEndedRoom,
                                    onRoomActiveTransfers = workflowVm::updateRoomTransferActivity,
                                    onExternalActivityChanged = workflowVm::setExternalActivityActive,
                                    onAcceptIncomingOffer = workflowVm::acceptIncomingOffer,
                                    onReceive = {
                                        c,
                                        b,
                                        r,
                                        qr,
                                        copyApproved,
                                        rememberLabel,
                                        rememberedRelationshipId,
                                        ->
                                        vm.startReceive(
                                            c,
                                            b,
                                            r,
                                            qr,
                                            copyApproved,
                                            rememberLabel,
                                            rememberedRelationshipId,
                                        )
                                    },
                                    onOpenReceived = ::openReceived,
                                    onShareReceived = ::shareReceived,
                                    onSend = {
                                        c,
                                        b,
                                        r,
                                        jobId,
                                        qr,
                                        rememberLabel,
                                        rememberedRelationshipId,
                                        ->
                                        vm.startSend(
                                            c,
                                            jobId,
                                            b,
                                            r,
                                            qr,
                                            rememberLabel,
                                            rememberedRelationshipId,
                                        )
                                    },
                                    discoveryViewModel = discoveryVm,
                                )
                            }
                        }
                        WorkflowScreen.Activity ->
                            TransferActivityScreen(
                                transfers = transfers,
                                onBack = workflowVm::navigateBack,
                                onPauseResume = { vm.pauseResume(it) },
                                onApproveReceive = { vm.approveReceive(it) },
                                onCancel = { vm.cancel(it) },
                                onRemove = { vm.remove(it) },
                                onOpen = { openReceived(it) },
                                onShare = { shareReceived(it) },
                            )
                        WorkflowScreen.Settings -> SettingsScreen(onBack = workflowVm::navigateBack)
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        captureSharedUris(intent)
        captureInvite(intent)
    }

    private fun captureInvite(intent: Intent?) {
        val value = intent?.dataString ?: return
        if (value.startsWith("envoix://invite/v2/") &&
            InviteCodec.parseForRouting(value) != null
        ) {
            workflowVm.joinRoom(value)
        }
    }

    override fun onStart() {
        super.onStart()
        workflowVm.setForeground(true)
        discoveryVm.setForeground(true)
    }

    override fun onStop() {
        // Rotation/recreation keeps the same retained ViewModels and must not
        // be interpreted as the user backgrounding the room.
        if (!isChangingConfigurations) {
            workflowVm.setForeground(false)
            discoveryVm.setForeground(false)
        }
        super.onStop()
    }

    private fun captureSharedUris(intent: Intent?) {
        val action = intent?.action ?: return
        val uris =
            when (action) {
                Intent.ACTION_SEND ->
                    listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                Intent.ACTION_SEND_MULTIPLE ->
                    IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
                else -> emptyList()
            }
        workflowVm.captureSharedUris(uris)
    }

    /** Open a received file (a Downloads content Uri) in whatever app handles it. */
    private fun openReceived(t: Transfer) {
        val uri = t.savedUri?.let { Uri.parse(it) } ?: return
        val view =
            Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "*/*")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        runCatching {
            startActivity(
                Intent.createChooser(
                    view,
                    AppText.value("Open with", "打开方式", SettingsStore.settings.value.language),
                ),
            )
        }
    }

    private fun shareReceived(t: Transfer) {
        val uris = t.savedUris.map(Uri::parse)
        if (uris.isEmpty()) return
        val share =
            if (uris.size == 1) {
                Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
            } else {
                Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(
                    Intent.EXTRA_STREAM,
                    ArrayList(uris),
                )
            }
        share.type = "*/*"
        share.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        runCatching {
            startActivity(
                Intent.createChooser(
                    share,
                    AppText.value("Share received items", "分享已接收项目", SettingsStore.settings.value.language),
                ),
            )
        }
    }
}
