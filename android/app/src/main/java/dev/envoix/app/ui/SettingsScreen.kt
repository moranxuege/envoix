package dev.envoix.app.ui

import android.Manifest
import android.os.Build
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
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import dev.envoix.app.AndroidWifiAwareCapabilityProbe
import dev.envoix.app.AndroidWifiAwareDiagnosticController
import dev.envoix.app.SettingsStore
import dev.envoix.app.WifiAwareAvailability
import dev.envoix.app.WifiAwareCapabilitySnapshot
import dev.envoix.app.WifiAwareProbeRole

@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val colors = Envoix.colors
    val settings by SettingsStore.settings.collectAsState()

    // local buffers for text fields; each commits into the store on change
    var broker by remember { mutableStateOf(settings.broker) }
    var relay by remember { mutableStateOf(settings.relay) }
    var dataStreamWindow by remember { mutableStateOf(settings.dataStreamWindow) }
    val context = LocalContext.current
    val folderPicker =
        rememberLauncherForActivityResult(
            ActivityResultContracts.OpenDocumentTree(),
        ) { uri -> if (uri != null) SettingsStore.setSaveTree(context, uri) }
    var allowText by remember { mutableStateOf(settings.candidatesAllow.joinToString("\n")) }
    var denyText by remember { mutableStateOf(settings.candidatesDeny.joinToString("\n")) }
    var logServer by remember { mutableStateOf(settings.logServer) }
    var showAdvanced by remember { mutableStateOf(false) }
    var showCompressionInfo by remember { mutableStateOf(false) }
    var wifiAwareCapability by remember { mutableStateOf<WifiAwareCapabilitySnapshot?>(null) }
    var wifiAwareRefreshKey by remember { mutableStateOf(0) }
    val wifiAwareController = remember(context) { AndroidWifiAwareDiagnosticController(context) }
    val wifiAwareProbe by wifiAwareController.snapshot.collectAsState()
    val wifiAwarePermissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
            wifiAwareRefreshKey += 1
            wifiAwareController.refresh()
        }

    DisposableEffect(wifiAwareController) {
        onDispose { wifiAwareController.close() }
    }

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
    LaunchedEffect(settings.devMode, wifiAwareRefreshKey) {
        wifiAwareCapability =
            if (settings.devMode) AndroidWifiAwareCapabilityProbe.read(context) else null
        if (settings.devMode) {
            wifiAwareController.refresh()
        } else {
            wifiAwareController.stop()
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
                contentDescription = appText("Back", "返回"),
                tint = colors.accent,
                modifier = Modifier.clip(CircleShape).clickable(onClick = onBack).padding(6.dp),
            )
            Spacer(Modifier.width(8.dp))
            Text(appText("Settings", "设置"), color = colors.text, fontSize = 26.sp, fontWeight = FontWeight.ExtraBold)
        }

        SectionLabel(appText("BASIC", "基本"))
        LabeledControl(appText("Language", "语言")) {
            LanguageToggle(settings.language) {
                SettingsStore.update { current -> current.copy(language = it) }
            }
        }
        Spacer(Modifier.height(18.dp))
        FolderPickerRow(
            label = SettingsStore.saveLabel(context),
            custom = settings.saveTreeUri.isNotBlank(),
            onPick = { folderPicker.launch(SettingsStore.savePickerInitialUri()) },
            onReset = { SettingsStore.setSaveTree(context, null) },
        )
        Spacer(Modifier.height(18.dp))
        LabeledControl(appText("Default role for a new code", "新配对码的默认角色")) {
            RoleToggle(settings.defaultRole) { SettingsStore.update { s -> s.copy(defaultRole = it) } }
        }
        Spacer(Modifier.height(18.dp))
        Column {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    appText("COMPRESSION", "压缩"),
                    color = colors.muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 1.sp,
                )
                Spacer(Modifier.width(6.dp))
                Icon(
                    Icons.Filled.Info,
                    contentDescription = appText("Compression info", "压缩说明"),
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
                SettingsStore.update { current -> current.copy(compressionPolicy = it) }
            }
        }
        Spacer(Modifier.height(18.dp))
        ToggleRow(
            title = appText("Avoid Tailscale addresses", "避开 Tailscale 地址"),
            subtitle =
                appText(
                    "Don't advertise your 100.x Tailscale IP, so transfers take the real WAN or relay path.",
                    "不公布 100.x Tailscale IP，使传输使用真实广域网或中继路径。",
                ),
            checked = SettingsStore.avoidsTailscale(settings),
        ) { SettingsStore.setAvoidTailscale(it) }

        Spacer(Modifier.height(26.dp))
        AdvancedHeader(showAdvanced) { showAdvanced = !showAdvanced }
        if (showAdvanced) {
            Spacer(Modifier.height(16.dp))
            SectionLabel(appText("SERVERS", "服务器"))
            Field(appText("Broker · rendezvous", "会合服务器"), broker) {
                broker = it
                SettingsStore.update { s -> s.copy(broker = it) }
            }
            Spacer(Modifier.height(12.dp))
            Field(appText("Relay · data path", "中继服务器 · 数据路径"), relay) {
                relay = it
                SettingsStore.update { s -> s.copy(relay = it) }
            }
            Spacer(Modifier.height(12.dp))
            Field(appText("Log server · diagnostics", "日志服务器 · 诊断"), logServer) {
                logServer = it
                SettingsStore.update { s -> s.copy(logServer = it) }
            }

            Spacer(Modifier.height(22.dp))
            SectionLabel("CONFIG.TOML")
            Field(appText("Data stream window · e.g. 32MB (default 16MB)", "数据流窗口 · 例如 32MB（默认 16MB）"), dataStreamWindow) {
                dataStreamWindow = it
                SettingsStore.update { s -> s.copy(dataStreamWindow = it) }
            }
            Spacer(Modifier.height(12.dp))
            MultilineField(appText("Candidate allow · one CIDR per line", "允许的候选地址 · 每行一个 CIDR"), allowText) {
                allowText = it
                SettingsStore.update { s -> s.copy(candidatesAllow = cidrLines(it)) }
            }
            Spacer(Modifier.height(12.dp))
            MultilineField(appText("Candidate deny · one CIDR per line", "拒绝的候选地址 · 每行一个 CIDR"), denyText) {
                denyText = it
                SettingsStore.update { s -> s.copy(candidatesDeny = cidrLines(it)) }
            }

            Spacer(Modifier.height(22.dp))
            SectionLabel(appText("DEVELOPER", "开发者"))
            ToggleRow(
                title = appText("Developer mode", "开发者模式"),
                subtitle = appText("Reveal diagnostics — verbose logging (and, later, log upload).", "显示诊断信息、详细日志及后续的日志上传功能。"),
                checked = settings.devMode,
            ) { SettingsStore.update { s -> s.copy(devMode = it) } }
            if (settings.devMode) {
                Spacer(Modifier.height(16.dp))
                ToggleRow(
                    title = appText("Verbose logging (-vv)", "详细日志（-vv）"),
                    subtitle =
                        appText(
                            "Also capture iroh internals: path selection, hole-punching. High volume.",
                            "同时记录 iroh 内部信息：路径选择与打洞。日志量较大。",
                        ),
                    checked = settings.verboseLog,
                ) {
                    SettingsStore.update { s -> s.copy(verboseLog = it) }
                    SettingsStore.applyLogLevel()
                }
                Spacer(Modifier.height(12.dp))
                ToggleRow(
                    title = appText("Trace iroh internals (-vvv)", "跟踪 iroh 内部状态（-vvv）"),
                    subtitle =
                        appText(
                            "Deepest: iroh path/QUIC state machine at trace. Very high volume — for chasing a crash.",
                            "最详细地跟踪 iroh 路径与 QUIC 状态机。日志量极大，仅用于排查崩溃。",
                        ),
                    checked = settings.traceIroh,
                ) {
                    SettingsStore.update { s -> s.copy(traceIroh = it) }
                    SettingsStore.applyLogLevel()
                }
                Spacer(Modifier.height(16.dp))
                Text(
                    "Wi-Fi Aware · ${wifiAwareCapability?.diagnosticSummary ?: "checking"}",
                    color = colors.muted,
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.height(10.dp))
                Text(
                    "Probe · ${wifiAwareProbe.diagnosticSummary}",
                    color =
                        if (wifiAwareProbe.phase == dev.envoix.app.WifiAwareProbePhase.FAILED) {
                            colors.danger
                        } else {
                            colors.muted
                        },
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.height(10.dp))
                if (
                    wifiAwareCapability?.availability == WifiAwareAvailability.PERMISSION_REQUIRED &&
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                ) {
                    OutlinedButton(
                        onClick = {
                            wifiAwarePermissionLauncher.launch(Manifest.permission.NEARBY_WIFI_DEVICES)
                        },
                    ) {
                        Text("Grant nearby Wi-Fi permission")
                    }
                    Spacer(Modifier.height(8.dp))
                }
                val probeEnabled =
                    wifiAwareCapability?.availability == WifiAwareAvailability.READY ||
                        wifiAwareCapability?.availability == WifiAwareAvailability.PAIRING_REQUIRED
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(
                        onClick = { wifiAwareController.start(WifiAwareProbeRole.PUBLISHER) },
                        enabled = probeEnabled,
                    ) {
                        Text("Receive probe")
                    }
                    OutlinedButton(
                        onClick = { wifiAwareController.start(WifiAwareProbeRole.SUBSCRIBER) },
                        enabled = probeEnabled,
                    ) {
                        Text("Send probe")
                    }
                    OutlinedButton(onClick = wifiAwareController::stop) {
                        Text("Stop")
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
        options = listOf("EN", "中文"),
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
        appText("SAVE RECEIVED FILES TO", "接收文件保存到"),
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
        Text(appText("Change", "更改"), color = colors.accent, fontSize = 13.sp, fontWeight = FontWeight.Bold)
    }
    if (custom) {
        Spacer(Modifier.height(6.dp))
        Text(
            appText("Reset to Downloads", "恢复为 Downloads"),
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
        options = listOf(appText("Send", "发送"), appText("Receive", "接收")),
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
        options = listOf(appText("Never", "从不"), appText("Always", "始终"), appText("Smart", "智能")),
        selectedIndex = when (policy) { "never" -> 0; "always" -> 1; else -> 2 },
        onSelect = { i -> onChange(when (i) { 0 -> "never"; 1 -> "always"; else -> "smart" }) },
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
                    )
                    .clip(RoundedCornerShape(8.dp))
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
                        }
                        .clip(RoundedCornerShape(8.dp))
                        .clickable { onSelect(index) }
                        .padding(horizontal = 16.dp, vertical = 7.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    val isSel = index == selectedIndex
                    val tc by animateColorAsState(
                        if (isSel) Color.White else colors.muted, tween(200), "tc$index",
                    )
                    Text(text, color = tc, fontSize = 13.sp, fontWeight = FontWeight.Bold)
                }
            }
        }
    }
}

@Composable
private fun CompressionInfoOverlay(
    onDismiss: () -> Unit,
) {
    val colors = Envoix.colors
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(appText("OK", "确定"))
            }
        },
        title = null,
        text = {
            Text(
                appText(
                    "Smart compression detects if the file type you're sending has already been compressed and avoids unnecessary recompression.",
                    "智能压缩会检测您发送的文件类型是否已经被压缩，避免不必要的重复压缩。",
                ),
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
        Text(appText("Advanced", "高级"), color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Icon(
            if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
            contentDescription = if (expanded) appText("Collapse", "收起") else appText("Expand", "展开"),
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
