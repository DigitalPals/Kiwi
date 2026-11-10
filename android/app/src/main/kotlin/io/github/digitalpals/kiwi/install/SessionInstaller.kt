package io.github.digitalpals.kiwi.install

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.IntentCompat
import io.github.digitalpals.kiwi.util.ForegroundTracker
import io.github.digitalpals.kiwi.work.Notifications
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * Installs an APK with a PackageInstaller session.
 *
 * On Android 12 and later the session asks for no confirmation (`USER_ACTION_NOT_REQUIRED`);
 * Android honours that for updates of apps whose installer of record is this app (Kiwi holds
 * `UPDATE_PACKAGES_WITHOUT_USER_ACTION`). On Android 14 and later a first install also makes
 * Kiwi the app's update owner (`setRequestUpdateOwnership`). Otherwise the session reports
 * `STATUS_PENDING_USER_ACTION` with Android's confirmation screen, which is shown right away
 * while Kiwi is on screen, or offered as a notification while it is in the background.
 */
class SessionInstaller(context: Context) {
    private val context = context.applicationContext
    private val installer: PackageInstaller = this.context.packageManager.packageInstaller

    /** Whether Android lets this app install apps ("Install unknown apps"). */
    fun canInstall(): Boolean = context.packageManager.canRequestPackageInstalls()

    /** Installs [apk] as [packageName] and waits for Android's verdict. */
    suspend fun install(apk: File, packageName: String, appName: String): InstallOutcome {
        val sessionId = withContext(Dispatchers.IO) { createSession(apk, packageName) }
        val result = CompletableDeferred<InstallOutcome>()
        pending[sessionId] = Pending(appName, result)
        try {
            withContext(Dispatchers.IO) { commit(sessionId) }
            return result.await()
        } catch (error: Throwable) {
            withContext(NonCancellable) { abandon(sessionId) }
            throw error
        } finally {
            pending.remove(sessionId)
            Notifications.cancelConfirmation(context, sessionId)
        }
    }

    private fun createSession(apk: File, packageName: String): Int {
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(packageName)
            setSize(apk.length())
            setInstallReason(PackageManager.INSTALL_REASON_USER)
            if (Build.VERSION.SDK_INT >= 31) {
                setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
            if (Build.VERSION.SDK_INT >= 33) {
                // Kiwi is where the owner gets these apps, like a store; Android then doesn't treat
                // them as sideloaded files (restricted settings).
                setPackageSource(PackageInstaller.PACKAGE_SOURCE_STORE)
            }
            if (Build.VERSION.SDK_INT >= 34) {
                // Android 14+: Kiwi becomes the app's update owner on its first install (a no-op for
                // updates), so another store can't replace Kiwi's build without asking, and Kiwi's
                // own updates of it stay silent.
                setRequestUpdateOwnership(true)
            }
        }
        val sessionId = installer.createSession(params)
        try {
            installer.openSession(sessionId).use { session ->
                session.openWrite("base.apk", 0, apk.length()).use { out ->
                    apk.inputStream().use { it.copyTo(out, BUFFER) }
                    session.fsync(out)
                }
            }
        } catch (error: Throwable) {
            abandon(sessionId)
            throw error
        }
        return sessionId
    }

    private fun commit(sessionId: Int) {
        val intent = Intent(context, InstallResultReceiver::class.java)
            .setPackage(context.packageName)
            .putExtra(EXTRA_KIWI_SESSION, sessionId)
        // Mutable: Android adds the status extras to this intent.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
        val status = PendingIntent.getBroadcast(context, sessionId, intent, flags)
        installer.openSession(sessionId).use { it.commit(status.intentSender) }
    }

    private fun abandon(sessionId: Int) {
        runCatching { installer.abandonSession(sessionId) }
    }

    /** Called by [InstallResultReceiver] for every status of a session. */
    internal fun onStatus(intent: Intent) {
        val sessionId = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, intent.getIntExtra(EXTRA_KIWI_SESSION, -1))
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val waiting = pending[sessionId]
        val appName = waiting?.appName ?: "the app"
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            val confirm = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java) ?: return
            if (ForegroundTracker.isForeground) {
                context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } else {
                Notifications.showConfirmation(context, sessionId, appName, confirm)
            }
            return
        }
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        waiting?.result?.complete(InstallOutcome.from(status, message, appName))
    }

    private class Pending(val appName: String, val result: CompletableDeferred<InstallOutcome>)

    companion object {
        private const val BUFFER = 64 * 1024
        private const val EXTRA_KIWI_SESSION = "io.github.digitalpals.kiwi.SESSION_ID"

        /** Sessions this process is waiting for, by session id. */
        private val pending = ConcurrentHashMap<Int, Pending>()
    }
}

/** Receives PackageInstaller's status broadcasts (an explicit, not exported receiver). */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext as io.github.digitalpals.kiwi.KiwiApplication
        app.container.sessionInstaller.onStatus(intent)
        // The process may have restarted since the install began; report what is installed now.
        if (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, -1) == PackageInstaller.STATUS_SUCCESS) {
            app.container.packageReporter.reportSoon()
        }
    }
}
