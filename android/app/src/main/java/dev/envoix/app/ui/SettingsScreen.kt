package dev.envoix.app.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Info
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.R
import dev.envoix.app.Settings
import dev.envoix.app.WifiAwareProbeRole
import kotlin.math.roundToInt

@Composable
internal fun SettingsScreen(
    settings: Settings,
    saveLocationLabel: String,
    savePickerInitialUri: Uri,
    avoidsTailscale: Boolean,
    onUpdateSettings: ((Settings) -> Settings) -> Unit,
    onSaveTreePicked: (Uri) -> Unit,
    onResetSaveTree: () -> Unit,
    onAvoidTailscaleChanged: (Boolean) -> Unit,
    onLoggingSettingsChanged: ((Settings) -> Settings) -> Unit,
    diagnostics: SettingsDiagnosticsUiState,
    onRequestNearbyWifiPermission: () -> Unit,
    onStartWifiAwareProbe: (WifiAwareProbeRole) -> Unit,
    onStopWifiAwareProbe: () -> Unit,
    onBack: () -> Unit,
) {
    val colors = Envoix.colors

    // Local buffers preserve in-progress text while each edit emits a settings intent.
    var broker by remember { mutableStateOf(settings.broker) }
    var relay by remember { mutableStateOf(settings.relay) }
    var dataStreamWindow by remember { mutableStateOf(settings.dataStreamWindow) }
    val folderPicker =
        rememberLauncherForActivityResult(
            ActivityResultContracts.OpenDocumentTree(),
        ) { uri -> if (uri != null) onSaveTreePicked(uri) }
    var allowText by remember { mutableStateOf(settings.candidatesAllow.joinToString("\n")) }
    var denyText by remember { mutableStateOf(settings.candidatesDeny.joinToString("\n")) }
    var logServer by remember { mutableStateOf(settings.logServer) }
    var showAdvanced by remember { mutableStateOf(false) }
    var showCompressionInfo by remember { mutableStateOf(false) }

    // Reflect external edits to the candidate lists (e.g. the Avoid-Tailscale
    // toggle mutating `deny`) back into the raw editors, without clobbering
    // in-progress typing (which already parses to the same list).
    LaunchedEffect(settings.candidatesDeny) {
        if (cidrLines(denyText) != settings.candidatesDeny) {
            denyText = settings.candidatesDeny.joinToString("\n")
        }
    }
    LaunchedEffect(settings.candidatesAllow) {
        if (cidrLines(allowText) != settings.candidatesAllow) {
            allowText = settings.candidatesAllow.joinToString("\n")
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(colors.bg),
    ) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp)
                .padding(bottom = 40.dp),
        ) {
            Row(
                Modifier.fillMaxWidth().padding(top = 20.dp, bottom = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = appString(R.string.common_back),
                    tint = colors.accent,
                    modifier = Modifier.clip(CircleShape).clickable(onClick = onBack).padding(6.dp),
                )
                Spacer(Modifier.width(8.dp))
                Text(appString(R.string.settings), color = colors.text, fontSize = 26.sp, fontWeight = FontWeight.ExtraBold)
            }

            SectionLabel(appString(R.string.settings_basic_section))
            LabeledControl(appString(R.string.settings_language)) {
                LanguageToggle(settings.language) {
                    onUpdateSettings { current -> current.copy(language = it) }
                }
            }
            Spacer(Modifier.height(18.dp))
            FolderPickerRow(
                label = saveLocationLabel,
                custom = settings.saveTreeUri.isNotBlank(),
                onPick = { folderPicker.launch(savePickerInitialUri) },
                onReset = onResetSaveTree,
            )
            Spacer(Modifier.height(18.dp))
            LabeledControl(appString(R.string.settings_default_transfer_role)) {
                RoleToggle(settings.defaultRole) { onUpdateSettings { s -> s.copy(defaultRole = it) } }
            }
            Spacer(Modifier.height(18.dp))
            Column {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        appString(R.string.settings_compression_section),
                        color = colors.muted,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        letterSpacing = 1.sp,
                    )
                    Spacer(Modifier.width(6.dp))
                    Icon(
                        Icons.Filled.Info,
                        contentDescription = appString(R.string.settings_compression_info),
                        tint = colors.muted,
                        modifier =
                            Modifier
                                .size(18.dp)
                                .clip(CircleShape)
                                .clickable { showCompressionInfo = true },
                    )
                }
                Spacer(Modifier.height(6.dp))
                CompressionToggle(settings.compressionPolicy) {
                    onUpdateSettings { current -> current.copy(compressionPolicy = it) }
                }
            }
            Spacer(Modifier.height(18.dp))
            ToggleRow(
                title = appString(R.string.settings_avoid_tailscale),
                subtitle = appString(R.string.settings_avoid_tailscale_description),
                checked = avoidsTailscale,
            ) { onAvoidTailscaleChanged(it) }

            Spacer(Modifier.height(26.dp))
            AdvancedHeader(showAdvanced) { showAdvanced = !showAdvanced }
            if (showAdvanced) {
                Spacer(Modifier.height(16.dp))
                SectionLabel(appString(R.string.settings_servers_section))
                Field(appString(R.string.settings_broker_label), broker) {
                    broker = it
                    onUpdateSettings { s -> s.copy(broker = it) }
                }
                Spacer(Modifier.height(12.dp))
                Field(appString(R.string.settings_relay_label), relay) {
                    relay = it
                    onUpdateSettings { s -> s.copy(relay = it) }
                }
                Spacer(Modifier.height(12.dp))
                Field(appString(R.string.settings_log_server_label), logServer) {
                    logServer = it
                    onUpdateSettings { s -> s.copy(logServer = it) }
                }

                Spacer(Modifier.height(22.dp))
                SectionLabel(appString(R.string.settings_config_file_section))
                Field(appString(R.string.settings_data_stream_window), dataStreamWindow) {
                    dataStreamWindow = it
                    onUpdateSettings { s -> s.copy(dataStreamWindow = it) }
                }
                Spacer(Modifier.height(12.dp))
                MultilineField(appString(R.string.settings_candidate_allow), allowText) {
                    allowText = it
                    onUpdateSettings { s -> s.copy(candidatesAllow = cidrLines(it)) }
                }
                Spacer(Modifier.height(12.dp))
                MultilineField(appString(R.string.settings_candidate_deny), denyText) {
                    denyText = it
                    onUpdateSettings { s -> s.copy(candidatesDeny = cidrLines(it)) }
                }

                Spacer(Modifier.height(22.dp))
                SectionLabel(appString(R.string.settings_developer_section))
                ToggleRow(
                    title = appString(R.string.settings_developer_mode),
                    subtitle = appString(R.string.settings_developer_mode_description),
                    checked = settings.devMode,
                ) { onUpdateSettings { s -> s.copy(devMode = it) } }
                if (settings.devMode) {
                    Spacer(Modifier.height(16.dp))
                    ToggleRow(
                        title = appString(R.string.settings_verbose_logging),
                        subtitle = appString(R.string.settings_verbose_logging_description),
                        checked = settings.verboseLog,
                    ) {
                        onLoggingSettingsChanged { s -> s.copy(verboseLog = it) }
                    }
                    Spacer(Modifier.height(12.dp))
                    ToggleRow(
                        title = appString(R.string.settings_trace_iroh),
                        subtitle = appString(R.string.settings_trace_iroh_description),
                        checked = settings.traceIroh,
                    ) {
                        onLoggingSettingsChanged { s -> s.copy(traceIroh = it) }
                    }
                    Spacer(Modifier.height(16.dp))
                    Text(
                        appString(
                            R.string.settings_wifi_aware_status,
                            diagnostics.capability?.diagnosticSummary
                                ?: appString(R.string.settings_checking),
                        ),
                        color = colors.muted,
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                    )
                    Spacer(Modifier.height(10.dp))
                    Text(
                        appString(
                            R.string.settings_probe_status,
                            diagnostics.probe.diagnosticSummary,
                        ),
                        color =
                            if (diagnostics.probe.phase == dev.envoix.app.WifiAwareProbePhase.FAILED) {
                                colors.danger
                            } else {
                                colors.muted
                            },
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                    )
                    Spacer(Modifier.height(10.dp))
                    if (
                        diagnostics.canRequestNearbyPermission
                    ) {
                        OutlinedButton(
                            onClick = onRequestNearbyWifiPermission,
                        ) {
                            Text(appString(R.string.settings_grant_nearby_wifi_permission))
                        }
                        Spacer(Modifier.height(8.dp))
                    }
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Button(
                            onClick = { onStartWifiAwareProbe(WifiAwareProbeRole.PUBLISHER) },
                            enabled = diagnostics.canStartProbe,
                            modifier = Modifier.weight(1f),
                        ) {
                            Text(appString(R.string.settings_receive_probe))
                        }
                        OutlinedButton(
                            onClick = { onStartWifiAwareProbe(WifiAwareProbeRole.SUBSCRIBER) },
                            enabled = diagnostics.canStartProbe,
                            modifier = Modifier.weight(1f),
                        ) {
                            Text(appString(R.string.settings_send_probe))
                        }
                    }
                    if (diagnostics.probeRunning) {
                        Spacer(Modifier.height(8.dp))
                        OutlinedButton(
                            onClick = onStopWifiAwareProbe,
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(appString(R.string.settings_stop_probe))
                        }
                    }
                }
            }
        }

        if (showCompressionInfo) {
            CompressionInfoOverlay(onDismiss = { showCompressionInfo = false })
        }
    }
}

