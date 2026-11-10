package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppActions
import io.github.digitalpals.kiwi.ui.AppItem
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Format
import io.github.digitalpals.kiwi.ui.Fresh
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.MyAppsViewModel
import io.github.digitalpals.kiwi.ui.SELF_UPDATE_KEY
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.Tint
import io.github.digitalpals.kiwi.ui.VersionChange
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.selfUpdateButton
import io.github.digitalpals.kiwi.ui.components.ActionButton
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.AppListRow
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.components.KiwiAppIcon
import io.github.digitalpals.kiwi.ui.components.KiwiGreen
import io.github.digitalpals.kiwi.ui.components.KiwiTopBar
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.components.SectionTitle
import io.github.digitalpals.kiwi.ui.components.TitleSummary
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.WarningCard
import io.github.digitalpals.kiwi.ui.components.caption
import io.github.digitalpals.kiwi.ui.components.versionChangeText
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi
import kotlinx.coroutines.delay

/** My Apps: updates, then the apps installed on this phone (`/api/updates`, `/api/library`). */
@Composable
fun MyAppsScreen(container: AppContainer, model: MyAppsViewModel, actions: AppActions, onBrowse: () -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    val lastChecked by model.lastChecked.collectAsStateWithLifecycle(initialValue = null)
    val context = LocalContext.current
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(30_000)
            now = System.currentTimeMillis()
        }
    }
    MyAppsContent(
        load = state,
        jobs = jobs,
        actions = actions,
        checked = lastChecked?.let { Format.checkedAgo(it, now) },
        onRefresh = { model.reload() },
        onUpdateAll = model::updateAll,
        selfActions = { offer ->
            ButtonActions(
                install = { container.installs.startSelfUpdate(offer) },
                open = {},
                remove = {},
                allowInstalls = {
                    container.installs.clear(SELF_UPDATE_KEY)
                    SystemActions.allowInstalls(context)
                },
                cancel = { container.installs.cancel(SELF_UPDATE_KEY) },
            )
        },
        onBrowse = onBrowse,
    )
}

