package dev.envoix.app.ui

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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.ui.platform.LocalContext
import dev.envoix.app.SettingsStore

@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val colors = Envoix.colors
    val settings by SettingsStore.settings.collectAsState()

    // local buffers for text fields; each commits into the store on change
    var broker by remember { mutableStateOf(settings.broker) }
    var relay by remember { mutableStateOf(settings.relay) }
    var chunkSize by remember { mutableStateOf(settings.chunkSize) }
    val context = LocalContext.current
    val folderPicker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri -> if (uri != null) SettingsStore.setSaveTree(context, uri) }
    var allowText by remember { mutableStateOf(settings.candidatesAllow.joinToString("\n")) }
    var denyText by remember { mutableStateOf(settings.candidatesDeny.joinToString("\n")) }
    var logServer by remember { mutableStateOf(settings.logServer) }
    var showAdvanced by remember { mutableStateOf(false) }

    // Reflect external edits to the candidate lists (e.g. the Avoid-Tailscale
    // toggle mutating `deny`) back into the raw editors, without clobbering
    // in-progress typing (which already parses to the same list).
    LaunchedEffect(settings.candidatesDeny) {
        if (cidrLines(denyText) != settings.candidatesDeny)
            denyText = settings.candidatesDeny.joinToString("\n")
    }
    LaunchedEffect(settings.candidatesAllow) {
        if (cidrLines(allowText) != settings.candidatesAllow)
            allowText = settings.candidatesAllow.joinToString("\n")
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.bg)
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
                contentDescription = "Back",
                tint = colors.accent,
                modifier = Modifier.clip(CircleShape).clickable(onClick = onBack).padding(6.dp),
            )
            Spacer(Modifier.width(8.dp))
            Text("Settings", color = colors.text, fontSize = 26.sp, fontWeight = FontWeight.ExtraBold)
        }

        SectionLabel("BASIC")
        FolderPickerRow(
            label = SettingsStore.saveLabel(context),
            custom = settings.saveTreeUri.isNotBlank(),
            onPick = { folderPicker.launch(SettingsStore.savePickerInitialUri()) },
            onReset = { SettingsStore.setSaveTree(context, null) },
        )
        Spacer(Modifier.height(18.dp))
        LabeledControl("Default role for a new code") {
            RoleToggle(settings.defaultRole) { SettingsStore.update { s -> s.copy(defaultRole = it) } }
        }
        Spacer(Modifier.height(18.dp))
        ToggleRow(
            title = "Avoid Tailscale addresses",
            subtitle = "Don't advertise your 100.x Tailscale IP, so transfers take the real WAN or relay path.",
            checked = SettingsStore.avoidsTailscale(settings),
        ) { SettingsStore.setAvoidTailscale(it) }

        Spacer(Modifier.height(18.dp))
        ToggleRow(
            title = "Internet pairing",
            subtitle = "Pair through the rendezvous broker — works anywhere.",
            checked = settings.useRoom,
        ) { SettingsStore.update { s -> s.copy(useRoom = it) } }
        Spacer(Modifier.height(18.dp))
        ToggleRow(
            title = "Local Wi-Fi pairing (mDNS)",
            subtitle = "Also try nearby devices on the same Wi-Fi — works with no internet.",
            checked = settings.useMdns,
        ) { SettingsStore.update { s -> s.copy(useMdns = it) } }

        Spacer(Modifier.height(26.dp))
        AdvancedHeader(showAdvanced) { showAdvanced = !showAdvanced }
        if (showAdvanced) {
            Spacer(Modifier.height(16.dp))
            SectionLabel("RENDEZVOUS")
            Field("Broker", broker) { broker = it; SettingsStore.update { s -> s.copy(broker = it) } }
            Spacer(Modifier.height(12.dp))
            Field("Relay", relay) { relay = it; SettingsStore.update { s -> s.copy(relay = it) } }
            Spacer(Modifier.height(12.dp))
            Field("Log server · rdz /logs endpoint", logServer) {
                logServer = it; SettingsStore.update { s -> s.copy(logServer = it) }
            }

            Spacer(Modifier.height(22.dp))
            SectionLabel("CONFIG.TOML")
            Field("Chunk size · e.g. 16MB", chunkSize) {
                chunkSize = it; SettingsStore.update { s -> s.copy(chunkSize = it) }
            }
            Spacer(Modifier.height(12.dp))
            MultilineField("Candidate allow · one CIDR per line", allowText) {
                allowText = it; SettingsStore.update { s -> s.copy(candidatesAllow = cidrLines(it)) }
            }
            Spacer(Modifier.height(12.dp))
            MultilineField("Candidate deny · one CIDR per line", denyText) {
                denyText = it; SettingsStore.update { s -> s.copy(candidatesDeny = cidrLines(it)) }
            }

            Spacer(Modifier.height(22.dp))
            SectionLabel("DEVELOPER")
            ToggleRow(
                title = "Developer mode",
                subtitle = "Reveal diagnostics — verbose logging (and, later, log upload).",
                checked = settings.devMode,
            ) { SettingsStore.update { s -> s.copy(devMode = it) } }
            if (settings.devMode) {
                Spacer(Modifier.height(16.dp))
                ToggleRow(
                    title = "Verbose logging (-vv)",
                    subtitle = "Also capture iroh internals: path selection, hole-punching. High volume.",
                    checked = settings.verboseLog,
                ) {
                    SettingsStore.update { s -> s.copy(verboseLog = it) }
                    SettingsStore.applyLogLevel()
                }
            }
        }
    }
}

