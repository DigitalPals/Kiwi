// The JSON types of Kiwi's HTTP API (docs/api.md, web/src/api/types.ts), as far as the
// Android app uses them. Unknown fields are ignored and new fields have defaults, so the app
// keeps working with a Kiwi that is older or newer than it.
package io.github.digitalpals.kiwi.api

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** The JSON settings every request and response uses. */
val KiwiJson: Json = Json {
    ignoreUnknownKeys = true
    // A null where the app expects a value falls back to the property's default.
    coerceInputValues = true
}

object Platforms {
    const val IOS = "ios"
    const val ANDROID = "android"
}

/** `Me.features`: what Kiwi shows on this device. */
object Features {
    const val IOS_APPS = "ios_apps"
    const val ANDROID_APPS = "android_apps"
}

@Serializable
data class Health(
    val ok: Boolean = false,
    val version: String = "",
)

@Serializable
data class Device(
    val id: String,
    val name: String,
    val kind: String = "tailscale",
    @SerialName("tailscale_name") val tailscaleName: String? = null,
    /** The effective platform: "ios" or "android". */
    val platform: String = Platforms.IOS,
    /** "chosen", "detected" or "assumed" (newer servers). */
    @SerialName("platform_source") val platformSource: String? = null,
    @SerialName("detected_platform") val detectedPlatform: String? = null,
    /** "web" or "android_app": the client of this request (newer servers). */
    val client: String? = null,
    @SerialName("os_version") val osVersion: String? = null,
    @SerialName("os_version_source") val osVersionSource: String? = null,
    val model: String? = null,
    @SerialName("last_seen") val lastSeen: String? = null,
)

@Serializable
data class ServerInfo(
    val version: String = "",
    @SerialName("public_url") val publicUrl: String = "",
)

@Serializable
data class Counts(
    val updates: Int = 0,
    val installed: Int = 0,
    val sources: Int = 0,
    @SerialName("source_errors") val sourceErrors: Int = 0,
    val preparing: Int = 0,
)

@Serializable
data class Warning(
    val level: String = "info",
    val code: String = "",
    val message: String = "",
    val link: String? = null,
)

@Serializable
data class HostInfo(
    val os: String = "other",
    @SerialName("ios_signing") val iosSigning: String? = null,
    val devicectl: Boolean = false,
    /**
     * SHA-256 (hex) of the certificate Kiwi signs Android apps with (Kiwi Builds' apps and this
     * app); null until Kiwi Builds has loaded its key, and on older servers.
     */
    @SerialName("android_signer_sha256") val androidSignerSha256: String? = null,
)

/** The Kiwi Android app (this app) as the server can serve it. */
@Serializable
data class AndroidApp(
    @SerialName("package") val packageName: String,
    @SerialName("version_name") val versionName: String,
    @SerialName("version_code") val versionCode: Long,
    val size: Long = 0,
    @SerialName("install_url") val installUrl: String,
)

@Serializable
data class Me(
    val device: Device,
    val server: ServerInfo = ServerInfo(),
    val counts: Counts = Counts(),
    val warnings: List<Warning> = emptyList(),
    /** Null on a Kiwi that predates onboarding. */
    val onboarded: Boolean? = null,
    val host: HostInfo? = null,
    val features: List<String> = emptyList(),
    @SerialName("android_app") val androidApp: AndroidApp? = null,
)

@Serializable
data class Release(
    val version: String,
    val build: String? = null,
    val revision: Int? = null,
    val date: String? = null,
    val notes: String? = null,
    val size: Long? = null,
    @SerialName("min_os") val minOs: String? = null,
    @SerialName("max_os") val maxOs: String? = null,
    val compatible: Boolean = true,
    val incompatibility: String? = null,
)

@Serializable
data class Installed(
    val version: String,
    val build: String? = null,
    val revision: Int? = null,
    @SerialName("signed_bundle_id") val signedBundleId: String,
    /** "kiwi" or "device". */
    val origin: String = "kiwi",
    val verified: Boolean = false,
    @SerialName("installed_at") val installedAt: String? = null,
    @SerialName("expires_at") val expiresAt: String? = null,
)

@Serializable
data class Replaces(
    val name: String,
    @SerialName("source_name") val sourceName: String,
    val version: String,
)

