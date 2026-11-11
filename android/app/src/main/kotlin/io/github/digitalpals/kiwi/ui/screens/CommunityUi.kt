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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.BuildLimitation
import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunityCandidate
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.BuildsViewModel
import io.github.digitalpals.kiwi.ui.Community
import io.github.digitalpals.kiwi.ui.CommunityFailure
import io.github.digitalpals.kiwi.ui.CommunitySearch
import io.github.digitalpals.kiwi.ui.CommunityTarget
import io.github.digitalpals.kiwi.ui.Format
import io.github.digitalpals.kiwi.ui.Fresh
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.FilledAction
import io.github.digitalpals.kiwi.ui.components.GroupRow
import io.github.digitalpals.kiwi.ui.components.IconCircle
import io.github.digitalpals.kiwi.ui.components.ListGroup
import io.github.digitalpals.kiwi.ui.components.SectionLabel
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.WarningCard
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi
import java.time.Instant

/**
 * The "Community" section of Builds, below Kiwi Builds: the owner's community apps (rows like
 * Kiwi Builds' own), popular apps of the Morphe community catalogue, and its search. Hidden on a
 * Kiwi without the catalogue.
 */
internal fun LazyListScope.communitySection(
    value: BuildsViewModel.Content,
    busy: Set<String>,
    pending: Set<String>,
    installs: Map<String, InstallJob>,
    actions: BuildsActions,
) {
    val community = value.community
    if (!value.off && community == null && value.communityError == null) return
    item(key = "community-label") { SectionLabel("Community") }
    item(key = "community-intro") {
        Text(
            Community.INTRO,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(start = 20.dp, end = 20.dp, bottom = 8.dp),
        )
    }
    if (value.off) {
        item(key = "community-off") {
            ListGroup {
                row {
                    GroupRow(
                        "Community apps need Kiwi Builds",
                        subtitle = AnnotatedString("Turn it on to browse and build them."),
                        leading = { IconCircle(KiwiIcons.Builds, MaterialTheme.colorScheme.primary) },
                        trailing = { TonalAction("Turn on", actions.turnOn, enabled = BuildsViewModel.ALL !in busy) },
                    )
                }
            }
        }
        return
    }
    if (community == null) {
        item(key = "community-error") {
            ListGroup {
                row {
                    GroupRow(
                        "Couldn't load community apps",
                        subtitle = AnnotatedString(value.communityError.orEmpty()),
                        leading = { IconCircle(KiwiIcons.Warning, MaterialTheme.kiwi.warningIcon) },
                        trailing = { TonalAction("Try again", actions.refresh) },
                    )
                }
            }
        }
        return
    }
    when {
        !community.available -> item(key = "community-unavailable") {
            NoticeCard(
                "Browse only on this Kiwi",
                community.reason ?: "Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the patcher in a sandbox.",
                Modifier.padding(bottom = 8.dp),
            )
        }
        community.index == null -> item(key = "community-loading") {
            NoticeCard("Loading the community list", community.reason ?: Community.LOADING, Modifier.padding(bottom = 8.dp), loading = true)
        }
    }
    val added = community.added
    if (added.isNotEmpty()) {
        item(key = "community-added") {
            ListGroup {
                added.forEach { app ->
                    val summary = value.published[app.bundleId]
                    val view = Community.row(
                        app = app,
                        summary = summary,
                        local = value.local[app.bundleId],
                        job = summary?.let { installs[it.key] },
                        pending = app.id in pending,
                        off = false,
                        available = community.available,
                    )
                    row {
                        BuildRow(app, view, value.tints[app.bundleId], app.id in busy, menu = true, actions) {
                            CommunityMenu(app, app.id in busy, community.available, actions)
                        }
                    }
                }
            }
        }
    }
    val popular = Community.popular(community.popular, added)
    if (popular.isNotEmpty()) {
        item(key = "community-popular-label") { SectionLabel("Popular") }
        item(key = "community-popular") {
            ListGroup {
                popular.forEach { app ->
                    row {
                        CommunityAppRow(app, community.available, app.id in busy, onInstall = { actions.installCommunity(app) })
                    }
                }
            }
        }
    }
    val index = community.index
    if (index != null) {
        item(key = "community-search") {
            val stale = Fresh.parseDate(index.updatedAt)?.takeIf { index.stale }
            ListGroup(Modifier.padding(top = 12.dp)) {
                row {
                    GroupRow(
                        Community.searchLabel(index.apps),
                        subtitle = AnnotatedString(
                            if (stale != null) "Kiwi couldn't refresh the list lately; it's from ${Format.day(stale)}." else "By name, package or patch",
                        ),
                        leading = { IconCircle(KiwiIcons.Search, MaterialTheme.colorScheme.primary) },
                        trailing = { Icon(KiwiIcons.Chevron, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                        onClick = actions.search,
                    )
                }
            }
        }
    }
}

/** The badge of community apps: their code comes from strangers. */
@Composable
fun CommunityTag(text: String = Community.TAG, modifier: Modifier = Modifier) {
    Surface(
        shape = RoundedCornerShape(6.dp),
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = modifier,
    ) {
        Text(text, style = MaterialTheme.typography.labelSmall, maxLines = 1, modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp))
    }
}

/** A calm card for how things are (not a problem on the phone): browse only, still loading. */
@Composable
fun NoticeCard(title: String, body: String, modifier: Modifier = Modifier, loading: Boolean = false) {
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = MaterialTheme.kiwi.card,
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp),
    ) {
        Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            if (loading) {
                CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.5.dp)
            } else {
                Icon(KiwiIcons.Info, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(22.dp))
            }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.bodyLarge)
                Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 2.dp))
            }
        }
    }
}

