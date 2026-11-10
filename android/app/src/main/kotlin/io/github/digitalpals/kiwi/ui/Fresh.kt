package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.work.UpdateNotices
import java.time.Instant
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.time.format.DateTimeParseException

/** A newer version of an installed app: "9.29.52" → "9.30.53". */
data class VersionChange(val from: String, val to: String)

/**
 * Browse's "Fresh this week" shelf and version changes, as in the web app
 * (web/src/lib/fresh.ts and `versionChange` in web/src/lib/action.ts).
 */
object Fresh {
    private const val WEEK_MS = 7 * 86_400_000L
    private const val MAX = 10

    /** Whether [installed] is exactly [release] (a build or revision that differs is newer). */
    fun isInstalledRelease(release: Release, installed: Installed?): Boolean {
        if (installed == null || installed.version != release.version) return false
        if (release.revision != installed.revision) return false
        return release.build == null || installed.build == null || release.build == installed.build
    }

    /** The version change of an update, also while it is preparing or installing; else null. */
    fun versionChange(state: AppState): VersionChange? {
        val installed = state.installed ?: return null
        val target = state.target ?: return null
        if (state.action == Action.Open || state.action == Action.Unavailable || state.action == Action.Get) return null
        if (isInstalledRelease(target, installed)) return null
        if (installed.version != target.version) return VersionChange(installed.version, target.version)
        return VersionChange(
            UpdateNotices.formatVersion(installed.version, installed.build, installed.revision),
            UpdateNotices.formatVersion(target),
        )
    }

    /**
     * Apps whose release came out in the last week and that still need something (get, update,
     * install), updates first, then the newest; at most ten.
     */
    fun apps(apps: List<AppSummary>, now: Instant = Instant.now()): List<AppSummary> {
        fun released(app: AppSummary): Long? = parseDate(app.state.target?.date)?.toEpochMilli()
        return apps
            .filter { app ->
                val action = app.state.action
                if (app.hidden || action == Action.Open || action == Action.Unavailable) return@filter false
                if (action == Action.Update && app.state.updateReason == "resign") return@filter false
                val date = released(app)
                date != null && now.toEpochMilli() - date <= WEEK_MS
            }
            .sortedWith(
                compareByDescending<AppSummary> { versionChange(it.state) != null }
                    .thenByDescending { released(it) ?: 0L },
            )
            .take(MAX)
    }

    /** The small label on a Fresh card. */
    fun label(app: AppSummary): String {
        val state = app.state
        if (versionChange(state) != null) {
            return if (state.installUrl != null && state.action == Action.Update) "Update ready" else "New version"
        }
        if (state.action == Action.Install && state.installUrl != null) return "Ready to install"
        return "New release"
    }

    /** An API date: RFC 3339, or a plain date taken as midnight UTC. Null when missing or bad. */
    fun parseDate(value: String?): Instant? {
        if (value.isNullOrBlank()) return null
        return try {
            if (DATE_ONLY.matches(value)) {
                LocalDate.parse(value).atStartOfDay().toInstant(ZoneOffset.UTC)
            } else {
                OffsetDateTime.parse(value).toInstant()
            }
        } catch (_: DateTimeParseException) {
            null
        }
    }

    private val DATE_ONLY = Regex("^\\d{4}-\\d{2}-\\d{2}$")
}