@Serializable
data class Build(
    val id: String,
    @SerialName("source_id") val sourceId: String = "",
    @SerialName("bundle_id") val bundleId: String = "",
    val version: String = "",
    val build: String? = null,
    val revision: Int? = null,
    /** "queued", "downloading", "signing", "packaging", "ready" or "failed". */
    val status: String,
    val step: String? = null,
    val progress: Double? = null,
    val error: String? = null,
    @SerialName("signed_bundle_id") val signedBundleId: String = "",
    val size: Long? = null,
    @SerialName("expires_at") val expiresAt: String? = null,
    @SerialName("created_at") val createdAt: String? = null,
    @SerialName("finished_at") val finishedAt: String? = null,
    @SerialName("install_url") val installUrl: String? = null,
) {
    val isReady: Boolean get() = status == "ready"
    val isFailed: Boolean get() = status == "failed"
    val isActive: Boolean get() = !isReady && !isFailed
}

/** `AppState.action`. Values a newer Kiwi may add are [Unknown]. */
enum class Action(val wire: String) {
    Get("get"),
    Preparing("preparing"),
    Install("install"),
    Installing("installing"),
    Open("open"),
    Update("update"),
    Retry("retry"),
    Unavailable("unavailable"),
    Unknown("");

    companion object {
        fun parse(value: String): Action = entries.firstOrNull { it.wire == value && it != Unknown } ?: Unknown
    }
}

@Serializable
data class AppState(
    @SerialName("action") val actionName: String = "unavailable",
    @SerialName("update_reason") val updateReason: String? = null,
    val installed: Installed? = null,
    val target: Release? = null,
    val build: Build? = null,
    @SerialName("install_url") val installUrl: String? = null,
    @SerialName("open_url") val openUrl: String? = null,
    val replaces: Replaces? = null,
    /** The package is installed on this phone with another signature (newer servers). */
    @SerialName("foreign_install") val foreignInstall: Boolean = false,
) {
    val action: Action get() = Action.parse(actionName)
}

/** The App Store app an iOS app is based on. Android apps have none. */
@Serializable
data class Original(
    @SerialName("bundle_id") val bundleId: String = "",
    val name: String = "",
    @SerialName("short_name") val shortName: String = "",
    val version: String = "",
    @SerialName("app_version") val appVersion: String? = null,
    val status: String = "unknown",
)

@Serializable
data class AppSummary(
    @SerialName("source_id") val sourceId: String,
    @SerialName("source_name") val sourceName: String = "",
    @SerialName("bundle_id") val bundleId: String,
    val platform: String = Platforms.IOS,
    val name: String,
    val developer: String? = null,
    val subtitle: String? = null,
    @SerialName("icon_url") val iconUrl: String? = null,
    @SerialName("tint_color") val tintColor: String? = null,
    val category: String? = null,
    val unavailable: String? = null,
    val orphaned: Boolean = false,
    val hidden: Boolean = false,
    @SerialName("is_new") val isNew: Boolean = false,
    val original: Original? = null,
    val state: AppState = AppState(),
) {
    /** Apps are keyed by source and bundle id (library rows by signed id, see [libraryKey]). */
    val key: String get() = "$sourceId/$bundleId"

    /** Library rows are installs: one per signed bundle id. */
    val libraryKey: String get() = state.installed?.signedBundleId ?: key

    /** The Android package this app installs as (an Android app's `bundle_id`). */
    val packageName: String get() = state.installed?.signedBundleId ?: bundleId
}

@Serializable
data class Permissions(
    val entitlements: List<String> = emptyList(),
)

@Serializable
data class AppDetail(
    @SerialName("source_id") val sourceId: String,
    @SerialName("source_name") val sourceName: String = "",
    @SerialName("bundle_id") val bundleId: String,
    val platform: String = Platforms.IOS,
    val name: String,
    val developer: String? = null,
    val subtitle: String? = null,
    @SerialName("icon_url") val iconUrl: String? = null,
    @SerialName("tint_color") val tintColor: String? = null,
    val category: String? = null,
    val unavailable: String? = null,
    val orphaned: Boolean = false,
    val hidden: Boolean = false,
    @SerialName("is_new") val isNew: Boolean = false,
    val original: Original? = null,
    val state: AppState = AppState(),
    val description: String? = null,
    val screenshots: List<String> = emptyList(),
    val releases: List<Release> = emptyList(),
    @SerialName("signed_bundle_id") val signedBundleId: String = bundleId,
    @SerialName("source_url") val sourceUrl: String = "",
) {
    /** The summary fields, for code shared with list rows. */
    val summary: AppSummary
        get() = AppSummary(
            sourceId = sourceId, sourceName = sourceName, bundleId = bundleId, platform = platform,
            name = name, developer = developer, subtitle = subtitle, iconUrl = iconUrl,
            tintColor = tintColor, category = category, unavailable = unavailable,
            orphaned = orphaned, hidden = hidden, isNew = isNew, original = original, state = state,
        )
}

