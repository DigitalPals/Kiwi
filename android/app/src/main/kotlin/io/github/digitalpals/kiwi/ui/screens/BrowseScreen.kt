package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.carousel.HorizontalMultiBrowseCarousel
import androidx.compose.material3.carousel.rememberCarouselState
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.R
import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppActions
import io.github.digitalpals.kiwi.ui.AppItem
import io.github.digitalpals.kiwi.ui.BrowseViewModel
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Fresh
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.Tint
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.AppListRow
import io.github.digitalpals.kiwi.ui.components.KiwiTopBar
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.components.ProgressRing
import io.github.digitalpals.kiwi.ui.components.SectionTitle
import io.github.digitalpals.kiwi.ui.components.appColor
import io.github.digitalpals.kiwi.ui.components.progressText
import io.github.digitalpals.kiwi.ui.components.versionChangeText
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.roundToInt

/** Browse: the apps Kiwi offers this phone (`GET /api/apps`). */
@Composable
fun BrowseScreen(container: AppContainer, model: BrowseViewModel, actions: AppActions, onOpenSettings: () -> Unit, onOpenBuilds: () -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val fresh by model.fresh.collectAsStateWithLifecycle()
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    BrowseContent(
        load = state,
        query = query,
        onQuery = model::setQuery,
        freshKeys = fresh,
        jobs = jobs,
        actions = actions,
        onRefresh = { model.reload() },
        onAvatar = onOpenSettings,
        onOpenBuilds = onOpenBuilds,
    )
}

/** C2: search, "Fresh this week" in the apps' colours, and all apps. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowseContent(
    load: Load<List<AppItem>>,
    query: String,
    onQuery: (String) -> Unit,
    freshKeys: List<String>?,
    jobs: Map<String, InstallJob>,
    actions: AppActions,
    onRefresh: () -> Unit,
    onAvatar: () -> Unit,
    onOpenBuilds: () -> Unit,
) {
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    Scaffold(
        topBar = {
            KiwiTopBar("Browse", scroll) {
                IconButton(onClick = onRefresh) { Icon(KiwiIcons.Refresh, contentDescription = "Refresh") }
            }
        },
        contentWindowInsets = WindowInsets(0),
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = load is Load.Ready && load.refreshing,
            onRefresh = onRefresh,
            modifier = Modifier.padding(padding),
        ) {
            LazyColumn(
                Modifier
                    .fillMaxSize()
                    .nestedScroll(scroll.nestedScrollConnection),
                contentPadding = PaddingValues(bottom = 16.dp),
            ) {
                item(key = "search") { SearchField(query, onQuery, onAvatar) }
                when (load) {
                    Load.Loading -> item(key = "loading") { Loading(Modifier.height(320.dp)) }
                    is Load.Failed -> item(key = "failed") { LoadFailed(load.error, onRetry = onRefresh, Modifier.height(420.dp)) }
                    is Load.Ready -> {
                        val items = load.value
                        val byKey = items.associateBy { it.app.key }
                        val fresh = if (query.isBlank()) freshKeys.orEmpty().mapNotNull { byKey[it] } else emptyList()
                        if (fresh.isNotEmpty()) {
                            item(key = "fresh-title") { SectionTitle("Fresh this week") }
                            item(key = "fresh") { FreshCarousel(fresh, jobs, actions) }
                        }
                        if (items.isEmpty()) {
                            item(key = "empty") {
                                if (query.isBlank()) {
                                    MessagePane(
                                        "No apps yet",
                                        "Your Kiwi has no Android apps yet. Add YouTube and more in Builds: Kiwi patches them and signs them with its own key.",
                                        Modifier.height(420.dp),
                                        icon = KiwiIcons.Builds,
                                        primary = "Open Builds" to onOpenBuilds,
                                    )
                                } else {
                                    MessagePane("No results", "No app matches “$query”.", Modifier.height(320.dp), icon = KiwiIcons.Search)
                                }
                            }
                        } else {
                            item(key = "all-title") {
                                SectionTitle(if (query.isBlank()) "All apps" else "Results", Modifier.padding(top = if (fresh.isNotEmpty()) 10.dp else 0.dp))
                            }
                            items(items, key = { it.app.key }) { item -> BrowseRow(item, jobs[item.app.key], actions) }
                        }
                    }
                }
            }
        }
    }
}

/** A row of Browse: what's new for updates, the installed version, else the subtitle. */
@Composable
fun BrowseRow(item: AppItem, job: InstallJob?, actions: AppActions) {
    val app = item.app
    val view = appButton(app, job, item.local)
    val change = Fresh.versionChange(app.state)
    val installed = app.state.installed
    val subtitle = when {
        change != null && (view is ButtonView.Install || view is ButtonView.Progress) -> versionChangeText(change)
        installed != null && app.state.action == Action.Open && item.local.installed -> AnnotatedString("${installed.version} · Installed")
        else -> AnnotatedString(app.subtitle ?: app.developer ?: app.sourceName)
    }
    AppListRow(app.name, app.iconUrl, app.tintColor, subtitle, view, actions.buttons(app), onClick = { actions.show(app) })
}

