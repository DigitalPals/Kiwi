package io.github.digitalpals.kiwi.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.ui.ConnectViewModel
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.components.IconCircle
import io.github.digitalpals.kiwi.ui.components.KiwiAppIcon
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi

/**
 * Connects the app to a Kiwi: the user types its address, or a `kiwi://connect` link fills it
 * in. Nothing is saved until Kiwi answered `/api/health` and `/api/me` for this phone.
 */
@Composable
fun ConnectScreen(
    container: AppContainer,
    initialUrl: String,
    fromLink: Boolean,
    onConnected: (String) -> Unit,
    onCancel: (() -> Unit)?,
) {
    val model: ConnectViewModel = viewModel(key = "connect:$initialUrl") { ConnectViewModel(container, initialUrl) }
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    BackHandler(enabled = onCancel != null) { onCancel?.invoke() }
    ConnectContent(
        state = state,
        fromLink = fromLink,
        onInput = model::setInput,
        onConnect = { model.connect(onConnected) },
        onCancel = onCancel,
        onOpenTailscale = { SystemActions.openTailscale(context) },
    )
}

/** C1: the Kiwi address, a reminder that Tailscale must be on, and Connect. */
@Composable
fun ConnectContent(
    state: ConnectViewModel.State,
    fromLink: Boolean,
    onInput: (String) -> Unit,
    onConnect: () -> Unit,
    onCancel: (() -> Unit)?,
    onOpenTailscale: () -> Unit,
) {
    Surface(color = MaterialTheme.colorScheme.background, modifier = Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .systemBarsPadding()
                .imePadding(),
        ) {
            Box(Modifier.height(56.dp).padding(start = 4.dp), contentAlignment = Alignment.CenterStart) {
                if (onCancel != null) {
                    IconButton(onClick = onCancel, enabled = !state.busy) { Icon(KiwiIcons.Back, contentDescription = "Cancel") }
                }
            }
            Column(
                Modifier
                    .weight(1f)
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp),
            ) {
                KiwiAppIcon(size = 68.dp)
                Text(
                    "Connect to your Kiwi",
                    style = MaterialTheme.typography.headlineLarge,
                    modifier = Modifier.padding(top = 28.dp, bottom = 10.dp),
                )
                Text(
                    if (fromLink) {
                        "A link asks to connect this app to the Kiwi below. Connect only if it is yours."
                    } else {
                        "Enter the address of your Kiwi server. Its Settings page shows it on any device that already uses Kiwi."
                    },
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                OutlinedTextField(
                    value = state.input,
                    onValueChange = onInput,
                    label = { Text("Kiwi address") },
                    placeholder = { Text("https://kiwi.example.ts.net") },
                    supportingText = { Text("Starts with https://") },
                    trailingIcon = if (state.input.isNotEmpty() && !state.busy) {
                        { IconButton(onClick = { onInput("") }) { Icon(KiwiIcons.Close, contentDescription = "Clear") } }
                    } else {
                        null
                    },
                    isError = state.problem != null && !state.problem.network,
                    singleLine = true,
                    enabled = !state.busy,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
                    keyboardActions = KeyboardActions(onGo = { onConnect() }),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 28.dp, bottom = 16.dp),
                )
                state.problem?.let { problem ->
                    InfoCard(
                        icon = KiwiIcons.Warning,
                        iconColor = MaterialTheme.colorScheme.error,
                        iconTint = MaterialTheme.colorScheme.onError,
                        title = problem.title,
                        body = problem.message,
                        container = MaterialTheme.colorScheme.errorContainer,
                        content = MaterialTheme.colorScheme.onErrorContainer,
                    )
                    Spacer(Modifier.height(12.dp))
                }
                InfoCard(
                    icon = KiwiIcons.Network,
                    iconColor = Color(0xFF30485C),
                    title = "Tailscale must be on",
                    body = "Kiwi is only reachable on your tailnet. Turn on Tailscale on this phone, then connect.",
                    action = "Open Tailscale" to onOpenTailscale,
                )
                Spacer(Modifier.height(24.dp))
            }
            Button(
                onClick = onConnect,
                enabled = !state.busy,
                shape = RoundedCornerShape(28.dp),
                modifier = Modifier
                    .padding(start = 24.dp, end = 24.dp, bottom = 24.dp)
                    .fillMaxWidth()
                    .height(56.dp),
            ) {
                if (state.busy) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(20.dp),
                        strokeWidth = 2.5.dp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.width(12.dp))
                    Text("Connecting…", style = MaterialTheme.typography.titleMedium)
                } else {
                    Text("Connect", style = MaterialTheme.typography.titleMedium)
                }
            }
        }
    }
}

/** A card with an icon on a circle, a title, a text and an optional text button. */
@Composable
private fun InfoCard(
    icon: ImageVector,
    iconColor: Color,
    title: String,
    iconTint: Color = Color.White,
    body: String,
    container: Color = MaterialTheme.kiwi.card,
    content: Color = MaterialTheme.colorScheme.onSurface,
    action: Pair<String, () -> Unit>? = null,
) {
    Surface(shape = RoundedCornerShape(20.dp), color = container, contentColor = content, modifier = Modifier.fillMaxWidth()) {
        Row(
            Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = if (action != null) 6.dp else 16.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            IconCircle(icon, iconColor, size = 36.dp, iconTint = iconTint)
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium)
                Text(
                    body,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (container == MaterialTheme.kiwi.card) MaterialTheme.colorScheme.onSurfaceVariant else content,
                    modifier = Modifier.padding(top = 2.dp),
                )
                if (action != null) {
                    TextButton(onClick = action.second, modifier = Modifier.offset(x = (-12).dp)) { Text(action.first) }
                }
            }
        }
    }
}

@Preview(name = "C1 Connect")
@Composable
private fun ConnectPreview() {
    KiwiTheme(darkTheme = false) {
        ConnectContent(ConnectViewModel.State("https://kiwi.example.ts.net"), false, {}, {}, null, {})
    }
}

@Preview(name = "C1 Connect, dark")
@Composable
private fun ConnectDarkPreview() {
    KiwiTheme(darkTheme = true) {
        ConnectContent(
            ConnectViewModel.State(
                "https://kiwi.example.ts.net",
                problem = ConnectViewModel.Problem("Can't Reach Kiwi", "Can't reach Kiwi at kiwi.example.ts.net. Is Tailscale on?", network = true),
            ),
            false, {}, {}, {}, {},
        )
    }
}
