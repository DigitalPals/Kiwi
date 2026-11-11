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
import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunityCandidate
import io.github.digitalpals.kiwi.api.CommunityIndex
import io.github.digitalpals.kiwi.api.CommunityInfo
import io.github.digitalpals.kiwi.api.CommunityOverview
import io.github.digitalpals.kiwi.api.CommunityUpdate
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
import io.github.digitalpals.kiwi.ui.BuildsViewModel
import io.github.digitalpals.kiwi.ui.Community
import io.github.digitalpals.kiwi.ui.CommunitySearch
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

    val tints: Map<String, String> = mapOf(
        youTube.bundleId to "#FF0000",
        youTubeMusic.bundleId to "#FF0000",
        reddit.bundleId to "#FF4500",
    )

    /** Reddit not added yet, for the sheet before its first build. */
    val redditToInstall: AutomationState =
        redditRecipe.copy(added = false, jobs = emptyList(), published = null, nextCheckAt = null)

    /** What blocks Reddit: the Google Play app is installed with Google's signature. */
    val redditBlocker: String =
        "Reddit is already installed on this phone from somewhere else (with another signature). Android can't " +
            "replace it with Kiwi's build: uninstall it first. Its data on this phone is lost."

    /** Builds with the published apps on the phone: YouTube to install, Music to update, Reddit to open. */
    val buildsContent = BuildsViewModel.Content(
        off = false,
        apps = builds,
        tints = tints,
        published = mapOf(youTube.bundleId to youTube, youTubeMusic.bundleId to youTubeMusic, reddit.bundleId to reddit),
        local = mapOf(youTube.bundleId to absent, youTubeMusic.bundleId to installedSilently, reddit.bundleId to installedSilently),
    )

    /** Nothing built yet: every app offers Install. */
    val buildsFirstRun = BuildsViewModel.Content(
        off = false,
        apps = builds.map { it.copy(added = false, jobs = emptyList(), published = null, nextCheckAt = null) },
        tints = tints,
    )

    /**
     * First builds on their way, all asked for with Install: YouTube building, YouTube Music
     * waiting behind it, Reddit failed.
     */
    val buildsInProgress = BuildsViewModel.Content(
        off = false,
        apps = listOf(
            youTubeRecipe.copy(
                jobs = listOf(AutomationJob("j1", "android-youtube", 1, "running", "Patching", "21.30.216", interactive = true)),
                published = null,
                nextCheckAt = null,
            ),
            musicRecipe.copy(
                jobs = listOf(AutomationJob("j2", "android-youtube-music", 1, "queued", "Waiting for YouTube", interactive = true, ahead = 1)),
                published = null,
            ),
            redditRecipe.copy(
                jobs = listOf(
                    AutomationJob(
                        "j3", "android-reddit", 1, "failed", "Needs attention", "2026.40.0",
                        error = "Google Play didn't offer Reddit 2026.40.0 to the build's device. Try again later.",
                        finishedAt = daysAgo(0),
                    ),
                ),
                published = null,
                nextCheckAt = null,
            ),
        ),
        tints = tints,
    )

    /** The apps waiting to be installed in [buildsInProgress]. */
    val pendingInstalls: Set<String> = setOf("android-youtube", "android-youtube-music", "android-reddit")

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

    // The Morphe community catalogue (step 4): apps by independent authors, built in a sandbox.

    private fun communityApp(pkg: String, name: String, category: String, installs: Long, patches: Int, bundles: Int = 2) =
        CommunityApp(
            id = Community.ID_PREFIX + pkg,
            packageName = pkg,
            name = name,
            category = category,
            installs = installs,
            bundles = bundles,
            patches = patches,
            featured = true,
        )

    val discord = communityApp("com.discord", "Discord", "Communication", 500_000_000, 79, bundles = 3)
    val popularCommunity: List<CommunityApp> = listOf(
        discord,
        communityApp("com.instagram.android", "Instagram", "Social", 5_000_000_000, 65),
        communityApp("com.twitter.android", "X", "News & Magazines", 1_000_000_000, 46, bundles = 4),
        communityApp("com.zhiliaoapp.musically", "TikTok", "Social", 1_000_000_000, 27),
        communityApp("tv.twitch.android.app", "Twitch", "Entertainment", 100_000_000, 42, bundles = 5),
        communityApp("com.pinterest", "Pinterest", "Lifestyle", 1_000_000_000, 35),
        communityApp("com.truecaller", "Truecaller", "Communication", 1_000_000_000, 12),
        communityApp("com.tumblr", "Tumblr", "Social", 100_000_000, 8, bundles = 1),
    )

    /** The bundle Twitch is built with: picked by Kiwi, with newer patches on offer. */
    val twitchInfo = CommunityInfo(
        packageName = "tv.twitch.android.app",
        repo = "jordanwoodson/lurk-morphe-patches",
        author = "jordanwoodson",
        tag = "v1.16.1",
        chosen = CommunityInfo.CHOSEN_AUTO,
        update = CommunityUpdate("v1.17.0", publishedAt = daysAgo(4)),
    )

    private val communityNotices = listOf(
        BuildLimitation("shield", "Third-party code, not reviewed by Kiwi", "These patches come from an independent author. Kiwi hasn't reviewed their code."),
        BuildLimitation("key", "Replaces the Play Store app", "Uninstall the Play Store app first; its data on this phone is lost."),
        BuildLimitation("bell", "Google sign-in and notifications may not work", "They are bound to the original app's signature."),
    )

    /** Twitch, built and installed, with an update of its patches offered. */
    val twitchCommunity = AutomationState(
        id = Community.ID_PREFIX + "tv.twitch.android.app",
        platform = "android",
        name = "Twitch",
        bundleId = "tv.twitch.android.app",
        tweak = "lurk-morphe-patches v1.16.1",
        summary = "No ads, chat tweaks",
        limitations = communityNotices,
        included = listOf(
            BuildFeature("ads", "Block video ads", "Removes pre-roll and mid-roll ads."),
            BuildFeature("chat", "Show deleted messages", "Keeps moderated messages visible, crossed out."),
            BuildFeature("autoclaim", "Auto-claim channel points"),
            BuildFeature("emotes", "Third-party emotes", "BTTV, FFZ and 7TV emotes in chat."),
            BuildFeature("pip", "Picture in picture"),
            BuildFeature("quality", "Remember video quality"),
            BuildFeature("debug", "Debug mode"),
            BuildFeature("timestamps", "Chat timestamps"),
        ),
        added = true,
        jobs = listOf(AutomationJob("c1", "community-tv.twitch.android.app", 1, "ready", "Available in Kiwi", "31.8.0", finishedAt = daysAgo(6))),
        published = PublishedBuild("31.8.0", date = daysAgo(6)),
        community = twitchInfo,
    )

    val twitchSummary = AppSummary(
        sourceId = KiwiBuilds.SOURCE_ID,
        sourceName = "Kiwi Builds",
        bundleId = "tv.twitch.android.app",
        platform = "android",
        name = "Twitch",
        tintColor = "#9146FF",
        state = AppState(
            actionName = "open",
            installed = Installed("31.8.0", signedBundleId = "tv.twitch.android.app", verified = true),
            target = Release("31.8.0", date = daysAgo(6)),
        ),
    )

    /** Discord's first build running, to install when ready. */
    val discordBuilding = AutomationState(
        id = discord.id,
        platform = "android",
        name = "Discord",
        bundleId = "com.discord",
        tweak = "Venus Patches v1.21.1",
        summary = "Patches by VenusIsJaded",
        limitations = communityNotices,
        added = true,
        jobs = listOf(AutomationJob("c2", discord.id, 1, "running", "Patching Discord", "351.11", interactive = true)),
        community = CommunityInfo("com.discord", "VenusIsJaded/Venus-Patches", "VenusIsJaded", "v1.21.1"),
    )

    /** TikTok waiting while Kiwi chooses its patches. */
    val tikTokQueued = AutomationState(
        id = Community.ID_PREFIX + "com.zhiliaoapp.musically",
        platform = "android",
        name = "TikTok",
        bundleId = "com.zhiliaoapp.musically",
        added = true,
        jobs = listOf(AutomationJob("c3", "community-com.zhiliaoapp.musically", 1, "queued", "Waiting for Discord", interactive = true, ahead = 1)),
        community = CommunityInfo("com.zhiliaoapp.musically", resolving = true),
    )

    /** X failed with its patches: choose other patches. */
    val xFailed = AutomationState(
        id = Community.ID_PREFIX + "com.twitter.android",
        platform = "android",
        name = "X",
        bundleId = "com.twitter.android",
        tweak = "x-twitter-patches v1.80.0",
        added = true,
        jobs = listOf(
            AutomationJob(
                "c4", "community-com.twitter.android", 1, "failed", "Needs attention", "12.40.1",
                error = "These patches turned nothing on for X.",
                finishedAt = daysAgo(0),
                errorKind = "other_bundle",
            ),
        ),
        community = CommunityInfo("com.twitter.android", "serajr/x-twitter-patches", "serajr", "v1.80.0"),
    )

    /** Chess can't be built from what Google Play offers: remove it. */
    val chessUnavailable = AutomationState(
        id = Community.ID_PREFIX + "com.chess",
        platform = "android",
        name = "Chess",
        bundleId = "com.chess",
        added = true,
        jobs = listOf(
            AutomationJob(
                "c5", "community-com.chess", 1, "failed", "Needs attention",
                error = "No community patches work with a version Google Play offers.",
                finishedAt = daysAgo(1),
                errorKind = "unavailable",
            ),
        ),
        community = CommunityInfo("com.chess"),
    )

    val communityAdded: List<AutomationState> = listOf(twitchCommunity, discordBuilding, tikTokQueued, xFailed, chessUnavailable)

    /** The apps of [buildsCommunity] waiting to be installed. */
    val communityPending: Set<String> = setOf(discordBuilding.id, tikTokQueued.id)

    val communityIndex = CommunityIndex(updatedAt = daysAgo(0), apps = 1713, bundles = 496)

    /** Builds with the community section on a Linux Kiwi: added apps in every state, then Popular. */
    val buildsCommunity = buildsContent.copy(
        tints = tints + ("tv.twitch.android.app" to "#9146FF"),
        published = buildsContent.published + ("tv.twitch.android.app" to twitchSummary),
        local = buildsContent.local + ("tv.twitch.android.app" to installedSilently),
        community = CommunityOverview(
            available = true,
            index = communityIndex,
            popular = popularCommunity,
            added = communityAdded,
        ),
    )

    const val MAC_REASON = "Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the patcher in a sandbox."

    /** John's Kiwi on a Mac: browse only. */
    val buildsCommunityMac = buildsContent.copy(
        community = CommunityOverview(available = false, reason = MAC_REASON, index = communityIndex, popular = popularCommunity),
    )

    /** Before Kiwi's first fetch of the index. */
    val buildsCommunityLoading = buildsContent.copy(
        community = CommunityOverview(available = true, reason = "Kiwi is loading the community list…", index = null),
    )

    /** The community list couldn't be loaded. */
    val buildsCommunityError = buildsContent.copy(communityError = "Can't reach Kiwi at kiwi.example.ts.net. Is Tailscale on?")

    /** What blocks Discord: the Play Store app is installed. */
    val discordBlocker: String =
        "Discord is already installed on this phone from somewhere else (with another signature). Android can't " +
            "replace it with Kiwi's build: uninstall it first. Its data on this phone is lost."

    val twitchCandidates: List<CommunityCandidate> = listOf(
        CommunityCandidate(
            "jordanwoodson/lurk-morphe-patches", "jordanwoodson", stars = 214, updatedAt = daysAgo(3), patches = 42,
            versions = listOf("31.8.0"), recommended = true, chosen = true,
        ),
        CommunityCandidate("Overhypedcat/overhypedcat-twitch-patches", "Overhypedcat", stars = 57, updatedAt = daysAgo(12), patches = 4, versions = listOf("31.8.0")),
        CommunityCandidate(
            "arandomhooman/hoomans-morphe-patches", "arandomhooman", stars = 388, updatedAt = daysAgo(40), patches = 9,
            versions = listOf("30.9.0", "31.0.1", "31.2.0", "31.4.1"),
        ),
        CommunityCandidate(
            "kveld9/kveld-morphe-patches", "kveld9", stars = 1207, updatedAt = daysAgo(1), patches = 6, versions = emptyList(),
            failed = "A patch needs the internet while patching.",
        ),
    )

    /** The search with "chat" typed: a first page of matches, more to load. */
    val communitySearch = CommunitySearch.State(
        query = "chat",
        results = listOf(
            discord,
            communityApp("org.telegram.messenger", "Telegram", "Communication", 1_000_000_000, 31),
            communityApp("com.whatsapp", "WhatsApp Messenger", "Communication", 10_000_000_000, 18, bundles = 3),
            communityApp("com.snapchat.android", "Snapchat", "Communication", 1_000_000_000, 14),
            communityApp("tv.twitch.android.app", "Twitch", "Entertainment", 100_000_000, 42, bundles = 5).copy(added = true),
            communityApp("com.kick.mobile", "Kick", "Entertainment", 10_000_000, 7, bundles = 1),
        ),
        total = 23,
        loaded = true,
    )

    val communitySearchEmpty = CommunitySearch.State(query = "zzzz", loaded = true)

    val communitySearchFailed = CommunitySearch.State(query = "chat", error = "Can't reach Kiwi at kiwi.example.ts.net. Is Tailscale on?")
}
