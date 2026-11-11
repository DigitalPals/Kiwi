package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppActions
import io.github.digitalpals.kiwi.ui.BuildRowAction
import io.github.digitalpals.kiwi.ui.BuildRowView
import io.github.digitalpals.kiwi.ui.BuildsViewModel
import io.github.digitalpals.kiwi.ui.Community
import io.github.digitalpals.kiwi.ui.Format
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.components.ActionButton
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.components.FilledAction
import io.github.digitalpals.kiwi.ui.components.GroupRow
import io.github.digitalpals.kiwi.ui.components.IconCircle
import io.github.digitalpals.kiwi.ui.components.KiwiSwitch
import io.github.digitalpals.kiwi.ui.components.KiwiTopBar
import io.github.digitalpals.kiwi.ui.components.ListGroup
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.LocalSnackbars
import io.github.digitalpals.kiwi.ui.components.SectionLabel
import io.github.digitalpals.kiwi.ui.components.TitleSummary
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.WarningCard
import io.github.digitalpals.kiwi.ui.components.caption
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi
import io.github.digitalpals.kiwi.work.Notifications
import kotlinx.coroutines.delay
import java.time.Instant

/** What the Builds screen can do; previews use the defaults (nothing). */
class BuildsActions(
    val refresh: () -> Unit = {},
    /** "Install" on an app not built yet: the sheet about its first build, or turning Kiwi Builds on first. */
    val install: (AutomationState) -> Unit = {},
    /** "Retry" on a failed first build: builds it again and installs it when ready. */
    val retry: (AutomationState) -> Unit = {},
    /** The buttons of a published build (install, update, open; SystemActions.appActions). */
    val buttons: (AppSummary) -> ButtonActions = { ButtonActions.None },
    val edit: (AutomationState) -> Unit = {},
    val check: (AutomationState, Boolean) -> Unit = { _, _ -> },
    val cancel: (AutomationState) -> Unit = {},
    val remove: (AutomationState) -> Unit = {},
    val checkAll: () -> Unit = {},
    val notifyBuilds: (Boolean) -> Unit = {},
    val turnOn: () -> Unit = {},
    /** "Install" on a community app not added yet: its sheet. */
    val installCommunity: (CommunityApp) -> Unit = {},
    /** "Choose other patches" of a community app. */
    val choosePatches: (AutomationState) -> Unit = {},
    /** Opens the community search. */
    val search: () -> Unit = {},
)

/**
 * Builds: every Android app Kiwi Builds can build (`/api/automation`), with one-tap Install.
 * Polls every 3 seconds while a job runs and every 30 seconds otherwise, while the screen is
 * visible. [editRequest] opens an app's patches and schedule (from its app page).
 */