@Composable
private fun LanguageToggle(
    language: String,
    onChange: (String) -> Unit,
) {
    SegmentedControl(
        options =
            listOf(
                appString(R.string.language_english_short),
                appString(R.string.language_chinese_short),
            ),
        selectedIndex = if (language == AppText.ENGLISH) 0 else 1,
        onSelect = { i -> onChange(if (i == 0) AppText.ENGLISH else AppText.SIMPLIFIED_CHINESE) },
    )
}

private fun cidrLines(t: String): List<String> = t.lines().map { it.trim() }.filter { it.isNotEmpty() }

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        color = Envoix.colors.accent,
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        letterSpacing = 1.1.sp,
        modifier = Modifier.padding(bottom = 10.dp),
    )
}

@Composable
private fun FolderPickerRow(
    label: String,
    custom: Boolean,
    onPick: () -> Unit,
    onReset: () -> Unit,
) {
    val colors = Envoix.colors
    Text(
        appString(R.string.settings_save_received_files_to),
        color = colors.muted,
        fontSize = 11.sp,
        fontWeight = FontWeight.Bold,
        letterSpacing = 1.sp,
    )
    Spacer(Modifier.height(6.dp))
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .border(1.dp, colors.line, RoundedCornerShape(12.dp))
            .clickable(onClick = onPick)
            .padding(horizontal = 14.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            label,
            color = colors.text,
            fontSize = 14.sp,
            fontFamily = FontFamily.Monospace,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        Spacer(Modifier.width(10.dp))
        Text(
            appString(R.string.settings_change_destination),
            color = colors.accent,
            fontSize = 13.sp,
            fontWeight = FontWeight.Bold,
        )
    }
    if (custom) {
        Spacer(Modifier.height(6.dp))
        Text(
            appString(R.string.settings_reset_to_downloads),
            color = colors.accent,
            fontSize = 12.sp,
            modifier =
                Modifier
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(onClick = onReset)
                    .padding(vertical = 3.dp, horizontal = 4.dp),
        )
    }
}

