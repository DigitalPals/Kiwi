package io.github.digitalpals.kiwi.work

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.install.ApkDownloader
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class UpdateNoticesTest {
    private fun update(name: String, from: Installed?, to: Release) = AppSummary(
        sourceId = "kiwi-youtube",
        bundleId = "pkg.$name",
        platform = "android",
        name = name,
        state = AppState(actionName = "update", installed = from, target = to),
    )

    @Test
    fun describesTheVersionChange() {
        val notice = UpdateNotices.notice(
            update("YouTube", Installed("21.43.5", signedBundleId = "pkg.YouTube"), Release("21.44.0")),
        )
        assertEquals("YouTube update available", notice.title)
        assertEquals("21.43.5 → 21.44.0", notice.body)
        assertEquals("update:pkg.YouTube", notice.tag)

        val rebuild = UpdateNotices.notice(
            update("Reddit", Installed("2025.1", build = "1", revision = 1, signedBundleId = "pkg.Reddit"), Release("2025.1", build = "1", revision = 2)),
        )
        assertEquals("2025.1 (1) · Kiwi 1 → 2025.1 (1) · Kiwi 2", rebuild.body)
    }

    @Test
    fun notifiesEachUpdateOnce() {
        val youtube = update("YouTube", Installed("1", signedBundleId = "pkg.YouTube"), Release("2"))
        val (first, remember) = UpdateNotices.fresh(listOf(youtube), emptySet())
        assertEquals(1, first.size)
        val (second, _) = UpdateNotices.fresh(listOf(youtube), remember)
        assertEquals(0, second.size)
        // A newer release is a new notification.
        val newer = update("YouTube", Installed("1", signedBundleId = "pkg.YouTube"), Release("3"))
        assertEquals(1, UpdateNotices.fresh(listOf(newer), remember).first.size)
        // Updates that went away are forgotten; the self-update's notice is kept.
        val self = UpdateNotices.selfKey(3)
        assertEquals(setOf(self), UpdateNotices.fresh(emptyList(), remember + self).second)
    }

    @Test
    fun readsTheDownloadSize() {
        assertEquals(1000L, ApkDownloader.totalSize("bytes 200-999/1000", 800, 200))
        assertEquals(1000L, ApkDownloader.totalSize(null, 800, 200))
        assertEquals(500L, ApkDownloader.totalSize(null, 500, 0))
        assertNull(ApkDownloader.totalSize("bytes 0-1/*", -1, 0))
    }
}
