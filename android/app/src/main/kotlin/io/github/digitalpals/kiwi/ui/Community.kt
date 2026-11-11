package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.AutomationJob
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.BuildLimitation
import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunityCandidate
import io.github.digitalpals.kiwi.api.CommunityInfo
import io.github.digitalpals.kiwi.api.ErrorKinds
import io.github.digitalpals.kiwi.install.InstallJob
import java.time.Instant
import java.time.temporal.ChronoUnit
import java.util.Locale

/** The one action after a failed community build (`AutomationJob.error_kind`). */
enum class CommunityFailure(val label: String) {
    /** Something passing went wrong: build again. */
    Retry("Retry"),

    /** The patches failed or don't fit: pick another bundle. */
    OtherPatches("Choose other patches"),

    /** No bundle works with what Google Play offers: nothing to do but remove it. */
    Unavailable("Remove"),
}

/** What the install sheet of a community app shows: a catalogue entry or an added app. */
data class CommunityTarget(
    val id: String,
    val name: String,
    val iconUrl: String?,
    /** The package the build installs as (the original's until Kiwi knows the output's). */
    val packageName: String,
    /** "Communication · 79 patches", or the added app's summary. */
    val detail: String,
    /** The bundle, once Kiwi picked one (an added app); null before the first build. */
    val info: CommunityInfo? = null,
)

/**
 * How the Morphe community catalogue reads in the app: apps by independent authors that Kiwi
 * builds in a sandbox, with the patches of one bundle each. Plain Kotlin, tested on the JVM.
 */
object Community {
    const val ID_PREFIX = "community-"

    /** The badge of every community app. */
    const val TAG = "Community · not reviewed"

    const val INTRO = "Patches by independent authors from the Morphe community. Kiwi hasn't reviewed them."

    /** While Kiwi has no index yet and doesn't say why. */
    const val LOADING = "Kiwi is loading the community list…"

    /** Whether [id] is a community app's (`community-<package>`). */
    fun isId(id: String): Boolean = id.startsWith(ID_PREFIX)

    /** "Communication · 79 patches", "1 patch", or "" when the index says nothing. */
    fun meta(app: CommunityApp): String = listOfNotNull(
        app.category?.trim()?.takeIf { it.isNotEmpty() },
        patchCount(app.patches),
    ).joinToString(" · ")

    fun patchCount(count: Int): String? = when {
        count <= 0 -> null
        count == 1 -> "1 patch"
        else -> "$count patches"
    }

    /** "1,713 community apps" for the search entry. */
    fun searchLabel(apps: Int): String =
        if (apps > 0) "Search all ${String.format(Locale.US, "%,d", apps)} community apps" else "Search community apps"

    fun target(app: CommunityApp): CommunityTarget =
        CommunityTarget(app.id, app.name, app.iconUrl, app.packageName.ifBlank { app.id.removePrefix(ID_PREFIX) }, meta(app))

    fun target(app: AutomationState): CommunityTarget = CommunityTarget(
        id = app.id,
        name = app.name,
        iconUrl = app.iconUrl,
        packageName = app.bundleId.ifBlank { app.community?.packageName ?: app.id.removePrefix(ID_PREFIX) },
        detail = app.summary.ifBlank { app.tweak },
        info = app.community?.takeIf { it.repo.isNotBlank() },
    )

