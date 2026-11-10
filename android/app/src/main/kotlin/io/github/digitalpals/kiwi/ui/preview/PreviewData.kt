package io.github.digitalpals.kiwi.ui.preview

import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.AppDetail
import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.BuildFeature
import io.github.digitalpals.kiwi.api.BuildLimitation
import io.github.digitalpals.kiwi.api.Counts
import io.github.digitalpals.kiwi.api.Device
import io.github.digitalpals.kiwi.api.HostInfo
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.api.PublishedBuild
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.api.ServerInfo
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppItem
import io.github.digitalpals.kiwi.ui.AppPage
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.LocalPackage
import io.github.digitalpals.kiwi.ui.MyAppsViewModel
import java.time.Instant

/**
 * Made-up data for previews and screenshot tests, matching the approved mockup: YouTube,
 * YouTube Music and Reddit from Kiwi Builds on a Pixel. Nothing here talks to a server.
 */
object PreviewData {
    /** 09:41 on the day of the screenshots. */
    val NOW: Instant = Instant.parse("2026-11-10T09:41:00Z")

    private fun daysAgo(days: Long) = NOW.minusSeconds(days * 86_400).toString()

    const val SERVER = "https://kiwi.example.ts.net/"

    val absent = LocalPackage(installed = false, launchable = false)
    val installedSilently = LocalPackage(installed = true, launchable = true, silentUpdate = true)

