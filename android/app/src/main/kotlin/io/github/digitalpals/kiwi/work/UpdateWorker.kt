package io.github.digitalpals.kiwi.work

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.KiwiApplication
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.install.InstallOutcome
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.first
import java.util.concurrent.TimeUnit

/**
 * Checks Kiwi for updates in the background, about every two hours while there is a network:
 * reports the installed packages, notifies new updates, and (with "Install updates
 * automatically") installs those Android lets this app install without asking. With "Notify
 * when a build finishes" it also notifies finished Kiwi Builds jobs.
 */
class UpdateWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val container = (applicationContext as KiwiApplication).container
        val kiwi = container.api() ?: return Result.success()
        return try {
            container.packageReporter.report()
            val updates = kiwi.updates().filter { it.platform == Platforms.ANDROID }
            container.settings.setLastChecked(System.currentTimeMillis())
            val autoUpdate = container.settings.autoUpdate.first()
            val installed = if (autoUpdate) installSilently(updates) else emptySet()
            notify(updates.filter { it.libraryKey !in installed })
            selfUpdate(autoUpdate)
            // A finished build of an installed app is already news as an update.
            checkBuilds(kiwi, updated = updates.map { it.packageName }.toSet())
            Result.success()
        } catch (error: CancellationException) {
            throw error
        } catch (error: KiwiException.Network) {
            // Tailscale is off or the Mac is asleep: try again at the next period.
            Log.i(TAG, "Kiwi unreachable: ${error.message}")
            Result.success()
        } catch (error: KiwiException.Http) {
            Log.w(TAG, "Update check failed: ${error.status} ${error.code}")
            if (error.status >= 500 && runAttemptCount < 3) Result.retry() else Result.success()
        } catch (error: KiwiException) {
            Log.w(TAG, "Update check failed", error)
            Result.success()
        }
    }

    /** Installs the updates Android allows without a confirmation. Returns their library keys. */
    private suspend fun installSilently(updates: List<AppSummary>): Set<String> {
        val container = (applicationContext as KiwiApplication).container
        val done = mutableSetOf<String>()
        for (app in updates) {
            if (!container.packageScanner.canUpdateSilently(app.packageName)) continue
            val outcome = try {
                container.installs.installInBackground(app)
            } catch (error: CancellationException) {
                if (isStopped) throw error
                // Took too long (a confirmation nobody answered): notify it instead.
                null
            }
            if (outcome == InstallOutcome.Success) done += app.libraryKey
            else if (outcome != null) Log.i(TAG, "Background update of ${app.name}: $outcome")
        }
        return done
    }

    private suspend fun notify(updates: List<AppSummary>) {
        val container = (applicationContext as KiwiApplication).container
        if (!container.settings.notifyUpdates.first() || !Notifications.allowed(applicationContext)) return
        val (notices, remember) = UpdateNotices.fresh(updates, container.settings.notified())
        Notifications.showUpdates(applicationContext, notices)
        container.settings.setNotified(remember)
    }

    /**
     * Notifies Kiwi Builds jobs that finished since the last look ("Notify when a build
     * finishes"), and looks again soon while a build is still running.
     */
    private suspend fun checkBuilds(kiwi: KiwiApi, updated: Set<String>) {
        val container = (applicationContext as KiwiApplication).container
        if (!container.settings.notifyBuilds.first() || !Notifications.allowed(applicationContext)) return
        val apps = try {
            kiwi.automation().apps
        } catch (error: KiwiException.Http) {
            if (error.isBuildsOff) return
            throw error
        }
        val (notices, seen) = BuildNotices.fresh(apps, container.settings.notifiedBuilds(), updated)
        Notifications.showBuilds(applicationContext, notices)
        container.settings.setNotifiedBuilds(seen)
        if (apps.any { app -> app.added && app.jobs.any { it.isActive } }) {
            // A follow-up run chains itself after this one; other callers keep a pending one.
            watchBuilds(applicationContext, chain = inputData.getBoolean(WATCH, false))
        }
    }

    /** Offers (or installs) a newer version of this app when Kiwi has one. */
    private suspend fun selfUpdate(autoUpdate: Boolean) {
        val container = (applicationContext as KiwiApplication).container
        val kiwi = container.api() ?: return
        val offer = kiwi.me().androidApp ?: return
        if (offer.packageName != applicationContext.packageName || offer.versionCode <= BuildConfig.VERSION_CODE) return
        if (autoUpdate && container.packageScanner.canUpdateSilently(applicationContext.packageName)) {
            // Android stops this process to replace the app; nothing runs after a success.
            runCatching { container.installs.selfUpdateInBackground(offer) }
            return
        }
        if (container.settings.notifyUpdates.first() && Notifications.allowed(applicationContext)) {
            val key = UpdateNotices.selfKey(offer.versionCode)
            val notified = container.settings.notified()
            if (key !in notified) {
                Notifications.showSelfUpdate(applicationContext, offer.versionName)
                container.settings.setNotified(notified + key)
            }
        }
    }

    companion object {
        private const val TAG = "KiwiUpdates"
        private const val PERIODIC = "kiwi-update-check"
        private const val NOW = "kiwi-update-check-now"
        private const val BUILD_WATCH = "kiwi-build-watch"
        private const val WATCH = "watch_builds"
        private const val BUILD_WATCH_MINUTES = 5L

        private val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()

        /** Schedules the periodic check (keeps an existing schedule). */
        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<UpdateWorker>(2, TimeUnit.HOURS, 30, TimeUnit.MINUTES)
                .setConstraints(constraints)
                .build()
            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, request)
        }

        /** Runs a check as soon as there is a network. */
        fun checkNow(context: Context) {
            val request = OneTimeWorkRequestBuilder<UpdateWorker>().setConstraints(constraints).build()
            WorkManager.getInstance(context).enqueueUniqueWork(NOW, ExistingWorkPolicy.REPLACE, request)
        }

        /**
         * Checks again in a few minutes while a Kiwi Builds job runs, so its build is notified (and
         * installed, where that needs no confirmation) soon after it finishes; the periodic check
         * comes only every two hours.
         */
        fun watchBuilds(context: Context, chain: Boolean = false) {
            val request = OneTimeWorkRequestBuilder<UpdateWorker>()
                .setConstraints(constraints)
                .setInitialDelay(BUILD_WATCH_MINUTES, TimeUnit.MINUTES)
                .setInputData(workDataOf(WATCH to true))
                .build()
            val policy = if (chain) ExistingWorkPolicy.APPEND_OR_REPLACE else ExistingWorkPolicy.KEEP
            WorkManager.getInstance(context).enqueueUniqueWork(BUILD_WATCH, policy, request)
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(PERIODIC)
            WorkManager.getInstance(context).cancelUniqueWork(NOW)
            WorkManager.getInstance(context).cancelUniqueWork(BUILD_WATCH)
        }
    }
}
