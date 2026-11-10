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
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.ui.BuildsViewModel
import io.github.digitalpals.kiwi.ui.Format
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.GroupRow
import io.github.digitalpals.kiwi.ui.components.IconCircle
import io.github.digitalpals.kiwi.ui.components.KiwiSwitch
import io.github.digitalpals.kiwi.ui.components.KiwiTopBar
import io.github.digitalpals.kiwi.ui.components.ListGroup
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.LocalSnackbars
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.components.SectionLabel
import io.github.digitalpals.kiwi.ui.components.TitleSummary
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.WarningCard
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi
import kotlinx.coroutines.delay
import java.time.Instant

/** What the Builds screen can do; previews use the defaults (nothing). */
class BuildsActions(
    val refresh: () -> Unit = {},
    val add: () -> Unit = {},
    val edit: (AutomationState) -> Unit = {},
    val check: (AutomationState, Boolean) -> Unit = { _, _ -> },
    val cancel: (AutomationState) -> Unit = {},
    val remove: (AutomationState) -> Unit = {},
    val checkAll: () -> Unit = {},
    val notifyBuilds: (Boolean) -> Unit = {},
    val turnOn: () -> Unit = {},
)

/**
 * Builds: Kiwi Builds' Android apps (`/api/automation`). Polls every 3 seconds while a job runs
 * and every 30 seconds otherwise, while the screen is visible. [editRequest] opens an app's
 * patches and schedule (from its app page).
 */
