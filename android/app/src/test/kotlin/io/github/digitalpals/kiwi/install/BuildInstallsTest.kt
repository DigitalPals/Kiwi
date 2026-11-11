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

    private fun community(vararg added: String) =
        json(200, """{"available":true,"reason":null,"index":null,"popular":[],"added":[${added.joinToString(",")}]}""")

    private fun discord(job: String, published: Boolean) =
        """{"id":"community-com.discord","platform":"android","name":"Discord","bundle_id":"com.discord",""" +
            """"added":true,"jobs":[$job],"published":${if (published) """{"version":"351.11"}""" else "null"},""" +
            """"community":{"package":"com.discord","repo":"VenusIsJaded/Venus-Patches","author":"VenusIsJaded","tag":"v1.21.1"}}"""

    @Test
    fun installsACommunityAppOnceItsBuildIsReady() = runTest {
        pending.value = setOf("community-com.discord")
        val installs = installs()
        server.enqueue(overview())
        server.enqueue(community(discord("""{"id":"j","status":"running","stage":"Patching Discord"}""", published = false)))
        installs.check()
        assertEquals(setOf("community-com.discord"), pending.value)
        assertEquals("/api/automation", server.takeRequest().url.encodedPath)
        assertEquals("/api/community", server.takeRequest().url.encodedPath)

        server.enqueue(overview())
        server.enqueue(community(discord("""{"id":"j","status":"ready","finished_at":"2026-11-10T10:00:00Z"}""", published = true)))
        server.enqueue(
            json(
                200,
                """[{"source_id":"kiwi-youtube","bundle_id":"com.discord","platform":"android","name":"Discord",""" +
                    """"state":{"action":"install","install_url":"/android/t/discord.apk"}}]""",
            ),
        )
        installs.check()
        assertEquals(emptySet<String>(), pending.value)
        assertEquals("community-com.discord", installed.single().first)
        assertEquals("com.discord", installed.single().second.bundleId)
    }

    @Test
    fun aCommunityBuildThatNeedsOtherPatchesEndsTheWaitWithItsReason() = runTest {
        pending.value = setOf("community-com.discord")
        server.enqueue(overview())
        server.enqueue(
            community(
                discord(
                    """{"id":"j","status":"failed","error":"These patches failed on Discord 351.11.","error_kind":"other_bundle",""" +
                        """"finished_at":"2026-11-10T10:00:00Z"}""",
                    published = false,
                ),
            ),
        )
        installs().check()
        assertEquals(emptySet<String>(), pending.value)
        assertEquals("These patches failed on Discord 351.11.", failures.single().body)
        assertEquals("community-com.discord", failures.single().app)
    }

    @Test
    fun anUnreadableCommunityListKeepsTheWaitButAKiwiWithoutItEndsIt() = runTest {
        pending.value = setOf("community-com.discord", "android-youtube")
        server.enqueue(overview(youTube("""{"id":"j","status":"running","stage":"Patching"}""", published = false)))
        server.enqueue(json(502, """{"error":{"code":"http","message":"Bad gateway"}}"""))
        installs().check()
        assertEquals(setOf("community-com.discord", "android-youtube"), pending.value)

        server.enqueue(overview(youTube("""{"id":"j","status":"running","stage":"Patching"}""", published = false)))
        server.enqueue(json(404, """{"error":{"code":"not_found","message":"Not found."}}"""))
        installs().check()
        assertEquals(setOf("android-youtube"), pending.value)
    }

    @Test
    fun kiwiBuildsAppsAloneDontAskForTheCommunityList() = runTest {
        pending.value = setOf("android-youtube")
        server.enqueue(overview(youTube("""{"id":"j","status":"running","stage":"Patching"}""", published = false)))
        installs().check()
        assertEquals(1, server.requestCount)
    }
}