@Composable
private fun Field(
    label: String,
    value: String,
    onChange: (String) -> Unit,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        singleLine = true,
        textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun MultilineField(
    label: String,
    value: String,
    onChange: (String) -> Unit,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp),
        modifier = Modifier.fillMaxWidth().heightIn(min = 84.dp),
    )
}

@Composable
private fun LabeledControl(
    title: String,
    control: @Composable () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            title,
            color = colors.text,
            fontSize = 16.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.weight(1f).padding(end = 12.dp),
        )
        control()
    }
}

@Composable
private fun RoleToggle(
    role: String,
    onChange: (String) -> Unit,
) {
    SegmentedControl(
        options =
            listOf(
                appString(R.string.send_action_title),
                appString(R.string.receive_action_title),
            ),
        selectedIndex = if (role == "send") 0 else 1,
        onSelect = { i -> onChange(if (i == 0) "send" else "receive") },
    )
}

@Composable
private fun CompressionToggle(
    policy: String,
    onChange: (String) -> Unit,
) {
    SegmentedControl(
        options =
            listOf(
                appString(R.string.settings_compression_never),
                appString(R.string.settings_compression_always),
                appString(R.string.settings_compression_smart),
            ),
        selectedIndex =
            when (policy) {
                "never" -> 0
                "always" -> 1
                else -> 2
            },
        onSelect = { i ->
            onChange(
                when (i) {
                    0 -> "never"
                    1 -> "always"
                    else -> "smart"
                },
            )
        },
        modifier = Modifier.fillMaxWidth(),
        equalWidth = true,
    )
}

