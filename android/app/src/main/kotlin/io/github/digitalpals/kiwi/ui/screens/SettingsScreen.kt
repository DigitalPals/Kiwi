package io.github.digitalpals.kiwi.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.ui.SessionViewModel
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.components.SectionHeader
import io.github.digitalpals.kiwi.work.UpdateWorker

/** Settings: the server, this device, updates and notifications, and about. */
@Composable
fun SettingsScreen(container: AppContainer, session: SessionViewModel, me: Me, serverUrl: String, onChangeServer: () -> Unit) {
    val context = LocalContext.current
    val autoUpdate by container.settings.autoUpdate.collectAsStateWithLifecycle(initialValue = true)
    val notifyUpdates by container.settings.notifyUpdates.collectAsStateWithLifecycle(initialValue = true)
    // Re-read the permissions whenever the screen comes back from Android's settings.
    var resumed by remember { mutableIntStateOf(0) }
    LifecycleResumeEffect(Unit) {
        resumed++
        onPauseOrDispose {}
    }
    val canInstall = remember(resumed) { container.sessionInstaller.canInstall() }
    var notificationsGranted by remember(resumed) { mutableStateOf(notificationsGranted(context)) }
    val requestNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        notificationsGranted = it
    }
    var confirmDisconnect by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        SectionHeader("Server")
        Info("Address", serverUrl.removeSuffix("/"))
        Info("Kiwi version", me.server.version)
        Row(Modifier.padding(horizontal = 8.dp)) {
            TextButton(onClick = onChangeServer) { Text("Change Server") }
            TextButton(onClick = { confirmDisconnect = true }) { Text("Disconnect") }
        }

        SectionHeader("This Device")
        Info("Name", me.device.name)
        Info("Platform", if (me.device.platform == "android") "Android" else "iPhone or iPad")
        Info("Android", me.device.osVersion ?: Build.VERSION.RELEASE)
        me.device.model?.let { Info("Model", it) }

        SectionHeader("Updates")
        Toggle(
            "Install updates automatically",
            "Kiwi checks about every two hours and installs updates of the apps it installed, where Android allows it without asking.",
            autoUpdate,
        ) { session.setAutoUpdate(it) }
        Toggle("Notify about updates", null, notifyUpdates) { enabled ->
            session.setNotifyUpdates(enabled)
            if (enabled && !notificationsGranted && Build.VERSION.SDK_INT >= 33) {
                requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
        if (notifyUpdates && !notificationsGranted) {
            Hint("Notifications are off for Kiwi.", "Allow") {
                if (Build.VERSION.SDK_INT >= 33) requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
        if (!canInstall) {
            Hint("Kiwi isn't allowed to install apps yet.", "Allow") { SystemActions.allowInstalls(context) }
        }
        TextButton(onClick = { UpdateWorker.checkNow(context) }, modifier = Modifier.padding(horizontal = 8.dp)) {
            Text("Check for Updates Now")
        }

        SectionHeader("About")
        Info("Kiwi for Android", "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
        Text(
            container.userAgent,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
    }

    if (confirmDisconnect) {
        AlertDialog(
            onDismissRequest = { confirmDisconnect = false },
            title = { Text("Disconnect from Kiwi?") },
            text = { Text("The apps stay installed. Connect again to get updates.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDisconnect = false
                    session.disconnect()
                }) { Text("Disconnect") }
            },
            dismissButton = { TextButton(onClick = { confirmDisconnect = false }) { Text("Cancel") } },
        )
    }
}

private fun notificationsGranted(context: android.content.Context): Boolean =
    Build.VERSION.SDK_INT < 33 ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

@Composable
private fun Info(label: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(label, Modifier.weight(1f), color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value)
    }
}

@Composable
private fun Toggle(title: String, detail: String?, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 16.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (detail != null) {
                Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun Hint(text: String, action: String, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(text, Modifier.weight(1f), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
        TextButton(onClick = onClick) { Text(action) }
    }
}
