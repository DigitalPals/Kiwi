package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.InstalledPackage
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.packages.PackageReport
import io.github.digitalpals.kiwi.work.UpdateNotices
import java.time.Instant

/** A Kiwi Builds row on the Builds tab: its second line, what runs below it and its button. */
data class BuildRowView(
    val subtitle: String,
    /** The second line is a problem (a failed build). */
    val error: Boolean = false,
    /** A build is running: a progress bar under the text. */
    val building: Boolean = false,
    /** A small line under the text: why a build failed, or what happens when it is ready. */
    val detail: String? = null,
    val action: BuildRowAction,
)

/** The button of a Kiwi Builds row. */
sealed interface BuildRowAction {
    /** Not built yet: the sheet that explains the first build (or turning Kiwi Builds on first). */
    data object Install : BuildRowAction

    /** The build failed: build again, and install when it is ready. */
    data object Retry : BuildRowAction

    /** A published build: the app's usual button (Install, Update, Open, its install's progress). */
    data class App(val summary: AppSummary, val view: ButtonView) : BuildRowAction

    /** A community build failed with these patches: choose another bundle. */
    data object ChoosePatches : BuildRowAction

    /** A community app no bundle can build: remove it. */
    data object Remove : BuildRowAction

    /** A build is on its way; the ⋮ menu can cancel it. */
    data object None : BuildRowAction
}

/** What became of an app the user asked to install when its build is ready. */
sealed interface PendingOutcome {
    /** Its build is queued or running. */
    data object Waiting : PendingOutcome

    /** Its build is published: install it (in the foreground) or say so (in the background). */
    data object Ready : PendingOutcome

    /** Its build failed; [reason] is the server's. */
    data class Failed(val job: AutomationJob, val reason: String) : PendingOutcome

    /** Nothing to wait for any more: the app was removed, its build cancelled or never queued. */
    data object Dropped : PendingOutcome
}

/**
 * How Kiwi Builds reads in the app, as in the web app (web/src/lib/automation.ts). Plain Kotlin,
 * tested on the JVM.
 */
object KiwiBuilds {
    /** The built-in source of Kiwi Builds' releases (its id predates the apps other than YouTube). */
    const val SOURCE_ID = "kiwi-youtube"

    /** The schedules the app offers; a different saved interval is offered too. */
    val INTERVALS: List<Int> = listOf(60, 180, 360, 720, 1440)

    /** The Android apps of the overview (this app manages only those). */
    fun androidApps(apps: List<AutomationState>): List<AutomationState> = apps.filter { it.platform == Platforms.ANDROID }

    fun activeJob(state: AutomationState): AutomationJob? = state.jobs.firstOrNull { it.isActive }

    /**
     * The Android apps of `automation/android.json`, for the rows shown while Kiwi Builds is off
     * (its API answers `builds_off` then). Kiwi's own list replaces them once it is on; a server
     * test checks these against the recipes.
     */
    val OFF_APPS: List<AutomationState> = listOf(
        offApp("android-youtube", "YouTube", "app.morphe.android.youtube", "No ads, background play, SponsorBlock"),
        offApp("android-youtube-music", "YouTube Music", "app.morphe.android.apps.youtube.music", "No ads, background play"),
        offApp("android-reddit", "Reddit", "com.reddit.frontpage", "No promoted posts or ads"),
    )

    private fun offApp(id: String, name: String, bundleId: String, summary: String) =
        AutomationState(id = id, platform = Platforms.ANDROID, name = name, bundleId = bundleId, summary = summary)

    /** How long a first build takes, as the app says it. */
    const val BUILD_TIME = "about 3–8 minutes"

    /** The pre-install sheet's explanation of what happens after "Build and install". */
    fun firstBuildNotice(name: String): String =
        "Kiwi builds $name from the original Google Play app with open-source patches and signs it with its own key. " +
            "The first build takes $BUILD_TIME; you can leave the app — Kiwi notifies you when it's ready."

    /**
     * A queued job's place: "Waiting — 2 ahead" (builds before it, the running one included),
     * "Starting" when it is next and the worker is free, else what the server says it waits for.
     */
    fun queueLine(job: AutomationJob): String = when (val ahead = job.ahead) {
        null -> job.stage.trim().ifEmpty { "Waiting" }
        0 -> "Starting"
        else -> "Waiting — $ahead ahead"
    }

