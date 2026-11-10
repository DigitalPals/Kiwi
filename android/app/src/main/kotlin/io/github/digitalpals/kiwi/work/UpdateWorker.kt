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
 * reports Kiwi's packages, notifies new updates, and (with "Install updates automatically")
 * installs those Android lets this app install without asking, on an unmetered network with a
 * battery that isn't low ([BackgroundConditions]); otherwise they are notified like the others.
 * With "Notify when a build finishes" it also notifies finished Kiwi Builds jobs.
 */
class UpdateWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    private val container get() = (applicationContext as KiwiApplication).container

    override suspend fun doWork(): Result {
        val kiwi = container.api() ?: return Result.success()
        return try {
            container.packageReporter.report()
            val autoUpdate = container.settings.autoUpdate.first()
            val installed = if (autoUpdate) installSilently(kiwi) else emptyList()
            val done = installed.map { it.libraryKey }.toSet()
            // Read after installing: what is left is notified.
            val updates = kiwi.updates().filter { it.platform == Platforms.ANDROID && it.libraryKey !in done }
            container.settings.setLastChecked(System.currentTimeMillis())
            Notifications.cancelUpdates(applicationContext, installed.map(UpdateNotices::tag))
            notify(updates)
            // A finished build of an installed app is already news as an update (or installed).
            checkBuilds(kiwi, updated = (updates + installed).map { it.packageName }.toSet())
            // Last: a silent self-update stops this process.
            selfUpdate(kiwi, autoUpdate)
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

    /**
     * Installs the updates Android allows without a confirmation, while [BackgroundConditions]
     * hold. One worker installs at a time; another one waits and then reads the updates that
     * are left. Returns the apps it updated.
     */
    private suspend fun installSilently(kiwi: KiwiApi): List<AppSummary> = container.installs.backgroundInstalls {
        val done = mutableListOf<AppSummary>()
        for (app in kiwi.updates().filter { it.platform == Platforms.ANDROID }) {
            if (!BackgroundConditions.met(applicationContext)) break
            if (!container.packageScanner.canUpdateSilently(app.packageName)) continue
            when (val outcome = attempt { container.installs.installInBackground(app) }) {
                InstallOutcome.Success -> done += app
                null -> {}
                else -> Log.i(TAG, "Background update of ${app.name}: $outcome")
            }
        }
        done
    }

    /**
     * Runs one background install. Null when it was skipped, cancelled (from the app's button)
     * or took too long (a confirmation nobody answered): then the update is notified instead.
     */
    private suspend fun attempt(install: suspend () -> InstallOutcome?): InstallOutcome? = try {
        install()
    } catch (error: CancellationException) {
        if (isStopped) throw error
        null
    }

    private suspend fun notify(updates: List<AppSummary>) {
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

    /**
     * Installs a newer version of this app when Kiwi has one and Android allows it without a
     * confirmation (see PackageScanner.canUpdateSilently), else notifies it once per version.
     */
    private suspend fun selfUpdate(kiwi: KiwiApi, autoUpdate: Boolean) {
        val own = applicationContext.packageName
        val offer = kiwi.me().androidApp ?: return
        if (offer.packageName != own || offer.versionCode <= BuildConfig.VERSION_CODE) return
        if (autoUpdate && container.packageScanner.canUpdateSilently(own)) {
            val outcome = container.installs.backgroundInstalls {
                if (BackgroundConditions.met(applicationContext)) attempt { container.installs.selfUpdateInBackground(offer) } else null
            }
            // Android stops this process to replace the app: a success never gets here.
            if (outcome == InstallOutcome.Success) return
            if (outcome != null) Log.i(TAG, "Background self-update: $outcome")
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
        private const val BUILD_WATCH = "kiwi-build-watch"
        private const val WATCH = "watch_builds"
        private const val BUILD_WATCH_MINUTES = 5L

        /** Any network: checking and notifying work on mobile data too (installs check more). */
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
            WorkManager.getInstance(context).cancelUniqueWork(BUILD_WATCH)
        }
    }
}