@Composable
fun BuildsScreen(model: BuildsViewModel, appActions: AppActions, editRequest: String?, onEditHandled: () -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val busy by model.busy.collectAsStateWithLifecycle()
    val notifyBuilds by model.notifyBuilds.collectAsStateWithLifecycle(initialValue = true)
    val pending by model.pending.collectAsStateWithLifecycle(initialValue = emptySet())
    val installs by model.installs.collectAsStateWithLifecycle()
    val snackbars = LocalSnackbars.current
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val content = (state as? Load.Ready)?.value
    val active = content?.active == true
    var installing by rememberSaveable { mutableStateOf<String?>(null) }
    var editing by rememberSaveable { mutableStateOf<String?>(null) }
    var removing by rememberSaveable { mutableStateOf<String?>(null) }
    var searching by rememberSaveable { mutableStateOf(false) }

    /** The community app whose "Choose other patches" sheet is open. */
    var choosing by rememberSaveable { mutableStateOf<String?>(null) }
    val search by model.search.state.collectAsStateWithLifecycle()
    val candidates by model.candidates.collectAsStateWithLifecycle()

    /** Turning Kiwi Builds on: the app whose Install asked for it, or [BuildsViewModel.ALL]. */
    var turningOn by rememberSaveable { mutableStateOf<String?>(null) }

    LaunchedEffect(active, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                delay(if (active) 3_000 else 30_000)
                model.reload(silent = true)
            }
        }
    }
    LaunchedEffect(model, snackbars) {
        model.messages.collect { snackbars?.showSnackbar(it) }
    }
    LaunchedEffect(editRequest) {
        if (editRequest != null) {
            editing = editRequest
            onEditHandled()
        }
    }

    BuildsContent(
        load = state,
        busy = busy,
        notifyBuilds = notifyBuilds,
        now = Instant.now(),
        pending = pending,
        installs = installs,
        actions = BuildsActions(
            refresh = { model.reload() },
            install = { app -> if (content?.off == true) turningOn = app.id else installing = app.id },
            retry = { app -> if (Community.isId(app.id)) model.retryCommunity(app) else model.retry(app) },
            buttons = { appActions.buttons(it) },
            edit = { editing = it.id },
            check = { app, force -> model.check(app, force) },
            cancel = model::cancel,
            remove = { removing = it.id },
            checkAll = { content?.let { model.checkAll(it.added) } },
            notifyBuilds = model::setNotifyBuilds,
            turnOn = { turningOn = BuildsViewModel.ALL },
            installCommunity = { installing = it.id },
            choosePatches = { choosing = it.id },
            search = { searching = true },
        ),
    )
    LaunchedEffect(choosing) {
        val id = choosing
        if (id != null) model.loadCandidates(id) else model.closeCandidates()
    }

    // The sheet waits for Kiwi's own state of the app (after turning Kiwi Builds on).
    val toInstall = content?.takeIf { !it.off }?.apps?.firstOrNull { it.id == installing && !Community.isId(it.id) }
    if (toInstall != null) {
        BuildSheet(onDismiss = { installing = null }) {
            InstallBuildSheetContent(
                app = toInstall,
                tint = content.tints[toInstall.bundleId],
                blocker = content.blockers[toInstall.id],
                notificationsAllowed = remember(toInstall.id) { Notifications.allowed(context) },
                busy = toInstall.id in busy,
                onBuild = { preferences -> model.buildAndInstall(toInstall, preferences) { installing = null } },
                onUninstall = { SystemActions.uninstall(context, toInstall.bundleId) },
                onNotificationSettings = { SystemActions.notificationSettings(context) },
            )
        }
    }
    val community = content?.community?.takeIf { !content.off }
    val communityTarget = installing?.takeIf(Community::isId)?.let { id ->
        content?.communityApps?.firstOrNull { it.id == id }?.let(Community::target)
            ?: (community?.popular.orEmpty() + search.results).firstOrNull { it.id == id }?.let(Community::target)
    }
    if (content != null && community != null && communityTarget != null) {
        val added = content.communityApps.firstOrNull { it.id == communityTarget.id }
        val lookedUp by produceState<String?>(null, communityTarget.id) {
            if (added == null) value = model.blockerFor(communityTarget.name, communityTarget.packageName)
        }
        val blocker = if (added != null) content.blockers[added.id] else lookedUp
        BuildSheet(onDismiss = { installing = null }) {
            CommunityInstallSheetContent(
                target = communityTarget,
                tint = content.tints[communityTarget.packageName],
                limitations = added?.limitations.orEmpty(),
                blocker = blocker,
                available = community.available,
                reason = community.reason,
                notificationsAllowed = remember(communityTarget.id) { Notifications.allowed(context) },
                busy = communityTarget.id in busy,
                onBuild = {
                    model.installCommunity(communityTarget.id, communityTarget.name, added?.let(Community::retryRepo)) {
                        installing = null
                        searching = false
                    }
                },
                onUninstall = { SystemActions.uninstall(context, communityTarget.packageName) },
                onNotificationSettings = { SystemActions.notificationSettings(context) },
            )
        }
    }
    val managed = editing?.takeIf(Community::isId)?.let { id -> content?.communityApps?.firstOrNull { it.id == id } }
    if (content != null && managed != null && community != null) {
        BuildSheet(onDismiss = { editing = null }) {
            CommunityAppSheetContent(
                app = managed,
                tint = content.tints[managed.bundleId],
                available = community.available,
                busy = managed.id in busy,
                onRetry = { model.retryCommunity(managed) },
                onChoose = { choosing = managed.id },
                onUpdate = { model.updateCommunity(managed) },
                onCancel = { model.cancel(managed) },
                onRemove = { removing = managed.id },
            )
        }
    }
    val choosingApp = choosing?.let { id -> content?.communityApps?.firstOrNull { it.id == id } }
    if (content != null && choosingApp != null && community != null) {
        BuildSheet(onDismiss = { choosing = null }) {
            CommunityCandidatesSheetContent(
                app = choosingApp,
                tint = content.tints[choosingApp.bundleId],
                load = candidates?.takeIf { it.id == choosingApp.id }?.load ?: Load.Loading,
                now = Instant.now(),
                available = community.available,
                busy = choosingApp.id in busy,
                onRetry = { model.loadCandidates(choosingApp.id) },
                onPick = { repo ->
                    model.installCommunity(choosingApp.id, choosingApp.name, repo) {
                        choosing = null
                        editing = null
                    }
                },
            )
        }
    }
    if (content != null && searching && community != null) {
        LaunchedEffect(Unit) { model.search.open() }
        SearchSheet(onDismiss = { searching = false }) {
            CommunitySearchContent(
                state = search,
                available = community.available,
                added = content.communityApps.map { it.id }.toSet(),
                busy = busy,
                onQuery = model.search::setQuery,
                onLoadMore = model.search::loadMore,
                onRetry = model.search::retry,
                onInstall = { installing = it.id },
                onOpen = { id ->
                    searching = false
                    editing = id
                },
            )
        }
    }
    val edited = content?.apps?.firstOrNull { it.id == editing && it.added }
    if (edited != null) {
        BuildSheet(onDismiss = { editing = null }) {
            EditBuildSheetContent(
                app = edited,
                tint = content.tints[edited.bundleId],
                busy = edited.id in busy,
                onSave = { model.save(edited, it) },
                onCheck = { force -> model.check(edited, force) },
                onCancel = { model.cancel(edited) },
                onRemove = { removing = edited.id },
            )
        }
    }
    content?.app(removing)?.let { app ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text(if (Community.isId(app.id)) "Remove ${app.name}?" else "Remove ${app.name} from Kiwi Builds?") },
            text = { Text("Installed copies keep working, but won't get updates. You can install it again later.") },
            confirmButton = {
                TextButton(onClick = {
                    removing = null
                    editing = null
                    model.remove(app)
                }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
    turningOn?.let { asked ->
        AlertDialog(
            onDismissRequest = { turningOn = null },
            title = { Text("Turn on Kiwi Builds?") },
            text = { Text(KiwiBuilds.TURN_ON_NOTICE) },
            confirmButton = {
                TextButton(onClick = {
                    turningOn = null
                    // Install continues with the app's sheet once Kiwi lists it.
                    model.turnOn { if (asked != BuildsViewModel.ALL) installing = asked }
                }) { Text("Turn on") }
            },
            dismissButton = { TextButton(onClick = { turningOn = null }) { Text("Cancel") } },
        )
    }
}

/**
 * C6: the Kiwi Builds section (every app Kiwi can build, each with its button), then the
 * schedule of added apps. Sections are separate list items, so more can follow below.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BuildsContent(
    load: Load<BuildsViewModel.Content>,
    busy: Set<String>,
    notifyBuilds: Boolean,
    now: Instant,
    actions: BuildsActions,
    pending: Set<String> = emptySet(),
    installs: Map<String, InstallJob> = emptyMap(),
) {
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    Scaffold(
        topBar = { KiwiTopBar("Builds", scroll) },
        contentWindowInsets = WindowInsets(0),
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        when (load) {
            Load.Loading -> Loading(Modifier.padding(padding))
            is Load.Failed -> LoadFailed(load.error, onRetry = actions.refresh, Modifier.padding(padding))
            is Load.Ready -> PullToRefreshBox(
                isRefreshing = load.refreshing,
                onRefresh = actions.refresh,
                modifier = Modifier.padding(padding),
            ) {
                val value = load.value
                LazyColumn(
                    Modifier
                        .fillMaxSize()
                        .nestedScroll(scroll.nestedScrollConnection),
                    contentPadding = PaddingValues(bottom = 16.dp),
                ) {
                    item(key = "summary") { TitleSummary("Patched and signed by your Kiwi") }
                    if (value.off) {
                        item(key = "off") {
                            ListGroup(Modifier.padding(top = 8.dp)) {
                                row {
                                    GroupRow(
                                        "Kiwi Builds is off on your Kiwi",
                                        subtitle = AnnotatedString("Turn it on to build these apps. Install asks first."),
                                        leading = { IconCircle(KiwiIcons.Builds, MaterialTheme.colorScheme.primary) },
                                        trailing = { TonalAction("Turn on", actions.turnOn, enabled = BuildsViewModel.ALL !in busy) },
                                    )
                                }
                            }
                        }
                    }
                    kiwiBuildsSection(value, busy, pending, installs, actions)
                    communitySection(value, busy, pending, installs, actions)
                    val added = if (value.off) emptyList() else value.added
                    if (added.isNotEmpty()) {
                        item(key = "schedule-label") { SectionLabel("Schedule") }
                        item(key = "schedule") { ScheduleGroup(added, busy, notifyBuilds, now, actions) }
                    }
                    item(key = "footer") {
                        Text(
                            "First builds take ${KiwiBuilds.BUILD_TIME}, one at a time; apps you install go before " +
                                "scheduled checks. Kiwi keeps building while this app is closed; if a build fails, the " +
                                "previous version stays available.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 28.dp, vertical = 12.dp),
                        )
                    }
                }
            }
        }
    }
}

/** The "Kiwi Builds" section: every app Kiwi can build, added or not. */
private fun LazyListScope.kiwiBuildsSection(
    value: BuildsViewModel.Content,
    busy: Set<String>,
    pending: Set<String>,
    installs: Map<String, InstallJob>,
    actions: BuildsActions,
) {
    item(key = "kiwi-builds-label") { SectionLabel("Kiwi Builds") }
    item(key = "kiwi-builds") {
        ListGroup {
            value.apps.forEach { app ->
                val summary = value.published[app.bundleId]
                val view = KiwiBuilds.row(
                    app = app,
                    summary = summary,
                    local = value.local[app.bundleId],
                    job = summary?.let { installs[it.key] },
                    pending = app.id in pending,
                    off = value.off,
                )
                val menu = app.added && !value.off
                row {
                    BuildRow(app, view, value.tints[app.bundleId], app.id in busy, menu, actions) {
                        if (menu) BuildMenu(app, app.id in busy, actions)
                    }
                }
            }
        }
    }
}

@Composable
private fun ScheduleGroup(added: List<AutomationState>, busy: Set<String>, notifyBuilds: Boolean, now: Instant, actions: BuildsActions) {
    val idle = added.any { KiwiBuilds.activeJob(it) == null }
    val next = KiwiBuilds.nextCheck(added)
    ListGroup {
        row {
            GroupRow(
                "Check for new versions",
                subtitle = AnnotatedString(
                    when {
                        !idle -> "Building now"
                        next != null -> Format.nextCheck(next, now)
                        else -> "Automatic checks are off"
                    },
                ),
                leading = { IconCircle(KiwiIcons.Clock, Color(0xFF1E8E9E)) },
                trailing = {
                    TonalAction("Build now", actions.checkAll, enabled = idle && BuildsViewModel.ALL !in busy)
                },
            )
        }
        row {
            GroupRow(
                "Notify when a build finishes",
                leading = { IconCircle(KiwiIcons.Bell, Color(0xFFE5483C)) },
                trailing = { KiwiSwitch(notifyBuilds, actions.notifyBuilds) },
                onClick = { actions.notifyBuilds(!notifyBuilds) },
            )
        }
    }
}

/**
 * An app of Kiwi Builds: what it is or how its build is going, and one button (Install, Retry,
 * or the published build's Update or Open); ⋮ for more once it is added.
 */
@Composable
internal fun BuildRow(
    app: AutomationState,
    view: BuildRowView,
    tint: String?,
    busy: Boolean,
    menu: Boolean,
    actions: BuildsActions,
    menuContent: @Composable () -> Unit = {},
) {
    val action = view.action
    val caption = (action as? BuildRowAction.App)?.view?.caption()
    GroupRow(
        app.name,
        subtitle = AnnotatedString(view.subtitle),
        subtitleColor = if (view.error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        leading = { AppIcon(app.name, app.iconUrl, tint, size = 44.dp) },
        trailing = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                when (action) {
                    BuildRowAction.Install -> FilledAction(if (busy) "Starting…" else "Install", { actions.install(app) }, enabled = !busy)
                    BuildRowAction.Retry -> FilledAction(if (busy) "Starting…" else "Retry", { actions.retry(app) }, enabled = !busy)
                    // Below the text: its label is too long for the row's end.
                    BuildRowAction.ChoosePatches -> {}
                    BuildRowAction.Remove -> TonalAction("Remove", { actions.remove(app) }, enabled = !busy)
                    is BuildRowAction.App -> ActionButton(action.view, actions.buttons(action.summary))
                    BuildRowAction.None -> {}
                }
                menuContent()
            }
        },
        onClick = { if (menu) actions.edit(app) else if (action == BuildRowAction.Install) actions.install(app) },
        below = {
            if (view.building) {
                LinearProgressIndicator(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 8.dp, bottom = 2.dp),
                )
            }
            val detail = caption?.first ?: view.detail
            if (detail != null) {
                Text(
                    detail,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (caption?.second == true) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 3,
                    modifier = Modifier.padding(top = 2.dp),
                )
            }
            if (action == BuildRowAction.ChoosePatches) {
                TonalAction("Choose other patches", { actions.choosePatches(app) }, Modifier.padding(top = 8.dp), enabled = !busy)
            }
        },
    )
}

