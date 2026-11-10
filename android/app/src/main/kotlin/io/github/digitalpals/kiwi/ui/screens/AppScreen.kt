package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.api.AppDetail
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.ui.AppViewModel
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.LocalPackage
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.components.ActionButton
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.SectionHeader
import io.github.digitalpals.kiwi.ui.foreignCaption
import io.github.digitalpals.kiwi.work.UpdateNotices

/** An app's page: its state and button, versions and description. */
@Composable
fun AppScreen(container: AppContainer, model: AppViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    when (val load = state) {
        Load.Loading -> Loading()
        is Load.Failed -> LoadFailed(load.error, onRetry = model::reload)
        is Load.Ready -> AppPage(container, load.value.first, load.value.second)
    }
}

@Composable
private fun AppPage(container: AppContainer, app: AppDetail, local: LocalPackage) {
    val context = LocalContext.current
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    val summary = app.summary
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(bottom = 24.dp),
    ) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            AppIcon(app.name, app.iconUrl?.let(container::resolveUrl), app.tintColor, size = 72.dp)
            Column(
                Modifier
                    .weight(1f)
                    .padding(horizontal = 16.dp),
            ) {
                Text(app.name, style = MaterialTheme.typography.headlineSmall)
                Text(
                    app.developer ?: app.sourceName,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.End) {
            ActionButton(
                appButton(summary, jobs[summary.key], local),
                SystemActions.buttonActions(context, container, summary),
            )
        }
        if (app.state.foreignInstall) {
            Text(
                "${foreignCaption(summary)}: it is installed with another signature, so Android won't update it with Kiwi's.",
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(16.dp),
            )
        }
        app.state.replaces?.let {
            Text(
                "Replaces ${it.name} (${it.sourceName})",
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            )
        }
        SectionHeader("Information")
        InfoRow("Source", app.sourceName)
        InfoRow("Package", app.bundleId)
        app.state.installed?.let { InfoRow("Installed", UpdateNotices.formatVersion(it.version, it.build, it.revision)) }
        app.state.target?.let { InfoRow("Latest", UpdateNotices.formatVersion(it)) }
        app.state.target?.size?.let { InfoRow("Size", formatBytes(it)) }
        if (!app.description.isNullOrBlank()) {
            SectionHeader("About")
            Text(app.description, modifier = Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodyMedium)
        }
        if (app.releases.isNotEmpty()) {
            SectionHeader("Versions")
            app.releases.take(MAX_RELEASES).forEach { ReleaseRow(it) }
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)) {
        Text(label, Modifier.weight(1f), color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value)
    }
}

@Composable
private fun ReleaseRow(release: Release) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        Row {
            Text(UpdateNotices.formatVersion(release), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            release.date?.let { Text(it.take(10), color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        release.notes?.takeIf { it.isNotBlank() }?.let {
            Spacer(Modifier.height(4.dp))
            Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 6)
        }
        if (!release.compatible && release.incompatibility != null) {
            Text(release.incompatibility, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
    }
    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
}

private const val MAX_RELEASES = 10

/** "48.2 MB", as the web app shows sizes. */
fun formatBytes(bytes: Long): String = when {
    bytes < 1_000 -> "$bytes bytes"
    bytes < 1_000_000 -> "${Math.round(bytes / 1e3)} KB"
    bytes < 1_000_000_000 -> String.format(java.util.Locale.US, "%.1f MB", bytes / 1e6)
    else -> String.format(java.util.Locale.US, "%.2f GB", bytes / 1e9)
}
