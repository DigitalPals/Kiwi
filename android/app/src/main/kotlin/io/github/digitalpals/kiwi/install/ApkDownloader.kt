package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.KiwiException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Call
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * Downloads an APK from Kiwi (`/ota/<token>/app.apk` or `/android/kiwi.apk`) into the app's
 * storage. A partial file is resumed with `Range`, so a download cut off by a lost connection
 * or by Android stopping background work continues where it was. Redirects are not followed:
 * the APK comes from Kiwi itself.
 */
class ApkDownloader(client: OkHttpClient) {
    private val client: OkHttpClient = client.newBuilder()
        .followRedirects(false)
        .followSslRedirects(false)
        .build()

    /** Bytes so far and the total when known. */
    fun interface Progress {
        fun update(done: Long, total: Long?)
    }

    /**
     * Downloads [url] to [target]. [expectedSize] is the size Kiwi announced, when it did: a
     * partial file is only resumed when the server's file still has that size, so a partial of
     * an older file at the same URL (Kiwi's own app) is never continued with a newer one.
     */
    suspend fun download(url: HttpUrl, target: File, expectedSize: Long? = null, progress: Progress): File {
        target.parentFile?.mkdirs()
        val partial = File(target.path + ".part")
        val expected = expectedSize?.takeIf { it > 0 }
        if (expected != null && partial.length() > expected) partial.delete()
        try {
            fetch(url, partial, expected, progress)
        } catch (_: StalePartial) {
            // The partial file belongs to another file (or is complete but unverifiable): start over.
            partial.delete()
            fetch(url, partial, expected, progress)
        }
        withContext(Dispatchers.IO) {
            if (!partial.renameTo(target)) {
                partial.copyTo(target, overwrite = true)
                partial.delete()
            }
        }
        return target
    }

    private suspend fun fetch(url: HttpUrl, partial: File, expected: Long?, progress: Progress) = coroutineScope {
        val offset = if (partial.exists()) partial.length() else 0L
        val request = Request.Builder().url(url).apply {
            if (offset > 0) header("Range", "bytes=$offset-")
        }.build()
        val call = client.newCall(request)
        // Cancelling the download cancels the call at once, also while a read is blocked.
        val canceller = launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                awaitCancellation()
            } finally {
                call.cancel()
            }
        }
        try {
            withContext(Dispatchers.IO) { receive(call, url, partial, offset, expected, progress) }
        } finally {
            canceller.cancel()
        }
    }

    /** Runs [call] and writes its body to [partial]; blocking, on an IO thread. */
    private suspend fun receive(call: Call, url: HttpUrl, partial: File, offset: Long, expected: Long?, progress: Progress) {
        val context = currentCoroutineContext()
        try {
            call.execute().use { response ->
                if (response.code == 416) throw StalePartial()
                if (!response.isSuccessful) {
                    throw KiwiException.Http(response.code, "download", "The download failed (${response.code}).")
                }
                val append = response.code == 206 && offset > 0
                if (append && !resumes(response, offset, expected)) throw StalePartial()
                val total = totalSize(response.header("Content-Range"), response.body.contentLength(), if (append) offset else 0)
                var done = if (append) offset else 0L
                progress.update(done, total)
                FileOutputStream(partial, append).use { out ->
                    response.body.byteStream().use { input ->
                        val buffer = ByteArray(BUFFER)
                        var lastReport = 0L
                        while (true) {
                            context.ensureActive()
                            val read = input.read(buffer)
                            if (read < 0) break
                            out.write(buffer, 0, read)
                            done += read
                            if (done - lastReport >= REPORT_EVERY) {
                                lastReport = done
                                progress.update(done, total)
                            }
                        }
                    }
                    out.fd.sync()
                }
                if (total != null && done != total) {
                    throw KiwiException.BadResponse("The download ended early. Try again.")
                }
                progress.update(done, total)
            }
        } catch (error: IOException) {
            context.ensureActive()
            throw KiwiException.Network(url.host, error)
        }
    }

    /** Whether a `206` continues the partial file: from its end, of a file of the expected size. */
    private fun resumes(response: Response, offset: Long, expected: Long?): Boolean {
        val range = parseContentRange(response.header("Content-Range")) ?: return false
        if (range.first != offset) return false
        return expected == null || range.second == null || range.second == expected
    }

    private class StalePartial : Exception()

    companion object {
        private const val BUFFER = 64 * 1024
        private const val REPORT_EVERY = 256 * 1024L

        /**
         * The whole file's size: from `Content-Range: bytes a-b/total` for a resumed download,
         * else the offset plus `Content-Length`. Null when the server doesn't say.
         */
        fun totalSize(contentRange: String?, contentLength: Long, offset: Long): Long? {
            contentRange?.substringAfterLast('/', "")?.toLongOrNull()?.let { return it }
            return if (contentLength >= 0) offset + contentLength else null
        }

        /** The first byte and the total of `Content-Range: bytes a-b/total` (total null for `*`). */
        fun parseContentRange(header: String?): Pair<Long, Long?>? {
            val value = header?.trim()?.takeIf { it.startsWith("bytes ") } ?: return null
            val range = value.removePrefix("bytes ").substringBefore('/').trim()
            val first = range.substringBefore('-').trim().toLongOrNull() ?: return null
            return first to value.substringAfterLast('/', "").trim().toLongOrNull()
        }
    }
}