/** ⋮ of an added app: build or cancel, rebuild, patches and schedule, remove. */
@Composable
private fun BuildMenu(app: AutomationState, busy: Boolean, actions: BuildsActions) {
    val job = KiwiBuilds.activeJob(app)
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, enabled = !busy) {
            Icon(KiwiIcons.More, contentDescription = "More for ${app.name}", tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            fun pick(action: () -> Unit) {
                open = false
                action()
            }
            if (job != null) {
                DropdownMenuItem(text = { Text("Cancel build") }, onClick = { pick { actions.cancel(app) } })
            } else {
                DropdownMenuItem(text = { Text(KiwiBuilds.buildLabel(app)) }, onClick = { pick { actions.check(app, false) } })
                if (app.published != null) {
                    DropdownMenuItem(text = { Text("Rebuild") }, onClick = { pick { actions.check(app, true) } })
                }
            }
            DropdownMenuItem(text = { Text("Patches and schedule") }, onClick = { pick { actions.edit(app) } })
            DropdownMenuItem(
                text = { Text("Remove") },
                enabled = job == null,
                onClick = { pick { actions.remove(app) } },
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun BuildSheet(onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Column(
            Modifier
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(bottom = 16.dp),
            content = content,
        )
    }
}

/**
 * Before the first build of an app: what Kiwi does and how long it takes, what to know, and
 * what is in the way on this phone ([blocker]), then "Build and install". "Customise patches"
 * shows the patches and schedule to choose before building.
 */
@Composable
fun InstallBuildSheetContent(
    app: AutomationState,
    tint: String?,
    blocker: String?,
    notificationsAllowed: Boolean,
    busy: Boolean,
    onBuild: (AutomationPreferences) -> Unit,
    onUninstall: () -> Unit,
    onNotificationSettings: () -> Unit,
) {
    var preferences by remember(app.id) { mutableStateOf(app.preferences) }
    var customising by rememberSaveable(app.id) { mutableStateOf(false) }
    SheetHeader(app, tint, app.summary.ifBlank { app.tweak })
    ListGroup(Modifier.padding(top = 8.dp)) {
        row {
            GroupRow(
                "First build: ${KiwiBuilds.BUILD_TIME}",
                subtitle = AnnotatedString(KiwiBuilds.firstBuildNotice(app.name)),
                leading = { IconCircle(KiwiIcons.Clock, Color(0xFF1E8E9E)) },
            )
        }
        if (!notificationsAllowed) {
            row {
                GroupRow(
                    "Notifications are off",
                    subtitle = AnnotatedString("Kiwi can't tell you when ${app.name} is ready. Open Kiwi again in a few minutes to install it."),
                    leading = { IconCircle(KiwiIcons.Bell, Color(0xFFE5483C)) },
                    trailing = { TonalAction("Turn on", onNotificationSettings) },
                )
            }
        }
    }
    if (blocker != null) {
        WarningCard("Uninstall the other ${app.name} first", blocker, Modifier.padding(top = 12.dp), action = "Uninstall", onAction = onUninstall)
    }
    app.warning?.let { WarningCard("Before you install ${app.name}", it, Modifier.padding(top = 12.dp)) }
    if (app.limitations.isNotEmpty()) {
        SectionLabel("What to know")
        ListGroup {
            app.limitations.forEach { limitation ->
                row { GroupRow(limitation.title, subtitle = AnnotatedString(limitation.detail)) }
            }
        }
    }
    if (customising) BuildOptions(app, preferences, enabled = !busy) { preferences = it }
    Column(Modifier.padding(horizontal = 24.dp, vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        FilledAction(
            if (busy) "Starting…" else "Build and install",
            { onBuild(preferences) },
            Modifier.fillMaxWidth(),
            small = false,
            enabled = !busy,
        )
        if (!customising && (app.availableTweaks.isNotEmpty() || app.names.isNotEmpty())) {
            TextButton(onClick = { customising = true }, enabled = !busy) { Text("Customise patches") }
        }
        Text(
            if (preferences.enabled) {
                "Kiwi then checks for new versions ${KiwiBuilds.intervalLabel(preferences.intervalMinutes, lower = true)} " +
                    "and builds them for you. You can change this later."
            } else {
                "Kiwi won't build new versions on its own. You can change this later."
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 6.dp),
        )
    }
}

/** An added app's sheet: its build, then schedule and patches to change, and Remove. */
@Composable
fun EditBuildSheetContent(
    app: AutomationState,
    tint: String?,
    busy: Boolean,
    onSave: (AutomationPreferences) -> Unit,
    onCheck: (Boolean) -> Unit,
    onCancel: () -> Unit,
    onRemove: () -> Unit,
) {
    var draft by remember(app.id, app.preferences) { mutableStateOf(app.preferences) }
    val dirty = !KiwiBuilds.samePreferences(draft, app.preferences)
    val job = KiwiBuilds.activeJob(app)
    SheetHeader(app, tint, KiwiBuilds.statusLine(app))
    if (job?.status == "running") {
        LinearProgressIndicator(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 4.dp),
        )
    }
    KiwiBuilds.attentionDetail(app)?.let {
        Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 24.dp))
    }
    Row(
        Modifier.padding(horizontal = 24.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (job != null) {
            TonalAction("Cancel build", onCancel, enabled = !busy)
        } else {
            TonalAction(KiwiBuilds.buildLabel(app), { onCheck(false) }, enabled = !busy && !dirty)
            if (app.published != null) TextButton(onClick = { onCheck(true) }, enabled = !busy && !dirty) { Text("Rebuild") }
        }
    }
    if (app.configurationChanged) {
        Text(
            "Your saved patches differ from the available build. ${if (job != null) "This build uses the choices saved when it started." else "Build changes to apply them."}",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 24.dp),
        )
    }
    BuildOptions(app, draft, enabled = !busy) { draft = it }
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (dirty) "Unsaved changes" else "Saved",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.weight(1f),
        )
        if (dirty) TextButton(onClick = { draft = app.preferences }) { Text("Discard") }
        TonalAction("Save", { onSave(draft) }, enabled = dirty && !busy)
    }
    TextButton(onClick = onRemove, enabled = job == null && !busy, modifier = Modifier.padding(horizontal = 12.dp)) {
        Text("Remove from Kiwi Builds", color = MaterialTheme.colorScheme.error)
    }
}

