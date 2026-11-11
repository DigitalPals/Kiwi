package io.github.digitalpals.kiwi.work

import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.Platforms

/** One "build finished" notification. */
data class BuildNotice(
    /** Identifies the app's notification (one per Kiwi Builds app). */
    val tag: String,
    val title: String,
    val body: String,
    val failed: Boolean,
    /** The built app's package, for opening its page from a ready build's notification. */
    val bundleId: String? = null,
    val appName: String = "",
    /** For an app the user asked to install when ready: tapping opens Builds, which installs it. */
    val install: Boolean = false,
    /** The Kiwi Builds app (recipe id). */
    val app: String = "",
)

/** Which finished Kiwi Builds jobs to notify, and how they read. Free of Android, so it is tested. */
object BuildNotices {
    private val NOTIFIED_STATUSES = setOf("ready", "failed", "blocked")

    /** The finished jobs of added Android apps that are worth a notification. */
    fun finished(apps: List<AutomationState>): List<Pair<AutomationState, AutomationJob>> =
        apps.filter { it.added && it.platform == Platforms.ANDROID }
            .flatMap { app -> app.jobs.filter { !it.isActive && it.status in NOTIFIED_STATUSES }.map { app to it } }

    /**
     * The notification of [job]; [pending] when the user asked to install the app once it is
     * ready ("<App> is ready — tap to install", which opens Builds, where Kiwi installs it).
     */
    fun notice(app: AutomationState, job: AutomationJob, pending: Boolean = false): BuildNotice {
        val version = job.version?.let { " $it" }.orEmpty()
        return if (job.status == "ready" && pending) {
            BuildNotice(
                "build:${app.id}",
                "${app.name} is ready — tap to install",
                "Kiwi built ${app.name}$version and signed it with its own key.",
                failed = false,
                appName = app.name,
                install = true,
                app = app.id,
            )
        } else if (job.status == "ready") {
            BuildNotice(
                "build:${app.id}",
                "${app.name}$version is ready",
                "Kiwi built and signed it. Tap to install it.",
                failed = false,
                bundleId = app.bundleId,
                appName = app.name,
                app = app.id,
            )
        } else {
            BuildNotice(
                "build:${app.id}",
                "${app.name}$version didn't build",
                job.error?.takeIf { it.isNotBlank() } ?: "Open Builds to see what happened.",
                failed = true,
                appName = app.name,
                app = app.id,
            )
        }
    }

    /**
     * The notices for jobs not seen yet, and the job ids to remember. The first look ([seen] is
     * null) only remembers: builds that finished before notifications were on aren't news. A
     * ready build of a package in [updated] is left out: its update is notified (or installed).
     * Apps in [pending] (by recipe id) wait to be installed: their ready build says so.
     */
    fun fresh(
        apps: List<AutomationState>,
        seen: Set<String>?,
        updated: Set<String> = emptySet(),
        pending: Set<String> = emptySet(),
    ): Pair<List<BuildNotice>, Set<String>> {
        val finished = finished(apps)
        val ids = finished.map { it.second.id }.toSet()
        if (seen == null) return emptyList<BuildNotice>() to ids
        // The newest job of an app wins: one notification per app.
        val notices = finished.filter { it.second.id !in seen }
            .distinctBy { it.first.id }
            .filterNot { (app, job) -> job.status == "ready" && app.bundleId in updated }
            .map { (app, job) -> notice(app, job, pending = app.id in pending) }
        return notices to ids
    }
}
