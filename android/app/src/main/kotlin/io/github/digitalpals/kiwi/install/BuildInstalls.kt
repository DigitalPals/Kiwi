package io.github.digitalpals.kiwi.install

import android.util.Log
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.ui.Community
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.PendingOutcome
import io.github.digitalpals.kiwi.work.BuildNotice
import io.github.digitalpals.kiwi.work.BuildNotices
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Installs Kiwi Builds apps (community apps too, by their `community-<package>` id) once their
 * build is ready, for "Build and install". The wish is
 * saved on the phone ([pending], SettingsStore.pendingInstalls) so it survives the app being
 * closed. While Kiwi is in the [foreground], this looks every few seconds and starts the
 * install of a ready build (Android asks to confirm it). In the background, UpdateWorker
 * notifies "<App> is ready — tap to install" instead: Android doesn't let an app open the
 * installer from there, and tapping the notification brings Kiwi to the foreground.
 *
 * A wish ends when the install starts, the build fails ([failed] gets its notice), or the
 * app was removed or its build cancelled.
 */
class BuildInstalls(
    private val scope: CoroutineScope,
    private val pending: Flow<Set<String>>,
    /** Forgets the wish of an app (SettingsStore.removePendingInstall). */
    private val forget: suspend (String) -> Unit,
    private val api: suspend () -> KiwiApi?,
    /** Starts installing the published build of a Kiwi Builds app (by app id; InstallManager.start). */
    private val install: (String, AppSummary) -> Unit,
    /** Whether a package is on the phone. */
    private val isInstalled: (String) -> Boolean,
    private val foreground: Flow<Boolean>,
    /** A build the user waited for failed: notify it where notifications are allowed. */
    private val failed: (BuildNotice) -> Unit,
    private val pollMs: Long = POLL_MS,
) {
    private val lock = Mutex()

    /** Watches while there is something to wait for and Kiwi is in the foreground. */
    @OptIn(ExperimentalCoroutinesApi::class)
    fun start() {
        scope.launch {
            combine(pending, foreground) { apps, visible -> apps.isNotEmpty() && visible }
                .distinctUntilChanged()
                .mapLatest { watching ->
                    while (watching) {
                        check()
                        delay(pollMs)
                    }
                }
                .collect {}
        }
    }

    /** Looks once at the apps waiting to be installed and acts on those whose build ended. */
    suspend fun check() {
        lock.withLock { checkLocked() }
    }

    private suspend fun checkLocked() {
        val waiting = pending.first()
        if (waiting.isEmpty()) return
        val kiwi = api() ?: return
        try {
            val loaded = try {
                if (waiting.any(Community::isId)) kiwi.buildApps() else BuildApps(kiwi.automation().apps, communityKnown = false)
            } catch (error: KiwiException.Http) {
                // Turning Kiwi Builds off cancelled the builds.
                if (!error.isBuildsOff) throw error
                waiting.forEach { forget(it) }
                return
            }
            val apps = KiwiBuilds.androidApps(loaded.apps)
            val ended = apps.filter { it.id in waiting && KiwiBuilds.activeJob(it) == null }
            val published = if (ended.any { it.published != null }) catalogue(kiwi) else emptyMap()
            for (id in waiting) {
                val app = apps.firstOrNull { it.id == id }
                // The community list couldn't be read now: look again later.
                if (app == null && Community.isId(id) && !loaded.communityKnown) continue
                if (app == null) {
                    forget(id)
                    continue
                }
                val summary = published[app.bundleId]
                when (val outcome = KiwiBuilds.pendingOutcome(app, installable = summary != null)) {
                    PendingOutcome.Waiting -> {}
                    PendingOutcome.Ready -> {
                        forget(id)
                        val ready = summary ?: continue
                        if (KiwiBuilds.needsInstall(ready, isInstalled(ready.packageName))) install(id, ready)
                    }
                    is PendingOutcome.Failed -> {
                        forget(id)
                        failed(BuildNotices.notice(app, outcome.job))
                    }
                    PendingOutcome.Dropped -> forget(id)
                }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (error: KiwiException) {
            // Kiwi is unreachable for now: look again later.
            Log.i(TAG, "Couldn't check pending installs: ${error.message}")
        }
    }

    /**
     * Kiwi Builds' Android apps in Kiwi's catalogue, by bundle id. Community builds are published
     * to the same source.
     */
    private suspend fun catalogue(kiwi: KiwiApi): Map<String, AppSummary> =
        kiwi.apps(hidden = true)
            .filter { it.sourceId == KiwiBuilds.SOURCE_ID && it.platform == Platforms.ANDROID }
            .associateBy { it.bundleId }

    companion object {
        private const val TAG = "KiwiBuildInstalls"
        const val POLL_MS = 5_000L
    }
}