@Composable
private fun SheetHeader(app: AutomationState, tint: String?, detail: String) {
    SheetHeader(app.name, app.iconUrl, tint, detail) { if (app.experimental) Tag("Experimental") }
}

/** A sheet's header: the app's icon and name, a badge, and a line about it. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun SheetHeader(name: String, iconUrl: String?, tint: String?, detail: String, badge: @Composable () -> Unit = {}) {
    Row(
        Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        AppIcon(name, iconUrl, tint, size = 56.dp)
        Column(Modifier.weight(1f)) {
            FlowRow(verticalArrangement = Arrangement.Center, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(name, style = MaterialTheme.typography.titleLarge, modifier = Modifier.align(Alignment.CenterVertically))
                Box(Modifier.align(Alignment.CenterVertically)) { badge() }
            }
            Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
internal fun Tag(text: String) {
    Surface(shape = RoundedCornerShape(6.dp), color = MaterialTheme.kiwi.warningContainer, contentColor = MaterialTheme.kiwi.warningIcon) {
        Text(text, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp))
    }
}

/** Schedule (automatic builds and how often) and patches, shared by Add and the app's sheet. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun BuildOptions(
    app: AutomationState,
    preferences: AutomationPreferences,
    enabled: Boolean,
    onChange: (AutomationPreferences) -> Unit,
) {
    SectionLabel("Schedule")
    ListGroup {
        row {
            GroupRow(
                "Build updates automatically",
                trailing = { KiwiSwitch(preferences.enabled, { onChange(preferences.copy(enabled = it)) }, enabled = enabled) },
                onClick = if (enabled) ({ onChange(preferences.copy(enabled = !preferences.enabled)) }) else null,
            )
        }
        row {
            Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 8.dp)) {
                Text("Check for new versions", style = MaterialTheme.typography.bodyLarge)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    KiwiBuilds.intervals(preferences.intervalMinutes).forEach { minutes ->
                        FilterChip(
                            selected = minutes == preferences.intervalMinutes,
                            onClick = { onChange(preferences.copy(intervalMinutes = minutes)) },
                            label = { Text(KiwiBuilds.shortInterval(minutes)) },
                            enabled = enabled && preferences.enabled,
                        )
                    }
                }
            }
        }
    }
    if (app.names.isNotEmpty()) {
        SectionLabel("App name")
        FlowRow(Modifier.padding(horizontal = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            app.names.forEach { name ->
                FilterChip(
                    selected = (preferences.name ?: app.names.first()) == name,
                    onClick = { onChange(preferences.copy(name = name)) },
                    label = { Text(name) },
                    enabled = enabled,
                )
            }
        }
    }
    if (app.included.isEmpty() && app.availableTweaks.isEmpty()) return
    val (on, total) = KiwiBuilds.patchCount(app, preferences.tweaks)
    SectionLabel("Patches · $on of $total on")
    ListGroup {
        app.included.forEach { feature ->
            row {
                GroupRow(
                    feature.name,
                    subtitle = feature.description.takeIf { it.isNotBlank() }?.let(::AnnotatedString),
                    trailing = { Text("Included", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant) },
                )
            }
        }
        app.availableTweaks.forEach { tweak ->
            val checked = tweak.id in preferences.tweaks
            val toggle = { value: Boolean ->
                onChange(preferences.copy(tweaks = if (value) preferences.tweaks + tweak.id else preferences.tweaks - tweak.id))
            }
            row {
                GroupRow(
                    tweak.name,
                    subtitle = tweak.description.takeIf { it.isNotBlank() }?.let(::AnnotatedString),
                    trailing = { KiwiSwitch(checked, toggle, enabled = enabled) },
                    onClick = if (enabled) ({ toggle(!checked) }) else null,
                )
            }
        }
    }
}

/** A sheet's content on its own, as the sheet shows it (for previews and screenshots). */
@Composable
fun SheetPreviewFrame(content: @Composable ColumnScope.() -> Unit) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerLow,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        Column(Modifier.verticalScroll(rememberScrollState())) {
            Box(Modifier.fillMaxWidth().heightIn(min = 36.dp), contentAlignment = Alignment.Center) {
                Surface(
                    shape = RoundedCornerShape(2.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.4f),
                    modifier = Modifier.size(width = 32.dp, height = 4.dp),
                ) {}
            }
            Spacer(Modifier.height(8.dp))
            content()
        }
    }
}

