package io.github.digitalpals.kiwi.ui

import android.Manifest
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavDestination
import androidx.navigation.NavDestination.Companion.hasRoute
import androidx.navigation.NavDestination.Companion.hierarchy
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import androidx.navigation.toRoute
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.LaunchRequest
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.LocalSnackbars
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.screens.AppScreen
import io.github.digitalpals.kiwi.ui.screens.BrowseScreen
import io.github.digitalpals.kiwi.ui.screens.BuildsScreen
import io.github.digitalpals.kiwi.ui.screens.ConnectScreen
import io.github.digitalpals.kiwi.ui.screens.MyAppsScreen
import io.github.digitalpals.kiwi.ui.screens.SettingsScreen
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

@Serializable
object BrowseRoute

@Serializable
object MyAppsRoute

/** Builds; [edit] opens that app's patches and schedule (from its app page). */
@Serializable
data class BuildsRoute(val edit: String? = null)

@Serializable
object SettingsRoute

@Serializable
data class AppRoute(val sourceId: String, val bundleId: String, val name: String)

private sealed interface ServerState {
    data object Loading : ServerState
    data class Known(val url: String?) : ServerState
}

/** The whole app: Connect until a server is saved (or a link asks to connect), else the tabs. */
@Composable
fun KiwiRoot(container: AppContainer, requests: StateFlow<LaunchRequest?>, onRequestHandled: () -> Unit) {
    val server by remember { container.settings.serverUrl.map { ServerState.Known(it) } }
        .collectAsStateWithLifecycle(initialValue = ServerState.Loading)
    val request by requests.collectAsStateWithLifecycle()
    var linkUrl by rememberSaveable { mutableStateOf<String?>(null) }
    var showingSetup by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(request) {
        val connect = request as? LaunchRequest.Connect ?: return@LaunchedEffect
        linkUrl = connect.url
        onRequestHandled()
    }

    Surface(color = MaterialTheme.colorScheme.background, modifier = Modifier.fillMaxSize()) {
        when (val current = server) {
            ServerState.Loading -> Loading()
            is ServerState.Known -> {
                val saved = current.url
                if (saved == null || linkUrl != null || showingSetup) {
                    ConnectScreen(
                        container = container,
                        initialUrl = linkUrl ?: saved?.removeSuffix("/").orEmpty(),
                        fromLink = linkUrl != null,
                        onConnected = {
                            linkUrl = null
                            showingSetup = false
                        },
                        onCancel = if (saved != null) {
                            {
                                linkUrl = null
                                showingSetup = false
                            }
                        } else {
                            null
                        },
                    )
                } else {
                    key(saved) {
                        MainScreen(container, saved, request, onRequestHandled, onShowSetup = { showingSetup = true })
                    }
                }
            }
        }
    }
}

/** The connected app: `/api/me` first, then the tabs. */
@Composable
private fun MainScreen(
    container: AppContainer,
    serverUrl: String,
    request: LaunchRequest?,
    onRequestHandled: () -> Unit,
    onShowSetup: () -> Unit,
) {
    val session: SessionViewModel = viewModel(key = "session:$serverUrl") { SessionViewModel(container) }
    val state by session.state.collectAsStateWithLifecycle()
    when (val load = state) {
        Load.Loading -> Loading()
        is Load.Failed -> {
            val error = load.error
            MessagePane(
                title = (error as? KiwiException)?.title() ?: "Something Went Wrong",
                message = error.userMessage(),
                icon = KiwiIcons.Warning,
                primary = "Try again" to { session.reload() },
                secondary = "Change server" to onShowSetup,
            )
        }
        is Load.Ready -> Tabs(container, session, load.value, serverUrl, request, onRequestHandled, onShowSetup)
    }
}