/** C4: updates washed in the apps' colours, blocked installs in amber, installed apps. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MyAppsContent(
    load: Load<MyAppsViewModel.Content>,
    jobs: Map<String, InstallJob>,
    actions: AppActions,
    checked: String?,
    onRefresh: () -> Unit,
    onUpdateAll: (MyAppsViewModel.Content) -> Unit,
    selfActions: (AndroidApp) -> ButtonActions,
    onBrowse: () -> Unit,
) {
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    Scaffold(
        topBar = {
            KiwiTopBar("My Apps", scroll) {
                IconButton(onClick = onRefresh) { Icon(KiwiIcons.Refresh, contentDescription = "Refresh") }
            }
        },
        contentWindowInsets = WindowInsets(0),
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        when (load) {
            Load.Loading -> Loading(Modifier.padding(padding))
            is Load.Failed -> LoadFailed(load.error, onRetry = onRefresh, Modifier.padding(padding))
            is Load.Ready -> PullToRefreshBox(
                isRefreshing = load.refreshing,
                onRefresh = onRefresh,
                modifier = Modifier.padding(padding),
            ) {
                val content = load.value
                if (content.isEmpty) {
                    MessagePane(
                        "No apps yet",
                        "Apps you install from Kiwi show up here, with their updates.",
                        icon = KiwiIcons.MyApps,
                        primary = "Browse apps" to onBrowse,
                    )
                    return@PullToRefreshBox
                }
                LazyColumn(
                    Modifier
                        .fillMaxSize()
                        .nestedScroll(scroll.nestedScrollConnection),
                    contentPadding = PaddingValues(bottom = 16.dp),
                ) {
                    if (checked != null) item(key = "checked") { TitleSummary(checked) }
                    val updateCount = content.updates.size + if (content.selfUpdate != null) 1 else 0
                    if (updateCount > 0) {
                        item(key = "updates-title") {
                            SectionTitle("Updates") {
                                if (updateCount > 1) TonalAction("Update all", { onUpdateAll(content) })
                            }
                        }
                        silentNote(content.updates)?.let { note -> item(key = "silent") { SilentNote(note) } }
                        items(content.updates, key = { "u:" + it.app.libraryKey }) { item ->
                            val app = item.app
                            UpdateCard(
                                icon = { AppIcon(app.name, app.iconUrl, app.tintColor) },
                                name = app.name,
                                change = Fresh.versionChange(app.state),
                                view = appButton(app, jobs[app.key], item.local),
                                actions = actions.buttons(app),
                                wash = Tint.vividTint(app.tintColor)?.let { Color(Tint.argb(it)) } ?: MaterialTheme.colorScheme.primary,
                                onClick = { actions.show(app) },
                            )
                        }
                        content.selfUpdate?.let { offer ->
                            item(key = "self") {
                                UpdateCard(
                                    icon = { KiwiAppIcon() },
                                    name = "Kiwi",
                                    change = VersionChange(BuildConfig.VERSION_NAME, offer.versionName),
                                    view = selfUpdateButton(jobs[SELF_UPDATE_KEY]),
                                    actions = selfActions(offer),
                                    wash = KiwiGreen,
                                    onClick = null,
                                )
                            }
                        }
                    }
                    items(content.foreign, key = { "f:" + it.app.key }) { item ->
                        val app = item.app
                        WarningCard(
                            "${app.name} can't be installed",
                            "Another ${app.name} is installed with a different signature. Remove it first, then install Kiwi's.",
                            Modifier.padding(top = 6.dp, bottom = 6.dp),
                            action = "Uninstall it",
                            onAction = { actions.remove(app, app.packageName) },
                        )
                    }
                    if (content.installed.isNotEmpty()) {
                        item(key = "installed-title") { SectionTitle("Installed", Modifier.padding(top = 6.dp)) }
                        items(content.installed, key = { "i:" + it.app.libraryKey }) { item ->
                            val app = item.app
                            AppListRow(
                                name = app.name,
                                iconUrl = app.iconUrl,
                                tint = app.tintColor,
                                subtitle = app.state.installed?.let { AnnotatedString(it.version) },
                                view = appButton(app, jobs[app.key], item.local),
                                actions = actions.buttons(app),
                                onClick = { actions.show(app) },
                            )
                        }
                    }
                }
            }
        }
    }
}

/** Whether the updates install without asking: all of them, some, or none (null). */
private fun silentNote(updates: List<AppItem>): String? {
    val silent = updates.count { it.local.silentUpdate }
    return when {
        updates.isEmpty() || silent == 0 -> null
        silent == updates.size -> "Installs silently: Kiwi installed these"
        else -> "Some install silently: Kiwi installed them"
    }
}

@Composable
private fun SilentNote(text: String) {
    Row(
        Modifier.padding(start = 20.dp, end = 20.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(KiwiIcons.Check, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(16.dp))
        Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** An update on a card washed with the app's colour. */
@Composable
private fun UpdateCard(
    icon: @Composable () -> Unit,
    name: String,
    change: VersionChange?,
    view: ButtonView,
    actions: ButtonActions,
    wash: Color,
    onClick: (() -> Unit)?,
) {
    val card = MaterialTheme.kiwi.card
    val tinted = lerp(card, wash, MaterialTheme.kiwi.washStrength)
    Row(
        Modifier
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .drawBehind {
                drawRect(
                    Brush.linearGradient(
                        0f to tinted,
                        0.62f to card,
                        start = Offset.Zero,
                        end = Offset(size.width, size.height * 0.6f),
                    ),
                )
            }
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        icon()
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            change?.let {
                Text(versionChangeText(it), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
            view.caption()?.let { (text, error) ->
                Text(
                    text,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                )
            }
        }
        ActionButton(view, actions, Modifier.height(36.dp))
    }
}

@Preview(name = "C4 My Apps")
@Composable
private fun MyAppsPreview() {
    KiwiTheme(darkTheme = false) {
        MyAppsContent(Load.Ready(PreviewData.myApps), emptyMap(), AppActions(), "Checked 4 minutes ago", {}, {}, { ButtonActions.None }, {})
    }
}

@Preview(name = "C4 My Apps, dark")
@Composable
private fun MyAppsDarkPreview() {
    KiwiTheme(darkTheme = true) {
        MyAppsContent(Load.Ready(PreviewData.myApps), PreviewData.downloading, AppActions(), "Checked just now", {}, {}, { ButtonActions.None }, {})
    }
}
