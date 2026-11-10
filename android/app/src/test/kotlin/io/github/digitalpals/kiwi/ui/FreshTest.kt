package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Release
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.time.Instant

/** The cases of the web app's web/src/lib/fresh.test.ts. */
class FreshTest {
    private val now = Instant.parse("2026-10-30T12:00:00Z")
    private val installed = Installed(version = "1.0", signedBundleId = "com.example.app", verified = true)

    private fun app(
        name: String,
        date: String?,
        action: String = "get",
        reason: String? = null,
        installed: Installed? = null,
        installUrl: String? = null,
        hidden: Boolean = false,
    ) = AppSummary(
        sourceId = "s",
        bundleId = "com.example.$name",
        name = name,
        hidden = hidden,
        state = AppState(
            actionName = action,
            updateReason = reason,
            installed = installed,
            target = Release("2.0", date = date),
            installUrl = installUrl,
        ),
    )

    @Test
    fun listsLastWeeksReleasesUpdatesFirstThenTheNewest() {
        val apps = listOf(
            app("old", "2026-10-01"),
            app("new", "2026-10-28"),
            app("newer", "2026-10-29T08:00:00Z"),
            app("update", "2026-10-24", "update", "new_version", installed),
            app("undated", null),
        )
        assertEquals(listOf("update", "newer", "new"), Fresh.apps(apps, now).map { it.name })
    }

    @Test
    fun keepsAnUpdateFirstWhileItIsPreparing() {
        val apps = listOf(app("new", "2026-10-29"), app("update", "2026-10-24", "preparing", installed = installed))
        assertEquals(listOf("update", "new"), Fresh.apps(apps, now).map { it.name })
    }

    @Test
    fun leavesOutWhatNeedsNothing() {
        val apps = listOf(
            app("open", "2026-10-29", "open"),
            app("unavailable", "2026-10-29", "unavailable"),
            app("hidden", "2026-10-29", hidden = true),
            app("resign", "2026-10-29", "update", "resign"),
            app("installing", "2026-10-29", "installing"),
        )
        assertEquals(listOf("installing"), Fresh.apps(apps, now).map { it.name })
    }

    @Test
    fun showsAtMostTen() {
        assertEquals(10, Fresh.apps((0 until 14).map { app("a$it", "2026-10-29") }, now).size)
    }

    @Test
    fun labelsWhetherAnUpdateOrInstallIsReady() {
        assertEquals("Update ready", Fresh.label(app("a", null, "update", "new_version", installed, installUrl = "https://k/a.apk")))
        assertEquals("New version", Fresh.label(app("a", null, "update", "new_version", installed)))
        assertEquals("New version", Fresh.label(app("a", null, "preparing", installed = installed)))
        assertEquals("Ready to install", Fresh.label(app("a", null, "install", installUrl = "https://k/a.apk")))
        assertEquals("New release", Fresh.label(app("a", null)))
    }

    @Test
    fun describesVersionChanges() {
        assertEquals(VersionChange("1.0", "2.0"), Fresh.versionChange(app("a", null, "update", installed = installed).state))
        assertNull(Fresh.versionChange(app("a", null, "open", installed = installed).state))
        assertNull(Fresh.versionChange(app("a", null, "update").state))
        // Only the build differs: versions with builds.
        val sameVersion = AppState(
            actionName = "update",
            installed = Installed("2.0", build = "40", signedBundleId = "x"),
            target = Release("2.0", build = "41"),
        )
        assertEquals(VersionChange("2.0 (40)", "2.0 (41)"), Fresh.versionChange(sameVersion))
        // The installed release itself is no change.
        val same = AppState(actionName = "update", installed = Installed("2.0", signedBundleId = "x"), target = Release("2.0"))
        assertNull(Fresh.versionChange(same))
    }

    @Test
    fun parsesApiDates() {
        assertEquals(Instant.parse("2026-10-29T00:00:00Z"), Fresh.parseDate("2026-10-29"))
        assertEquals(Instant.parse("2026-10-29T08:00:00Z"), Fresh.parseDate("2026-10-29T10:00:00+02:00"))
        assertNull(Fresh.parseDate("yesterday"))
        assertNull(Fresh.parseDate(null))
    }
}
