package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationPreferences
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.work.UpdateNotices
import java.time.Instant

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
        if (words.size > 1 && words.last().first().isDigit()) words = words.dropLast(1)
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
