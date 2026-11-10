package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.BuildFeature
import io.github.digitalpals.kiwi.api.PublishedBuild
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class KiwiBuildsTest {
    private fun state(
        jobs: List<AutomationJob> = emptyList(),
        published: PublishedBuild? = PublishedBuild("21.30.216"),
        preferences: AutomationPreferences = AutomationPreferences(intervalMinutes = 360),
        changed: Boolean = false,
        added: Boolean = true,
        next: String? = null,
        platform: String = "android",
    ) = AutomationState(
        id = "android-youtube",
        platform = platform,
        name = "YouTube",
        bundleId = "app.morphe.android.youtube",
        tweak = "Morphe Patches 2.1.0",
        included = listOf(BuildFeature("ads", "Remove ads"), BuildFeature("bg", "Background playback")),
        availableTweaks = listOf(BuildFeature("sb", "SponsorBlock"), BuildFeature("rd", "Return YouTube Dislike"), BuildFeature("sh", "Hide Shorts")),
        added = added,
        preferences = preferences,
        jobs = jobs,
        published = published,
        configurationChanged = changed,
        nextCheckAt = next,
    )

    private val done = "2026-11-09T10:00:00Z"

    @Test
    fun describesAnAddedAppsBuildAndSchedule() {
        assertEquals("21.30.216 · every 6 hours", KiwiBuilds.statusLine(state()))
        assertEquals("21.30.216 · every day", KiwiBuilds.statusLine(state(preferences = AutomationPreferences(intervalMinutes = 1440))))
        assertEquals("21.30.216 · automatic builds off", KiwiBuilds.statusLine(state(preferences = AutomationPreferences(enabled = false))))
        assertEquals("21.30.216 · changes to build", KiwiBuilds.statusLine(state(changed = true)))
        assertEquals("Ready for your first build", KiwiBuilds.statusLine(state(published = null)))
        assertEquals(
            "21.30.216 · needs attention",
            KiwiBuilds.statusLine(state(jobs = listOf(AutomationJob("j", status = "failed", error = "Google Play refused", finishedAt = done)))),
        )
    }

    @Test
    fun describesRunningAndQueuedJobs() {
        val running = AutomationJob("j1", status = "running", stage = "Patching", version = "9.30.53")
        assertEquals("Building 9.30.53 · patching", KiwiBuilds.statusLine(state(jobs = listOf(running))))
        assertEquals("Waiting for YouTube Music", KiwiBuilds.jobLine(AutomationJob("j2", status = "queued", stage = "Waiting for YouTube Music")))
        assertEquals("Downloading", KiwiBuilds.jobLine(AutomationJob("j3", status = "running", stage = "Downloading")))
        assertEquals(running, KiwiBuilds.activeJob(state(jobs = listOf(running))))
    }

    @Test
    fun labelsIntervals() {
        assertEquals("Every day", KiwiBuilds.intervalLabel(1440))
        assertEquals("Every hour", KiwiBuilds.intervalLabel(60))
        assertEquals("Every 6 hours", KiwiBuilds.intervalLabel(360))
        assertEquals("every 45 minutes", KiwiBuilds.intervalLabel(45, lower = true))
        assertEquals("1 day", KiwiBuilds.shortInterval(1440))
        assertEquals("12 hours", KiwiBuilds.shortInterval(720))
        assertEquals("45 min", KiwiBuilds.shortInterval(45))
        assertEquals(listOf(45, 60, 180, 360, 720, 1440), KiwiBuilds.intervals(45))
        assertEquals(KiwiBuilds.INTERVALS, KiwiBuilds.intervals(360))
    }

    @Test
    fun namesThePatcher() {
        assertEquals("Morphe", KiwiBuilds.patcherName("Morphe Patches 2.1.0"))
        assertEquals("NeoFreeBird", KiwiBuilds.patcherName("NeoFreeBird 7.3.0"))
        assertEquals("Patches", KiwiBuilds.patcherName("Patches"))
    }

    @Test
    fun countsPatches() {
        assertEquals(4 to 5, KiwiBuilds.patchCount(state(), listOf("sb", "rd")))
        assertEquals(2 to 5, KiwiBuilds.patchCount(state(), listOf("unknown")))
        val built = state(published = PublishedBuild("1", tweaks = listOf("sb")), preferences = AutomationPreferences(tweaks = listOf("sb", "rd")))
        assertEquals(listOf("sb"), KiwiBuilds.builtTweaks(built))
        assertEquals(listOf("sb", "rd"), KiwiBuilds.builtTweaks(state(published = PublishedBuild("1"), preferences = AutomationPreferences(tweaks = listOf("sb", "rd")))))
    }

    @Test
    fun comparesPreferencesWithoutTweakOrder() {
        assertTrue(KiwiBuilds.samePreferences(AutomationPreferences(tweaks = listOf("a", "b")), AutomationPreferences(tweaks = listOf("b", "a"))))
        assertFalse(KiwiBuilds.samePreferences(AutomationPreferences(intervalMinutes = 60), AutomationPreferences(intervalMinutes = 360)))
    }

    @Test
    fun labelsTheBuildButton() {
        assertEquals("Build now", KiwiBuilds.buildLabel(state(published = null)))
        assertEquals("Build changes", KiwiBuilds.buildLabel(state(changed = true)))
        assertEquals("Check now", KiwiBuilds.buildLabel(state()))
    }

    @Test
    fun findsTheNextCheckAndRecipes() {
        val apps = listOf(
            state(next = "2026-11-10T15:00:00Z"),
            state(next = "2026-11-10T14:00:00Z").copy(id = "b"),
            state(next = "2026-11-10T09:00:00Z", added = false).copy(id = "c"),
        )
        assertEquals(Instant.parse("2026-11-10T14:00:00Z"), KiwiBuilds.nextCheck(apps))
        assertNull(KiwiBuilds.nextCheck(listOf(state())))
        assertEquals("android-youtube", KiwiBuilds.recipeFor(apps, "app.morphe.android.youtube")?.id)
        assertNull(KiwiBuilds.recipeFor(listOf(state(platform = "ios")), "app.morphe.android.youtube"))
        assertEquals(1, KiwiBuilds.androidApps(listOf(state(), state(platform = "ios"))).size)
    }
}
