package dev.envoix.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.envoix.app.SettingsStore

@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val colors = Envoix.colors
    val settings by SettingsStore.settings.collectAsState()
    var broker by remember { mutableStateOf(settings.broker) }
    var relay by remember { mutableStateOf(settings.relay) }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.bg)
            .padding(horizontal = 20.dp),
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

        SectionLabel("RENDEZVOUS")
        Field("Broker", broker) { broker = it; SettingsStore.update { s -> s.copy(broker = it) } }
        Spacer(Modifier.height(12.dp))
        Field("Relay", relay) { relay = it; SettingsStore.update { s -> s.copy(relay = it) } }

        Spacer(Modifier.height(28.dp))
        SectionLabel("NETWORK")
        ToggleRow(
            title = "Exclude VPN / Tailscale",
            subtitle = "Never send over a VPN interface — keeps the direct/relay path clean.",
            checked = settings.blockVpn,
        ) { SettingsStore.update { s -> s.copy(blockVpn = it) } }
    }
}

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
private fun Field(label: String, value: String, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        singleLine = true,
        textStyle = androidx.compose.ui.text.TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp),
        modifier = Modifier.fillMaxWidth(),
    )
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