    /**
     * The row of [app]: [summary] is its published build in Kiwi's catalogue (null before the
     * first build), [local] the phone's copy of it, [job] this app's install of it, [pending] the
     * user asked to install it when ready, [off] Kiwi Builds is turned off.
     */
    fun row(
        app: AutomationState,
        summary: AppSummary?,
        local: LocalPackage?,
        job: InstallJob?,
        pending: Boolean,
        off: Boolean,
    ): BuildRowView {
        if (off) return BuildRowView(app.summary, action = BuildRowAction.Install)
        val build = activeJob(app)
        val published = summary?.let { BuildRowAction.App(it, appButton(it, job, local ?: LocalPackage(false, false))) }
        if (build != null) {
            val waiting = if (pending) "Installs when it's ready · first builds take $BUILD_TIME" else null
            return BuildRowView(
                subtitle = if (build.status == "queued") queueLine(build) else jobLine(build),
                building = build.status == "running",
                detail = waiting,
                action = published ?: BuildRowAction.None,
            )
        }
        if (published != null) {
            return BuildRowView(
                statusLine(app),
                error = needsAttention(app),
                detail = attentionDetail(app),
                action = published,
            )
        }
        if (!app.added) return BuildRowView(app.summary, action = BuildRowAction.Install)
        if (needsAttention(app)) {
            return BuildRowView(
                "Didn't build",
                error = true,
                detail = attentionDetail(app) ?: "Open the app's settings to see what happened.",
                action = BuildRowAction.Retry,
            )
        }
        if (app.published != null && app.sourceEnabled == false) {
            return BuildRowView(statusLine(app), detail = "The Kiwi Builds source is turned off on your Kiwi.", action = BuildRowAction.None)
        }
        return BuildRowView(
            if (app.jobs.firstOrNull()?.status == "cancelled") "Build cancelled" else app.summary,
            action = BuildRowAction.Install,
        )
    }

    /**
     * What became of a pending install of [app]; [installable] when Kiwi's catalogue offers its
     * build to this phone. A pending install is only saved for an app without a published build,
     * so a published build is the one asked for.
     */
    fun pendingOutcome(app: AutomationState, installable: Boolean): PendingOutcome {
        val newest = app.jobs.firstOrNull()
        return when {
            !app.added -> PendingOutcome.Dropped
            activeJob(app) != null -> PendingOutcome.Waiting
            installable -> PendingOutcome.Ready
            newest != null && (newest.status == "failed" || newest.status == "blocked") ->
                PendingOutcome.Failed(newest, newest.error?.takeIf { it.isNotBlank() } ?: "Kiwi couldn't build ${app.name}.")
            else -> PendingOutcome.Dropped
        }
    }

    /** Whether a published build of an app needs installing (it isn't on the phone in this version). */
    fun needsInstall(summary: AppSummary, installed: Boolean): Boolean =
        !installed || summary.state.action == Action.Install || summary.state.action == Action.Update

    /**
     * Why the build of [name] couldn't be installed on this phone, found before spending minutes
     * on it: a copy of its package is [installed] with a signature that isn't Kiwi's ([signer],
     * `Me.host.android_signer_sha256`, when Kiwi knows it), or Kiwi says so ([foreign]). Null
     * when nothing is in the way.
     */
    fun blocker(name: String, installed: InstalledPackage?, signer: String?, foreign: Boolean = false): String? {
        if (installed == null) return null
        val other = foreign ||
            (signer != null && installed.signerSha256.isNotEmpty() && !PackageReport.signedBy(installed.signerSha256, signer))
        return when {
            other ->
                "$name is already installed on this phone from somewhere else (with another signature). Android can't " +
                    "replace it with Kiwi's build: uninstall it first. Its data on this phone is lost."
            signer == null ->
                "$name is already installed on this phone. If it isn't from your Kiwi, uninstall it first: Android " +
                    "can't replace an app signed by someone else, and its data on this phone is lost."
            else -> null
        }
    }

    fun needsAttention(state: AutomationState): Boolean =
        state.jobs.firstOrNull()?.status.let { it == "failed" || it == "blocked" }

    /** "Every 6 hours"; [lower] for use after a version ("21.30.216 · every 6 hours"). */
    fun intervalLabel(minutes: Int, lower: Boolean = false): String {
        val label = when {
            minutes == 1440 -> "Every day"
            minutes == 60 -> "Every hour"
            minutes % 60 == 0 -> "Every ${minutes / 60} hours"
            else -> "Every $minutes minutes"
        }
        return if (lower) label.replaceFirstChar { it.lowercase() } else label
    }

    /** A schedule chip's label: "1 hour", "6 hours", "1 day", "45 min". */
    fun shortInterval(minutes: Int): String = when {
        minutes == 1440 -> "1 day"
        minutes == 60 -> "1 hour"
        minutes % 60 == 0 -> "${minutes / 60} hours"
        else -> "$minutes min"
    }

