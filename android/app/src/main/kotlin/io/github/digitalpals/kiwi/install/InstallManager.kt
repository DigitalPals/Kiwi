package io.github.digitalpals.kiwi.install

import android.util.Log
import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Build
import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.api.PrepareRequest
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.packages.ArchiveInfo
import io.github.digitalpals.kiwi.packages.PackageLookup
import io.github.digitalpals.kiwi.packages.PackageReport
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.joinAll
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

/** What [InstallManager] needs from Android's PackageInstaller ([SessionInstaller]); faked in tests. */
interface Installer {
    /** Whether Android lets this app install apps ("Install unknown apps"). */
    fun canInstall(): Boolean

    /** Installs [apk] as [packageName] and waits for Android's verdict. */
    suspend fun install(apk: File, packageName: String, appName: String): InstallOutcome
}

/**
 * Gets an app onto the phone: asks Kiwi to prepare the build when needed, downloads the APK,
 * checks it, installs it with an [Installer] and tells Kiwi what is installed now ([report]).
 *
 * Every install, from a screen or from the background worker, is one job per key (an app's
 * [AppSummary.key], or [SELF] for Kiwi itself), registered before it starts: a second request
 * for a key that has a job joins that job instead of starting another. Jobs run in [scope], so
 * they continue when the user leaves the screen, and [cancel] stops any of them.
 */
