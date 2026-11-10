package io.github.digitalpals.kiwi.ui

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppDetail
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.api.ServerUrl
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.install.InstallManager
import io.github.digitalpals.kiwi.work.UpdateWorker
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** Data a screen loads from Kiwi. */
sealed interface Load<out T> {
    data object Loading : Load<Nothing>

    /** Loaded; [error] when the last refresh failed (the value is the previous one). */
    data class Ready<T>(val value: T, val refreshing: Boolean = false, val error: Throwable? = null) : Load<T>

    data class Failed(val error: Throwable) : Load<Nothing>
}

/** An app with what the phone knows about its package. */
data class AppItem(val app: AppSummary, val local: LocalPackage)

/** Looks up packages on the phone for the app buttons, and makes icon URLs absolute. */
class LocalPackages(context: Context, private val container: AppContainer) {
    private val pm = context.applicationContext.packageManager

    fun of(packageName: String): LocalPackage {
        val installed = container.packageScanner.isInstalled(packageName)
        return LocalPackage(
            installed = installed,
            launchable = installed && pm.getLaunchIntentForPackage(packageName) != null,
            silentUpdate = installed && container.packageScanner.canUpdateSilently(packageName),
        )
    }

    suspend fun items(apps: List<AppSummary>): List<AppItem> = withContext(Dispatchers.IO) {
        apps.map { AppItem(resolved(it), of(it.packageName)) }
    }

    fun resolved(app: AppSummary): AppSummary = app.copy(iconUrl = app.iconUrl?.let(::icon))

    fun icon(url: String): String = container.resolveUrl(url) ?: url
}

/** Shared loading code: one value, reloaded on demand and after installs or package changes. */
abstract class LoadingViewModel<T>(protected val container: AppContainer) : ViewModel() {
    private val _state = MutableStateFlow<Load<T>>(Load.Loading)
    val state: StateFlow<Load<T>> = _state
    private var loading: Job? = null

    protected abstract suspend fun fetch(kiwi: KiwiApi): T

    init {
        viewModelScope.launch {
            combine(container.installs.finished, container.packageReporter.changes) { a, b -> a to b }
                .drop(1)
                .collect { reload(silent = true) }
        }
    }

    /**
     * Loads again. A failed refresh keeps what was loaded and reports the error with it;
     * [silent] refreshes (polling, package changes) don't show the refresh indicator.
     */
    fun reload(silent: Boolean = false) {
        loading?.cancel()
        loading = viewModelScope.launch {
            if (!silent) _state.update { if (it is Load.Ready) it.copy(refreshing = true, error = null) else Load.Loading }
            val result = try {
                val kiwi = container.api() ?: throw KiwiException.BadResponse("Connect to your Kiwi first.")
                Result.success(fetch(kiwi))
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                Result.failure(error)
            }
            _state.update { current ->
                result.fold(
                    onSuccess = { Load.Ready(it) },
                    onFailure = { error ->
                        if (current is Load.Ready) current.copy(refreshing = false, error = error) else Load.Failed(error)
                    },
                )
            }
        }
    }
}

@OptIn(FlowPreview::class)
class BrowseViewModel(container: AppContainer, private val local: LocalPackages) : LoadingViewModel<List<AppItem>>(container) {
    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query

    /** The keys of the "Fresh this week" cards, picked once so a card stays put while it installs. */
    private val _fresh = MutableStateFlow<List<String>?>(null)
    val fresh: StateFlow<List<String>?> = _fresh

    init {
        reload()
        viewModelScope.launch {
            _query.drop(1).debounce(SEARCH_DELAY_MS).distinctUntilChanged().collect { reload(silent = true) }
        }
    }

    fun setQuery(value: String) {
        _query.value = value
    }

    override suspend fun fetch(kiwi: KiwiApi): List<AppItem> {
        val query = _query.value
        val items = local.items(kiwi.apps(query))
        if (query.isBlank() && _fresh.value == null) _fresh.value = Fresh.apps(items.map { it.app }).map { it.key }
        return items
    }

    private companion object {
        const val SEARCH_DELAY_MS = 300L
    }
}

class MyAppsViewModel(container: AppContainer, private val local: LocalPackages) :
    LoadingViewModel<MyAppsViewModel.Content>(container) {

    data class Content(
        val updates: List<AppItem>,
        /** Installed apps without an update. */
        val installed: List<AppItem>,
        /** Kiwi apps installed on this phone with another signature. */
        val foreign: List<AppItem>,
        val selfUpdate: AndroidApp?,
    ) {
        val isEmpty: Boolean get() = updates.isEmpty() && installed.isEmpty() && foreign.isEmpty() && selfUpdate == null
    }

    val lastChecked: Flow<Long?> = container.settings.lastChecked

    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): Content = coroutineScope {
        val updates = async { kiwi.updates() }
        val library = async { kiwi.library() }
        val me = async { kiwi.me() }
        val apps = async {
            try {
                kiwi.apps()
            } catch (_: KiwiException) {
                emptyList()
            }
        }
        val updateItems = local.items(updates.await())
        val withUpdate = updateItems.map { it.app.libraryKey }.toSet()
        val installed = local.items(library.await().filter { it.libraryKey !in withUpdate })
        val foreign = local.items(apps.await().filter { it.state.foreignInstall && it.platform == Platforms.ANDROID })
            .filter { it.local.installed }
        val content = Content(updateItems, installed, foreign, selfUpdateOffer(me.await()))
        container.settings.setLastChecked(System.currentTimeMillis())
        content
    }

    /**
     * Starts every update that isn't being installed already; Kiwi's own update once they have
     * all finished, since installing it restarts the app.
     */
    fun updateAll(content: Content) {
        container.installs.startAll(content.updates.map { it.app }, content.selfUpdate)
    }
}

