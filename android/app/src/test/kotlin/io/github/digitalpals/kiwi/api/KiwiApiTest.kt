package io.github.digitalpals.kiwi.api

import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class KiwiApiTest {
    private val server = MockWebServer()
    private lateinit var api: KiwiApi

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

    @Test
    fun putsPackagesWithTheMutationHeaders() = runTest {
        server.enqueue(MockResponse.Builder().code(204).build())
        api.putPackages(PackagesReport(listOf(InstalledPackage("com.example.app", "1", 1, listOf("ab"), "com.android.vending"))))
        val request = server.takeRequest()
        assertEquals("PUT", request.method)
        assertEquals("/api/me/packages", request.url.encodedPath)
        assertEquals("1", request.headers["X-Kiwi"])
        assertTrue(request.headers["Content-Type"]!!.startsWith("application/json"))
        assertTrue(request.body!!.utf8().contains("\"signer_sha256\":[\"ab\"]"))
    }

    @Test
    fun encodesAppPathSegments() = runTest {
        server.enqueue(json(202, """{"id":"b1","status":"ready","install_url":"/ota/t/app.apk"}"""))
        val build = api.prepare("kiwi-youtube", "app.morphe.android.youtube")
        assertEquals("/ota/t/app.apk", build.installUrl)
        val request = server.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/api/apps/kiwi-youtube/app.morphe.android.youtube/prepare", request.url.encodedPath)
        assertEquals("{}", request.body!!.utf8())
    }

    @Test
    fun reportsNotOwnerAndUnidentifiedDevices() = runTest {
        server.enqueue(json(403, """{"error":{"code":"not_owner","message":"This Kiwi belongs to alex@example.com."}}"""))
        try {
            api.me()
            fail()
        } catch (error: KiwiException.Http) {
            assertTrue(error.isNotOwner)
            assertEquals("Not Your Kiwi", error.title())
            assertEquals("This Kiwi belongs to alex@example.com.", error.message)
        }
        server.enqueue(json(503, """{"error":{"code":"device_unidentified","message":"Couldn't identify this device through Tailscale. Try again."}}"""))
        try {
            api.me()
            fail()
        } catch (error: KiwiException.Http) {
            assertTrue(error.isDeviceUnidentified)
            assertEquals(503, error.status)
        }
    }

    @Test
    fun healthRejectsServersThatAreNotKiwi() = runTest {
        server.enqueue(MockResponse.Builder().code(200).body("<html>hello</html>").build())
        try {
            api.health()
            fail()
        } catch (error: KiwiException.BadResponse) {
            assertTrue(error.message!!.contains("doesn't look like a Kiwi"))
        }
        server.enqueue(json(200, """{"ok":true,"version":"1.0.0","pid":42}"""))
        assertEquals("1.0.0", api.health().version)
    }

    @Test
    fun networkFailuresAskAboutTailscale() = runTest {
        val unreachable = KiwiApi("https://127.0.0.1:1/".toHttpUrl(), OkHttpClient())
        try {
            unreachable.me()
            fail()
        } catch (error: KiwiException.Network) {
            assertTrue(error.message!!.contains("Is Tailscale on?"))
        }
    }

    @Test
    fun resolvesRelativeUrlsAndChecksTheOrigin() {
        val resolved = api.resolve("/api/icons/abc.png")!!
        assertEquals(server.url("/api/icons/abc.png"), resolved)
        assertTrue(api.isOwnUrl(resolved))
        assertTrue(!api.isOwnUrl("https://evil.example.com/app.apk".toHttpUrl()))
        assertNull(api.resolve("http://["))
    }

    @Test
    fun addsAndChangesKiwiBuildsAppsWithEveryPreference() = runTest {
        val state = """{"id":"android-youtube","platform":"android","name":"YouTube","added":true}"""
        server.enqueue(json(202, state))
        api.addAutomation("android-youtube", AutomationPreferences(enabled = true, intervalMinutes = 360, tweaks = listOf("sb")))
        val add = server.takeRequest()
        assertEquals("POST", add.method)
        assertEquals("/api/automation/android-youtube/add", add.url.encodedPath)
        assertEquals("1", add.headers["X-Kiwi"])
        // Defaults are sent too; a missing name stays out (Kiwi rejects one for apps without names).
        assertEquals("""{"enabled":true,"interval_minutes":360,"tweaks":["sb"]}""", add.body!!.utf8())

        server.enqueue(json(200, state))
        api.updateAutomation("android-youtube", AutomationPreferences(enabled = false, intervalMinutes = 60, name = "Twitter"))
        val patch = server.takeRequest()
        assertEquals("PATCH", patch.method)
        assertEquals("/api/automation/android-youtube", patch.url.encodedPath)
        assertEquals("""{"enabled":false,"interval_minutes":60,"tweaks":[],"name":"Twitter"}""", patch.body!!.utf8())
    }

    @Test
    fun buildsChecksCancelsAndRemoves() = runTest {
        val state = """{"id":"android-reddit","platform":"android","name":"Reddit","added":true}"""
        repeat(3) { server.enqueue(json(200, state)) }
        api.checkAutomation("android-reddit", force = true)
        api.cancelAutomation("android-reddit")
        assertEquals("Reddit", api.removeAutomation("android-reddit").name)
        val check = server.takeRequest()
        assertEquals("/api/automation/android-reddit/check", check.url.encodedPath)
        assertEquals("""{"force":true}""", check.body!!.utf8())
        assertEquals("/api/automation/android-reddit/cancel", server.takeRequest().url.encodedPath)
        val remove = server.takeRequest()
        assertEquals("DELETE", remove.method)
        assertEquals("1", remove.headers["X-Kiwi"])
        assertEquals("/api/automation/android-reddit", remove.url.encodedPath)
    }

    @Test
    fun reportsThatKiwiBuildsIsOff() = runTest {
        server.enqueue(json(409, """{"error":{"code":"builds_off","message":"Kiwi Builds is turned off in Settings."}}"""))
        try {
            api.automation()
            fail()
        } catch (error: KiwiException.Http) {
            assertTrue(error.isBuildsOff)
            assertEquals(409, error.status)
        }
        assertEquals("/api/automation", server.takeRequest().url.encodedPath)
        server.enqueue(json(200, """{"refresh_interval_minutes":60,"kiwi_builds":true}"""))
        val settings = api.patchSettings(kotlinx.serialization.json.buildJsonObject { put("kiwi_builds", kotlinx.serialization.json.JsonPrimitive(true)) })
        assertTrue(settings.kiwiBuilds)
        val request = server.takeRequest()
        assertEquals("PATCH", request.method)
        assertEquals("/api/settings", request.url.encodedPath)
        assertEquals("""{"kiwi_builds":true}""", request.body!!.utf8())
    }
}
