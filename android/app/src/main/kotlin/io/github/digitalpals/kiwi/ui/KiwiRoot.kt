package io.github.digitalpals.kiwi.ui

import android.Manifest
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.List
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
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
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
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
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.MessagePane
import io.github.digitalpals.kiwi.ui.screens.AppScreen
import io.github.digitalpals.kiwi.ui.screens.BrowseScreen
import io.github.digitalpals.kiwi.ui.screens.ConnectScreen
import io.github.digitalpals.kiwi.ui.screens.MyAppsScreen
import io.github.digitalpals.kiwi.ui.screens.SettingsScreen
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

@Serializable
object BrowseRoute

@Serializable
object MyAppsRoute

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
    var changingServer by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(request) {
        val connect = request as? LaunchRequest.Connect ?: return@LaunchedEffect
        linkUrl = connect.url
        onRequestHandled()
    }

    when (val current = server) {
        ServerState.Loading -> Loading()
        is ServerState.Known -> {
            val saved = current.url
            if (saved == null || linkUrl != null || changingServer) {
                ConnectScreen(
                    container = container,
                    initialUrl = linkUrl ?: saved?.removeSuffix("/").orEmpty(),
                    fromLink = linkUrl != null,
                    onConnected = {
                        linkUrl = null
                        changingServer = false
                    },
                    onCancel = if (saved != null) {
                        {
                            linkUrl = null
                            changingServer = false
                        }
                    } else {
                        null
                    },
                )
            } else {
                key(saved) {
                    MainScreen(container, saved, request, onRequestHandled, onChangeServer = { changingServer = true })
                }
            }
        }
    }
}

/** The connected app: `/api/me` first, then Browse, My Apps and Settings. */
@Composable
private fun MainScreen(
    container: AppContainer,
    serverUrl: String,
    request: LaunchRequest?,
    onRequestHandled: () -> Unit,
    onChangeServer: () -> Unit,
) {
    val context = LocalContext.current
    val session: SessionViewModel = viewModel(key = "session:$serverUrl") { SessionViewModel(context, container) }
    val state by session.state.collectAsStateWithLifecycle()
    when (val load = state) {
        Load.Loading -> Loading()
        is Load.Failed -> {
            val error = load.error
            MessagePane(
                title = (error as? KiwiException)?.title() ?: "Something Went Wrong",
                message = error.userMessage(),
                primary = "Try Again" to session::reload,
                secondary = "Change Server" to onChangeServer,
            )
        }
        is Load.Ready -> Tabs(container, session, load.value, serverUrl, request, onRequestHandled, onChangeServer)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Tabs(
    container: AppContainer,
    session: SessionViewModel,
    me: io.github.digitalpals.kiwi.api.Me,
    serverUrl: String,
    request: LaunchRequest?,
    onRequestHandled: () -> Unit,
    onChangeServer: () -> Unit,
) {
    val context = LocalContext.current
    val nav = rememberNavController()
    val entry by nav.currentBackStackEntryAsState()
    val destination = entry?.destination
    val local = remember { LocalPackages(context, container) }
    AskForNotificationsOnce(container)

    LaunchedEffect(request) {
        if (request == LaunchRequest.MyApps) {
            nav.selectTab(MyAppsRoute)
            onRequestHandled()
        }
    }

    val onApp = destination?.hasRoute(AppRoute::class) == true
    val title = when {
        onApp -> entry?.toRoute<AppRoute>()?.name.orEmpty()
        destination?.hasRoute(MyAppsRoute::class) == true -> "My Apps"
        destination?.hasRoute(SettingsRoute::class) == true -> "Settings"
        else -> "Browse"
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(title) },
                navigationIcon = {
                    if (onApp) {
                        IconButton(onClick = { nav.popBackStack() }) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    }
                },
            )
        },
        bottomBar = {
            NavigationBar {
                Tab(nav, destination, BrowseRoute, "Browse", Icons.Filled.Search)
                Tab(nav, destination, MyAppsRoute, "My Apps", Icons.AutoMirrored.Filled.List, badge = me.counts.updates)
                Tab(nav, destination, SettingsRoute, "Settings", Icons.Filled.Settings)
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            NavHost(nav, startDestination = BrowseRoute) {
                val openApp: (io.github.digitalpals.kiwi.api.AppSummary) -> Unit = {
                    nav.navigate(AppRoute(it.sourceId, it.bundleId, it.name))
                }
                composable<BrowseRoute> {
                    val model: BrowseViewModel = viewModel { BrowseViewModel(container, local) }
                    BrowseScreen(container, model, openApp)
                }
                composable<MyAppsRoute> {
                    val model: MyAppsViewModel = viewModel { MyAppsViewModel(container, local) }
                    MyAppsScreen(container, model, openApp)
                }
                composable<SettingsRoute> {
                    SettingsScreen(container, session, me, serverUrl, onChangeServer)
                }
                composable<AppRoute> { backStack ->
                    val route = backStack.toRoute<AppRoute>()
                    val model: AppViewModel = viewModel { AppViewModel(container, local, route.sourceId, route.bundleId) }
                    AppScreen(container, model)
                }
            }
        }
    }
}

@Composable
private fun androidx.compose.foundation.layout.RowScope.Tab(
    nav: NavHostController,
    destination: androidx.navigation.NavDestination?,
    route: Any,
    label: String,
    icon: ImageVector,
    badge: Int = 0,
) {
    val selected = destination?.hierarchy?.any { it.hasRoute(route::class) } == true
    NavigationBarItem(
        selected = selected,
        onClick = { nav.selectTab(route) },
        icon = { Icon(icon, contentDescription = null) },
        label = { Text(if (badge > 0) "$label ($badge)" else label) },
    )
}

private fun NavHostController.selectTab(route: Any) {
    navigate(route) {
        popUpTo(graph.findStartDestination().id) { saveState = true }
        launchSingleTop = true
        restoreState = true
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