/** An app's page: the app, its package on the phone, and its Kiwi Builds recipe when it has one. */
data class AppPage(val detail: AppDetail, val local: LocalPackage, val recipe: AutomationState? = null)

class AppViewModel(
    container: AppContainer,
    private val local: LocalPackages,
    private val sourceId: String,
    private val bundleId: String,
) : LoadingViewModel<AppPage>(container) {
    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): AppPage = coroutineScope {
        val recipe = async { if (sourceId == KiwiBuilds.SOURCE_ID) recipe(kiwi) else null }
        val detail = kiwi.app(sourceId, bundleId).let { it.copy(iconUrl = it.iconUrl?.let(local::icon)) }
        val packageState = withContext(Dispatchers.IO) { local.of(detail.summary.packageName) }
        AppPage(detail, packageState, recipe.await()?.takeIf { detail.platform == Platforms.ANDROID })
    }

    /** The Kiwi Builds recipe of this app; null when Kiwi Builds is off or unreachable. */
    private suspend fun recipe(kiwi: KiwiApi): AutomationState? = try {
        KiwiBuilds.recipeFor(kiwi.automation().apps, bundleId)
    } catch (_: KiwiException) {
        null
    }
}

/** A newer Kiwi app the server offers, or null. */
fun selfUpdateOffer(me: Me): AndroidApp? =
    me.androidApp?.takeIf { it.versionCode > BuildConfig.VERSION_CODE && it.packageName == BuildConfig.APPLICATION_ID }

/** My Apps' badge: Kiwi's update count plus this app's own update. */
fun updateBadge(me: Me): Int = me.counts.updates + if (selfUpdateOffer(me) != null) 1 else 0

/**
 * The signed-in state of the main screens: `/api/me`, loaded first. Once it works, the app
 * reports its packages, finishes onboarding on a Kiwi that has it, and schedules update checks.
 */
class SessionViewModel(container: AppContainer) : LoadingViewModel<Me>(container) {
    private var started = false

    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): Me {
        var me = kiwi.me()
        if (!started) {
            started = true
            viewModelScope.launch { container.packageReporter.report() }
            UpdateWorker.schedule(container.application)
            // The native app is set up by connecting; the web app's onboarding is not needed.
            if (me.onboarded == false) {
                me = try {
                    kiwi.patchMe(buildJsonObject { put("onboarded", JsonPrimitive(true)) })
                } catch (_: KiwiException) {
                    me
                }
            }
        }
        return me
    }

    fun setAutoUpdate(enabled: Boolean) = viewModelScope.launch { container.settings.setAutoUpdate(enabled) }

    fun disconnect() = viewModelScope.launch {
        UpdateWorker.cancel(container.application)
        container.settings.setServerUrl(null)
    }
}

/**
 * Kiwi Builds' Android apps (`/api/automation`): adding, removing, patches and schedules, builds.
 * While Kiwi Builds is off, [Content.off] is true and nothing else is loaded.
 */
