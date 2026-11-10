package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.KiwiException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * Downloads an APK from Kiwi (`/ota/<token>/app.apk` or `/android/kiwi.apk`) into the app's
 * storage. A partial file is resumed with `Range`, so a download cut off by a lost connection
 * or by Android stopping background work continues where it was.
 */
class ApkDownloader(private val client: OkHttpClient) {

    /** Bytes so far and the total when known. */
    fun interface Progress {
        fun update(done: Long, total: Long?)
    }

    suspend fun download(url: HttpUrl, target: File, progress: Progress): File = withContext(Dispatchers.IO) {
        target.parentFile?.mkdirs()
        val partial = File(target.path + ".part")
        try {
            fetch(url, partial, progress)
        } catch (error: RangeNotSatisfiable) {
            // The partial file belongs to another file (or is complete but unverifiable): start over.
            partial.delete()
            fetch(url, partial, progress)
        }
        if (!partial.renameTo(target)) {
            partial.copyTo(target, overwrite = true)
            partial.delete()
        }
        target
    }

    private suspend fun fetch(url: HttpUrl, partial: File, progress: Progress) {
        val offset = if (partial.exists()) partial.length() else 0L
        val request = Request.Builder().url(url).apply {
            if (offset > 0) header("Range", "bytes=$offset-")
        }.build()
        val call = client.newCall(request)
        val job = currentCoroutineContext()[kotlinx.coroutines.Job]
        val handle = job?.invokeOnCompletion { call.cancel() }
        try {
            call.execute().use { response ->
                if (response.code == 416) throw RangeNotSatisfiable()
                if (!response.isSuccessful) {
                    throw KiwiException.Http(response.code, "download", "The download failed (${response.code}).")
                }
                val append = response.code == 206 && offset > 0
                val total = totalSize(response.header("Content-Range"), response.body.contentLength(), if (append) offset else 0)
                var done = if (append) offset else 0L
                progress.update(done, total)
                FileOutputStream(partial, append).use { out ->
                    response.body.byteStream().use { input ->
                        val buffer = ByteArray(BUFFER)
                        var lastReport = 0L
                        while (true) {
                            currentCoroutineContext().ensureActive()
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
            currentCoroutineContext().ensureActive()
            throw KiwiException.Network(url.host, error)
        } finally {
            handle?.dispose()
        }
    }

    private class RangeNotSatisfiable : Exception()

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
    }
}
