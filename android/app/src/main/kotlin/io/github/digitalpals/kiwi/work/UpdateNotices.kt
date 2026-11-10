package io.github.digitalpals.kiwi.work

import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Release

/** One update notification. */
data class UpdateNotice(
    /** Identifies the app's notification (one per install). */
    val tag: String,
    /** Identifies this update: the app and the release it updates to. */
    val key: String,
    val title: String,
    val body: String,
)

/** Which updates to notify, and how they read. Kept free of Android so it can be tested. */
object UpdateNotices {
    fun notice(app: AppSummary): UpdateNotice {
        val target = app.state.target
        val installed = app.state.installed
        val change = when {
            target == null -> null
            installed == null -> "Version ${formatVersion(target)}"
            else -> versionChange(installed, target)
        }
        return UpdateNotice(
            tag = tag(app),
            key = key(app),
            title = "${app.name} update available",
            body = change ?: "A new version is available",
        )
    }

    /** "21.43.5 → 21.44.0", or with builds when only those differ. */
    fun versionChange(installed: Installed, target: Release): String =
        if (installed.version != target.version) {
            "${installed.version} → ${target.version}"
        } else {
            "${formatVersion(installed.version, installed.build, installed.revision)} → ${formatVersion(target)}"
        }

    /** As the web app shows versions: "1.2 (45) · Kiwi 3". */
    fun formatVersion(release: Release): String = formatVersion(release.version, release.build, release.revision)

    fun formatVersion(version: String, build: String?, revision: Int?): String {
        val base = if (build != null && build != version) "$version ($build)" else version
        return if (revision != null) "$base · Kiwi $revision" else base
    }

    /** Identifies [app]'s update notification, so it can be replaced or removed. */
    fun tag(app: AppSummary): String = "update:${app.libraryKey}"

    /** The key of the notice that Kiwi offers a newer version of this app. */
    fun selfKey(versionCode: Long): String = "$SELF_PREFIX$versionCode"

    fun key(app: AppSummary): String {
        val target = app.state.target
        return "${app.libraryKey}@${target?.version}/${target?.build}/${target?.revision}"
    }

    /**
     * The notices for updates not notified yet, and the keys to remember: only the current
     * updates' (so an update that went away and comes back is notified again) and the
     * self-update's.
     */
    fun fresh(updates: List<AppSummary>, notified: Set<String>): Pair<List<UpdateNotice>, Set<String>> {
        val notices = updates.map(::notice)
        val current = notices.map { it.key }.toSet()
        return notices.filter { it.key !in notified } to current + notified.filter { it.startsWith(SELF_PREFIX) }
    }

    private const val SELF_PREFIX = "self@"
}
