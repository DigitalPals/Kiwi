package io.github.digitalpals.kiwi.install

import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.KiwiException

/**
 * The apps Kiwi builds: Kiwi Builds' own (`/api/automation`) and the owner's community apps
 * (`/api/community`, `added`). [communityKnown] is false when the community list couldn't be
 * read this time: then a community app missing from [apps] may still exist.
 */
data class BuildApps(val apps: List<AutomationState>, val communityKnown: Boolean)

/**
 * Loads [BuildApps]. `409 builds_off` passes through (from `/api/automation`); a Kiwi without
 * the community catalogue (404) has no community apps.
 */
suspend fun KiwiApi.buildApps(): BuildApps {
    val apps = automation().apps
    val added = try {
        community().added
    } catch (error: KiwiException.Http) {
        if (error.isBuildsOff) throw error
        return BuildApps(apps, communityKnown = error.status == 404)
    } catch (error: KiwiException) {
        return BuildApps(apps, communityKnown = false)
    }
    val ids = apps.map { it.id }.toSet()
    return BuildApps(apps + added.filter { it.id !in ids }, communityKnown = true)
}