    /** The row of an added community app: as a Kiwi Builds row, with community words and failure actions. */
    fun row(
        app: AutomationState,
        summary: AppSummary?,
        local: LocalPackage?,
        job: InstallJob?,
        pending: Boolean,
        off: Boolean,
        available: Boolean,
    ): BuildRowView {
        val base = KiwiBuilds.row(app, summary, local, job, pending, off)
        if (off) return base
        val active = KiwiBuilds.activeJob(app)
        var view = base
        if (active == null && summary != null) {
            view = view.copy(subtitle = statusLine(app))
            val update = app.community?.let(::updateLine)
            if (!view.error && update != null) view = view.copy(detail = update)
        }
        if (view.action == BuildRowAction.Retry) {
            view = when (failure(app.jobs.firstOrNull())) {
                CommunityFailure.Retry -> view
                CommunityFailure.OtherPatches -> view.copy(action = BuildRowAction.ChoosePatches)
                CommunityFailure.Unavailable -> view.copy(subtitle = "Can't be built", action = BuildRowAction.Remove)
            }
        }
        // Without the sandbox, nothing can be built here; installed copies keep their buttons.
        if (!available && view.action.builds) view = view.copy(action = BuildRowAction.None)
        return view
    }

    private val BuildRowAction.builds: Boolean
        get() = this == BuildRowAction.Install || this == BuildRowAction.Retry || this == BuildRowAction.ChoosePatches

    /** The action a failed [job] calls for; Retry when Kiwi doesn't say. */
    fun failure(job: AutomationJob?): CommunityFailure = when (job?.errorKind) {
        ErrorKinds.OTHER_BUNDLE -> CommunityFailure.OtherPatches
        ErrorKinds.UNAVAILABLE -> CommunityFailure.Unavailable
        else -> CommunityFailure.Retry
    }

    /** What the unavailable case means, under Kiwi's own reason. */
    const val UNAVAILABLE_NOTE =
        "None of the community patches work with a version Google Play offers you. Remove it; you can add it again later."

    /**
     * The second line of an added community app: the running job, "Choosing patches", or the
     * build and whose patches it has ("351.11 · patches by VenusIsJaded").
     */
    fun statusLine(app: AutomationState): String {
        KiwiBuilds.activeJob(app)?.let { return KiwiBuilds.jobLine(it) }
        val info = app.community
        val version = KiwiBuilds.publishedVersion(app)
        val by = info?.let(::byLine)
        return when {
            info?.resolving == true -> "Choosing patches"
            version == null && KiwiBuilds.needsAttention(app) -> "Needs attention"
            version == null -> "Not built yet"
            KiwiBuilds.needsAttention(app) -> "$version · needs attention"
            by != null -> "$version · $by"
            else -> version
        }
    }

    /** "patches by VenusIsJaded", or null before Kiwi picked a bundle. */
    fun byLine(info: CommunityInfo): String? {
        val author = info.author.ifBlank { info.repo.substringBefore('/') }
        return if (author.isBlank()) null else "patches by $author"
    }

    /** "VenusIsJaded/Venus-Patches · v1.21.1". */
    fun bundleLine(info: CommunityInfo): String = listOf(info.repo, info.tag).filter { it.isNotBlank() }.joinToString(" · ")

    fun chosenLabel(info: CommunityInfo): String = if (info.isManual) "Chosen by you" else "Picked automatically"

    /** "New patches: v1.21.1 → v1.22.0", or null without an offer. */
    fun updateLine(info: CommunityInfo): String? {
        val update = info.update ?: return null
        return if (info.tag.isBlank()) "New patches: ${update.tag}" else "New patches: ${info.tag} → ${update.tag}"
    }

    /** What Update does, for the app's sheet. */
    fun updateNotice(name: String, info: CommunityInfo): String {
        val from = info.author.ifBlank { info.repo }.ifBlank { "the same author" }
        return "Kiwi rebuilds $name with the newer patches from $from and installs it when it's ready. " +
            "Kiwi never updates community apps on its own."
    }

    /** The pre-install sheet's explanation of "Build and install". */
    fun firstBuildNotice(name: String): String =
        "Kiwi picks the best-matching community patches, builds $name from the original Google Play app in a " +
            "sandbox and signs it with its own key. The first build takes ${KiwiBuilds.BUILD_TIME}; you can leave the " +
            "app — Kiwi notifies you when it's ready."

    /** Below "Build and install". */
    const val NO_SCHEDULE_NOTICE =
        "Kiwi doesn't rebuild community apps on its own: it tells you when newer patches are out, and you choose when to update."