private fun cidrLines(t: String): List<String> =
    t.lines().map { it.trim() }.filter { it.isNotEmpty() }

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
private fun FolderPickerRow(label: String, custom: Boolean, onPick: () -> Unit, onReset: () -> Unit) {
    val colors = Envoix.colors
    Text(
        "SAVE RECEIVED FILES TO",
        color = colors.muted, fontSize = 11.sp, fontWeight = FontWeight.Bold, letterSpacing = 1.sp,
    )
    Spacer(Modifier.height(6.dp))
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
            .border(1.dp, colors.line, RoundedCornerShape(12.dp))
            .clickable(onClick = onPick).padding(horizontal = 14.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            label, color = colors.text, fontSize = 14.sp, fontFamily = FontFamily.Monospace,
            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false),
        )
        Spacer(Modifier.width(10.dp))
        Text("Change", color = colors.accent, fontSize = 13.sp, fontWeight = FontWeight.Bold)
    }
    if (custom) {
        Spacer(Modifier.height(6.dp))
        Text(
            "Reset to Downloads",
            color = colors.accent, fontSize = 12.sp,
            modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable(onClick = onReset)
                .padding(vertical = 3.dp, horizontal = 4.dp),
        )
    }
}

@Composable
private fun Field(label: String, value: String, onChange: (String) -> Unit) {
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
private fun MultilineField(label: String, value: String, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp),
        modifier = Modifier.fillMaxWidth().heightIn(min = 84.dp),
    )
}

@Composable
private fun LabeledControl(title: String, control: @Composable () -> Unit) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            title,
            color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold,
            modifier = Modifier.weight(1f).padding(end = 12.dp),
        )
        control()
    }
}

@Composable
private fun RoleToggle(role: String, onChange: (String) -> Unit) {
    val colors = Envoix.colors
    Row(
        Modifier.clip(RoundedCornerShape(10.dp)).background(colors.bg)
            .border(1.dp, colors.line, RoundedCornerShape(10.dp)).padding(3.dp),
    ) {
        RoleSeg("Send", role == "send") { onChange("send") }
        RoleSeg("Receive", role == "receive") { onChange("receive") }
    }
}

@Composable
private fun RoleSeg(text: String, selected: Boolean, onClick: () -> Unit) {
    val colors = Envoix.colors
    Box(
        Modifier.clip(RoundedCornerShape(8.dp))
            .background(if (selected) colors.accent else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 7.dp),
    ) {
        Text(
            text,
            color = if (selected) Color.White else colors.muted,
            fontSize = 13.sp, fontWeight = FontWeight.Bold,
        )
    }
}

@Composable
private fun AdvancedHeader(expanded: Boolean, onToggle: () -> Unit) {
    val colors = Envoix.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable(onClick = onToggle)
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Advanced", color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Icon(
            if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
            contentDescription = if (expanded) "Collapse" else "Expand",
            tint = colors.muted,
        )
    }
}

@Composable
private fun ToggleRow(title: String, subtitle: String, checked: Boolean, onChange: (Boolean) -> Unit) {
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
            colors = SwitchDefaults.colors(
                checkedThumbColor = Color.White,
                checkedTrackColor = colors.accent,
            ),
        )
    }
}
