package io.github.digitalpals.kiwi.packages

import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiJson
import io.github.digitalpals.kiwi.api.PackagesReport
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.util.Collections

/** The package report against a fake Kiwi: only Kiwi's packages are looked up and sent. */
class PackageReporterTest {
    private val server = MockWebServer()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val sent: MutableList<PackagesReport> = Collections.synchronizedList(mutableListOf())

    @Volatile
    private var appsStatus = 200

    /** The phone: a few of Kiwi's apps and apps Kiwi has nothing to do with. */
    private val phone = FakePhone(
        "io.github.digitalpals.kiwi",
        "app.morphe.android.youtube",
        "com.example.orphan",
        "com.example.bank",
        "com.example.messenger",
    )
    private lateinit var reporter: PackageReporter

    @Before
    fun setUp() {
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when (request.url.encodedPath) {
                "/api/apps" -> if (request.url.queryParameter("hidden") != "1") {
                    MockResponse.Builder().code(400).build()
                } else {
                    json(appsStatus, APPS)
                }
                "/api/library" -> json(200, LIBRARY)
                "/api/me/packages" -> {
                    sent += KiwiJson.decodeFromString(PackagesReport.serializer(), request.body!!.utf8())
                    MockResponse.Builder().code(204).build()
                }
                else -> MockResponse.Builder().code(404).build()
            }
        }
        server.start()
        val api = KiwiApi(server.url("/"), OkHttpClient())
        reporter = PackageReporter(phone, scope, api = { api }, ownPackage = "io.github.digitalpals.kiwi")
    }

    @After
    fun tearDown() {
        scope.cancel()
        server.close()
    }

    @Test
    fun reportsOnlyKiwisPackages() = runBlocking {
        assertTrue(reporter.report())
        // Looked up by name: Kiwi itself, the Android apps Kiwi offers (hidden ones too) and its
        // recorded installs. The iOS app's id is never asked about.
        assertEquals(
            setOf("io.github.digitalpals.kiwi", "app.morphe.android.youtube", "com.reddit.frontpage", "com.example.orphan"),
            phone.asked.single(),
        )
        assertEquals(
            listOf("app.morphe.android.youtube", "com.example.orphan", "io.github.digitalpals.kiwi"),
            sent.single().packages.map { it.packageName },
        )
    }

    @Test
    fun sendsNothingWithoutKiwisAppList() = runBlocking {
        // Kiwi would take every package missing from the report for a removed app.
        appsStatus = 500
        assertFalse(reporter.report())
        assertTrue(sent.isEmpty())
        assertTrue(phone.asked.isEmpty())
    }

    private fun json(code: Int, body: String) =
        MockResponse.Builder().code(code).addHeader("Content-Type", "application/json").body(body).build()

    private class FakePhone(vararg installed: String) : PackageLookup {
        private val installed = installed.toSet()
        val asked: MutableList<Set<String>> = Collections.synchronizedList(mutableListOf())

        override fun scan(packages: Set<String>): List<ScannedPackage> {
            asked += packages
            return packages.filter { it in installed }.map {
                ScannedPackage(it, "1.0", 1, listOf(it.toByteArray()), "io.github.digitalpals.kiwi", system = false, updatedSystem = false)
            }
        }

        override fun find(packageName: String): InstalledPackage? = null

        override fun archive(path: String): ArchiveInfo? = null
    }

    private companion object {
        const val APPS = """[
            {"source_id":"kiwi-youtube","bundle_id":"app.morphe.android.youtube","platform":"android","name":"YouTube"},
            {"source_id":"kiwi-youtube","bundle_id":"com.reddit.frontpage","platform":"android","name":"Reddit","hidden":true},
            {"source_id":"altstore","bundle_id":"com.example.messenger","platform":"ios","name":"Messenger"}
        ]"""
        const val LIBRARY = """[
            {"source_id":"gone","bundle_id":"com.example.orphan","platform":"android","name":"Orphan","orphaned":true,
             "state":{"action":"open","installed":{"version":"1","signed_bundle_id":"com.example.orphan"}}}
        ]"""
    }
}