@Serializable
data class Settings(
    @SerialName("refresh_interval_minutes") val refreshIntervalMinutes: Int = 60,
    @SerialName("auto_prepare_updates") val autoPrepareUpdates: Boolean = true,
    @SerialName("notify_updates") val notifyUpdates: Boolean = true,
    @SerialName("notify_expiry") val notifyExpiry: Boolean = true,
    @SerialName("devicectl_sync") val devicectlSync: Boolean = true,
    @SerialName("devicectl_sync_minutes") val devicectlSyncMinutes: Int = 10,
    @SerialName("strip_watch_apps") val stripWatchApps: Boolean = true,
    @SerialName("app_store_country") val appStoreCountry: String = "us",
    @SerialName("kiwi_builds") val kiwiBuilds: Boolean = false,
)

@Serializable
data class PrepareRequest(
    val version: String? = null,
    val build: String? = null,
    val revision: Int? = null,
)

/** One package of `PUT /api/me/packages`. */
@Serializable
data class InstalledPackage(
    @SerialName("package") val packageName: String,
    @SerialName("version_name") val versionName: String,
    @SerialName("version_code") val versionCode: Long,
    /** SHA-256 of each signing certificate (DER), lowercase hex. */
    @SerialName("signer_sha256") val signerSha256: List<String>,
    /** The installer of record, or null when Android doesn't say. */
    val installer: String?,
)

@Serializable
data class PackagesReport(
    val packages: List<InstalledPackage>,
)

@Serializable
data class ErrorDetail(
    val code: String = "http",
    val message: String = "",
)

@Serializable
data class ErrorBody(
    val error: ErrorDetail,
)

/** Kiwi Builds preferences of one app (`PATCH /api/automation/:app` replaces them). */
@Serializable
data class AutomationPreferences(
    val enabled: Boolean = true,
    @SerialName("interval_minutes") val intervalMinutes: Int = 360,
    val tweaks: List<String> = emptyList(),
    /** Home Screen name, only for apps that offer a choice (`names`). */
    val name: String? = null,
)

/** One Kiwi Builds job. Poll while [finishedAt] is null. */
@Serializable
data class AutomationJob(
    val id: String,
    val app: String = "",
    val sequence: Int = 0,
    /** "queued", "running", "ready", "unchanged", "failed", "blocked" or "cancelled". */
    val status: String,
    /** What the job is doing, or for a queued job what it waits for. */
    val stage: String = "",
    val version: String? = null,
    val error: String? = null,
    @SerialName("created_at") val createdAt: String? = null,
    @SerialName("finished_at") val finishedAt: String? = null,
    val preferences: AutomationPreferences? = null,
    /** Someone asked for it (add, check); it starts before scheduled jobs. */
    val interactive: Boolean = false,
    /** A queued job: how many builds run before it starts, the running one included (newer Kiwis). */
    val ahead: Int? = null,
    /**
     * A failed community build: what would help, [ErrorKinds] ("retry", "other_bundle",
     * "unavailable"); null when Kiwi doesn't say.
     */
    @SerialName("error_kind") val errorKind: String? = null,
) {
    val isActive: Boolean get() = finishedAt == null
}

/** A patch: always built (`included`) or optional (`available_tweaks`). */
@Serializable
data class BuildFeature(
    val id: String,
    val name: String,
    val description: String = "",
)

@Serializable
data class BuildLimitation(
    val icon: String = "info",
    val title: String,
    val detail: String = "",
)

@Serializable
data class SetupCheck(
    val name: String,
    val ready: Boolean = false,
)

/** The build Kiwi Builds published last. */
@Serializable
data class PublishedBuild(
    val version: String,
    val build: String? = null,
    val revision: Int? = null,
    val date: String? = null,
    /** The optional patches it was built with; null for an older release. */
    val tweaks: List<String>? = null,
    val name: String? = null,
)

/** One app Kiwi Builds can patch and sign (`GET /api/automation/:app`). */
@Serializable
data class AutomationState(
    val id: String,
    val platform: String = Platforms.IOS,
    val name: String,
    /** The built app's bundle id; for Android the package the patched app installs as. */
    @SerialName("bundle_id") val bundleId: String = "",
    @SerialName("icon_url") val iconUrl: String? = null,
    /** The patches and their version, e.g. "Morphe Patches 2.1.0". */
    val tweak: String = "",
    val summary: String = "",
    val experimental: Boolean = false,
    val warning: String? = null,
    val names: List<String> = emptyList(),
    @SerialName("app_version") val appVersion: String? = null,
    val limitations: List<BuildLimitation> = emptyList(),
    val included: List<BuildFeature> = emptyList(),
    @SerialName("available_tweaks") val availableTweaks: List<BuildFeature> = emptyList(),
    val added: Boolean = false,
    val preferences: AutomationPreferences = AutomationPreferences(),
    val jobs: List<AutomationJob> = emptyList(),
    @SerialName("last_checked_at") val lastCheckedAt: String? = null,
    @SerialName("failed_attempts") val failedAttempts: Int = 0,
    val published: PublishedBuild? = null,
    @SerialName("source_enabled") val sourceEnabled: Boolean? = null,
    @SerialName("configuration_changed") val configurationChanged: Boolean = false,
    @SerialName("next_check_at") val nextCheckAt: String? = null,
    /** A Morphe community app: the patches it is built with. Null for Kiwi Builds' own apps. */
    val community: CommunityInfo? = null,
)

