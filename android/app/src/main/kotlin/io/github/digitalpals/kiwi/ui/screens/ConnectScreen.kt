package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.ui.ConnectViewModel

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
    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Connect to Kiwi", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(8.dp))
        Text(
            if (fromLink) {
                "A link asks to connect this app to the Kiwi below. Connect only if it is yours."
            } else {
                "Enter the address of your Kiwi. Tailscale must be on, signed in as Kiwi's owner."
            },
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = state.input,
            onValueChange = model::setInput,
            label = { Text("Kiwi address") },
            placeholder = { Text("https://mac.example.ts.net") },
            singleLine = true,
            enabled = !state.busy,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onGo = { model.connect(onConnected) }),
            modifier = Modifier.fillMaxWidth(),
        )
        state.problem?.let { problem ->
            Spacer(Modifier.height(16.dp))
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer)) {
                Column(Modifier.padding(16.dp)) {
                    Text(problem.title, style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.onErrorContainer)
                    Text(problem.message, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onErrorContainer)
                }
            }
        }
        Spacer(Modifier.height(24.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = { model.connect(onConnected) }, enabled = !state.busy) { Text("Connect") }
            if (state.busy) {
                Spacer(Modifier.padding(start = 16.dp))
                CircularProgressIndicator(modifier = Modifier.padding(start = 16.dp))
            }
            if (onCancel != null) {
                Spacer(Modifier.weight(1f))
                TextButton(onClick = onCancel, enabled = !state.busy) { Text("Cancel") }
            }
        }
    }
}