@Preview(name = "C6 Builds")
@Composable
private fun BuildsPreview() {
    KiwiTheme(darkTheme = false) {
        BuildsContent(Load.Ready(PreviewData.buildsContent), emptySet(), notifyBuilds = true, now = PreviewData.NOW, actions = BuildsActions())
    }
}

@Preview(name = "C6 Builds, dark")
@Composable
private fun BuildsDarkPreview() {
    KiwiTheme(darkTheme = true) {
        BuildsContent(Load.Ready(PreviewData.buildsContent), emptySet(), notifyBuilds = true, now = PreviewData.NOW, actions = BuildsActions())
    }
}

@Preview(name = "C6 Builds, first run")
@Composable
private fun BuildsFirstRunPreview() {
    KiwiTheme {
        BuildsContent(Load.Ready(PreviewData.buildsFirstRun), emptySet(), true, PreviewData.NOW, BuildsActions())
    }
}

@Preview(name = "C6 Builds, first builds on their way")
@Composable
private fun BuildsInProgressPreview() {
    KiwiTheme {
        BuildsContent(
            Load.Ready(PreviewData.buildsInProgress),
            emptySet(),
            true,
            PreviewData.NOW,
            BuildsActions(),
            pending = PreviewData.pendingInstalls,
        )
    }
}

@Preview(name = "C6 Builds off")
@Composable
private fun BuildsOffPreview() {
    KiwiTheme {
        BuildsContent(Load.Ready(BuildsViewModel.Content(off = true, apps = KiwiBuilds.OFF_APPS)), emptySet(), true, PreviewData.NOW, BuildsActions())
    }
}

@Preview(name = "C6 Install sheet")
@Composable
private fun InstallSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame {
            InstallBuildSheetContent(PreviewData.redditToInstall, "#FF4500", PreviewData.redditBlocker, false, false, {}, {}, {})
        }
    }
}

@Preview(name = "C6 App sheet")
@Composable
private fun EditSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame { EditBuildSheetContent(PreviewData.youTubeRecipe, "#FF0000", false, {}, {}, {}, {}) }
    }
}
