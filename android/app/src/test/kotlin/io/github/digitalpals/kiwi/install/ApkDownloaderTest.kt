package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.KiwiException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.TimeUnit

class ApkDownloaderTest {
    @get:Rule
    val temp = TemporaryFolder()

    private val server = MockWebServer()
    private val downloader = ApkDownloader(OkHttpClient.Builder().readTimeout(60, TimeUnit.SECONDS).build())
    private lateinit var target: File
    private lateinit var partial: File

    @Before
    fun setUp() {
        server.start()
        target = File(temp.newFolder("apks"), "app.apk")
        partial = File(target.path + ".part")
    }

    @After
    fun tearDown() {
        server.close()
    }

    @Test
    fun resumesAPartialDownloadOfTheSameFile() = runBlocking {
        partial.writeText("0123")
        server.enqueue(MockResponse.Builder().code(206).addHeader("Content-Range", "bytes 4-9/10").body("456789").build())
        downloader.download(server.url("/android/kiwi.apk"), target, expectedSize = 10) { _, _ -> }
        assertEquals("0123456789", target.readText())
        assertEquals("bytes=4-", server.takeRequest().headers["Range"])
    }

    @Test
    fun startsOverWhenTheServersFileIsNotTheExpectedOne() = runBlocking {
        // A partial of the previous version: the server's file now has another size.
        partial.writeText("OLD!")
        server.enqueue(MockResponse.Builder().code(206).addHeader("Content-Range", "bytes 4-9/10").body("456789").build())
        server.enqueue(MockResponse.Builder().code(200).body("new version!").build())
        downloader.download(server.url("/android/kiwi.apk"), target, expectedSize = 12) { _, _ -> }
        assertEquals("new version!", target.readText())
        assertEquals("bytes=4-", server.takeRequest().headers["Range"])
        assertNull(server.takeRequest().headers["Range"])
    }

    @Test
    fun dropsAPartialLargerThanTheExpectedFile() = runBlocking {
        partial.writeText("far too long for this file")
        server.enqueue(MockResponse.Builder().code(200).body("small").build())
        downloader.download(server.url("/android/kiwi.apk"), target, expectedSize = 5) { _, _ -> }
        assertEquals("small", target.readText())
        assertNull(server.takeRequest().headers["Range"])
    }

    @Test
    fun startsOverWhenTheRangeIsRefused() = runBlocking {
        partial.writeText("0123456789")
        server.enqueue(MockResponse.Builder().code(416).addHeader("Content-Range", "bytes */10").build())
        server.enqueue(MockResponse.Builder().code(200).body("0123456789").build())
        downloader.download(server.url("/ota/t/app.apk"), target) { _, _ -> }
        assertEquals("0123456789", target.readText())
    }

    @Test
    fun doesNotFollowRedirects() = runBlocking {
        val elsewhere = MockWebServer()
        elsewhere.start()
        try {
            server.enqueue(MockResponse.Builder().code(302).addHeader("Location", elsewhere.url("/evil.apk").toString()).build())
            try {
                downloader.download(server.url("/ota/t/app.apk"), target) { _, _ -> }
                fail()
            } catch (error: KiwiException.Http) {
                assertEquals(302, error.status)
            }
            assertEquals(0, elsewhere.requestCount)
        } finally {
            elsewhere.close()
        }
    }

    @Test
    fun cancellingStopsAStalledDownloadAtOnce() = runBlocking {
        // One byte every 10 seconds: after the first one, the read blocks.
        server.enqueue(MockResponse.Builder().code(200).body("x".repeat(100)).throttleBody(1, 10, TimeUnit.SECONDS).build())
        val download = async(Dispatchers.Default) {
            downloader.download(server.url("/ota/t/app.apk"), target) { _, _ -> }
        }
        withTimeout(10_000) { while (server.requestCount == 0) delay(10) }
        delay(500)
        val before = System.nanoTime()
        download.cancel()
        withTimeout(5_000) { download.join() }
        val waited = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - before)
        assertTrue("took $waited ms", waited < 5_000)
        assertTrue(download.isCancelled)
        assertFalse(target.exists())
    }

    @Test
    fun parsesContentRange() {
        assertEquals(4L to 10L, ApkDownloader.parseContentRange("bytes 4-9/10"))
        assertEquals(0L to null, ApkDownloader.parseContentRange("bytes 0-9/*"))
        assertNull(ApkDownloader.parseContentRange(null))
        assertNull(ApkDownloader.parseContentRange("items 1-2/3"))
    }
}