/**
 * A catalogue app: icon, name, its category and patches, the community badge and Install
 * (none while builds aren't possible here). An app the owner [added] opens its sheet instead.
 */
@Composable
fun CommunityAppRow(
    app: CommunityApp,
    available: Boolean,
    busy: Boolean,
    onInstall: () -> Unit,
    added: Boolean = app.added,
    onOpen: () -> Unit = {},
) {
    val meta = Community.meta(app)
    GroupRow(
        app.name,
        subtitle = meta.takeIf { it.isNotEmpty() }?.let(::AnnotatedString),
        leading = { AppIcon(app.name, app.iconUrl, null, size = 44.dp) },
        trailing = {
            when {
                added -> Text("Added", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                available -> FilledAction(if (busy) "Starting…" else "Install", onInstall, enabled = !busy)
            }
        },
        onClick = when {
            added -> onOpen
            available && !busy -> onInstall
            else -> null
        },
        below = { CommunityTag(modifier = Modifier.padding(top = 4.dp)) },
    )
}

/** ⋮ of an added community app: cancel its build, its patches, other patches, remove. */
@Composable
private fun CommunityMenu(app: AutomationState, busy: Boolean, available: Boolean, actions: BuildsActions) {
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
            if (job != null) DropdownMenuItem(text = { Text("Cancel build") }, onClick = { pick { actions.cancel(app) } })
            DropdownMenuItem(text = { Text("Patches") }, onClick = { pick { actions.edit(app) } })
            if (available && job == null) {
                DropdownMenuItem(text = { Text("Choose other patches") }, onClick = { pick { actions.choosePatches(app) } })
            }
            DropdownMenuItem(text = { Text("Remove") }, enabled = job == null, onClick = { pick { actions.remove(app) } })
        }
    }
}

/**
 * Before building a community app: how long it takes, what is in the way on this phone
 * ([blocker]: the Play Store app has its package), and what to know: code not reviewed by Kiwi,
 * it replaces the Play app, Google sign-in and notifications. [limitations] are Kiwi's own
 * notices for an added app; the app's are shown before that.
 */
@Composable
fun CommunityInstallSheetContent(
    target: CommunityTarget,
    tint: String?,
    limitations: List<BuildLimitation>,
    blocker: String?,
    available: Boolean,
    reason: String?,
    notificationsAllowed: Boolean,
    busy: Boolean,
    onBuild: () -> Unit,
    onUninstall: () -> Unit,
    onNotificationSettings: () -> Unit,
) {
    SheetHeader(target.name, target.iconUrl, tint, target.detail.ifBlank { "Morphe community" }) { CommunityTag() }
    if (!available) {
        NoticeCard("Browse only on this Kiwi", reason ?: "Kiwi can't build community apps here.", Modifier.padding(top = 8.dp))
    } else {
        ListGroup(Modifier.padding(top = 8.dp)) {
            row {
                GroupRow(
                    "First build: ${KiwiBuilds.BUILD_TIME}",
                    subtitle = AnnotatedString(Community.firstBuildNotice(target.name)),
                    leading = { IconCircle(KiwiIcons.Clock, Color(0xFF1E8E9E)) },
                )
            }
            if (!notificationsAllowed) {
                row {
                    GroupRow(
                        "Notifications are off",
                        subtitle = AnnotatedString("Kiwi can't tell you when ${target.name} is ready. Open Kiwi again in a few minutes to install it."),
                        leading = { IconCircle(KiwiIcons.Bell, Color(0xFFE5483C)) },
                        trailing = { TonalAction("Turn on", onNotificationSettings) },
                    )
                }
            }
        }
    }
    if (blocker != null) {
        WarningCard("Uninstall the other ${target.name} first", blocker, Modifier.padding(top = 12.dp), action = "Uninstall", onAction = onUninstall)
    }
    SectionLabel("What to know")
    ListGroup {
        limitations.ifEmpty { Community.notices(target.name, target.info) }.forEach { limitation ->
            row { GroupRow(limitation.title, subtitle = AnnotatedString(limitation.detail)) }
        }
    }
    if (available) {
        Column(Modifier.padding(horizontal = 24.dp, vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            FilledAction(if (busy) "Starting…" else "Build and install", onBuild, Modifier.fillMaxWidth(), small = false, enabled = !busy)
            Text(
                Community.NO_SCHEDULE_NOTICE,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 6.dp),
            )
        }
    } else {
        Spacer(Modifier.height(16.dp))
    }
}

