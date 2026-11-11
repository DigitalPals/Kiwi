package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.BuildFeature
import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.PublishedBuild
import io.github.digitalpals.kiwi.install.InstallJob
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

    private val catalogued = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        bundleId = "app.morphe.android.youtube",
        platform = "android",
        name = "YouTube",
        state = AppState(actionName = "install", installUrl = "/android/t/youtube.apk"),
    )
    private val absent = LocalPackage(installed = false, launchable = false)

    @Test
    fun anAppNotBuiltYetOffersInstallWithItsSummary() {
        val fresh = state(published = null, added = false).copy(summary = "No ads, background play")
        val row = KiwiBuilds.row(fresh, summary = null, local = null, job = null, pending = false, off = false)
        assertEquals("No ads, background play", row.subtitle)
        assertEquals(BuildRowAction.Install, row.action)
        // Kiwi Builds is off: the same row, Install asks to turn it on.
        assertEquals(BuildRowAction.Install, KiwiBuilds.row(fresh, null, null, null, pending = false, off = true).action)
        // Added before but never built (or its build was cancelled): Install again.
        assertEquals(BuildRowAction.Install, KiwiBuilds.row(state(published = null), null, null, null, false, false).action)
        val cancelled = state(published = null, jobs = listOf(AutomationJob("c", status = "cancelled", finishedAt = done)))
        assertEquals("Build cancelled", KiwiBuilds.row(cancelled, null, null, null, false, false).subtitle)
    }

    @Test
    fun aQueuedBuildShowsItsPlaceAndARunningOneItsStage() {
        val queued = AutomationJob("q", status = "queued", stage = "Waiting for Reddit", ahead = 2, interactive = true)
        val row = KiwiBuilds.row(state(published = null, jobs = listOf(queued)), null, null, null, pending = true, off = false)
        assertEquals("Waiting — 2 ahead", row.subtitle)
        assertFalse(row.building)
        assertEquals(BuildRowAction.None, row.action)
        assertEquals("Installs when it's ready · first builds take about 3–8 minutes", row.detail)
        assertEquals("Starting", KiwiBuilds.queueLine(queued.copy(ahead = 0)))
        // An older Kiwi has no place: what the job waits for.
        assertEquals("Waiting for Reddit", KiwiBuilds.queueLine(queued.copy(ahead = null)))

        val running = AutomationJob("r", status = "running", stage = "Patching", version = "21.30.216")
        val building = KiwiBuilds.row(state(published = null, jobs = listOf(running)), null, null, null, pending = false, off = false)
        assertEquals("Building 21.30.216 · patching", building.subtitle)
        assertTrue(building.building)
        assertNull(building.detail)
    }

    @Test
    fun aFailedFirstBuildShowsItsReasonAndRetry() {
        val failed = AutomationJob("f", status = "failed", error = "Google Play refused the download.", finishedAt = done)
        val row = KiwiBuilds.row(state(published = null, jobs = listOf(failed)), null, null, null, pending = false, off = false)
        assertEquals("Didn't build", row.subtitle)
        assertTrue(row.error)
        assertEquals("Google Play refused the download.", row.detail)
        assertEquals(BuildRowAction.Retry, row.action)
    }

    @Test
    fun aPublishedBuildUsesTheAppsButton() {
        val install = KiwiBuilds.row(state(), catalogued, absent, null, pending = false, off = false)
        assertEquals(BuildRowAction.App(catalogued, ButtonView.Install("Install")), install.action)
        assertEquals("21.30.216 · every 6 hours", install.subtitle)
        val opened = catalogued.copy(state = catalogued.state.copy(actionName = "open"))
        val open = KiwiBuilds.row(state(), opened, LocalPackage(installed = true, launchable = true), null, false, false)
        assertEquals(ButtonView.Open("app.morphe.android.youtube"), (open.action as BuildRowAction.App).view)
        // An update building keeps the button of the installed build.
        val running = AutomationJob("r", status = "running", stage = "Patching", version = "21.31.0")
        val updating = KiwiBuilds.row(state(jobs = listOf(running)), opened, LocalPackage(true, true), null, false, false)
        assertTrue(updating.building)
        assertTrue(updating.action is BuildRowAction.App)
        // This app's own install shows its progress.
        val downloading = InstallJob(catalogued.key, "YouTube", InstallJob.Phase.Downloading, progress = 0.5f)
        val progress = KiwiBuilds.row(state(), catalogued, absent, downloading, false, false)
        assertEquals(ButtonView.Progress("Downloading", 0.5f, cancellable = true), (progress.action as BuildRowAction.App).view)
    }

    @Test
    fun aPendingInstallEndsWhenItsBuildIsReadyFailsOrIsGone() {
        val running = AutomationJob("r", status = "running", stage = "Patching")
        assertEquals(PendingOutcome.Waiting, KiwiBuilds.pendingOutcome(state(published = null, jobs = listOf(running)), installable = false))
        val ready = AutomationJob("d", status = "ready", finishedAt = done)
        assertEquals(PendingOutcome.Ready, KiwiBuilds.pendingOutcome(state(jobs = listOf(ready)), installable = true))
        val failed = AutomationJob("f", status = "blocked", error = "Google Play needs a newer device profile.", finishedAt = done)
        val outcome = KiwiBuilds.pendingOutcome(state(published = null, jobs = listOf(failed)), installable = false)
        assertEquals(PendingOutcome.Failed(failed, "Google Play needs a newer device profile."), outcome)
        val cancelled = AutomationJob("c", status = "cancelled", finishedAt = done)
        assertEquals(PendingOutcome.Dropped, KiwiBuilds.pendingOutcome(state(published = null, jobs = listOf(cancelled)), false))
        assertEquals(PendingOutcome.Dropped, KiwiBuilds.pendingOutcome(state(added = false, published = null), false))
        // Never queued (the app closed before the request): nothing to wait for.
        assertEquals(PendingOutcome.Dropped, KiwiBuilds.pendingOutcome(state(published = null), false))
    }

    @Test
    fun findsCopiesOnThePhoneThatAKiwiBuildCantReplace() {
        val kiwi = "ab".repeat(32)
        val play = InstalledPackage("com.reddit.frontpage", "2026.40.0", 1, listOf("cd".repeat(32)), "com.android.vending")
        assertNull(KiwiBuilds.blocker("Reddit", installed = null, signer = kiwi))
        assertTrue(KiwiBuilds.blocker("Reddit", play, kiwi)!!.startsWith("Reddit is already installed on this phone from somewhere else"))
        assertNull(KiwiBuilds.blocker("Reddit", play.copy(signerSha256 = listOf(kiwi)), kiwi))
        // Kiwi's key isn't known: it might be Kiwi's own copy, so the warning says "if".
        assertTrue(KiwiBuilds.blocker("Reddit", play, signer = null)!!.contains("If it isn't from your Kiwi"))
        // Kiwi saw the phone report another signature.
        assertTrue(KiwiBuilds.blocker("Reddit", play.copy(signerSha256 = listOf(kiwi)), kiwi, foreign = true) != null)
    }

    @Test
    fun explainsTheFirstBuildAndListsEveryAndroidAppWhileOff() {
        assertEquals(
            "Kiwi builds YouTube from the original Google Play app with open-source patches and signs it with its own key. " +
                "The first build takes about 3–8 minutes; you can leave the app — Kiwi notifies you when it's ready.",
            KiwiBuilds.firstBuildNotice("YouTube"),
        )
        assertEquals(listOf("android-youtube", "android-youtube-music", "android-reddit"), KiwiBuilds.OFF_APPS.map { it.id })
        assertTrue(KiwiBuilds.OFF_APPS.all { it.platform == "android" && !it.added })
    }

    @Test
    fun aPublishedBuildNeedsInstallingUnlessThePhoneHasIt() {
        assertTrue(KiwiBuilds.needsInstall(catalogued, installed = false))
        val opened = catalogued.copy(state = catalogued.state.copy(actionName = "open"))
        assertFalse(KiwiBuilds.needsInstall(opened, installed = true))
        assertTrue(KiwiBuilds.needsInstall(opened, installed = false))
    }
}