class BuildsViewModel(container: AppContainer, private val local: LocalPackages) :
    LoadingViewModel<BuildsViewModel.Content>(container) {

    data class Content(
        val off: Boolean,
        val apps: List<AutomationState>,
        /** The apps' colours (`tint_color` of their catalogue entries) by bundle id. */
        val tints: Map<String, String> = emptyMap(),
    ) {
        val added: List<AutomationState> get() = apps.filter { it.added }
        val available: List<AutomationState> get() = apps.filter { !it.added }
        val active: Boolean get() = apps.any { KiwiBuilds.activeJob(it) != null }
    }

    private val _busy = MutableStateFlow<Set<String>>(emptySet())

    /** Apps (by id, or [ALL]) with a request on its way. */
    val busy: StateFlow<Set<String>> = _busy

    private val _messages = Channel<String>(Channel.BUFFERED)

    /** Errors and confirmations for a snackbar. */
    val messages: Flow<String> = _messages.receiveAsFlow()

    val notifyBuilds: Flow<Boolean> = container.settings.notifyBuilds

    init {
        reload()
    }

    /** Tints looked up once from Browse's apps; the automation API has none. */
    private var tints: Map<String, String> = emptyMap()
    private var looked: Set<String> = emptySet()

    override suspend fun fetch(kiwi: KiwiApi): Content = try {
        val apps = KiwiBuilds.androidApps(kiwi.automation().apps).map { it.copy(iconUrl = it.iconUrl?.let(local::icon)) }
        val added = apps.filter { it.added }.map { it.bundleId }.toSet()
        if (!looked.containsAll(added)) {
            tints = appTints(kiwi)
            looked = looked + added
        }
        Content(off = false, apps = apps, tints = tints)
    } catch (error: KiwiException.Http) {
        if (!error.isBuildsOff) throw error
        Content(off = true, apps = emptyList())
    }

    private suspend fun appTints(kiwi: KiwiApi): Map<String, String> = try {
        kiwi.apps().filter { it.sourceId == KiwiBuilds.SOURCE_ID }
            .mapNotNull { app -> app.tintColor?.let { app.bundleId to it } }.toMap()
    } catch (_: KiwiException) {
        tints
    }

    fun add(app: AutomationState, preferences: AutomationPreferences, onAdded: () -> Unit = {}) =
        act(app.id, "Added ${app.name}. Its first build is on its way.", watch = true) {
            it.addAutomation(app.id, preferences)
            onAdded()
        }

    fun save(app: AutomationState, preferences: AutomationPreferences) =
        act(app.id, "Saved. The next build of ${app.name} uses these settings.") { it.updateAutomation(app.id, preferences) }

    fun remove(app: AutomationState) = act(app.id, "Removed ${app.name} from Kiwi Builds.") { it.removeAutomation(app.id) }

    fun check(app: AutomationState, force: Boolean = false) = act(app.id, null, watch = true) { it.checkAutomation(app.id, force) }

    fun cancel(app: AutomationState) = act(app.id, null) { it.cancelAutomation(app.id) }

    /** "Build now": checks every added app that has no job yet. */
    fun checkAll(apps: List<AutomationState>) = act(ALL, null, watch = true) { kiwi ->
        apps.filter { it.added && KiwiBuilds.activeJob(it) == null }.forEach { kiwi.checkAutomation(it.id) }
    }

    /** Turns on Kiwi Builds (`PATCH /api/settings`), after the user confirmed what it does. */
    fun turnOn() = act(ALL, null) { it.patchSettings(buildJsonObject { put("kiwi_builds", JsonPrimitive(true)) }) }

    fun setNotifyBuilds(enabled: Boolean) = viewModelScope.launch { container.settings.setNotifyBuilds(enabled) }

    private fun act(id: String, done: String?, watch: Boolean = false, block: suspend (KiwiApi) -> Unit) {
        if (id in _busy.value) return
        _busy.update { it + id }
        viewModelScope.launch {
            try {
                val kiwi = container.api() ?: return@launch
                block(kiwi)
                if (done != null) _messages.send(done)
                if (watch) UpdateWorker.watchBuilds(container.application)
            } catch (error: CancellationException) {
                throw error
            } catch (error: KiwiException) {
                _messages.send(error.userMessage())
            } finally {
                _busy.update { it - id }
                reload(silent = true)
            }
        }
    }

    companion object {
        const val ALL = "*"
    }
}

/**
 * Connecting to a Kiwi: checks `/api/health`, then `/api/me`, then saves the address. The result
 * is part of [state] ([State.connected]), not a callback, so a connect that finishes while the
 * screen is recreated (a rotation) still reaches the screen that is shown then.
 */
class ConnectViewModel(private val container: AppContainer, initial: String) : ViewModel() {
    data class Problem(val title: String, val message: String, val network: Boolean = false)

    data class State(
        val input: String,
        val busy: Boolean = false,
        val problem: Problem? = null,
        /** The saved address once connecting worked, until the screen has handled it. */
        val connected: String? = null,
    )

    private val _state = MutableStateFlow(State(initial))
    val state: StateFlow<State> = _state

    fun setInput(value: String) {
        _state.update { it.copy(input = value, problem = null) }
    }

    /** The screen has left for the connected app (see [State.connected]). */
    fun connectedHandled() {
        _state.update { it.copy(connected = null) }
    }

    fun connect() {
        val parsed = ServerUrl.parse(_state.value.input)
        if (parsed is ServerUrl.Parsed.Invalid) {
            _state.update { it.copy(problem = Problem("Check the address", parsed.message)) }
            return
        }
        val url = (parsed as ServerUrl.Parsed.Ok).url
        _state.update { it.copy(busy = true, problem = null) }
        viewModelScope.launch {
            try {
                val kiwi = container.apiFor(url)
                kiwi.health()
                kiwi.me()
                container.settings.setServerUrl(url.toString())
                _state.update { it.copy(busy = false, connected = url.toString()) }
            } catch (error: CancellationException) {
                throw error
            } catch (error: KiwiException) {
                _state.update { it.copy(busy = false, problem = problemFor(error)) }
            }
        }
    }

    companion object {
        fun problemFor(error: KiwiException): Problem = when {
            error is KiwiException.Http && error.code == KiwiException.BAD_HOST -> Problem(
                "Check the address",
                "Use your Kiwi's address exactly as tailscale serve shows it, like https://mac.example.ts.net.",
            )
            else -> Problem(error.title(), error.userMessage(), network = error is KiwiException.Network)
        }
    }
}

/** The install key of Kiwi's own update, for screens that show its progress. */
const val SELF_UPDATE_KEY = InstallManager.SELF