    val youTube = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        sourceName = "Kiwi Builds",
        bundleId = "app.morphe.android.youtube",
        platform = "android",
        name = "YouTube",
        developer = "Private Kiwi build",
        subtitle = "No ads, background play",
        tintColor = "#FF0000",
        state = AppState(
            actionName = "install",
            target = Release("21.30.216", date = daysAgo(1), size = 156_000_000, compatible = true),
            installUrl = "${SERVER}android/t/youtube.apk",
        ),
    )

    val youTubeMusic = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        sourceName = "Kiwi Builds",
        bundleId = "app.morphe.android.apps.youtube.music",
        platform = "android",
        name = "YouTube Music",
        developer = "Private Kiwi build",
        subtitle = "No ads, background play",
        tintColor = "#FF0000",
        state = AppState(
            actionName = "update",
            installed = Installed("9.29.52", signedBundleId = "app.morphe.android.apps.youtube.music", verified = true),
            target = Release("9.30.53", date = daysAgo(2), size = 61_000_000),
            installUrl = "${SERVER}android/t/music.apk",
        ),
    )

    val reddit = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        sourceName = "Kiwi Builds",
        bundleId = "com.reddit.frontpage",
        platform = "android",
        name = "Reddit",
        developer = "Private Kiwi build",
        subtitle = "No promoted posts or ads",
        tintColor = "#FF4500",
        state = AppState(
            actionName = "open",
            installed = Installed("2026.40.0", signedBundleId = "com.reddit.frontpage", verified = true),
            target = Release("2026.40.0", date = daysAgo(3)),
        ),
    )

    val browse: List<AppItem> = listOf(
        AppItem(youTube, absent),
        AppItem(youTubeMusic, installedSilently),
        AppItem(reddit, installedSilently),
    )

    val freshKeys: List<String> = listOf(youTube.key, youTubeMusic.key)

    val kiwiOffer = AndroidApp(
        packageName = "io.github.digitalpals.kiwi",
        versionName = "1.1",
        versionCode = 2,
        size = 9_000_000,
        installUrl = "${SERVER}android/kiwi.apk",
    )

    val me = Me(
        device = Device(
            id = "d1",
            name = "pixel-9a",
            platform = "android",
            client = "android_app",
            osVersion = "16",
            model = "Google Pixel 9a",
        ),
        server = ServerInfo(version = "0.3.0", publicUrl = SERVER),
        counts = Counts(updates = 1, installed = 2),
        onboarded = true,
        host = HostInfo(os = "macos"),
        features = listOf("android_apps"),
        androidApp = kiwiOffer,
    )

    val myApps = MyAppsViewModel.Content(
        updates = listOf(AppItem(youTubeMusic, installedSilently)),
        installed = listOf(AppItem(reddit, installedSilently)),
        foreign = listOf(AppItem(youTube.copy(state = youTube.state.copy(foreignInstall = true)), LocalPackage(true, true))),
        selfUpdate = kiwiOffer,
    )

    /** YouTube Music installing: 62 % downloaded. */
    val downloading: Map<String, InstallJob> = mapOf(
        youTubeMusic.key to InstallJob(youTubeMusic.key, "YouTube Music", InstallJob.Phase.Downloading, progress = 0.62f),
    )

    private val youTubeIncluded = listOf(
        BuildFeature("ads", "Remove ads", "Video ads, banners and promoted results are gone."),
        BuildFeature("background", "Background playback", "Keeps playing with the screen off."),
    )
    private val youTubeOptional = listOf(
        BuildFeature("sponsorblock", "SponsorBlock", "Skips sponsored segments."),
        BuildFeature("dislikes", "Return YouTube Dislike", "Shows dislike counts again."),
        BuildFeature("shorts", "Hide Shorts", "Removes Shorts from the home feed."),
    )

    val youTubeRecipe = AutomationState(
        id = "android-youtube",
        platform = "android",
        name = "YouTube",
        bundleId = youTube.bundleId,
        tweak = "Morphe Patches 2.1.0",
        summary = "No ads, background play, SponsorBlock",
        limitations = listOf(
            BuildLimitation("key", "Signed with your Kiwi's key", "Remove another YouTube first; Android keeps one per package."),
            BuildLimitation("bell", "No Google sign-in", "Sign in with a browser account instead."),
        ),
        included = youTubeIncluded,
        availableTweaks = youTubeOptional,
        added = true,
        preferences = AutomationPreferences(enabled = true, intervalMinutes = 360, tweaks = listOf("sponsorblock", "dislikes")),
        jobs = listOf(AutomationJob("j3", "android-youtube", 3, "ready", "Available in Kiwi", "21.30.216", finishedAt = daysAgo(1))),
        published = PublishedBuild("21.30.216", revision = null, date = daysAgo(1), tweaks = listOf("sponsorblock", "dislikes")),
        nextCheckAt = "2026-11-10T14:00:00Z",
    )

    val musicRecipe = AutomationState(
        id = "android-youtube-music",
        platform = "android",
        name = "YouTube Music",
        bundleId = youTubeMusic.bundleId,
        tweak = "Morphe Patches 2.1.0",
        summary = "No ads, background play",
        included = listOf(BuildFeature("ads", "Remove ads"), BuildFeature("background", "Background playback")),
        added = true,
        preferences = AutomationPreferences(intervalMinutes = 360),
        jobs = listOf(AutomationJob("j9", "android-youtube-music", 9, "running", "Patching", "9.30.53")),
        published = PublishedBuild("9.29.52"),
    )

    val redditRecipe = AutomationState(
        id = "android-reddit",
        platform = "android",
        name = "Reddit",
        bundleId = reddit.bundleId,
        tweak = "Morphe Patches 2.1.0",
        summary = "No promoted posts or ads",
        warning = "Reddit may stop working with patched apps at any time.",
        limitations = listOf(BuildLimitation("info", "Links open in the app", "Reddit links from other apps open here.")),
        included = listOf(BuildFeature("ads", "Hide ads and promoted posts")),
        availableTweaks = listOf(
            BuildFeature("links", "Open links directly", "Skips Reddit's link redirect."),
            BuildFeature("share", "Clean shared links", "Removes tracking from shared links."),
        ),
        added = true,
        preferences = AutomationPreferences(intervalMinutes = 1440, tweaks = listOf("links", "share")),
        jobs = listOf(AutomationJob("j5", "android-reddit", 5, "ready", "Available in Kiwi", "2026.40.0", finishedAt = daysAgo(3))),
        published = PublishedBuild("2026.40.0", tweaks = listOf("links", "share")),
        nextCheckAt = "2026-11-11T07:00:00Z",
    )

    val builds: List<AutomationState> = listOf(youTubeRecipe, musicRecipe, redditRecipe)

    /** Reddit not added yet, for the Add sheet. */
    val buildsToAdd: List<AutomationState> = listOf(
        redditRecipe.copy(added = false, jobs = emptyList(), published = null, nextCheckAt = null),
    )

    val tints: Map<String, String> = mapOf(
        youTube.bundleId to "#FF0000",
        youTubeMusic.bundleId to "#FF0000",
        reddit.bundleId to "#FF4500",
    )

    val youTubePage = AppPage(
        detail = AppDetail(
            sourceId = youTube.sourceId,
            sourceName = youTube.sourceName,
            bundleId = youTube.bundleId,
            platform = "android",
            name = "YouTube",
            developer = "Private Kiwi build",
            subtitle = "YouTube without ads",
            tintColor = "#FF0000",
            state = youTube.state,
            description = "Built privately from the original Google Play app and the pinned Morphe patches, then signed " +
                "with this Kiwi's own key. Ads are removed, playback continues in the background, and SponsorBlock " +
                "and Return YouTube Dislike are included.",
            releases = listOf(youTube.state.target!!),
        ),
        local = absent,
        recipe = youTubeRecipe,
    )

    /** YouTube installing from its page. */
    val youTubeInstalling: InstallJob =
        InstallJob(youTube.key, "YouTube", InstallJob.Phase.Downloading, progress = 0.62f)
}