@Composable
fun BuildsScreen(model: BuildsViewModel, editRequest: String?, onEditHandled: () -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val busy by model.busy.collectAsStateWithLifecycle()
    val notifyBuilds by model.notifyBuilds.collectAsStateWithLifecycle(initialValue = true)
    val snackbars = LocalSnackbars.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val content = (state as? Load.Ready)?.value
    val active = content?.active == true
    var adding by rememberSaveable { mutableStateOf(false) }
    var editing by rememberSaveable { mutableStateOf<String?>(null) }
    var removing by rememberSaveable { mutableStateOf<String?>(null) }
    var turningOn by rememberSaveable { mutableStateOf(false) }

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
        actions = BuildsActions(
            refresh = { model.reload() },
            add = { adding = true },
            edit = { editing = it.id },
            check = { app, force -> model.check(app, force) },
            cancel = model::cancel,
            remove = { removing = it.id },
            checkAll = { content?.let { model.checkAll(it.added) } },
            notifyBuilds = model::setNotifyBuilds,
            turnOn = { turningOn = true },
        ),
    )

    if (adding && content != null) {
        BuildSheet(onDismiss = { adding = false }) {
            AddBuildSheetContent(content.available, content.tints, busy) { app, preferences ->
                model.add(app, preferences) { adding = false }
            }
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
    content?.apps?.firstOrNull { it.id == removing }?.let { app ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${app.name} from Kiwi Builds?") },
            text = { Text("Installed copies keep working, but won't get updates. You can add it again later.") },
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
    if (turningOn) {
        AlertDialog(
            onDismissRequest = { turningOn = false },
            title = { Text("Turn on Kiwi Builds?") },
            text = { Text(KiwiBuilds.TURN_ON_NOTICE) },
            confirmButton = {
                TextButton(onClick = {
                    turningOn = false
                    model.turnOn()
                }) { Text("Turn on") }
            },
            dismissButton = { TextButton(onClick = { turningOn = false }) { Text("Cancel") } },
        )
    }
}

/** C6: your apps with their versions, schedules and running builds; the schedule; +. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BuildsContent(
    load: Load<BuildsViewModel.Content>,
    busy: Set<String>,
    notifyBuilds: Boolean,
    now: Instant,
    actions: BuildsActions,
) {
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    val content = (load as? Load.Ready)?.value
    Scaffold(
        topBar = {
            KiwiTopBar("Builds", scroll) {
                if (content != null && !content.off) {
                    IconButton(onClick = actions.add, enabled = content.available.isNotEmpty()) {
                        Icon(KiwiIcons.Plus, contentDescription = "Add an app")
                    }
                }
            }
        },
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
                if (value.off) {
                    MessagePane(
                        "Kiwi Builds is off on your Kiwi",
                        "Turn it on to patch Android apps like YouTube with open-source patches and sign them with your Kiwi's own key.",
                        icon = KiwiIcons.Builds,
                        primary = "Turn on Kiwi Builds" to actions.turnOn,
                    )
                    return@PullToRefreshBox
                }
                LazyColumn(
                    Modifier
                        .fillMaxSize()
                        .nestedScroll(scroll.nestedScrollConnection),
                    contentPadding = PaddingValues(bottom = 16.dp),
                ) {
                    item(key = "summary") { TitleSummary("Patched and signed by your Kiwi") }
                    val added = value.added
                    if (added.isEmpty()) {
                        item(key = "empty") {
                            MessagePane(
                                "No apps yet",
                                "Add YouTube, YouTube Music or Reddit: Kiwi downloads them from Google Play, patches them and signs them with its own key.",
                                Modifier.height(420.dp),
                                icon = KiwiIcons.Builds,
                                primary = if (value.available.isNotEmpty()) "Add an app" to actions.add else null,
                            )
                        }
                        return@LazyColumn
                    }
                    item(key = "apps-label") { SectionLabel("Your apps") }
                    item(key = "apps") {
                        ListGroup {
                            added.forEach { app -> row { BuildRow(app, value.tints[app.bundleId], app.id in busy, actions) } }
                        }
                    }
                    item(key = "schedule-label") { SectionLabel("Schedule") }
                    item(key = "schedule") {
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
                    item(key = "footer") {
                        Text(
                            "One build runs at a time. Kiwi keeps building while this app is closed; if a build fails, the previous version stays available.",
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

/** An added app: version and schedule, or its running build with a progress bar; ⋮ for more. */
@Composable
private fun BuildRow(app: AutomationState, tint: String?, busy: Boolean, actions: BuildsActions) {
    val job = KiwiBuilds.activeJob(app)
    val attention = KiwiBuilds.needsAttention(app) && job == null
    var menu by remember { mutableStateOf(false) }
    GroupRow(
        app.name,
        subtitle = AnnotatedString(KiwiBuilds.statusLine(app)),
        subtitleColor = if (attention) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        leading = { AppIcon(app.name, app.iconUrl, tint, size = 44.dp) },
        trailing = {
            Box {
                IconButton(onClick = { menu = true }, enabled = !busy) {
                    Icon(KiwiIcons.More, contentDescription = "More for ${app.name}", tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    fun pick(action: () -> Unit) {
                        menu = false
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
        },
        onClick = { actions.edit(app) },
        below = {
            if (job?.status == "running") {
                LinearProgressIndicator(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 8.dp, bottom = 2.dp),
                )
            }
            if (attention) {
                KiwiBuilds.attentionDetail(app)?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 3)
                }
            }
        },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BuildSheet(onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
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

/** The + sheet: Android apps not added yet, then the chosen one's patches and schedule. */
@Composable
fun AddBuildSheetContent(
    available: List<AutomationState>,
    tints: Map<String, String>,
    busy: Set<String>,
    onAdd: (AutomationState, AutomationPreferences) -> Unit,
) {
    var chosen by rememberSaveable { mutableStateOf(available.singleOrNull()?.id) }
    val app = available.firstOrNull { it.id == chosen }
    if (app == null) {
        SheetTitle("Add an app", "Kiwi downloads it from Google Play, patches it with open-source patches and signs it with its own key.")
        if (available.isEmpty()) {
            Text(
                "Every Android app Kiwi can build is added.",
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.padding(24.dp),
            )
        }
        ListGroup {
            available.forEach { recipe ->
                row {
                    GroupRow(
                        recipe.name,
                        subtitle = AnnotatedString(recipe.summary),
                        leading = { AppIcon(recipe.name, recipe.iconUrl, tints[recipe.bundleId], size = 44.dp) },
                        trailing = { Icon(KiwiIcons.Chevron, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                        onClick = { chosen = recipe.id },
                    )
                }
            }
        }
        return
    }
    var preferences by remember(app.id) { mutableStateOf(app.preferences) }
    SheetHeader(app, tints[app.bundleId], app.tweak)
    app.warning?.let { WarningCard("Before you add ${app.name}", it, Modifier.padding(bottom = 8.dp)) }
    if (app.limitations.isNotEmpty()) {
        SectionLabel("What to know")
        ListGroup {
            app.limitations.forEach { limitation ->
                row { GroupRow(limitation.title, subtitle = AnnotatedString(limitation.detail)) }
            }
        }
    }
    BuildOptions(app, preferences, enabled = app.id !in busy) { preferences = it }
    Column(Modifier.padding(horizontal = 24.dp, vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        TonalAction(
            if (app.id in busy) "Adding…" else "Add ${app.name}",
            { onAdd(app, preferences) },
            Modifier.fillMaxWidth(),
            small = false,
            enabled = app.id !in busy,
        )
        Text(
            "Downloads ${app.name} from Google Play, patches it and signs it with your Kiwi's own key. You can change these choices later.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 10.dp),
        )
        if (available.size > 1) TextButton(onClick = { chosen = null }) { Text("Choose another app") }
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
private fun SheetTitle(title: String, detail: String) {
    Column(Modifier.padding(start = 24.dp, end = 24.dp, bottom = 12.dp)) {
        Text(title, style = MaterialTheme.typography.headlineSmall)
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 4.dp))
    }
}

@Composable
private fun SheetHeader(app: AutomationState, tint: String?, detail: String) {
    Row(
        Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        AppIcon(app.name, app.iconUrl, tint, size = 56.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(app.name, style = MaterialTheme.typography.titleLarge)
                if (app.experimental) Tag("Experimental")
            }
            Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun Tag(text: String) {
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
        BuildsContent(
            Load.Ready(BuildsViewModel.Content(off = false, apps = PreviewData.builds, tints = PreviewData.tints)),
            emptySet(),
            notifyBuilds = true,
            now = PreviewData.NOW,
            actions = BuildsActions(),
        )
    }
}

@Preview(name = "C6 Builds, dark")
@Composable
private fun BuildsDarkPreview() {
    KiwiTheme(darkTheme = true) {
        BuildsContent(
            Load.Ready(BuildsViewModel.Content(off = false, apps = PreviewData.builds, tints = PreviewData.tints)),
            emptySet(),
            notifyBuilds = true,
            now = PreviewData.NOW,
            actions = BuildsActions(),
        )
    }
}

@Preview(name = "C6 Builds off")
@Composable
private fun BuildsOffPreview() {
    KiwiTheme {
        BuildsContent(Load.Ready(BuildsViewModel.Content(off = true, apps = emptyList())), emptySet(), true, PreviewData.NOW, BuildsActions())
    }
}

@Preview(name = "C6 Add sheet")
@Composable
private fun AddSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame { AddBuildSheetContent(PreviewData.buildsToAdd, PreviewData.tints, emptySet()) { _, _ -> } }
    }
}

@Preview(name = "C6 App sheet")
@Composable
private fun EditSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame { EditBuildSheetContent(PreviewData.youTubeRecipe, "#FF0000", false, {}, {}, {}, {}) }
    }
}
