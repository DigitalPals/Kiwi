package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.work.BuildNotice
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class BuildInstallsTest {
    private val server = MockWebServer()
    private lateinit var api: KiwiApi
    private val pending = MutableStateFlow(setOf<String>())
    private val installed = mutableListOf<Pair<String, AppSummary>>()
    private val failures = mutableListOf<BuildNotice>()
    private var onPhone = false

    @Before
    fun setUp() {
        server.start()
        api = KiwiApi(server.url("/"), OkHttpClient())
    }

    @After
    fun tearDown() {
        server.close()
    }

    private fun json(code: Int, body: String) =
        MockResponse.Builder().code(code).addHeader("Content-Type", "application/json").body(body).build()

    private fun TestScope.installs() = BuildInstalls(
        scope = this,
        pending = pending,
        forget = { app -> pending.update { it - app } },
        api = { api },
        install = { app, summary -> installed += app to summary },
        isInstalled = { onPhone },
        foreground = MutableStateFlow(true),
        failed = { failures += it },
    )

    private fun overview(vararg apps: String) = json(200, """{"apps":[${apps.joinToString(",")}],"checks":[]}""")

    private fun youTube(job: String, published: Boolean) =
        """{"id":"android-youtube","platform":"android","name":"YouTube","bundle_id":"app.morphe.android.youtube",""" +
            """"added":true,"jobs":[$job],"published":${if (published) """{"version":"21.30.216"}""" else "null"}}"""

    private val catalogue = json(
        200,
        """[{"source_id":"kiwi-youtube","bundle_id":"app.morphe.android.youtube","platform":"android","name":"YouTube",""" +
            """"state":{"action":"install","install_url":"/android/t/youtube.apk"}},""" +
            """{"source_id":"other","bundle_id":"app.morphe.android.youtube","platform":"android","name":"Someone else's"}]""",
    )

    @Test
    fun waitsWhileTheBuildRunsAndInstallsItOnceReady() = runTest {
        pending.value = setOf("android-youtube")
        val installs = installs()
        server.enqueue(overview(youTube("""{"id":"j","status":"running","stage":"Patching"}""", published = false)))
        installs.check()
        assertEquals(setOf("android-youtube"), pending.value)
        assertTrue(installed.isEmpty())
        assertEquals(1, server.requestCount)

        server.enqueue(overview(youTube("""{"id":"j","status":"ready","finished_at":"2026-11-10T10:00:00Z"}""", published = true)))
        server.enqueue(catalogue)
        installs.check()
        assertEquals(emptySet<String>(), pending.value)
        assertEquals(listOf("android-youtube"), installed.map { it.first })
        assertEquals("kiwi-youtube", installed.single().second.sourceId)
        assertEquals("/api/automation", server.takeRequest().url.encodedPath)
        assertEquals("/api/automation", server.takeRequest().url.encodedPath)
        val apps = server.takeRequest()
        assertEquals("/api/apps", apps.url.encodedPath)
        assertEquals("1", apps.url.queryParameter("hidden"))
    }

    @Test
    fun aFailedBuildEndsTheWaitWithItsReason() = runTest {
        pending.value = setOf("android-youtube", "android-gone")
        server.enqueue(
            overview(
                youTube(
                    """{"id":"j","status":"failed","error":"Google Play refused the download.","finished_at":"2026-11-10T10:00:00Z"}""",
                    published = false,
                ),
            ),
        )
        installs().check()
        assertEquals(emptySet<String>(), pending.value)
        assertTrue(installed.isEmpty())
        assertEquals("Google Play refused the download.", failures.single().body)
        assertTrue(failures.single().failed)
    }

    @Test
    fun anInstalledCopyIsNotInstalledAgainAndTurningKiwiBuildsOffEndsEveryWait() = runTest {
        pending.value = setOf("android-youtube")
        onPhone = true
        val opened = json(
            200,
            """[{"source_id":"kiwi-youtube","bundle_id":"app.morphe.android.youtube","platform":"android","name":"YouTube",""" +
                """"state":{"action":"open"}}]""",
        )
        server.enqueue(overview(youTube("""{"id":"j","status":"ready","finished_at":"2026-11-10T10:00:00Z"}""", published = true)))
        server.enqueue(opened)
        installs().check()
        assertEquals(emptySet<String>(), pending.value)
        assertTrue(installed.isEmpty())

        pending.value = setOf("android-youtube", "android-reddit")
        server.enqueue(json(409, """{"error":{"code":"builds_off","message":"Kiwi Builds is turned off in Settings."}}"""))
        installs().check()
        assertEquals(emptySet<String>(), pending.value)
    }

    @Test
    fun anUnreachableKiwiKeepsTheWait() = runTest {
        pending.value = setOf("android-youtube")
        server.enqueue(json(502, """{"error":{"code":"http","message":"Bad gateway"}}"""))
        installs().check()
        assertEquals(setOf("android-youtube"), pending.value)
    }
}
