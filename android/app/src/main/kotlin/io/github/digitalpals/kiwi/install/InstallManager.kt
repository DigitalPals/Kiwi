package io.github.digitalpals.kiwi.install

import android.content.Context
import android.util.Log
import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.PrepareRequest
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.packages.PackageReport
import io.github.digitalpals.kiwi.packages.PackageReporter
import io.github.digitalpals.kiwi.packages.PackageScanner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import okhttp3.HttpUrl
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap

/** An install this app is working on, shown on the app's button. */
data class InstallJob(
    val key: String,
    val appName: String,
    val phase: Phase,
    /** 0..1 while downloading, when the size is known. */
    val progress: Float? = null,
    val message: String? = null,
) {
    enum class Phase { Preparing, Downloading, Installing, NeedsPermission, Conflict, Failed }

    val isActive: Boolean get() = phase == Phase.Preparing || phase == Phase.Downloading || phase == Phase.Installing
}

/**
 * Gets an app onto the phone: asks Kiwi to prepare the build when needed, downloads the APK,
 * checks it, installs it with [SessionInstaller] and tells Kiwi what is installed now.
 * Jobs run in the app's scope, so they continue when the user leaves the screen.
 */
class InstallManager(
    context: Context,
    private val scope: CoroutineScope,
    private val api: suspend () -> KiwiApi?,
    private val downloader: ApkDownloader,
    private val installer: SessionInstaller,
    private val scanner: PackageScanner,
    private val reporter: PackageReporter,
) {
    private val context = context.applicationContext
    private val _jobs = MutableStateFlow<Map<String, InstallJob>>(emptyMap())
    private val running = ConcurrentHashMap<String, Job>()
    private val sessionLock = Mutex()
    private val _finished = MutableStateFlow(0)

    /** Current jobs by key (an app's [AppSummary.key], or [SELF] for Kiwi itself). */
    val jobs: StateFlow<Map<String, InstallJob>> = _jobs

    /** Increases after every successful install, so screens can reload their apps. */
    val finished: StateFlow<Int> = _finished

    /** Starts installing or updating [app]. Does nothing while a job for it runs. */
    fun start(app: AppSummary) {
        launch(app.key) { kiwi -> runApp(kiwi, app) }
    }

    /** Installs the newer Kiwi app the server offers (`Me.android_app`). */
    fun startSelfUpdate(update: AndroidApp) {
        launch(SELF) { kiwi -> runSelfUpdate(kiwi, update) }
    }

    /** Stops a job (an abandoned session is never installed) and forgets its state. */
    fun cancel(key: String) {
        val job = running.remove(key)
        if (job == null) {
            clear(key)
            return
        }
        // Cleared once the job has stopped, so a last progress update can't bring it back.
        job.invokeOnCompletion { clear(key) }
        job.cancel()
    }

    /** Forgets a failed job, so the button shows the server's state again. */
    fun clear(key: String) {
        _jobs.update { it - key }
    }

    /**
     * Installs [app] from the background worker and returns the outcome; null when it was
     * skipped (no server, a job already runs, or Android doesn't allow installing).
     */
    suspend fun installInBackground(app: AppSummary): InstallOutcome? {
        val kiwi = api() ?: return null
        if (running.containsKey(app.key) || !installer.canInstall()) return null
        return try {
            withTimeout(BACKGROUND_LIMIT_MS) { runApp(kiwi, app) }
        } finally {
            clear(app.key)
        }
    }

    /** Self-update from the background worker; see [installInBackground]. */
    suspend fun selfUpdateInBackground(update: AndroidApp): InstallOutcome? {
        val kiwi = api() ?: return null
        if (running.containsKey(SELF) || !installer.canInstall()) return null
        return try {
            withTimeout(BACKGROUND_LIMIT_MS) { runSelfUpdate(kiwi, update) }
        } finally {
            clear(SELF)
        }
    }

    private fun launch(key: String, block: suspend (KiwiApi) -> InstallOutcome?) {
        if (running[key]?.isActive == true) return
        val job = scope.launch {
            try {
                val kiwi = api() ?: return@launch
                if (!installer.canInstall()) {
                    set(key, "", InstallJob.Phase.NeedsPermission)
                    return@launch
                }
                block(kiwi)
            } finally {
                running.remove(key)
            }
        }
        running[key] = job
    }

    private suspend fun runApp(kiwi: KiwiApi, app: AppSummary): InstallOutcome? = guarded(app.key, app.name) {
        set(app.key, app.name, InstallJob.Phase.Preparing)
        val url = installUrl(kiwi, app)
        install(kiwi, app.key, app.name, app.bundleId, url)
    }

    private suspend fun runSelfUpdate(kiwi: KiwiApi, update: AndroidApp): InstallOutcome? = guarded(SELF, "Kiwi") {
        set(SELF, "Kiwi", InstallJob.Phase.Preparing)
        install(kiwi, SELF, "Kiwi", context.packageName, update.installUrl)
    }

    /** Runs [block] and turns its errors into the job's state. */
    private suspend fun guarded(key: String, name: String, block: suspend () -> InstallOutcome?): InstallOutcome? = try {
        block()
    } catch (error: CancellationException) {
        throw error
    } catch (error: Exception) {
        Log.w(TAG, "Installing $name failed", error)
        set(key, name, InstallJob.Phase.Failed, message = error.userMessage())
        InstallOutcome.Failure(error.userMessage())
    }

    /**
     * The APK's URL for [app]: the state's `install_url` when Kiwi has a ready build, else the
     * build that `POST .../prepare` returns, polled until it is ready.
     */
    private suspend fun installUrl(kiwi: KiwiApi, app: AppSummary): String {
        val state = app.state
        val ready = state.installUrl
        if (ready != null && state.action in DIRECT_ACTIONS) return ready
        var build = state.build?.takeIf { state.action == Action.Preparing && it.isActive }
            ?: kiwi.prepare(app.sourceId, app.bundleId, PrepareRequest())
        withTimeout(PREPARE_LIMIT_MS) {
            while (build.isActive) {
                delay(POLL_MS)
                build = kiwi.build(build.id)
            }
        }
        if (build.isFailed) throw KiwiException.BadResponse(build.error ?: "Kiwi couldn't prepare ${app.name}.")
        return build.installUrl ?: throw KiwiException.BadResponse("Kiwi has no download for ${app.name}.")
    }

    private suspend fun install(kiwi: KiwiApi, key: String, name: String, packageName: String, installUrl: String): InstallOutcome {
        val url: HttpUrl = kiwi.resolve(installUrl)?.takeIf(kiwi::isOwnUrl)
            ?: throw KiwiException.BadResponse("Kiwi offered $name from another address; it wasn't downloaded.")
        val file = apkFile(packageName, url)
        set(key, name, InstallJob.Phase.Downloading, progress = 0f)
        downloader.download(url, file) { done, total ->
            val fraction = total?.takeIf { it > 0 }?.let { (done.toDouble() / it).toFloat().coerceIn(0f, 1f) }
            set(key, name, InstallJob.Phase.Downloading, progress = fraction)
        }
        try {
            checkApk(file, packageName, name)?.let { conflict ->
                set(key, name, InstallJob.Phase.Conflict, message = conflict.message)
                return conflict
            }
            set(key, name, InstallJob.Phase.Installing)
            val outcome = sessionLock.withLock { installer.install(file, packageName, name) }
            when (outcome) {
                InstallOutcome.Success -> {
                    clear(key)
                    reporter.report()
                    _finished.value += 1
                }
                InstallOutcome.Cancelled -> clear(key)
                is InstallOutcome.SignatureConflict -> set(key, name, InstallJob.Phase.Conflict, message = outcome.message)
                is InstallOutcome.Failure -> set(key, name, InstallJob.Phase.Failed, message = outcome.message)
            }
            return outcome
        } finally {
            file.delete()
        }
    }

    /**
     * Checks the downloaded file before handing it to Android: it must be the expected
     * package, and an installed copy must share its signer (otherwise Android refuses the update
     * with a less helpful message).
     */
    private fun checkApk(file: File, packageName: String, name: String): InstallOutcome.SignatureConflict? {
        val archive = scanner.archiveSigners(file.path)
            ?: throw KiwiException.BadResponse("The downloaded $name isn't a valid app.")
        if (archive.packageName != packageName) {
            throw KiwiException.BadResponse("The download isn't $name (it is ${archive.packageName}).")
        }
        val installed = scanner.find(packageName) ?: return null
        if (installed.versionCode > archive.versionCode) {
            throw KiwiException.BadResponse("A newer version of $name is already installed.")
        }
        if (!PackageReport.signersCompatible(installed.signerSha256, archive.signers)) {
            return InstallOutcome.SignatureConflict(InstallOutcome.signatureConflictMessage(name))
        }
        return null
    }

    /**
     * Where an APK is downloaded to: one file per package and URL, so only a download of the same
     * file is resumed. Older files of the package are removed.
     */
    private fun apkFile(packageName: String, url: HttpUrl): File {
        val dir = File(context.cacheDir, "apks").apply { mkdirs() }
        val digest = MessageDigest.getInstance("SHA-256").digest(url.toString().toByteArray())
            .joinToString("") { "%02x".format(it) }.take(16)
        val name = "$packageName-$digest.apk"
        dir.listFiles()?.filter { it.name.startsWith("$packageName-") && !it.name.startsWith(name) }?.forEach { it.delete() }
        return File(dir, name)
    }

    private fun set(key: String, name: String, phase: InstallJob.Phase, progress: Float? = null, message: String? = null) {
        _jobs.update { it + (key to InstallJob(key, name, phase, progress, message)) }
    }

    companion object {
        const val SELF = "kiwi:self"
        private const val TAG = "KiwiInstall"
        private const val POLL_MS = 2_000L
        private const val PREPARE_LIMIT_MS = 30 * 60_000L
        private const val BACKGROUND_LIMIT_MS = 8 * 60_000L

        /** Actions whose `install_url` is the APK to install (Android builds need no preparing). */
        private val DIRECT_ACTIONS = setOf(Action.Install, Action.Update, Action.Installing, Action.Open)
    }
}