    /**
     * What to know before installing a community app; [info] names the bundle once Kiwi picked
     * one. Kiwi's own notices replace these once the app is added (`AutomationState.limitations`).
     */
    fun notices(name: String, info: CommunityInfo?): List<BuildLimitation> {
        val source = if (info != null && info.repo.isNotBlank()) {
            val author = info.author.takeIf { it.isNotBlank() }?.let { ", by $it" }.orEmpty()
            "These patches come from ${info.repo}$author. Kiwi hasn't reviewed their code, and some patches unlock paid features. " +
                "You can choose others later."
        } else {
            "Independent authors write these patches; Kiwi hasn't reviewed their code, and some patches unlock paid features. " +
                "Kiwi picks the best-matching patches; you can choose others later."
        }
        return listOf(
            BuildLimitation("shield", "Third-party code, not reviewed by Kiwi", source),
            BuildLimitation(
                "key",
                "Replaces the Play Store app",
                "Kiwi's $name uses the same package as the one from Google Play, so it can't sit beside it. " +
                    "Uninstall the Play Store app first; its data on this phone is lost.",
            ),
            BuildLimitation(
                "bell",
                "Google sign-in and notifications may not work",
                "Apps signed with your Kiwi's key can't use Google sign-in or push notifications meant for the original. " +
                    "Sign in with email or a password where the app allows it.",
            ),
        )
    }

    /** A candidate's first line: "VenusIsJaded · ★ 120 · updated 3 days ago". */
    fun candidateLine(candidate: CommunityCandidate, now: Instant): String = listOfNotNull(
        candidate.author.ifBlank { candidate.repo.substringBefore('/') }.takeIf { it.isNotBlank() },
        "★ ${String.format(Locale.US, "%,d", candidate.stars)}",
        Fresh.parseDate(candidate.updatedAt)?.let { "updated ${ago(it, now)}" },
    ).joinToString(" · ")

    /** A candidate's second line: "78 patches · for 351.11", "for any version", "for 1.2, 1.3 and 2 more". */
    fun candidateDetail(candidate: CommunityCandidate): String {
        val versions = candidate.versions.filter { it.isNotBlank() }
        val target = when {
            versions.isEmpty() -> "for any version"
            versions.size <= 2 -> "for ${versions.joinToString(" and ")}"
            else -> "for ${versions.take(2).joinToString(", ")} and ${versions.size - 2} more"
        }
        return listOfNotNull(patchCount(candidate.patches), target).joinToString(" · ")
    }

    /** "today", "yesterday", "3 days ago", "2 months ago", "a year ago". */
    fun ago(at: Instant, now: Instant): String {
        val days = ChronoUnit.DAYS.between(at, now).coerceAtLeast(0)
        return when {
            days < 1 -> "today"
            days == 1L -> "yesterday"
            days < 30 -> "$days days ago"
            days < 60 -> "a month ago"
            days < 365 -> "${days / 30} months ago"
            days < 730 -> "a year ago"
            else -> "${days / 365} years ago"
        }
    }

    /** The candidates in the order to show: recommended, the current one, then the rest as Kiwi sent them. */
    fun ordered(candidates: List<CommunityCandidate>): List<CommunityCandidate> =
        candidates.sortedWith(compareByDescending<CommunityCandidate> { it.recommended }.thenByDescending { it.chosen })

    /** The request body's repo for building [app] again: its own bundle when the owner chose it, else Kiwi's pick. */
    fun retryRepo(app: AutomationState): String? = app.community?.takeIf { it.isManual && it.repo.isNotBlank() }?.repo

    /** Popular apps not added yet (added ones are listed above them). */
    fun popular(popular: List<CommunityApp>, added: List<AutomationState>): List<CommunityApp> {
        val ids = added.map { it.id }.toSet()
        return popular.filterNot { it.id in ids }.distinctBy { it.id }
    }
}
