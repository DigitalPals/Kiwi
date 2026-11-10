package io.github.digitalpals.kiwi.packages

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.util.Log
import androidx.core.content.ContextCompat
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Sends the installed packages to Kiwi (`PUT /api/me/packages`): when the app starts, after an
 * install and when a package changes while the app runs. Kiwi then knows which of its apps are
 * really installed and which are installed with another signature.
 */
class PackageReporter(
    private val context: Context,
    private val scanner: PackageScanner,
    private val scope: CoroutineScope,
    private val api: suspend () -> KiwiApi?,
) {
    private val mutex = Mutex()
    private var pending: Job? = null
    private val _changes = MutableStateFlow(0)

    /** Increases whenever a package was added, replaced or removed while the app runs. */
    val changes: StateFlow<Int> = _changes

    /** Scans and reports now. Returns false when nothing could be sent. */
    suspend fun report(): Boolean = mutex.withLock {
        val kiwi = api() ?: return false
        val report = withContext(Dispatchers.IO) { PackageReport.build(scanner.scan()) }
        try {
            kiwi.putPackages(report)
            true
        } catch (error: KiwiException.Http) {
            // An older Kiwi has no such endpoint (404); an iOS device gets 400. Neither is an error
            // the user can do anything about.
            Log.i(TAG, "Kiwi didn't take the package list: ${error.status} ${error.code}")
            false
        } catch (error: KiwiException) {
            Log.i(TAG, "Couldn't send the package list: ${error.message}")
            false
        }
    }

    /** Reports after a short pause, so a burst of package changes sends one report. */
    fun reportSoon() {
        pending?.cancel()
        pending = scope.launch {
            delay(DEBOUNCE_MS)
            report()
        }
    }

    /** Listens for package changes for as long as the process runs. */
    fun watch() {
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REPLACED)
            addAction(Intent.ACTION_PACKAGE_FULLY_REMOVED)
            addDataScheme("package")
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                _changes.value += 1
                reportSoon()
            }
        }
        // Package broadcasts come from the system; nothing else needs to reach this receiver.
        ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    private companion object {
        const val TAG = "KiwiPackages"
        const val DEBOUNCE_MS = 2_000L
    }
}