@Composable
private fun SegmentedControl(
    options: List<String>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    modifier: Modifier = Modifier,
    equalWidth: Boolean = false,
) {
    val colors = Envoix.colors
    val density = LocalDensity.current
    val offsets = remember { mutableStateMapOf<Int, Int>() }
    val widths = remember { mutableStateMapOf<Int, Int>() }
    var rowHeightPx by remember { mutableStateOf(0f) }

    val targetX = offsets[selectedIndex] ?: 0
    val targetW = widths[selectedIndex] ?: 0
    val animX by animateFloatAsState(targetX.toFloat(), tween<Float>(300), label = "seg_x")
    val animW by animateFloatAsState(targetW.toFloat(), tween<Float>(300), label = "seg_w")

    Box(
        modifier
            .clip(RoundedCornerShape(10.dp))
            .background(colors.bg)
            .border(1.dp, colors.line, RoundedCornerShape(10.dp)),
    ) {
        if (animW > 0f && rowHeightPx > 0f) {
            Box(
                Modifier
                    .padding(3.dp)
                    .offset { IntOffset(animX.roundToInt(), 0) }
                    .size(
                        width = with(density) { animW.toDp() },
                        height = with(density) { rowHeightPx.toDp() },
                    ).clip(RoundedCornerShape(8.dp))
                    .background(colors.accent),
            )
        }

        Row(
            Modifier
                .padding(3.dp)
                .onSizeChanged { rowHeightPx = it.height.toFloat() },
        ) {
            options.forEachIndexed { index, text ->
                val wm = if (equalWidth) Modifier.weight(1f) else Modifier
                Box(
                    wm
                        .onGloballyPositioned { c ->
                            val pos = c.positionInParent()
                            offsets[index] = pos.x.roundToInt()
                            widths[index] = c.size.width
                        }.clip(RoundedCornerShape(8.dp))
                        .clickable { onSelect(index) }
                        .padding(horizontal = 16.dp, vertical = 7.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    val isSel = index == selectedIndex
                    val tc by animateColorAsState(
                        if (isSel) Color.White else colors.muted,
                        tween(200),
                        "tc$index",
                    )
                    Text(text, color = tc, fontSize = 13.sp, fontWeight = FontWeight.Bold)
                }
            }
        }
    }
}

@Composable
private fun CompressionInfoOverlay(onDismiss: () -> Unit) {
    val colors = Envoix.colors
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appString(R.string.common_ok))
            }
        },
        title = null,
        text = {
            Text(
                appString(R.string.settings_compression_explanation),
                color = colors.text,
                fontSize = 15.sp,
            )
        },
    )
}

@Composable
private fun AdvancedHeader(
    expanded: Boolean,
    onToggle: () -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onToggle)
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            appString(R.string.settings_advanced),
            color = colors.text,
            fontSize = 16.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.weight(1f))
        Icon(
            if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
            contentDescription =
                if (expanded) {
                    appString(R.string.common_collapse)
                } else {
                    appString(R.string.common_expand)
                },
            tint = colors.muted,
        )
    }
}

@Composable
private fun ToggleRow(
    title: String,
    subtitle: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            Text(subtitle, color = colors.muted, fontSize = 13.sp)
        }
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            colors =
                SwitchDefaults.colors(
                    checkedThumbColor = Color.White,
                    checkedTrackColor = colors.accent,
                ),
        )
    }
}