/**
 * An added community app's sheet: its build and what to do about a failure, an offered update,
 * the bundle it is built with (and choosing another), its applied patches, and Remove.
 */
@Composable
fun CommunityAppSheetContent(
    app: AutomationState,
    tint: String?,
    available: Boolean,
    busy: Boolean,
    onRetry: () -> Unit,
    onChoose: () -> Unit,
    onUpdate: () -> Unit,
    onCancel: () -> Unit,
    onRemove: () -> Unit,
) {
    val info = app.community
    val job = KiwiBuilds.activeJob(app)
    val failed = job == null && KiwiBuilds.needsAttention(app)
    SheetHeader(app.name, app.iconUrl, tint, Community.statusLine(app)) { CommunityTag() }
    if (job?.status == "running") {
        LinearProgressIndicator(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 4.dp),
        )
    }
    val failure = Community.failure(app.jobs.firstOrNull())
    if (failed) {
        KiwiBuilds.attentionDetail(app)?.let {
            Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 24.dp))
        }
        if (failure == CommunityFailure.Unavailable) {
            Text(
                Community.UNAVAILABLE_NOTE,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 4.dp),
            )
        }
    }
    val buildable = available && !busy
    if (job != null || failed) {
        Row(Modifier.padding(horizontal = 24.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                job != null -> TonalAction("Cancel build", onCancel, enabled = !busy)
                failure == CommunityFailure.Retry -> FilledAction(if (busy) "Starting…" else "Retry", onRetry, enabled = buildable)
                failure == CommunityFailure.OtherPatches -> FilledAction("Choose other patches", onChoose, enabled = buildable)
                else -> TonalAction("Remove", onRemove, enabled = !busy)
            }
        }
    }
    val update = info?.let(Community::updateLine)
    if (info != null && update != null && job == null) {
        SectionLabel("Update")
        ListGroup {
            row {
                GroupRow(
                    update,
                    subtitle = AnnotatedString(Community.updateNotice(app.name, info)),
                    leading = { IconCircle(KiwiIcons.Download, MaterialTheme.colorScheme.primary) },
                    trailing = { FilledAction(if (busy) "Starting…" else "Update", onUpdate, enabled = buildable) },
                )
            }
        }
    }
    SectionLabel("Patches")
    ListGroup {
        row {
            if (info != null && info.repo.isNotBlank()) {
                GroupRow(
                    info.repo,
                    subtitle = AnnotatedString(
                        listOf(info.author.takeIf { it.isNotBlank() }?.let { "By $it" }, info.tag.ifBlank { null }, Community.chosenLabel(info))
                            .filterNotNull().joinToString(" · "),
                    ),
                    leading = { IconCircle(KiwiIcons.Shield, Color(0xFF6B6FD4)) },
                )
            } else {
                GroupRow(
                    "Kiwi is choosing patches",
                    subtitle = AnnotatedString("It picks the bundle that best matches the version Google Play offers."),
                    leading = { IconCircle(KiwiIcons.Shield, Color(0xFF6B6FD4)) },
                )
            }
        }
        if (available && job == null) {
            row {
                GroupRow(
                    "Choose other patches",
                    subtitle = AnnotatedString("Every bundle that can build ${app.name}"),
                    trailing = { Icon(KiwiIcons.Chevron, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                    onClick = if (!busy) onChoose else null,
                )
            }
        }
    }
    if (app.included.isNotEmpty()) {
        var all by rememberSaveable(app.id) { mutableStateOf(false) }
        val shown = if (all) app.included else app.included.take(PATCHES_SHOWN)
        SectionLabel("Applied patches · ${app.included.size}")
        ListGroup {
            shown.forEach { feature ->
                row { GroupRow(feature.name, subtitle = feature.description.takeIf { it.isNotBlank() }?.let(::AnnotatedString)) }
            }
            if (shown.size < app.included.size) {
                row { GroupRow("Show all ${app.included.size} patches", titleColor = MaterialTheme.colorScheme.primary, onClick = { all = true }) }
            }
        }
    }
    if (app.limitations.isNotEmpty()) {
        SectionLabel("What to know")
        ListGroup {
            app.limitations.forEach { limitation -> row { GroupRow(limitation.title, subtitle = AnnotatedString(limitation.detail)) } }
        }
    }
    TextButton(onClick = onRemove, enabled = job == null && !busy, modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
        Text("Remove ${app.name}", color = MaterialTheme.colorScheme.error)
    }
}

private const val PATCHES_SHOWN = 6

/**
 * "Choose other patches": every bundle that can build the app, recommended first, with its
 * author, stars, freshness, patches and versions; one that failed says why. Picking one
 * rebuilds the app with it.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun CommunityCandidatesSheetContent(
    app: AutomationState,
    tint: String?,
    load: Load<List<CommunityCandidate>>,
    now: Instant,
    available: Boolean,
    busy: Boolean,
    onRetry: () -> Unit,
    onPick: (String) -> Unit,
) {
    SheetHeader(app.name, app.iconUrl, tint, "Choose patches") { CommunityTag() }
    Text(
        "Bundles by different authors that patch ${app.name}. Kiwi recommends the best match for the version Google Play " +
            "offers. Picking another rebuilds ${app.name} with it and installs it when it's ready.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp),
    )
    when (load) {
        Load.Loading -> Box(Modifier.fillMaxWidth().height(120.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        is Load.Failed -> Column(Modifier.padding(horizontal = 24.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                load.error.message ?: "Couldn't load the patches.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
            )
            TonalAction("Try again", onRetry)
        }
        is Load.Ready -> if (load.value.isEmpty()) {
            Text(
                "Kiwi knows no other bundles for ${app.name}.",
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 12.dp),
            )
        } else {
            ListGroup(Modifier.padding(bottom = 16.dp)) {
                load.value.forEach { candidate ->
                    row {
                        GroupRow(
                            candidate.repo,
                            subtitle = AnnotatedString(Community.candidateLine(candidate, now)),
                            trailing = {
                                if (candidate.chosen) {
                                    Text("Current", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                } else if (available) {
                                    TonalAction("Use", { onPick(candidate.repo) }, enabled = !busy)
                                }
                            },
                            below = {
                                Text(
                                    Community.candidateDetail(candidate),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                if (candidate.recommended) {
                                    FlowRow(Modifier.padding(top = 4.dp)) { RecommendedTag() }
                                }
                                candidate.failed?.let {
                                    Text(
                                        "Didn't build: $it",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.error,
                                        modifier = Modifier.padding(top = 2.dp),
                                    )
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun RecommendedTag() {
    Surface(shape = RoundedCornerShape(6.dp), color = MaterialTheme.colorScheme.primaryContainer, contentColor = MaterialTheme.colorScheme.onPrimaryContainer) {
        Text("Recommended", style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp))
    }
}

/** The search's sheet: as tall as the screen, its list scrolls on its own. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SearchSheet(onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Column(Modifier.fillMaxSize().navigationBarsPadding(), content = content)
    }
}

/**
 * Searching the community catalogue: a search field, then the matches page by page (the next
 * page loads as the end comes into view). Apps the owner [added] open their sheet.
 */
@Composable
fun CommunitySearchContent(
    state: CommunitySearch.State,
    available: Boolean,
    added: Set<String>,
    busy: Set<String>,
    onQuery: (String) -> Unit,
    onLoadMore: () -> Unit,
    onRetry: () -> Unit,
    onInstall: (CommunityApp) -> Unit,
    onOpen: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxSize()) {
        Text("Community apps", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp))
        CommunitySearchField(state.query, onQuery)
        Box(Modifier.fillMaxWidth().height(4.dp)) {
            if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = 24.dp))
        }
        LazyColumn(Modifier.weight(1f), contentPadding = PaddingValues(bottom = 16.dp)) {
            if (state.loaded && state.error == null) {
                item(key = "count") {
                    Text(
                        countLine(state),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 24.dp, vertical = 6.dp),
                    )
                }
            }
            when {
                state.error != null -> item(key = "error") {
                    SearchMessage(state.error, error = true) { TonalAction("Try again", onRetry) }
                }
                state.empty -> item(key = "empty") {
                    SearchMessage(
                        if (state.query.isBlank()) "Kiwi has no community apps yet." else "No community apps match “${state.query.trim()}”.",
                    )
                }
                else -> {
                    items(state.results, key = { it.id }) { app ->
                        val isAdded = app.added || app.id in added
                        CommunityAppRow(app, available, app.id in busy, onInstall = { onInstall(app) }, added = isAdded, onOpen = { onOpen(app.id) })
                    }
                    item(key = "more") {
                        when {
                            state.moreError != null -> SearchMessage(state.moreError, error = true) { TonalAction("Try again", onRetry) }
                            state.loadingMore || state.canLoadMore -> {
                                if (state.canLoadMore) LaunchedEffect(state.results.size) { onLoadMore() }
                                Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.5.dp)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun countLine(state: CommunitySearch.State): String {
    val total = String.format(java.util.Locale.US, "%,d", state.total)
    return when {
        state.query.isBlank() -> "$total apps, most installed first"
        state.total == 1 -> "1 match"
        else -> "$total matches"
    }
}

@Composable
private fun SearchMessage(text: String, error: Boolean = false, action: (@Composable () -> Unit)? = null) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp, vertical = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium,
            color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        action?.invoke()
    }
}

@Composable
private fun CommunitySearchField(query: String, onQuery: (String) -> Unit) {
    val focus = LocalFocusManager.current
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = Modifier
            .padding(start = 16.dp, end = 16.dp, bottom = 6.dp)
            .fillMaxWidth()
            .heightIn(min = 56.dp),
    ) {
        Row(Modifier.padding(start = 18.dp, end = 6.dp).height(56.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(KiwiIcons.Search, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(14.dp))
            BasicTextField(
                value = query,
                onValueChange = onQuery,
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
                modifier = Modifier.weight(1f),
                decorationBox = { field ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (query.isEmpty()) {
                            Text(
                                "Search apps and patches",
                                style = MaterialTheme.typography.bodyLarge,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        field()
                    }
                },
            )
            if (query.isNotEmpty()) {
                IconButton(onClick = { onQuery("") }) {
                    Icon(KiwiIcons.Close, contentDescription = "Clear search", tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Preview(name = "C6 Builds, community")
@Composable
private fun CommunityPreview() {
    KiwiTheme(darkTheme = false) {
        BuildsContent(Load.Ready(PreviewData.buildsCommunity), emptySet(), true, PreviewData.NOW, BuildsActions(), pending = PreviewData.communityPending)
    }
}

@Preview(name = "C6 Builds, community, dark")
@Composable
private fun CommunityDarkPreview() {
    KiwiTheme(darkTheme = true) {
        BuildsContent(Load.Ready(PreviewData.buildsCommunity), emptySet(), true, PreviewData.NOW, BuildsActions(), pending = PreviewData.communityPending)
    }
}

@Preview(name = "C6 Builds, community on a Mac")
@Composable
private fun CommunityMacPreview() {
    KiwiTheme {
        BuildsContent(Load.Ready(PreviewData.buildsCommunityMac), emptySet(), true, PreviewData.NOW, BuildsActions())
    }
}

@Preview(name = "C6 Community install sheet")
@Composable
private fun CommunityInstallSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame {
            CommunityInstallSheetContent(
                Community.target(PreviewData.discord), null, emptyList(), PreviewData.discordBlocker, true, null, true, false, {}, {}, {},
            )
        }
    }
}

@Preview(name = "C6 Community app sheet")
@Composable
private fun CommunityAppSheetPreview() {
    KiwiTheme {
        SheetPreviewFrame { CommunityAppSheetContent(PreviewData.twitchCommunity, null, true, false, {}, {}, {}, {}, {}) }
    }
}

@Preview(name = "C6 Community candidates")
@Composable
private fun CommunityCandidatesPreview() {
    KiwiTheme {
        SheetPreviewFrame {
            CommunityCandidatesSheetContent(PreviewData.twitchCommunity, null, Load.Ready(PreviewData.twitchCandidates), PreviewData.NOW, true, false, {}, {})
        }
    }
}

@Preview(name = "C6 Community search")
@Composable
private fun CommunitySearchPreview() {
    KiwiTheme {
        Surface(color = MaterialTheme.colorScheme.surfaceContainerLow) {
            CommunitySearchContent(PreviewData.communitySearch, true, emptySet(), emptySet(), {}, {}, {}, {}, {})
        }
    }
}
