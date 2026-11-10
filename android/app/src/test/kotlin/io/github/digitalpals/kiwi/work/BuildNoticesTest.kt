package io.github.digitalpals.kiwi.work

import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BuildNoticesTest {
    private val done = "2026-11-10T08:00:00Z"

    private fun app(id: String, name: String, bundle: String, vararg jobs: AutomationJob, added: Boolean = true) =
        AutomationState(id = id, platform = "android", name = name, bundleId = bundle, added = added, jobs = jobs.toList())

    private val youTube = app(
        "android-youtube", "YouTube", "app.morphe.android.youtube",
        AutomationJob("y2", status = "ready", version = "21.31.0", finishedAt = done),
        AutomationJob("y1", status = "ready", version = "21.30.216", finishedAt = done),
    )
    private val reddit = app(
        "android-reddit", "Reddit", "com.reddit.frontpage",
        AutomationJob("r2", status = "failed", version = "2026.41.0", error = "Google Play refused the download.", finishedAt = done),
    )
    private val music = app(
        "android-youtube-music", "YouTube Music", "app.morphe.android.apps.youtube.music",
        AutomationJob("m2", status = "running", stage = "Patching"),
        AutomationJob("m1", status = "unchanged", finishedAt = done),
    )

    @Test
    fun theFirstLookOnlyRemembers() {
        val (notices, seen) = BuildNotices.fresh(listOf(youTube, reddit, music), seen = null)
        assertTrue(notices.isEmpty())
        assertEquals(setOf("y2", "y1", "r2"), seen)
    }

    @Test
    fun notifiesNewReadyAndFailedBuildsOncePerApp() {
        val (notices, seen) = BuildNotices.fresh(listOf(youTube, reddit, music), seen = setOf("y1"))
        assertEquals(
            listOf(
                BuildNotice(
                    "build:android-youtube",
                    "YouTube 21.31.0 is ready",
                    "Kiwi built and signed it. Tap to install it.",
                    failed = false,
                    bundleId = "app.morphe.android.youtube",
                    appName = "YouTube",
                ),
                BuildNotice(
                    "build:android-reddit",
                    "Reddit 2026.41.0 didn't build",
                    "Google Play refused the download.",
                    failed = true,
                    appName = "Reddit",
                ),
            ),
            notices,
        )
        assertEquals(setOf("y2", "y1", "r2"), seen)
        assertTrue(BuildNotices.fresh(listOf(youTube, reddit, music), seen).first.isEmpty())
    }

    @Test
    fun leavesReadyBuildsOfUpdatedAppsToTheUpdateNotice() {
        val (notices, _) = BuildNotices.fresh(listOf(youTube, reddit), seen = emptySet(), updated = setOf("app.morphe.android.youtube"))
        assertEquals(listOf("build:android-reddit"), notices.map { it.tag })
    }

    @Test
    fun ignoresAppsThatAreNotAddedOrNotAndroid() {
        val removed = youTube.copy(added = false)
        val ios = youTube.copy(id = "youtube", platform = "ios")
        assertTrue(BuildNotices.fresh(listOf(removed, ios), seen = emptySet()).first.isEmpty())
    }
}
