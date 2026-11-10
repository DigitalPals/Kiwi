package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.install.InstallManager
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.MyAppsViewModel
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.components.ActionButton
import io.github.digitalpals.kiwi.ui.components.AppRow
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.components.SectionHeader
import io.github.digitalpals.kiwi.ui.foreignCaption
import io.github.digitalpals.kiwi.work.UpdateNotices

/** My Apps: updates, then the apps installed on this phone (`/api/updates`, `/api/library`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MyAppsScreen(container: AppContainer, model: MyAppsViewModel, onOpenApp: (AppSummary) -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    val context = LocalContext.current
    when (val load = state) {
        Load.Loading -> Loading()
        is Load.Failed -> LoadFailed(load.error, onRetry = model::reload)
        is Load.Ready -> PullToRefreshBox(isRefreshing = load.refreshing, onRefresh = model::reload) {
            val content = load.value
            if (content.updates.isEmpty() && content.installed.isEmpty() && content.selfUpdate == null) {
                MessagePane("No Apps Yet", "Apps you install from Kiwi show up here.")
                return@PullToRefreshBox
            }
            LazyColumn(Modifier.fillMaxSize()) {
                content.selfUpdate?.let { offer ->
                    item(key = "self") { SelfUpdateCard(container, offer, jobs[InstallManager.SELF]) }
                }
                if (content.updates.isNotEmpty()) {
                    item(key = "updates-header") {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) { SectionHeader("Updates") }
                            TextButton(onClick = { model.updateAll(content.updates) }, modifier = Modifier.padding(end = 8.dp)) {
                                Text("Update All")
                            }
                        }
                    }
                    items(content.updates, key = { "u:" + it.app.libraryKey }) { item ->
                        AppListRow(container, item, jobs[item.app.key], onOpenApp)
                    }
                }
                if (content.installed.isNotEmpty()) {
                    item(key = "installed-header") { SectionHeader("Installed") }
                    items(content.installed, key = { "i:" + it.app.libraryKey }) { item ->
                        val app = item.app
                        AppRow(
                            name = app.name,
                            subtitle = app.state.installed?.let { UpdateNotices.formatVersion(it.version, it.build, it.revision) },
                            iconUrl = app.iconUrl?.let(container::resolveUrl),
                            tint = app.tintColor,
                            view = appButton(app, jobs[app.key], item.local),
                            actions = SystemActions.buttonActions(context, container, app),
                            onClick = { onOpenApp(app) },
                            warning = if (app.state.foreignInstall) foreignCaption(app) else null,
                        )
                    }
                }
            }
        }
    }
}

/** "Kiwi 1.2 is available": the server offers a newer version of this app. */
@Composable
private fun SelfUpdateCard(container: AppContainer, offer: AndroidApp, job: InstallJob?) {
    val context = LocalContext.current
    Card(Modifier.fillMaxWidth().padding(16.dp)) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Kiwi ${offer.versionName} is available", style = MaterialTheme.typography.titleMedium)
                Text(
                    "This app is version ${BuildConfig.VERSION_NAME}.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            val view = when {
                job == null -> ButtonView.Install("Update")
                job.phase == InstallJob.Phase.Downloading -> ButtonView.Progress("Downloading", job.progress, cancellable = true)
                job.isActive -> ButtonView.Progress("Installing", null, cancellable = true)
                job.phase == InstallJob.Phase.NeedsPermission -> ButtonView.AllowInstalls("Allow Kiwi to install apps, then try again.")
                else -> ButtonView.Install("Retry", job.message, error = true)
            }
            ActionButton(
                view,
                ButtonActions(
                    install = { container.installs.startSelfUpdate(offer) },
                    open = {},
                    remove = {},
                    allowInstalls = {
                        container.installs.clear(InstallManager.SELF)
                        SystemActions.allowInstalls(context)
                    },
                    cancel = { container.installs.cancel(InstallManager.SELF) },
                ),
            )
        }
    }
}
