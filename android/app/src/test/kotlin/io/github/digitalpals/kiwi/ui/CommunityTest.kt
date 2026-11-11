package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunityCandidate
import io.github.digitalpals.kiwi.api.CommunityInfo
import io.github.digitalpals.kiwi.api.CommunityUpdate
import io.github.digitalpals.kiwi.api.PublishedBuild
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class CommunityTest {
    private val info = CommunityInfo("com.discord", "VenusIsJaded/Venus-Patches", "VenusIsJaded", "v1.21.1")

    private fun discord(
        jobs: List<AutomationJob> = emptyList(),
        published: PublishedBuild? = null,
        community: CommunityInfo? = info,
    ) = AutomationState(
        id = "community-com.discord",
        platform = "android",
        name = "Discord",
        bundleId = "com.discord",
        added = true,
        jobs = jobs,
        published = published,
        community = community,
    )

    private fun failed(kind: String?) =
        AutomationJob("j", status = "failed", error = "It didn't work.", finishedAt = "2026-11-10T10:00:00Z", errorKind = kind)

    private val summary = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        bundleId = "com.discord",
        platform = "android",
        name = "Discord",
        state = AppState(actionName = "open"),
    )

    private fun row(app: AutomationState, summary: AppSummary? = null, available: Boolean = true, pending: Boolean = false) =
        Community.row(app, summary, LocalPackage(summary != null, summary != null), null, pending, off = false, available = available)

    @Test
    fun knowsCommunityIdsAndDescribesCatalogueApps() {
        assertTrue(Community.isId("community-com.discord"))
        assertFalse(Community.isId("android-youtube"))
        val app = CommunityApp("community-com.discord", "com.discord", "Discord", category = "Communication", patches = 79)
        assertEquals("Communication · 79 patches", Community.meta(app))
        assertEquals("1 patch", Community.meta(app.copy(category = null, patches = 1)))
        assertEquals("", Community.meta(app.copy(category = " ", patches = 0)))
        assertEquals("Search all 1,713 community apps", Community.searchLabel(1713))
        assertEquals("Search community apps", Community.searchLabel(0))
        val target = Community.target(app.copy(packageName = ""))
        assertEquals("com.discord", target.packageName)
        assertNull(target.info)
        assertEquals(info, Community.target(discord()).info)
        // Before Kiwi picked a bundle, the sheet names none.
        assertNull(Community.target(discord(community = CommunityInfo("com.discord", resolving = true))).info)
    }

    @Test
    fun mapsEachFailureKindToOneAction() {
        assertEquals(BuildRowAction.Retry, row(discord(listOf(failed("retry")))).action)
        assertEquals(BuildRowAction.Retry, row(discord(listOf(failed(null)))).action)
        assertEquals(BuildRowAction.Retry, row(discord(listOf(failed("something_new")))).action)
        val other = row(discord(listOf(failed("other_bundle"))))
        assertEquals(BuildRowAction.ChoosePatches, other.action)
        assertTrue(other.error)
        assertEquals("It didn't work.", other.detail)
        val unavailable = row(discord(listOf(failed("unavailable"))))
        assertEquals(BuildRowAction.Remove, unavailable.action)
        assertEquals("Can't be built", unavailable.subtitle)
        assertEquals(CommunityFailure.OtherPatches, Community.failure(failed("other_bundle")))
        assertEquals("Choose other patches", CommunityFailure.OtherPatches.label)
    }

    @Test
    fun withoutTheSandboxNothingIsBuiltButInstalledCopiesKeepTheirButtons() {
        assertEquals(BuildRowAction.None, row(discord(listOf(failed("retry"))), available = false).action)
        assertEquals(BuildRowAction.None, row(discord(listOf(failed("other_bundle"))), available = false).action)
        assertEquals(BuildRowAction.Remove, row(discord(listOf(failed("unavailable"))), available = false).action)
        val built = row(discord(published = PublishedBuild("351.11")), summary, available = false)
        assertTrue(built.action is BuildRowAction.App)
    }

    @Test
    fun aBuildOnItsWayShowsItsQueueAndTheWish() {
        val queued = row(
            discord(listOf(AutomationJob("j", status = "queued", stage = "Choosing patches", ahead = 2))),
            pending = true,
        )
        assertEquals("Waiting — 2 ahead", queued.subtitle)
        assertTrue(queued.detail!!.startsWith("Installs when it's ready"))
        assertEquals(BuildRowAction.None, queued.action)
        val running = row(discord(listOf(AutomationJob("j", status = "running", stage = "Checking VenusIsJaded/Venus-Patches"))))
        assertTrue(running.building)
        assertEquals("Checking VenusIsJaded/Venus-Patches", running.subtitle)
    }

    @Test
    fun aPublishedBuildSaysWhosePatchesItHasAndOffersNewerOnes() {
        val app = discord(
            jobs = listOf(AutomationJob("j", status = "ready", version = "351.11", finishedAt = "2026-11-10T10:00:00Z")),
            published = PublishedBuild("351.11"),
        )
        val view = row(app, summary)
        assertEquals("351.11 · patches by VenusIsJaded", view.subtitle)
        assertNull(view.detail)
        assertTrue(view.action is BuildRowAction.App)
        val offered = row(app.copy(community = info.copy(update = CommunityUpdate("v1.22.0"))), summary)
        assertEquals("New patches: v1.21.1 → v1.22.0", offered.detail)
        // A failed rebuild keeps the build (and says what failed) rather than the offer.
        val broken = row(app.copy(jobs = listOf(failed("retry")) + app.jobs, community = info.copy(update = CommunityUpdate("v1.22.0"))), summary)
        assertEquals("351.11 · needs attention", broken.subtitle)
        assertEquals("It didn't work.", broken.detail)
    }

    @Test
    fun statusLines() {
        assertEquals("Not built yet", Community.statusLine(discord()))
        assertEquals("Choosing patches", Community.statusLine(discord(community = CommunityInfo("com.discord", resolving = true))))
        assertEquals("Needs attention", Community.statusLine(discord(listOf(failed("retry")))))
        assertEquals("351.11", Community.statusLine(discord(published = PublishedBuild("351.11"), community = CommunityInfo())))
        assertEquals("patches by owner", Community.byLine(CommunityInfo(repo = "owner/name")))
        assertNull(Community.byLine(CommunityInfo()))
        assertEquals("VenusIsJaded/Venus-Patches · v1.21.1", Community.bundleLine(info))
        assertEquals("Picked automatically", Community.chosenLabel(info))
        assertEquals("Chosen by you", Community.chosenLabel(info.copy(chosen = "manual")))
        assertNull(Community.updateLine(info))
        assertEquals("New patches: v2", Community.updateLine(CommunityInfo(update = CommunityUpdate("v2"))))
    }

    @Test
    fun noticesNameTheBundleOnceKnown() {
        val before = Community.notices("Discord", null)
        assertEquals(
            listOf("Third-party code, not reviewed by Kiwi", "Replaces the Play Store app", "Google sign-in and notifications may not work"),
            before.map { it.title },
        )
        assertTrue(before[0].detail.contains("Kiwi picks the best-matching patches; you can choose others later."))
        assertTrue(before[1].detail.contains("Uninstall the Play Store app first"))
        val after = Community.notices("Discord", info)
        assertTrue(after[0].detail.contains("VenusIsJaded/Venus-Patches, by VenusIsJaded"))
    }

    @Test
    fun describesCandidatesAndOrdersThem() {
        val now = Instant.parse("2026-11-10T09:41:00Z")
        val candidate = CommunityCandidate(
            "VenusIsJaded/Venus-Patches", "VenusIsJaded", stars = 1207, updatedAt = "2026-11-07T09:00:00Z", patches = 78,
            versions = listOf("351.11"),
        )
        assertEquals("VenusIsJaded · ★ 1,207 · updated 3 days ago", Community.candidateLine(candidate, now))
        assertEquals("owner · ★ 0", Community.candidateLine(CommunityCandidate("owner/name"), now))
        assertEquals("78 patches · for 351.11", Community.candidateDetail(candidate))
        assertEquals("for any version", Community.candidateDetail(candidate.copy(patches = 0, versions = emptyList())))
        assertEquals("for 1 and 2", Community.candidateDetail(candidate.copy(patches = 0, versions = listOf("1", "2"))))
        assertEquals("for 1, 2 and 2 more", Community.candidateDetail(candidate.copy(patches = 0, versions = listOf("1", "2", "3", "4"))))
        assertEquals("today", Community.ago(now.minusSeconds(3600), now))
        assertEquals("yesterday", Community.ago(now.minusSeconds(86_400), now))
        assertEquals("a month ago", Community.ago(now.minusSeconds(40L * 86_400), now))
        assertEquals("3 months ago", Community.ago(now.minusSeconds(95L * 86_400), now))
        assertEquals("2 years ago", Community.ago(now.minusSeconds(800L * 86_400), now))
        val ordered = Community.ordered(
            listOf(
                CommunityCandidate("a/plain"),
                CommunityCandidate("b/chosen", chosen = true),
                CommunityCandidate("c/recommended", recommended = true),
            ),
        )
        assertEquals(listOf("c/recommended", "b/chosen", "a/plain"), ordered.map { it.repo })
    }

    @Test
    fun retriesWithTheOwnersChoiceAndListsPopularAppsOnce() {
        assertNull(Community.retryRepo(discord()))
        assertEquals("VenusIsJaded/Venus-Patches", Community.retryRepo(discord(community = info.copy(chosen = "manual"))))
        val popular = listOf(
            CommunityApp("community-com.discord", name = "Discord"),
            CommunityApp("community-com.twitch", name = "Twitch"),
            CommunityApp("community-com.twitch", name = "Twitch again"),
        )
        assertEquals(listOf("Twitch"), Community.popular(popular, listOf(discord())).map { it.name })
    }

    @Test
    fun patcherNamesDropVersionTags() {
        assertEquals("Venus", KiwiBuilds.patcherName("Venus Patches v1.21.1"))
        assertEquals("lurk-morphe-patches", KiwiBuilds.patcherName("lurk-morphe-patches v1.16.1"))
        assertEquals("Morphe", KiwiBuilds.patcherName("Morphe Patches 2.1.0"))
        assertEquals("vanced", KiwiBuilds.patcherName("vanced"))
    }
}