    /** The intervals to offer for [current]: the usual ones and the saved one, in order. */
    fun intervals(current: Int): List<Int> = (INTERVALS + current).distinct().sorted()

    /** The published build's version, as the web app shows it ("21.30.216 · Kiwi 3"). */
    fun publishedVersion(state: AutomationState): String? = state.published?.let {
        UpdateNotices.formatVersion(it.version, it.build, it.revision)
    }

    /**
     * The second line of an added app's row: the running job ("Building 9.30.53 · patching"),
     * or the available build and its schedule ("21.30.216 · every 6 hours").
     */
    fun statusLine(state: AutomationState): String {
        activeJob(state)?.let { job -> return jobLine(job) }
        val version = publishedVersion(state)
        val schedule = if (state.preferences.enabled) {
            intervalLabel(state.preferences.intervalMinutes, lower = true)
        } else {
            "automatic builds off"
        }
        return when {
            version == null && needsAttention(state) -> "Needs attention"
            version == null -> if (state.preferences.enabled) "Ready for your first build" else "Automatic builds off"
            needsAttention(state) -> "$version · needs attention"
            state.configurationChanged -> "$version · changes to build"
            else -> "$version · $schedule"
        }
    }

    /** A job in words: "Building 9.30.53 · patching", or what a queued job waits for. */
    fun jobLine(job: AutomationJob): String {
        val stage = job.stage.trim()
        return when {
            job.status == "queued" -> stage.ifEmpty { "Queued" }
            job.version != null && stage.isNotEmpty() -> "Building ${job.version} · ${stage.replaceFirstChar { it.lowercase() }}"
            job.version != null -> "Building ${job.version}"
            else -> stage.ifEmpty { "Building" }
        }
    }

    /** The error of the newest job when it needs attention. */
    fun attentionDetail(state: AutomationState): String? =
        if (needsAttention(state)) state.jobs.firstOrNull()?.error?.takeIf { it.isNotBlank() } else null

    /** The next automatic check of any added app, or null when none is scheduled. */
    fun nextCheck(apps: List<AutomationState>): Instant? =
        apps.filter { it.added }.mapNotNull { Fresh.parseDate(it.nextCheckAt) }.minOrNull()

    /**
     * The patcher's name for "Patched with Morphe": the tweak without its version and a
     * trailing "Patches" ("Morphe Patches 2.1.0" → "Morphe", "NeoFreeBird 7.3.0" → "NeoFreeBird").
     */
    fun patcherName(tweak: String): String {
        var words = tweak.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }
        if (words.size > 1 && words.last().removePrefix("v").firstOrNull()?.isDigit() == true) words = words.dropLast(1)
        if (words.size > 1 && words.last().equals("Patches", ignoreCase = true)) words = words.dropLast(1)
        return words.joinToString(" ")
    }

    /**
     * How many patches the app is built with, of how many: the always-included ones plus the
     * chosen optional ones ([tweaks]).
     */
    fun patchCount(state: AutomationState, tweaks: List<String>): Pair<Int, Int> {
        val optional = state.availableTweaks.map { it.id }
        val on = state.included.size + tweaks.count { it in optional }
        return on to state.included.size + optional.size
    }

    /** The optional patches of the published build, else the saved choice. */
    fun builtTweaks(state: AutomationState): List<String> = state.published?.tweaks ?: state.preferences.tweaks

    /** Whether two preferences are the same (tweak order doesn't matter). */
    fun samePreferences(a: AutomationPreferences, b: AutomationPreferences): Boolean =
        a.enabled == b.enabled && a.intervalMinutes == b.intervalMinutes && a.name == b.name &&
            a.tweaks.sorted() == b.tweaks.sorted()

    /** The label of the build button: the first build, saved changes to build, or a check. */
    fun buildLabel(state: AutomationState): String = when {
        state.published == null -> "Build now"
        state.configurationChanged -> "Build changes"
        else -> "Check now"
    }

    /** The recipe that builds the app with [bundleId], when it is an Android app of Kiwi Builds. */
    fun recipeFor(apps: List<AutomationState>, bundleId: String): AutomationState? =
        apps.firstOrNull { it.platform == Platforms.ANDROID && it.bundleId == bundleId }

    /** What turning on Kiwi Builds means, shown before it is turned on (as in the web app). */
    const val TURN_ON_NOTICE =
        "Kiwi will download Android apps from Google Play, patch them with open-source patches and sign " +
            "them with your Kiwi's own key. Patched apps may break the app's terms of service. Use it only " +
            "for apps you may use, on your own devices, at your own risk."
}