class InstallManager(
    private val scope: CoroutineScope,
    private val api: suspend () -> KiwiApi?,
    private val downloader: ApkDownloader,
    private val installer: Installer,
    private val packages: PackageLookup,
    /** Sends the package report after an install (PackageReporter.report). */
    private val report: suspend () -> Unit,
    /** Where APKs are downloaded to (the app's cache directory). */
    private val cacheDir: File,
    /** This app's package: Kiwi's own update installs as it. */
    private val ownPackage: String,
) {
    private val _jobs = MutableStateFlow<Map<String, InstallJob>>(emptyMap())
    private val running = ConcurrentHashMap<String, Job>()
    private val sessionLock = Mutex()
    private val backgroundLock = Mutex()
    private val _finished = MutableStateFlow(0)

    /** Current jobs by key (an app's [AppSummary.key], or [SELF] for Kiwi itself). */
    val jobs: StateFlow<Map<String, InstallJob>> = _jobs

    /** Increases after every successful install, so screens can reload their apps. */
    val finished: StateFlow<Int> = _finished

    /**
     * Starts installing or updating [app]. Returns its job: a new one, or the one already working
     * on the app (nothing else is started then).
     */
    fun start(app: AppSummary): Job = startJob(app.key, app.name) { kiwi -> runApp(kiwi, app) }

    /** Installs the newer Kiwi app the server offers (`Me.android_app`); see [start]. */
    fun startSelfUpdate(update: AndroidApp): Job = startJob(SELF, KIWI) { kiwi -> runSelfUpdate(kiwi, update) }

    /**
     * "Update all": starts every app of [apps] that has no job yet, and [selfUpdate] once all of
     * them (and those already running) have finished, since installing Kiwi's own update stops
     * the app.
     */
    fun startAll(apps: List<AppSummary>, selfUpdate: AndroidApp?) {
        val started = apps.map(::start)
        if (selfUpdate == null) return
        if (started.all { it.isCompleted }) {
            startSelfUpdate(selfUpdate)
            return
        }
        scope.launch {
            started.joinAll()
            startSelfUpdate(selfUpdate)
        }
    }

    /**
     * Stops the job of [key], also one the background worker runs (an abandoned session is never
     * installed), and forgets its state once it has stopped.
     */
    fun cancel(key: String) {
        val job = running[key]
        if (job == null) clear(key) else job.cancel()
    }

    /**
     * Forgets a finished job's state (failed, a conflict, missing permission), so the button
     * shows the server's state again. A job that is still working keeps its state.
     */
    fun clear(key: String) {
        _jobs.update { if (running.containsKey(key)) it else it - key }
    }

    /**
     * Installs [app] for the background worker and returns the outcome; null when it was
     * skipped (no server, a job already works on it, or Android doesn't allow installing). The
     * job shows on the app's button like any other and can be cancelled there; it is cancelled
     * when the worker stops or after [BACKGROUND_LIMIT_MS] (a confirmation nobody answered).
     */
    suspend fun installInBackground(app: AppSummary): InstallOutcome? =
        background(app.key, app.name) { kiwi -> runApp(kiwi, app) }

    /** Self-update from the background worker; see [installInBackground]. */
    suspend fun selfUpdateInBackground(update: AndroidApp): InstallOutcome? =
        background(SELF, KIWI) { kiwi -> runSelfUpdate(kiwi, update) }

    /**
     * Runs [block] while no other caller is in it: the periodic check and the build watch are
     * separate works that can overlap, and only one of them installs at a time.
     */
    suspend fun <T> backgroundInstalls(block: suspend () -> T): T = backgroundLock.withLock { block() }

    private fun startJob(key: String, name: String, block: suspend (KiwiApi) -> InstallOutcome?): Job =
        register(key, name, background = false, block) ?: running[key] ?: Job().apply { complete() }

    private suspend fun background(key: String, name: String, block: suspend (KiwiApi) -> InstallOutcome?): InstallOutcome? {
        if (!installer.canInstall()) return null
        val job = register(key, name, background = true, block) ?: return null
        return try {
            withTimeout(BACKGROUND_LIMIT_MS) { job.await() }
        } catch (error: CancellationException) {
            // The worker stopped, the time ran out or the user cancelled: the install stops too.
            job.cancel()
            throw error
        }
    }

    /**
     * Registers and starts the job of [key], or returns null when one is already working on
     * it. The job's Preparing state is published before this returns, so callers that wait for
     * jobs see it at once.
     */
    private fun register(
        key: String,
        name: String,
        background: Boolean,
        block: suspend (KiwiApi) -> InstallOutcome?,
    ): Deferred<InstallOutcome?>? {
        val job = scope.async(start = CoroutineStart.LAZY) { execute(key, name, background, block) }
        if (running.putIfAbsent(key, job) != null) {
            job.cancel()
            return null
        }
        set(key, name, InstallJob.Phase.Preparing)
        job.invokeOnCompletion { cause ->
            running.remove(key, job)
            // A cancelled job leaves nothing on the button; the worker's jobs never do.
            if (cause != null || background) clear(key)
        }
        job.start()
        return job
    }

    private suspend fun execute(
        key: String,
        name: String,
        background: Boolean,
        block: suspend (KiwiApi) -> InstallOutcome?,
    ): InstallOutcome? = guarded(key, name) {
        val kiwi = api()
        when {
            kiwi == null -> {
                forget(key)
                null
            }
            !installer.canInstall() -> {
                if (background) forget(key) else set(key, name, InstallJob.Phase.NeedsPermission)
                null
            }
            else -> block(kiwi)
        }
    }

    private suspend fun runApp(kiwi: KiwiApi, app: AppSummary): InstallOutcome {
        val build = readyBuild(kiwi, app)
        val url = build.installUrl ?: throw KiwiException.BadResponse("Kiwi has no download for ${app.name}.")
        val offer = Offer(
            key = app.key,
            name = app.name,
            packageName = app.bundleId,
            url = url,
            versionCode = build.build?.toLongOrNull(),
            size = build.size,
            kiwiSigned = app.sourceId == KiwiBuilds.SOURCE_ID,
        )
        return install(kiwi, offer)
    }

    private suspend fun runSelfUpdate(kiwi: KiwiApi, update: AndroidApp): InstallOutcome {
        val offer = Offer(
            key = SELF,
            name = KIWI,
            packageName = ownPackage,
            url = update.installUrl,
            versionCode = update.versionCode,
            size = update.size,
            kiwiSigned = true,
        )
        return install(kiwi, offer)
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
     * The ready build of [app]: the state's build when Kiwi has one (its `install_url` is the
     * state's), else the build that `POST .../prepare` returns, polled until it is ready.
     */
    private suspend fun readyBuild(kiwi: KiwiApi, app: AppSummary): Build {
        val state = app.state
        val ready = state.installUrl
        if (ready != null && state.action in DIRECT_ACTIONS) {
            return state.build?.copy(installUrl = ready) ?: Build(id = "", status = "ready", installUrl = ready)
        }
        var build = state.build?.takeIf { state.action == Action.Preparing && it.isActive }
            ?: kiwi.prepare(app.sourceId, app.bundleId, PrepareRequest())
        withTimeout(PREPARE_LIMIT_MS) {
            while (build.isActive) {
                delay(POLL_MS)
                build = kiwi.build(build.id)
            }
        }
        if (build.isFailed) throw KiwiException.BadResponse(build.error ?: "Kiwi couldn't prepare ${app.name}.")
        return build
    }

    private suspend fun install(kiwi: KiwiApi, offer: Offer): InstallOutcome {
        val key = offer.key
        val name = offer.name
        val url: HttpUrl = kiwi.resolve(offer.url)?.takeIf(kiwi::isOwnUrl)
            ?: throw KiwiException.BadResponse("Kiwi offered $name from another address; it wasn't downloaded.")
        // The certificate Kiwi signs its Android apps with, when Kiwi knows it already.
        val signer = if (offer.kiwiSigned) kiwi.me().host?.androidSignerSha256?.takeIf { it.isNotBlank() } else null
        val file = apkFile(offer, url)
        set(key, name, InstallJob.Phase.Downloading, progress = 0f)
        downloader.download(url, file, offer.size) { done, total ->
            val fraction = total?.takeIf { it > 0 }?.let { (done.toDouble() / it).toFloat().coerceIn(0f, 1f) }
            set(key, name, InstallJob.Phase.Downloading, progress = fraction)
        }
        try {
            val archive = packages.archive(file.path)
                ?: throw KiwiException.BadResponse("The downloaded $name isn't a valid app.")
            verify(archive, offer, signer, packages.find(offer.packageName))?.let { conflict ->
                set(key, name, InstallJob.Phase.Conflict, message = conflict.message)
                return conflict
            }
            set(key, name, InstallJob.Phase.Installing)
            val outcome = sessionLock.withLock { installer.install(file, offer.packageName, name) }
            when (outcome) {
                InstallOutcome.Success -> {
                    forget(key)
                    report()
                    _finished.update { it + 1 }
                }
                InstallOutcome.Cancelled -> forget(key)
                is InstallOutcome.SignatureConflict -> set(key, name, InstallJob.Phase.Conflict, message = outcome.message)
                is InstallOutcome.Failure -> set(key, name, InstallJob.Phase.Failed, message = outcome.message)
            }
            return outcome
        } finally {
            file.delete()
        }
    }

    /**
     * Where an APK is downloaded to: one file per package, URL, version code and size, so only
     * a download of the same file is resumed (Kiwi's own app has one URL for every version).
     * Older files of the package are removed.
     */
    private fun apkFile(offer: Offer, url: HttpUrl): File {
        val dir = File(cacheDir, "apks").apply { mkdirs() }
        val name = apkFileName(offer.packageName, url.toString(), offer.versionCode, offer.size)
        dir.listFiles()?.filter { it.name.startsWith("${offer.packageName}-") && !it.name.startsWith(name) }?.forEach { it.delete() }
        return File(dir, name)
    }

    /** Sets the state of a job; only the job itself calls this (and [register]). */
    private fun set(key: String, name: String, phase: InstallJob.Phase, progress: Float? = null, message: String? = null) {
        _jobs.update { it + (key to InstallJob(key, name, phase, progress, message)) }
    }

    /** Forgets the state of a job; only the job itself calls this. */
    private fun forget(key: String) {
        _jobs.update { it - key }
    }

    /** What Kiwi offered to install: the package, and its version code and size when Kiwi says. */
    internal data class Offer(
        val key: String,
        val name: String,
        val packageName: String,
        val url: String,
        val versionCode: Long?,
        val size: Long?,
        /** Kiwi signed it with its Android key (Kiwi Builds' apps and Kiwi itself). */
        val kiwiSigned: Boolean,
    )

    companion object {
        const val SELF = "kiwi:self"
        private const val KIWI = "Kiwi"
        private const val TAG = "KiwiInstall"
        private const val POLL_MS = 2_000L
        private const val PREPARE_LIMIT_MS = 30 * 60_000L
        const val BACKGROUND_LIMIT_MS = 8 * 60_000L

        /** Actions whose `install_url` is the APK to install (Android builds need no preparing). */
        private val DIRECT_ACTIONS = setOf(Action.Install, Action.Update, Action.Installing, Action.Open)

        /**
         * Checks a downloaded APK before it goes to Android: it must be the package Kiwi offered,
         * in the version Kiwi offered, signed with Kiwi's key when Kiwi signed it and says which
         * ([signer]), and not older than an [installed] copy. Returns a conflict when the installed
         * copy has another signer (Android would refuse the update with a less helpful message);
         * throws for everything else that is wrong.
         */
        internal fun verify(
            archive: ArchiveInfo,
            offer: Offer,
            signer: String?,
            installed: InstalledPackage?,
        ): InstallOutcome.SignatureConflict? {
            val name = offer.name
            if (archive.packageName != offer.packageName) {
                throw KiwiException.BadResponse("The download isn't $name (it is ${archive.packageName}).")
            }
            val expected = offer.versionCode
            if (expected != null && archive.versionCode != expected) {
                throw KiwiException.BadResponse(
                    "The downloaded $name isn't the version Kiwi offered (version code ${archive.versionCode} " +
                        "instead of $expected). Try again.",
                )
            }
            if (offer.kiwiSigned && signer != null && !PackageReport.signedBy(archive.signers, signer)) {
                throw KiwiException.BadResponse("The downloaded $name isn't signed with your Kiwi's key, so it wasn't installed.")
            }
            if (installed == null) return null
            if (installed.versionCode > archive.versionCode) {
                throw KiwiException.BadResponse("A newer version of $name is already installed.")
            }
            if (!PackageReport.signersCompatible(installed.signerSha256, archive.signers)) {
                return InstallOutcome.SignatureConflict(InstallOutcome.signatureConflictMessage(name))
            }
            return null
        }

        /** The download's file name: the package and a digest of the URL, version code and size. */
        internal fun apkFileName(packageName: String, url: String, versionCode: Long?, size: Long?): String {
            val digest = MessageDigest.getInstance("SHA-256").digest("$url\n$versionCode\n$size".toByteArray())
                .joinToString("") { "%02x".format(it) }.take(16)
            return "$packageName-$digest.apk"
        }
    }
}