/** Material 3's search bar: search Kiwi's apps; the Kiwi avatar opens Settings. */
@Composable
private fun SearchField(query: String, onQuery: (String) -> Unit, onAvatar: () -> Unit) {
    val focus = LocalFocusManager.current
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = Modifier
            .padding(start = 16.dp, end = 16.dp, top = 2.dp, bottom = 6.dp)
            .fillMaxWidth()
            .height(56.dp),
    ) {
        Row(Modifier.padding(start = 18.dp, end = 6.dp), verticalAlignment = Alignment.CenterVertically) {
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
                            Text("Search apps", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        field()
                    }
                },
            )
            if (query.isNotEmpty()) {
                IconButton(onClick = { onQuery("") }) {
                    Icon(KiwiIcons.Close, contentDescription = "Clear search", tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            } else {
                Box(
                    Modifier
                        .padding(end = 4.dp)
                        .size(36.dp)
                        .clip(CircleShape)
                        .background(Color.White)
                        .clickable(onClickLabel = "Settings", onClick = onAvatar),
                    contentAlignment = Alignment.Center,
                ) {
                    Image(painterResource(R.drawable.kiwi_slice), contentDescription = "Your Kiwi", modifier = Modifier.size(34.dp))
                }
            }
        }
    }
}

/** "Fresh this week": Material 3's multi-browse carousel, each card in its app's colour. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FreshCarousel(apps: List<AppItem>, jobs: Map<String, InstallJob>, actions: AppActions) {
    val state = rememberCarouselState { apps.size }
    HorizontalMultiBrowseCarousel(
        state = state,
        preferredItemWidth = 244.dp,
        itemSpacing = 8.dp,
        minSmallItemWidth = 40.dp,
        maxSmallItemWidth = 76.dp,
        contentPadding = PaddingValues(horizontal = 16.dp),
        modifier = Modifier
            .fillMaxWidth()
            .height(156.dp),
    ) { index ->
        val item = apps[index]
        val info = carouselItemDrawInfo
        val range = info.maxSize - info.minSize
        val open = if (range <= 0f) 1f else ((info.size - info.minSize) / range).coerceIn(0f, 1f)
        FreshCard(
            item,
            jobs[item.app.key],
            actions,
            detailAlpha = ((open - 0.5f) * 2f).coerceIn(0f, 1f),
            mask = { info.maskRect },
            modifier = Modifier.maskClip(MaterialTheme.shapes.extraLarge),
        )
    }
}

/** A Fresh card: the app's colour, its icon, a white button, and what's new. */
@Composable
private fun FreshCard(
    item: AppItem,
    job: InstallJob?,
    actions: AppActions,
    detailAlpha: Float,
    mask: () -> Rect,
    modifier: Modifier = Modifier,
) {
    val app = item.app
    val hero = Tint.heroColors(Tint.appColor(app.name, app.tintColor))
    val base = Color(Tint.argb(hero.base))
    val deep = Color(Tint.argb(hero.deep))
    val change = Fresh.versionChange(app.state)
    val detail = when {
        change != null -> "${change.from} → ${change.to}"
        else -> listOfNotNull(app.state.target?.version, app.subtitle ?: app.developer).joinToString(" · ")
    }
    Box(
        modifier
            .fillMaxSize()
            .drawBehind {
                drawRect(Brush.linearGradient(listOf(base, deep), start = Offset.Zero, end = Offset(size.width, size.height * 1.2f)))
                // A soft disc of light at the top right, as on the web app's cards.
                val center = Offset(size.width * 0.8f, size.height * 0.15f)
                val far = max(hypot(center.x, size.height - center.y), hypot(center.x, center.y))
                drawCircle(Color.White.copy(alpha = 0.14f), radius = far * 0.34f, center = center)
            }
            .clickable { actions.show(app) },
    ) {
        // The carousel shows the middle of a small card: keep the icon at the visible part's start.
        AppIcon(
            app.name,
            app.iconUrl,
            app.tintColor,
            size = 46.dp,
            shadow = Color.Black.copy(alpha = 0.35f),
            modifier = Modifier.offset {
                val inset = 14.dp.toPx()
                val visible = mask()
                val x = if (visible.width <= 0f) inset else visible.left + ((visible.width - 46.dp.toPx()) / 2).coerceIn(0f, inset)
                IntOffset(x.roundToInt(), inset.roundToInt())
            },
        )
        Box(
            Modifier
                .align(Alignment.TopEnd)
                .padding(14.dp)
                .alpha(detailAlpha),
        ) {
            // Hidden on a small card: no tap may install from there.
            CardButton(appButton(app, job, item.local), base, actions, item, shown = detailAlpha > 0.5f)
        }
        Column(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = 16.dp, end = 16.dp, bottom = 14.dp)
                .alpha(detailAlpha),
        ) {
            Text(
                Fresh.label(app).uppercase(),
                color = Color.White,
                fontSize = 11.sp,
                fontWeight = FontWeight.Medium,
                letterSpacing = 0.6.sp,
                maxLines = 1,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(Color.White.copy(alpha = 0.2f))
                    .padding(horizontal = 8.dp, vertical = 3.dp),
            )
            Text(
                app.name,
                color = Color.White,
                fontSize = 21.sp,
                lineHeight = 26.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 5.dp),
            )
            Text(detail, color = Color.White.copy(alpha = 0.85f), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** The white button of a Fresh card, in the card's colour. */
@Composable
private fun CardButton(view: ButtonView, color: Color, actions: AppActions, item: AppItem, shown: Boolean) {
    val buttons = actions.buttons(item.app)
    val (label, onClick) = when (view) {
        is ButtonView.Install -> view.label to buttons.install
        is ButtonView.Open -> "Open" to { buttons.open(view.packageName) }
        is ButtonView.Remove -> "Remove" to { buttons.remove(view.packageName) }
        is ButtonView.AllowInstalls -> "Allow" to buttons.allowInstalls
        is ButtonView.Progress -> progressText(view) to buttons.cancel
        is ButtonView.Disabled -> view.label to {}
    }
    Surface(
        onClick = onClick,
        enabled = shown && view !is ButtonView.Disabled && (view !is ButtonView.Progress || view.cancellable),
        shape = CircleShape,
        color = Color.White,
        contentColor = color,
        modifier = Modifier.height(36.dp),
    ) {
        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
            if (view is ButtonView.Progress) {
                ProgressRing(view.fraction, color, size = 16.dp)
                Spacer(Modifier.width(8.dp))
            }
            Text(label, style = MaterialTheme.typography.labelLarge)
        }
    }
}

/** A screen's data could not be loaded. */
@Composable
fun LoadFailed(error: Throwable, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    val title = (error as? KiwiException)?.title() ?: "Something Went Wrong"
    MessagePane(title, error.userMessage(), modifier, icon = KiwiIcons.Warning, primary = "Try again" to onRetry)
}

@Preview(name = "C2 Browse")
@Composable
private fun BrowsePreview() {
    KiwiTheme(darkTheme = false) {
        BrowseContent(Load.Ready(PreviewData.browse), "", {}, PreviewData.freshKeys, emptyMap(), AppActions(), {}, {}, {})
    }
}

@Preview(name = "C2 Browse, dark")
@Composable
private fun BrowseDarkPreview() {
    KiwiTheme(darkTheme = true) {
        BrowseContent(Load.Ready(PreviewData.browse), "", {}, PreviewData.freshKeys, PreviewData.downloading, AppActions(), {}, {}, {})
    }
}
