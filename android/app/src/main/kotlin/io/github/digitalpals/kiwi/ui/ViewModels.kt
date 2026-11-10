package io.github.digitalpals.kiwi.ui

import android.app.Application
import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppDetail
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.api.ServerUrl
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.work.UpdateWorker
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** Data a screen loads from Kiwi. */
sealed interface Load<out T> {
    data object Loading : Load<Nothing>
    data class Ready<T>(val value: T, val refreshing: Boolean = false) : Load<T>
    data class Failed(val error: Throwable) : Load<Nothing>
}

/** An app with what the phone knows about its package. */
data class AppItem(val app: AppSummary, val local: LocalPackage)

/** Looks up packages on the phone for the app buttons. */
class LocalPackages(context: Context, private val container: AppContainer) {
    private val pm = context.applicationContext.packageManager

    fun of(packageName: String): LocalPackage {
        val installed = container.packageScanner.isInstalled(packageName)
        return LocalPackage(installed, installed && pm.getLaunchIntentForPackage(packageName) != null)
    }

    suspend fun items(apps: List<AppSummary>): List<AppItem> = withContext(Dispatchers.IO) {
        apps.map { AppItem(it, of(it.packageName)) }
    }
}

/** Shared loading code: one value, reloaded on demand and after installs or package changes. */
abstract class LoadingViewModel<T>(protected val container: AppContainer) : ViewModel() {
    private val _state = MutableStateFlow<Load<T>>(Load.Loading)
    val state: StateFlow<Load<T>> = _state

    protected abstract suspend fun fetch(kiwi: KiwiApi): T

    init {
        viewModelScope.launch {
            combine(container.installs.finished, container.packageReporter.changes) { a, b -> a to b }
                .drop(1)
                .collect { reload() }
        }
    }

    fun reload() {
        viewModelScope.launch {
            _state.update { if (it is Load.Ready) it.copy(refreshing = true) else Load.Loading }
            _state.value = try {
                val kiwi = container.api() ?: throw KiwiException.BadResponse("Connect to your Kiwi first.")
                Load.Ready(fetch(kiwi))
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                Load.Failed(error)
            }
        }
    }
}

class BrowseViewModel(container: AppContainer, private val local: LocalPackages) : LoadingViewModel<List<AppItem>>(container) {
    val query = MutableStateFlow("")

    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): List<AppItem> = local.items(kiwi.apps(query.value))
}

class MyAppsViewModel(container: AppContainer, private val local: LocalPackages) :
    LoadingViewModel<MyAppsViewModel.Content>(container) {

    data class Content(val updates: List<AppItem>, val installed: List<AppItem>, val selfUpdate: AndroidApp?)

    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): Content {
        val updates = kiwi.updates()
        val library = kiwi.library()
        val me = kiwi.me()
        return Content(local.items(updates), local.items(library), selfUpdateOffer(me))
    }

    fun updateAll(updates: List<AppItem>) {
        updates.forEach { container.installs.start(it.app) }
    }
}

class AppViewModel(
    container: AppContainer,
    private val local: LocalPackages,
    private val sourceId: String,
    private val bundleId: String,
) : LoadingViewModel<Pair<AppDetail, LocalPackage>>(container) {
    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): Pair<AppDetail, LocalPackage> {
        val detail = kiwi.app(sourceId, bundleId)
        return detail to withContext(Dispatchers.IO) { local.of(detail.summary.packageName) }
    }
}

/** A newer Kiwi app the server offers, or null. */
fun selfUpdateOffer(me: Me): AndroidApp? =
    me.androidApp?.takeIf { it.versionCode > BuildConfig.VERSION_CODE && it.packageName == BuildConfig.APPLICATION_ID }

/**
 * The signed-in state of the main screens: `/api/me`, loaded first. Once it works, the app
 * reports its packages, finishes onboarding on a Kiwi that has it, and schedules update checks.
 */
class SessionViewModel(context: Context, container: AppContainer) : LoadingViewModel<Me>(container) {
    private val application = context.applicationContext as Application
    private var started = false

    init {
        reload()
    }

    override suspend fun fetch(kiwi: KiwiApi): Me {
        var me = kiwi.me()
        if (!started) {
            started = true
            viewModelScope.launch { container.packageReporter.report() }
            UpdateWorker.schedule(application)
            // The native app is set up by connecting; the web app's onboarding is not needed.
            if (me.onboarded == false) {
                me = runCatching { kiwi.patchMe(buildJsonObject { put("onboarded", JsonPrimitive(true)) }) }.getOrDefault(me)
            }
        }
        return me
    }

    fun setAutoUpdate(enabled: Boolean) = viewModelScope.launch { container.settings.setAutoUpdate(enabled) }

    fun setNotifyUpdates(enabled: Boolean) = viewModelScope.launch { container.settings.setNotifyUpdates(enabled) }

    fun disconnect() = viewModelScope.launch {
        UpdateWorker.cancel(application)
        container.settings.setServerUrl(null)
    }
}

/** Connecting to a Kiwi: checks `/api/health`, then `/api/me`, then saves the address. */
class ConnectViewModel(private val container: AppContainer, initial: String) : ViewModel() {
    data class Problem(val title: String, val message: String)

    data class State(val input: String, val busy: Boolean = false, val problem: Problem? = null)

    private val _state = MutableStateFlow(State(initial))
    val state: StateFlow<State> = _state

    fun setInput(value: String) {
        _state.update { it.copy(input = value, problem = null) }
    }

    fun connect(onConnected: (String) -> Unit) {
        val parsed = ServerUrl.parse(_state.value.input)
        if (parsed is ServerUrl.Parsed.Invalid) {
            _state.update { it.copy(problem = Problem("Check the Address", parsed.message)) }
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
                _state.update { it.copy(busy = false) }
                onConnected(url.toString())
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
                "Check the Address",
                "Use your Kiwi's address exactly as tailscale serve shows it, like https://mac.example.ts.net.",
            )
            else -> Problem(error.title(), error.userMessage())
        }
    }
}