/** Every app Kiwi Builds can build (`GET /api/automation`). */
@Serializable
data class AutomationOverview(
    val apps: List<AutomationState> = emptyList(),
    val checks: List<SetupCheck> = emptyList(),
)

/** `AutomationJob.error_kind`: the one thing that helps after a failed community build. */
object ErrorKinds {
    /** Something passing went wrong (a download, a rate limit): try again. */
    const val RETRY = "retry"

    /** These patches failed or don't fit: choose another bundle. */
    const val OTHER_BUNDLE = "other_bundle"

    /** No bundle works with a version Google Play offers (or the app is paid, or not in this region). */
    const val UNAVAILABLE = "unavailable"
}

/** A newer stable release of the pinned bundle, offered once it is 72 hours old. */
@Serializable
data class CommunityUpdate(
    val tag: String,
    @SerialName("published_at") val publishedAt: String? = null,
)

/** The bundle a community app is built with (`AutomationState.community`). */
@Serializable
data class CommunityInfo(
    /** The original app's package. */
    @SerialName("package") val packageName: String = "",
    /** "owner/name" on GitHub; empty while Kiwi is still choosing. */
    val repo: String = "",
    val author: String = "",
    /** The release the build is pinned to; empty while Kiwi is still choosing. */
    val tag: String = "",
    /** "auto" (Kiwi picked it) or "manual" (the owner chose it). */
    val chosen: String = CHOSEN_AUTO,
    val update: CommunityUpdate? = null,
    /** Kiwi is picking a bundle (no tag yet). */
    val resolving: Boolean = false,
) {
    val isManual: Boolean get() = chosen == CHOSEN_MANUAL

    companion object {
        const val CHOSEN_AUTO = "auto"
        const val CHOSEN_MANUAL = "manual"
    }
}

/** One app of the Morphe community index (`GET /api/community`, search). */
@Serializable
data class CommunityApp(
    /** "community-<package>". */
    val id: String,
    @SerialName("package") val packageName: String = "",
    val name: String,
    @SerialName("icon_url") val iconUrl: String? = null,
    val category: String? = null,
    val installs: Long = 0,
    val description: String? = null,
    /** How many community bundles patch this app. */
    val bundles: Int = 0,
    /** The largest patch count any bundle has for it (informational). */
    val patches: Int = 0,
    val featured: Boolean = false,
    /** The owner added it (it is in `CommunityOverview.added`). */
    val added: Boolean = false,
)

/** The community index Kiwi has (null before its first successful fetch). */
@Serializable
data class CommunityIndex(
    @SerialName("updated_at") val updatedAt: String? = null,
    val apps: Int = 0,
    val bundles: Int = 0,
    /** Kiwi couldn't refresh it lately and uses its last good copy. */
    val stale: Boolean = false,
)

/** `GET /api/community`: the community section of Builds. */
@Serializable
data class CommunityOverview(
    /** Community builds work on this Kiwi (Linux or Docker with a working sandbox). */
    val available: Boolean = false,
    /** Why not, or that the list is still loading: one sentence to show as is. */
    val reason: String? = null,
    val index: CommunityIndex? = null,
    /** Up to 30 popular apps. */
    val popular: List<CommunityApp> = emptyList(),
    /** The community apps the owner installed or added, as Kiwi Builds states. */
    val added: List<AutomationState> = emptyList(),
)

/** `GET /api/community/search`: one page of matches and how many there are in all. */
@Serializable
data class CommunitySearchPage(
    val results: List<CommunityApp> = emptyList(),
    val total: Int = 0,
)

/** One bundle that can build a community app (`GET /api/community/:id/candidates`). */
@Serializable
data class CommunityCandidate(
    /** "owner/name". */
    val repo: String,
    val author: String = "",
    val stars: Int = 0,
    @SerialName("updated_at") val updatedAt: String? = null,
    val patches: Int = 0,
    /** The app versions it targets (empty: any version). */
    val versions: List<String> = emptyList(),
    val recommended: Boolean = false,
    /** The app is built with it now. */
    val chosen: Boolean = false,
    /** Why a build with it failed, or null. */
    val failed: String? = null,
)