@Composable
private fun Tabs(
    container: AppContainer,
    session: SessionViewModel,
    me: Me,
    serverUrl: String,
    request: LaunchRequest?,
    onRequestHandled: () -> Unit,
    onShowSetup: () -> Unit,
) {
    val context = LocalContext.current
    val nav = rememberNavController()
    val entry by nav.currentBackStackEntryAsState()
    val destination = entry?.destination
    val local = remember { LocalPackages(context, container) }
    val snackbars = remember { SnackbarHostState() }
    val actions = remember(nav) {
        SystemActions.appActions(context, container) { nav.navigate(AppRoute(it.sourceId, it.bundleId, it.name)) }
    }
    AskForNotificationsOnce(container)

    LaunchedEffect(request) {
        when (request) {
            LaunchRequest.MyApps -> nav.selectTab(MyAppsRoute)
            LaunchRequest.Builds -> nav.selectTab(BuildsRoute())
            is LaunchRequest.App -> {
                nav.selectTab(BrowseRoute)
                nav.navigate(AppRoute(request.sourceId, request.bundleId, request.name))
            }
            else -> return@LaunchedEffect
        }
        onRequestHandled()
    }

    CompositionLocalProvider(LocalSnackbars provides snackbars) {
        Scaffold(
            snackbarHost = { SnackbarHost(snackbars) },
            bottomBar = {
                KiwiNavigationBar(selectedTab(destination), badge = updateBadge(me)) { tab ->
                    nav.selectTab(
                        when (tab) {
                            KiwiTab.Browse -> BrowseRoute
                            KiwiTab.MyApps -> MyAppsRoute
                            KiwiTab.Builds -> BuildsRoute()
                            KiwiTab.Settings -> SettingsRoute
                        },
                    )
                }
            },
            // Each screen draws its own top bar under the status bar.
            contentWindowInsets = WindowInsets(0),
        ) { padding ->
            NavHost(nav, startDestination = BrowseRoute, modifier = Modifier.padding(padding)) {
                composable<BrowseRoute> {
                    val model: BrowseViewModel = viewModel { BrowseViewModel(container, local) }
                    BrowseScreen(
                        container,
                        model,
                        actions,
                        onOpenSettings = { nav.selectTab(SettingsRoute) },
                        onOpenBuilds = { nav.selectTab(BuildsRoute()) },
                    )
                }
                composable<MyAppsRoute> {
                    val model: MyAppsViewModel = viewModel { MyAppsViewModel(container, local) }
                    MyAppsScreen(container, model, actions, onBrowse = { nav.selectTab(BrowseRoute) })
                }
                composable<BuildsRoute> { backStack ->
                    val model: BuildsViewModel = viewModel { BuildsViewModel(container, local) }
                    var edit by rememberSaveable { mutableStateOf(backStack.toRoute<BuildsRoute>().edit) }
                    BuildsScreen(model, editRequest = edit, onEditHandled = { edit = null })
                }
                composable<SettingsRoute> {
                    SettingsScreen(container, session, me, serverUrl, onShowSetup)
                }
                composable<AppRoute> { backStack ->
                    val route = backStack.toRoute<AppRoute>()
                    val model: AppViewModel = viewModel { AppViewModel(container, local, route.sourceId, route.bundleId) }
                    AppScreen(
                        container,
                        model,
                        actions,
                        onBack = { nav.popBackStack() },
                        onEditBuild = { nav.selectTab(BuildsRoute(edit = it), restore = false) },
                    )
                }
            }
        }
    }
}

/** The four tabs of the navigation bar. */
enum class KiwiTab(val label: String, val icon: ImageVector, val selectedIcon: ImageVector) {
    Browse("Browse", KiwiIcons.Browse, KiwiIcons.BrowseSelected),
    MyApps("My Apps", KiwiIcons.MyApps, KiwiIcons.MyAppsSelected),
    Builds("Builds", KiwiIcons.Builds, KiwiIcons.BuildsSelected),
    Settings("Settings", KiwiIcons.Settings, KiwiIcons.SettingsSelected),
}

/** The tab of a destination; an app page belongs to Browse, where it was opened from. */
private fun selectedTab(destination: NavDestination?): KiwiTab? {
    val routes = destination?.hierarchy?.toList().orEmpty()
    return when {
        routes.any { it.hasRoute(BrowseRoute::class) || it.hasRoute(AppRoute::class) } -> KiwiTab.Browse
        routes.any { it.hasRoute(MyAppsRoute::class) } -> KiwiTab.MyApps
        routes.any { it.hasRoute(BuildsRoute::class) } -> KiwiTab.Builds
        routes.any { it.hasRoute(SettingsRoute::class) } -> KiwiTab.Settings
        else -> null
    }
}

/** Material 3's navigation bar: Browse, My Apps (with the number of updates), Builds, Settings. */
@Composable
fun KiwiNavigationBar(selected: KiwiTab?, badge: Int, onSelect: (KiwiTab) -> Unit) {
    NavigationBar {
        KiwiTab.entries.forEach { tab -> NavigationTab(tab, tab == selected, if (tab == KiwiTab.MyApps) badge else 0, onSelect) }
    }
}

@Composable
private fun RowScope.NavigationTab(tab: KiwiTab, selected: Boolean, badge: Int, onSelect: (KiwiTab) -> Unit) {
    NavigationBarItem(
        selected = selected,
        onClick = { onSelect(tab) },
        icon = {
            BadgedBox(badge = { if (badge > 0) Badge { Text(badge.toString()) } }) {
                Icon(if (selected) tab.selectedIcon else tab.icon, contentDescription = null)
            }
        },
        label = { Text(tab.label, fontWeight = if (selected) FontWeight.Bold else FontWeight.Medium) },
    )
}

/** Switches tabs, keeping each tab's state; [restore] false opens the tab's route as given. */
private fun NavHostController.selectTab(route: Any, restore: Boolean = true) {
    navigate(route) {
        popUpTo(graph.findStartDestination().id) { saveState = restore }
        launchSingleTop = true
        restoreState = restore
    }
}

/** Asks for the notification permission (Android 13+) once, after connecting. */
@Composable
private fun AskForNotificationsOnce(container: AppContainer) {
    if (Build.VERSION.SDK_INT < 33) return
    val scope = rememberCoroutineScope()
    val asked by container.settings.askedNotifications.collectAsStateWithLifecycle(initialValue = true)
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    LaunchedEffect(asked) {
        if (!asked) {
            scope.launch { container.settings.setAskedNotifications() }
            launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}
