package dev.envoix.app

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.nfc.NdefMessage
import android.nfc.NfcAdapter
import android.os.BadParcelableException
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.WindowManager
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
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import dev.envoix.app.discovery.BleVerificationInvitation
import dev.envoix.app.discovery.DiscoveryMode
import dev.envoix.app.discovery.DiscoveryPermissions
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.DiscoveryViewModel
import dev.envoix.app.discovery.NearbyVisibility
import dev.envoix.app.ffi.FfiRoomControlInvite
import dev.envoix.app.ffi.parseRoomControlInvite
import dev.envoix.app.ui.AppText
import dev.envoix.app.ui.ConnectionHubScreen
import dev.envoix.app.ui.ConnectionWorkflowUiState
import dev.envoix.app.ui.ConnectionWorkflowViewModel
import dev.envoix.app.ui.DeviceRoomScreen
import dev.envoix.app.ui.EnvoixTheme
import dev.envoix.app.ui.LocalAppLanguage
import dev.envoix.app.ui.NfcInvitationOverlay
import dev.envoix.app.ui.RememberedRoomConnectionManager
import dev.envoix.app.ui.RememberedRoomDetailScreen
import dev.envoix.app.ui.RememberedRoomsScreen
import dev.envoix.app.ui.RememberedRoomsViewModel
import dev.envoix.app.ui.RoomControlInviteFormat
import dev.envoix.app.ui.RoomControlPhase
import dev.envoix.app.ui.SettingsScreen
import dev.envoix.app.ui.TransferActivityScreen
import dev.envoix.app.ui.WorkflowScreen
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val vm: TransferViewModel by viewModels()
    private val discoveryVm: DiscoveryViewModel by viewModels()
    private val workflowVm: ConnectionWorkflowViewModel by viewModels()
    private val rememberedRoomsVm: RememberedRoomsViewModel by viewModels()
    private val rememberedRoomConnections by lazy {
        RememberedRoomConnectionManager.get(this)
    }
    private val nfcInvitationController by lazy {
        NfcInvitationController { candidate ->
            validatedNfcInvitation(
                value = candidate,
                validateRoomInvite = ::validateNativeRoomNfcInvitation,
                validateTransferInvite = { transferInvite ->
                    InviteCodec.parseForRouting(transferInvite) != null
                },
            ) != null
        }
    }
    private val nfcInvitationHostController by lazy {
        NfcInvitationHostController(this)
    }
    private val nfcInvitationReaderController by lazy {
        NfcInvitationReaderController(this) { invitation ->
            nfcInvitationController.acceptDiscoveredInvitation(invitation)
        }
    }
    private var activeNfcReadinessOfferId: String? = null

    private val requestNotif =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}
    private val requestNearbyPermissions =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            discoveryVm.start()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestNotif.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        TransferService.restoreAll(this)
        captureSharedUris(intent)
        captureInvite(intent)
        observeHostedNfcInvitations()
        observeNfcRoleCoordination()
        setContent {
            val settings by SettingsStore.settings.collectAsState()
            CompositionLocalProvider(LocalAppLanguage provides settings.language) {
                EnvoixTheme {
                    val transfers by vm.transfers.collectAsState()
                    val workflow by workflowVm.uiState.collectAsState()
                    val discovery by discoveryVm.uiState.collectAsState()
                    val rememberedRooms by rememberedRoomsVm.uiState.collectAsState()
                    val nfcInvitation by nfcInvitationController.state.collectAsState()
                    val nfcPhoneHosting by nfcInvitationHostController.state.collectAsState()
                    val nfcPhoneReader by nfcInvitationReaderController.state.collectAsState()
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
                                { } // Paused by default — user starts explicitly.
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
                    val protectsVerificationCode =
                        workflow.control.verificationCode != null ||
                            discovery.incomingRendezvousOffers.any {
                                it.source == DiscoverySource.Bluetooth &&
                                    BleVerificationInvitation.isPublicOffer(it.invite)
                            }
                    LaunchedEffect(protectsVerificationCode) {
                        if (protectsVerificationCode) {
                            window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
                        } else {
                            window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
                        }
                    }

                    if (workflow.screen != WorkflowScreen.Hub) {
                        BackHandler { workflowVm.navigateBack() }
                    }
                    when (workflow.screen) {
                        WorkflowScreen.Hub ->
                            ConnectionHubScreen(
                                control = workflow.control,
                                onShareViaNfc = ::beginNfcPresentation,
                                onStopNfcSharing = nfcInvitationHostController::cancelPresentation,
                                onScanNfc = ::beginNfcReading,
                                onRevealInvite = workflowVm::revealRoomInvite,
                                onHideInvite = workflowVm::hideRoomInvite,
                                onRefreshInvite = workflowVm::refreshRoomInvite,
                                onEndWaitingRoom = ::endWaitingRoom,
                                onJoinInvite = { workflowVm.joinRoom(it) },
                                nfcPhoneHosting = nfcPhoneHosting,
                                nfcPhoneReader = nfcPhoneReader,
                                onNearbyRoom = workflowVm::startNearbyRoom,
                                onReturnToRoom = workflowVm::returnToCurrentRoom,
                                onActivity = workflowVm::openActivity,
                                onRooms = workflowVm::openRooms,
                                onSettings = workflowVm::openSettings,
                                onAcceptIncomingOffer = workflowVm::acceptIncomingOffer,
                                onCancelReplacement = workflowVm::cancelReplacement,
                                onConfirmReplacement = workflowVm::confirmReplacement,
                                onExternalActivityChanged = ::setExternalActivityActive,
                                pendingShareCount = workflow.pendingShares.size,
                                discovery = discovery,
                                nearbyDisplayName = settings.nearbyDisplayName,
                                nearbyVisibility =
                                    NearbyVisibility.fromPersisted(settings.nearbyVisibility),
                                onToggleDiscovery = {
                                    toggleNearbyDiscovery(discovery.active)
                                },
                                onRequestNearbyPermission = ::requestNearbyPermission,
                                onOfferNearbyInvite = discoveryVm::offerInvite,
                                onConsumeNearbyOffer = discoveryVm::consumeRendezvousOffer,
                                onSaveNearbyDisplayName = SettingsStore::setNearbyDisplayName,
                                onSetNearbyVisibility = {
                                    SettingsStore.setNearbyVisibility(it.persistedValue)
                                },
                            )
                        WorkflowScreen.Room -> {
                            val draft = workflow.room
                            if (draft == null) {
                                ConnectionHubScreen(
                                    control = workflow.control,
                                    onShareViaNfc = ::beginNfcPresentation,
                                    onStopNfcSharing = nfcInvitationHostController::cancelPresentation,
                                    onScanNfc = ::beginNfcReading,
                                    onRevealInvite = workflowVm::revealRoomInvite,
                                    onHideInvite = workflowVm::hideRoomInvite,
                                    onRefreshInvite = workflowVm::refreshRoomInvite,
                                    onEndWaitingRoom = ::endWaitingRoom,
                                    onJoinInvite = { workflowVm.joinRoom(it) },
                                    nfcPhoneHosting = nfcPhoneHosting,
                                    nfcPhoneReader = nfcPhoneReader,
                                    onNearbyRoom = workflowVm::startNearbyRoom,
                                    onReturnToRoom = workflowVm::returnToCurrentRoom,
                                    onActivity = workflowVm::openActivity,
                                    onRooms = workflowVm::openRooms,
                                    onSettings = workflowVm::openSettings,
                                    onAcceptIncomingOffer = workflowVm::acceptIncomingOffer,
                                    onCancelReplacement = workflowVm::cancelReplacement,
                                    onConfirmReplacement = workflowVm::confirmReplacement,
                                    onExternalActivityChanged = ::setExternalActivityActive,
                                    pendingShareCount = workflow.pendingShares.size,
                                    discovery = discovery,
                                    nearbyDisplayName = settings.nearbyDisplayName,
                                    nearbyVisibility =
                                        NearbyVisibility.fromPersisted(settings.nearbyVisibility),
                                    onToggleDiscovery = {
                                        toggleNearbyDiscovery(discovery.active)
                                    },
                                    onRequestNearbyPermission = ::requestNearbyPermission,
                                    onOfferNearbyInvite = discoveryVm::offerInvite,
                                    onConsumeNearbyOffer = discoveryVm::consumeRendezvousOffer,
                                    onSaveNearbyDisplayName = SettingsStore::setNearbyDisplayName,
                                    onSetNearbyVisibility = {
                                        SettingsStore.setNearbyVisibility(it.persistedValue)
                                    },
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
                                    onExternalActivityChanged = ::setExternalActivityActive,
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
                        WorkflowScreen.Rooms ->
                            RememberedRoomsScreen(
                                state = rememberedRooms,
                                onBack = workflowVm::navigateBack,
                                onOpenRoom = workflowVm::openRememberedRoom,
                                onDismissError = rememberedRoomsVm::clearError,
                            )
                        WorkflowScreen.RememberedRoom -> {
                            val relationshipId = workflow.selectedRememberedRelationshipId
                            val peer =
                                rememberedRooms.peers.firstOrNull {
                                    it.relationshipId == relationshipId
                                }
                            RememberedRoomDetailScreen(
                                peer = peer,
                                connection = rememberedRooms.connections[relationshipId],
                                transferState = rememberedRooms.transfers[relationshipId],
                                error = rememberedRooms.error,
                                connectionManager = rememberedRoomConnections,
                                onBack = workflowVm::navigateBack,
                                onRetry = rememberedRoomsVm::retry,
                                onRename = rememberedRoomsVm::rename,
                                onForget = rememberedRoomsVm::forget,
                                onQueuePrepared = rememberedRoomsVm::enqueuePrepared,
                                onRetryOutbox = rememberedRoomsVm::retryOutbox,
                                onRemoveOutbox = rememberedRoomsVm::removeOutbox,
                                onAcceptIncoming = rememberedRoomsVm::acceptIncoming,
                                onRejectIncoming = rememberedRoomsVm::rejectIncoming,
                                onClearTransferError = rememberedRoomsVm::clearTransferError,
                                onOpenReceived = ::openReceived,
                                onShareReceived = ::shareReceived,
                                onExternalActivityChanged = ::setExternalActivityActive,
                                onDismissError = rememberedRoomsVm::clearError,
                            )
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
                    NfcInvitationOverlay(
                        state = nfcInvitation,
                        onConfirm = {
                            nfcInvitationController.confirmInvitation { invitation ->
                                workflowVm.joinRoom(invitation)
                            }
                        },
                        onCancelConfirmation = nfcInvitationController::cancelConfirmation,
                        onDismissFailure = nfcInvitationController::dismissFailure,
                    )
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
        if (intent?.action == NfcAdapter.ACTION_NDEF_DISCOVERED) {
            val messages =
                try {
                    ndefMessages(intent)
                } finally {
                    consumeNfcIntent(intent)
                }
            nfcInvitationController.acceptDiscoveredMessages(messages.orEmpty())
            return
        }
        val value = intent?.dataString ?: return
        if (value.startsWith(NfcInvitationContract.CARRIER_PREFIX)) {
            consumeNfcIntent(intent)
            nfcInvitationController.acceptDiscoveredCarrier(value)
            return
        }
        if (value.startsWith("envoix://invite/v2/") ||
            value.startsWith("envoix://room/")
        ) {
            consumeNfcIntent(intent)
            nfcInvitationController.acceptDiscoveredInvitation(
                validatedRawNfcViewInvitation(
                    value = value,
                    validateRoomInvite = ::validateNativeRoomNfcInvitation,
                    validateTransferInvite = { candidate ->
                        InviteCodec.parseForRouting(candidate) != null
                    },
                ),
            )
        }
    }

    private fun validateNativeRoomNfcInvitation(value: String): Boolean {
        val settings = SettingsStore.settings.value
        return isStrictNativeRoomNfcInvitation(
            value = value,
            fallbackBroker = settings.broker,
            fallbackRelay = settings.relay,
        )
    }

    override fun onStart() {
        super.onStart()
        workflowVm.setForeground(true)
        discoveryVm.setForeground(true)
        rememberedRoomConnections.setForeground(true)
    }

    override fun onResume() {
        super.onResume()
        nfcInvitationHostController.onResume()
        nfcInvitationReaderController.onResume()
        nfcInvitationHostController.setInvitation(
            activeHostedNfcInvitation(workflowVm.uiState.value),
        )
    }

    private fun observeHostedNfcInvitations() {
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.RESUMED) {
                workflowVm.uiState
                    .map { state -> activeHostedNfcInvitation(state) }
                    .distinctUntilChanged()
                    .collect(nfcInvitationHostController::setInvitation)
            }
        }
    }

    private fun observeNfcRoleCoordination() {
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.RESUMED) {
                launch {
                    workflowVm.uiState
                        .map { state -> state.screen }
                        .distinctUntilChanged()
                        .collect { screen ->
                            if (screen == WorkflowScreen.Hub) {
                                nfcInvitationReaderController.enterConnect()
                                nfcInvitationReaderController.resetAutomaticGate()
                            } else {
                                nfcInvitationHostController.leaveConnect()
                                nfcInvitationReaderController.leaveConnect()
                            }
                        }
                }
                launch {
                    nfcInvitationHostController.state
                        .map { state -> state.armed }
                        .distinctUntilChanged()
                        .collect { armed ->
                            if (armed) {
                                nfcInvitationReaderController.stop()
                                startNfcReadinessIfNeeded()
                            } else {
                                activeNfcReadinessOfferId?.let(
                                    discoveryVm::stopNfcReadinessOffer,
                                )
                                activeNfcReadinessOfferId = null
                            }
                        }
                }
                launch {
                    discoveryVm.uiState
                        .map { state -> state.nfcReadinessOffer }
                        .distinctUntilChanged()
                        .collect { offer ->
                            offer ?: return@collect
                            discoveryVm.consumeNfcReadinessOffer(offer.offerId)
                            if (workflowVm.uiState.value.screen != WorkflowScreen.Hub ||
                                nfcInvitationHostController.state.value.armed
                            ) {
                                return@collect
                            }
                            nfcInvitationHostController.cancelPresentation()
                            nfcInvitationReaderController.startAutomatic(
                                offer = offer,
                                nowMs = SystemClock.elapsedRealtime(),
                            )
                        }
                }
            }
        }
    }

    private fun beginNfcPresentation() {
        nfcInvitationReaderController.stop()
        activeNfcReadinessOfferId?.let(discoveryVm::stopNfcReadinessOffer)
        activeNfcReadinessOfferId = null
        nfcInvitationHostController.beginPresentation(
            activeHostedNfcInvitation(workflowVm.uiState.value),
        )
        workflowVm.shareRoomViaNfc()
        if (nfcInvitationHostController.state.value.armed) {
            startNfcReadinessIfNeeded()
        }
    }

    private fun startNfcReadinessIfNeeded() {
        if (activeNfcReadinessOfferId != null ||
            !nfcInvitationHostController.state.value.armed
        ) {
            return
        }
        activeNfcReadinessOfferId = discoveryVm.startNfcReadinessOffer()
    }

    private fun beginNfcReading() {
        if (nfcInvitationReaderController.state.value.scanning) {
            nfcInvitationReaderController.stop()
            return
        }
        nfcInvitationHostController.cancelPresentation()
        activeNfcReadinessOfferId?.let(discoveryVm::stopNfcReadinessOffer)
        activeNfcReadinessOfferId = null
        nfcInvitationReaderController.enterConnect()
        nfcInvitationReaderController.startManual()
    }

    override fun onPause() {
        nfcInvitationReaderController.onPause()
        nfcInvitationHostController.onPause()
        activeNfcReadinessOfferId?.let(discoveryVm::stopNfcReadinessOffer)
        activeNfcReadinessOfferId = null
        super.onPause()
    }

    override fun onStop() {
        // Rotation/recreation keeps the same retained ViewModels and must not
        // be interpreted as the user backgrounding the room.
        if (!isChangingConfigurations) {
            nfcInvitationHostController.clear()
            nfcInvitationController.stop()
            workflowVm.setForeground(false)
            discoveryVm.setForeground(false)
            rememberedRoomConnections.setForeground(false)
        }
        super.onStop()
    }

    override fun onDestroy() {
        nfcInvitationReaderController.close()
        nfcInvitationHostController.close()
        nfcInvitationController.close()
        super.onDestroy()
    }

    private fun endWaitingRoom() {
        nfcInvitationHostController.clear()
        workflowVm.endWaitingRoom()
    }

    @Suppress("DEPRECATION")
    private fun ndefMessages(intent: Intent): List<NdefMessage>? {
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent
                    .getParcelableArrayExtra(
                        NfcAdapter.EXTRA_NDEF_MESSAGES,
                        NdefMessage::class.java,
                    )?.toList()
            } else {
                val rawMessages =
                    intent.getParcelableArrayExtra(NfcAdapter.EXTRA_NDEF_MESSAGES)
                        ?: return null
                if (!rawMessages.all { it is NdefMessage }) return null
                rawMessages.map { it as NdefMessage }
            }
        } catch (_: BadParcelableException) {
            null
        } catch (_: ClassCastException) {
            null
        }
    }

    private fun consumeNfcIntent(intent: Intent) {
        intent.action = null
        intent.data = null
        intent.clipData = null
        intent.replaceExtras(Bundle())
        setIntent(intent)
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

    private fun toggleNearbyDiscovery(active: Boolean) {
        if (active) {
            discoveryVm.stop()
        } else if (DiscoveryPermissions.hasBluetoothPermissions(this)) {
            discoveryVm.start()
        } else {
            requestNearbyPermission()
        }
    }

    private fun requestNearbyPermission() {
        requestNearbyPermissions.launch(
            DiscoveryPermissions.bluetoothRuntimePermissions(),
        )
    }

    private fun setExternalActivityActive(active: Boolean) {
        workflowVm.setExternalActivityActive(active)
        rememberedRoomConnections.setExternalActivityActive(active)
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

internal fun activeHostedNfcInvitation(
    workflow: ConnectionWorkflowUiState,
    nowEpochMs: Long = System.currentTimeMillis(),
): String? {
    if (workflow.screen != WorkflowScreen.Hub) return null
    if (workflow.control.phase != RoomControlPhase.Hosting) return null
    if (workflow.control.verificationCode != null) return null
    val invitation = workflow.control.invite ?: return null
    val payload = invitation.payload
    return payload.takeIf {
        invitation.expiresAtEpochMs > nowEpochMs &&
            RoomControlInviteFormat.looksLikeRoomInvite(payload) &&
            NfcInvitationContract.isCanonicalInvitation(payload)
    }
}

internal fun validatedRawNfcViewInvitation(
    value: String,
    validateRoomInvite: (String) -> Boolean,
    validateTransferInvite: (String) -> Boolean,
): String? {
    val decoded =
        NfcInvitationContract.decode(
            value.toByteArray(Charsets.UTF_8),
        )
    if (decoded != value) return null
    return validatedNfcInvitation(
        value = decoded,
        validateRoomInvite = validateRoomInvite,
        validateTransferInvite = validateTransferInvite,
    )
}

internal fun validatedNfcInvitation(
    value: String,
    validateRoomInvite: (String) -> Boolean,
    validateTransferInvite: (String) -> Boolean,
): String? =
    when {
        RoomControlInviteFormat.looksLikeRoomInvite(value) &&
            validateRoomInvite(value) -> value
        value.startsWith("envoix://invite/v2/") &&
            validateTransferInvite(value) -> value
        else -> null
    }

internal fun isStrictNativeRoomNfcInvitation(
    value: String,
    fallbackBroker: String,
    fallbackRelay: String,
    nowEpochMs: Long = System.currentTimeMillis(),
    parseRoomInvite: (String, String, String) -> FfiRoomControlInvite =
        ::parseRoomControlInvite,
): Boolean {
    if (!NfcInvitationContract.isCanonicalInvitation(value) ||
        !RoomControlInviteFormat.looksLikeRoomInvite(value)
    ) {
        return false
    }
    return runCatching {
        val parsed = parseRoomInvite(value, fallbackBroker, fallbackRelay)
        parsed.payload == value &&
            parsed.expiresAtEpochMs <= Long.MAX_VALUE.toULong() &&
            parsed.expiresAtEpochMs.toLong() > nowEpochMs
    }.getOrDefault(false)
}
