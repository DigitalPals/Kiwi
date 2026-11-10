package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Build
import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.packages.ArchiveInfo
import io.github.digitalpals.kiwi.packages.PackageLookup
import io.github.digitalpals.kiwi.packages.ScannedPackage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.Collections
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/** The install flow on the JVM: Kiwi is a MockWebServer, Android's installer and PackageManager are fakes. */
class InstallManagerTest {
    @get:Rule
    val temp = TemporaryFolder()

    private val server = MockWebServer()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val installer = FakeInstaller()
    private val packages = FakePackages()
    private val reports = AtomicInteger()

    /** `Me.host.android_signer_sha256`. */
    @Volatile
    private var kiwiSigner: String? = KIWI_SIGNER
    private lateinit var manager: InstallManager

    @Before
    fun setUp() {
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when {
                request.url.encodedPath == "/api/me" -> json(me())
                request.url.encodedPath.endsWith(".apk") -> MockResponse.Builder().code(200).body(APK_BYTES).build()
                else -> MockResponse.Builder().code(404).build()
            }
        }
        server.start()
        val api = KiwiApi(server.url("/"), OkHttpClient())
        manager = InstallManager(
            scope = scope,
            api = { api },
            downloader = ApkDownloader(OkHttpClient()),
            installer = installer,
            packages = packages,
            report = { reports.incrementAndGet() },
            cacheDir = temp.newFolder("cache"),
            ownPackage = OWN,
        )
    }

    @After
    fun tearDown() {
        scope.cancel()
        server.close()
    }

    @Test
    fun aSecondStartJoinsTheRunningJob() = runBlocking {
        val first = manager.start(app(YOUTUBE))
        assertEquals(YOUTUBE, installer.next())
        assertSame(first, manager.start(app(YOUTUBE)))
        installer.finish(YOUTUBE)
        first.join()
        assertEquals(listOf(YOUTUBE), installer.calls)
        assertNull(manager.jobs.value[key(YOUTUBE)])
        assertEquals(1, manager.finished.value)
        assertEquals(1, reports.get())
    }

    @Test
    fun theWorkerSkipsAnAppThatIsInstalling() = runBlocking {
        val job = manager.start(app(YOUTUBE))
        assertEquals(YOUTUBE, installer.next())
        assertNull(manager.installInBackground(app(YOUTUBE)))
        installer.finish(YOUTUBE)
        job.join()
        assertEquals(listOf(YOUTUBE), installer.calls)
    }

    @Test
    fun aBackgroundInstallIsRegisteredAndCancellable() = runBlocking {
        val background = async { runCatching { manager.installInBackground(app(YOUTUBE)) } }
        assertEquals(YOUTUBE, installer.next())
        // The screen sees the worker's job and joins it instead of starting a second install.
        assertEquals(InstallJob.Phase.Installing, manager.jobs.value[key(YOUTUBE)]?.phase)
        val joined = manager.start(app(YOUTUBE))
        manager.startAll(listOf(app(YOUTUBE)), null)
        manager.cancel(key(YOUTUBE))
        joined.join()
        assertTrue(background.await().exceptionOrNull() is CancellationException)
        assertEquals(listOf(YOUTUBE), installer.calls)
        assertEquals(listOf(YOUTUBE), installer.cancelled)
        withTimeout(5_000) { manager.jobs.first { key(YOUTUBE) !in it } }

        // Nothing is left behind: the app can be installed again.
        val again = manager.start(app(YOUTUBE))
        assertEquals(YOUTUBE, installer.next())
        installer.finish(YOUTUBE)
        again.join()
        assertEquals(1, manager.finished.value)
    }

    @Test
    fun backgroundInstallsRunOneAtATime() = runBlocking {
        val inside = AtomicInteger()
        val most = AtomicInteger()
        val runs = List(3) {
            launch(Dispatchers.Default) {
                manager.backgroundInstalls {
                    most.accumulateAndGet(inside.incrementAndGet(), ::maxOf)
                    delay(50)
                    inside.decrementAndGet()
                }
            }
        }
        runs.forEach { it.join() }
        assertEquals(1, most.get())
    }

    @Test
    fun updateAllInstallsKiwiItselfLast() = runBlocking {
        manager.startAll(listOf(app(YOUTUBE), app(REDDIT)), selfUpdate())
        // Published at once, so nothing that waits for the jobs can miss them.
        assertEquals(setOf(key(YOUTUBE), key(REDDIT)), manager.jobs.value.keys)
        val first = installer.next()
        installer.finish(first)
        val second = installer.next()
        assertNotEquals(first, second)
        assertNull(manager.jobs.value[InstallManager.SELF])
        installer.finish(second)
        assertEquals(OWN, installer.next())
        installer.finish(OWN)
        withTimeout(5_000) { manager.finished.first { it == 3 } }
        assertEquals(OWN, installer.calls.last())
    }

    @Test
    fun updateAllSkipsAppsThatAreInstalling() = runBlocking {
        val running = manager.start(app(YOUTUBE))
        assertEquals(YOUTUBE, installer.next())
        manager.startAll(listOf(app(YOUTUBE), app(REDDIT)), selfUpdate())
        installer.finish(YOUTUBE)
        assertEquals(REDDIT, installer.next())
        installer.finish(REDDIT)
        assertEquals(OWN, installer.next())
        installer.finish(OWN)
        running.join()
        withTimeout(5_000) { manager.finished.first { it == 3 } }
        assertEquals(listOf(YOUTUBE, REDDIT, OWN), installer.calls)
    }

    @Test
    fun refusesAnApkNotSignedWithKiwisKey() = runBlocking {
        packages.signers[YOUTUBE] = listOf("ff".repeat(32))
        manager.start(app(YOUTUBE)).join()
        val job = manager.jobs.value[key(YOUTUBE)]!!
        assertEquals(InstallJob.Phase.Failed, job.phase)
        assertTrue(job.message!!.contains("isn't signed with your Kiwi's key"))
        assertTrue(installer.calls.isEmpty())
    }

    @Test
    fun acceptsAnyApkSignerWhileKiwiDoesntKnowItsKey() = runBlocking {
        kiwiSigner = null
        packages.signers[YOUTUBE] = listOf("ff".repeat(32))
        val job = manager.start(app(YOUTUBE))
        assertEquals(YOUTUBE, installer.next())
        installer.finish(YOUTUBE)
        job.join()
        assertNull(manager.jobs.value[key(YOUTUBE)])
    }

    @Test
    fun refusesAnApkOfAnotherVersion() = runBlocking {
        packages.versionCodes[YOUTUBE] = 41
        manager.start(app(YOUTUBE, versionCode = 42)).join()
        val job = manager.jobs.value[key(YOUTUBE)]!!
        assertEquals(InstallJob.Phase.Failed, job.phase)
        assertTrue(job.message!!.contains("isn't the version Kiwi offered"))
        assertTrue(installer.calls.isEmpty())
    }

    @Test
    fun checksTheSelfUpdatesVersion() = runBlocking {
        packages.versionCodes[OWN] = 1_000_000
        manager.startSelfUpdate(selfUpdate(versionCode = 1_001_000)).join()
        assertTrue(manager.jobs.value[InstallManager.SELF]!!.message!!.contains("isn't the version Kiwi offered"))
        assertTrue(installer.calls.isEmpty())
    }

    @Test
    fun asksForThePermissionFirst() = runBlocking {
        installer.allowed = false
        manager.start(app(YOUTUBE)).join()
        assertEquals(InstallJob.Phase.NeedsPermission, manager.jobs.value[key(YOUTUBE)]?.phase)
        assertNull(manager.installInBackground(app(YOUTUBE)))
        manager.clear(key(YOUTUBE))
        assertNull(manager.jobs.value[key(YOUTUBE)])
    }

    @Test
    fun verifiesTheArchive() {
        val offer = InstallManager.Offer("k", "YouTube", YOUTUBE, "/ota/t/app.apk", versionCode = 42, size = 10, kiwiSigned = true)
        val good = ArchiveInfo(YOUTUBE, 42, listOf(KIWI_SIGNER))
        assertNull(InstallManager.verify(good, offer, KIWI_SIGNER, null))
        // Colons and case don't matter.
        val colons = KIWI_SIGNER.uppercase().chunked(2).joinToString(":")
        assertNull(InstallManager.verify(good, offer, colons, null))
        // Not signed by Kiwi, or unreadable signers.
        expectProblem("isn't signed with your Kiwi's key") { InstallManager.verify(good.copy(signers = listOf("ab")), offer, KIWI_SIGNER, null) }
        expectProblem("isn't signed with your Kiwi's key") { InstallManager.verify(good.copy(signers = emptyList()), offer, KIWI_SIGNER, null) }
        // Apps Kiwi didn't sign, or a Kiwi that doesn't know its key yet: no signer check.
        assertNull(InstallManager.verify(good.copy(signers = listOf("ab")), offer.copy(kiwiSigned = false), KIWI_SIGNER, null))
        assertNull(InstallManager.verify(good.copy(signers = listOf("ab")), offer, null, null))
        // The package and version Kiwi offered.
        expectProblem("The download isn't YouTube") { InstallManager.verify(good.copy(packageName = "com.other"), offer, KIWI_SIGNER, null) }
        expectProblem("version code 43 instead of 42") { InstallManager.verify(good.copy(versionCode = 43), offer, KIWI_SIGNER, null) }
        assertNull(InstallManager.verify(good.copy(versionCode = 43), offer.copy(versionCode = null), KIWI_SIGNER, null))
        // An installed copy: not newer, and signed alike.
        val installed = InstalledPackage(YOUTUBE, "1", 40, listOf(KIWI_SIGNER), OWN)
        assertNull(InstallManager.verify(good, offer, KIWI_SIGNER, installed))
        expectProblem("A newer version") { InstallManager.verify(good, offer, KIWI_SIGNER, installed.copy(versionCode = 50)) }
        val conflict = InstallManager.verify(good, offer, KIWI_SIGNER, installed.copy(signerSha256 = listOf("cd")))
        assertTrue(conflict is InstallOutcome.SignatureConflict)
    }

    @Test
    fun keysDownloadsByUrlVersionAndSize() {
        val url = "https://kiwi.example.ts.net/android/kiwi.apk"
        val v1 = InstallManager.apkFileName(OWN, url, 1_000_000, 2_000)
        assertEquals(v1, InstallManager.apkFileName(OWN, url, 1_000_000, 2_000))
        assertTrue(v1.startsWith("$OWN-") && v1.endsWith(".apk"))
        // Kiwi's app has one URL for every version: a partial of another version is a different file.
        assertNotEquals(v1, InstallManager.apkFileName(OWN, url, 1_001_000, 2_000))
        assertNotEquals(v1, InstallManager.apkFileName(OWN, url, 1_000_000, 2_001))
        assertNotEquals(v1, InstallManager.apkFileName(OWN, "$url?x", 1_000_000, 2_000))
    }

    private fun expectProblem(text: String, block: () -> Unit) {
        try {
            block()
            fail("expected \"$text\"")
        } catch (error: KiwiException.BadResponse) {
            assertTrue(error.message, error.message!!.contains(text))
        }
    }

    private fun me(): String {
        val signer = kiwiSigner?.let { "\"$it\"" } ?: "null"
        return """{"device":{"id":"d1","name":"Pixel","platform":"android"},""" +
            """"host":{"os":"linux","android_signer_sha256":$signer}}"""
    }

    private fun json(body: String) =
        MockResponse.Builder().code(200).addHeader("Content-Type", "application/json").body(body).build()

    private fun key(packageName: String) = "kiwi-youtube/$packageName"

    private fun app(packageName: String, versionCode: Long = 42): AppSummary {
        val url = "/ota/tok-$packageName/app.apk"
        return AppSummary(
            sourceId = "kiwi-youtube",
            bundleId = packageName,
            platform = "android",
            name = packageName.substringAfterLast('.'),
            state = AppState(
                actionName = "install",
                installUrl = url,
                build = Build(id = "b-$packageName", status = "ready", build = versionCode.toString(), size = APK_SIZE, installUrl = url),
            ),
        )
    }

    private fun selfUpdate(versionCode: Long = 42) =
        AndroidApp(OWN, "1.0.0", versionCode, APK_SIZE, server.url("/android/kiwi.apk").toString())

    /** PackageManager: archives carry the package of their file name, nothing is installed. */
    private class FakePackages : PackageLookup {
        val signers = ConcurrentHashMap<String, List<String>>()
        val versionCodes = ConcurrentHashMap<String, Long>()

        override fun scan(packages: Set<String>): List<ScannedPackage> = emptyList()

        override fun find(packageName: String): InstalledPackage? = null

        override fun archive(path: String): ArchiveInfo {
            val packageName = File(path).name.substringBeforeLast('-')
            return ArchiveInfo(packageName, versionCodes[packageName] ?: 42, signers[packageName] ?: listOf(KIWI_SIGNER))
        }
    }

    /** Android's installer: each install waits until the test finishes it. */
    private class FakeInstaller : Installer {
        @Volatile
        var allowed = true
        val calls: MutableList<String> = Collections.synchronizedList(mutableListOf())
        val cancelled: MutableList<String> = Collections.synchronizedList(mutableListOf())
        private val started = Channel<String>(Channel.UNLIMITED)
        private val results = ConcurrentHashMap<String, CompletableDeferred<InstallOutcome>>()

        override fun canInstall(): Boolean = allowed

        override suspend fun install(apk: File, packageName: String, appName: String): InstallOutcome {
            assertTrue(apk.length() == APK_SIZE)
            calls += packageName
            started.send(packageName)
            return try {
                result(packageName).await()
            } catch (error: CancellationException) {
                cancelled += packageName
                throw error
            } finally {
                results.remove(packageName)
            }
        }

        /** The package of the next install that reached Android. */
        suspend fun next(): String = withTimeout(10_000) { started.receive() }

        fun finish(packageName: String, outcome: InstallOutcome = InstallOutcome.Success) {
            result(packageName).complete(outcome)
        }

        private fun result(packageName: String) = results.getOrPut(packageName) { CompletableDeferred() }
    }

    private companion object {
        const val OWN = "io.github.digitalpals.kiwi"
        const val YOUTUBE = "app.morphe.android.youtube"
        const val REDDIT = "com.reddit.frontpage"
        val KIWI_SIGNER = "ab".repeat(32)
        const val APK_BYTES = "not really an apk"
        val APK_SIZE = APK_BYTES.length.toLong()
    }
}
