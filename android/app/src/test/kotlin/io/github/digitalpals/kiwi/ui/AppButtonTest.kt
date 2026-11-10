package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.AppState
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Build
import io.github.digitalpals.kiwi.api.Installed
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.install.InstallManager
import org.junit.Assert.assertEquals
import org.junit.Test

class AppButtonTest {
    private val absent = LocalPackage(installed = false, launchable = false)
    private val present = LocalPackage(installed = true, launchable = true)

    private fun app(
        action: String,
        platform: String = "android",
        installed: Installed? = null,
        target: Release? = Release("2.0"),
        build: Build? = null,
        unavailable: String? = null,
        foreign: Boolean = false,
    ) = AppSummary(
        sourceId = "kiwi-youtube",
        bundleId = "app.morphe.android.youtube",
        platform = platform,
        name = "YouTube",
        unavailable = unavailable,
        state = AppState(
            actionName = action,
            installed = installed,
            target = target,
            build = build,
            installUrl = if (action == "install" || action == "update") "https://mac.example.ts.net/ota/t/app.apk" else null,
            foreignInstall = foreign,
        ),
    )

    private val installed = Installed(version = "1.0", signedBundleId = "app.morphe.android.youtube")

    @Test
    fun mapsServerActions() {
        assertEquals(ButtonView.Install("Get"), appButton(app("get"), null, absent))
        assertEquals(ButtonView.Install("Install"), appButton(app("install"), null, absent))
        assertEquals(
            ButtonView.Install("Update", caption = "1.0 → 2.0"),
            appButton(app("update", installed = installed), null, present),
        )
        assertEquals(
            ButtonView.Install("Retry", "Download failed", error = true),
            appButton(app("retry", build = Build(id = "b", status = "failed", error = "Download failed")), null, absent),
        )
        assertEquals(
            ButtonView.Progress("Signing 2 of 3", 0.5f),
            appButton(app("preparing", build = Build(id = "b", status = "signing", step = "Signing 2 of 3", progress = 0.5)), null, absent),
        )
        assertEquals(
            ButtonView.Disabled("Get", "Made for iPhone and iPad"),
            appButton(app("unavailable", unavailable = "Made for iPhone and iPad"), null, absent),
        )
    }

    @Test
    fun thePhoneDecidesWhetherAnAppIsInstalled() {
        assertEquals(ButtonView.Open("app.morphe.android.youtube"), appButton(app("open", installed = installed), null, present))
        assertEquals(ButtonView.Disabled("Installed"), appButton(app("open", installed = installed), null, LocalPackage(true, false)))
        // Kiwi recorded the download, but the install never finished.
        assertEquals(ButtonView.Install("Install"), appButton(app("open", installed = installed), null, absent))
        // A download Kiwi saw start without this app.
        assertEquals(ButtonView.Install("Install"), appButton(app("installing"), null, absent))
        assertEquals(ButtonView.Install("Update"), appButton(app("installing", installed = installed), null, present))
    }

    @Test
    fun warnsAboutAnotherSignature() {
        assertEquals(
            ButtonView.Remove("app.morphe.android.youtube", "Remove the other YouTube first"),
            appButton(app("install", foreign = true), null, present),
        )
        // Not on the phone any more: install normally.
        assertEquals(ButtonView.Install("Install"), appButton(app("install", foreign = true), null, absent))
    }

    @Test
    fun iosAppsCannotBeInstalled() {
        assertEquals(
            ButtonView.Disabled("Get", "Made for iPhone and iPad"),
            appButton(app("get", platform = "ios"), null, absent),
        )
    }

    @Test
    fun localJobsWin() {
        val base = app("install")
        fun job(phase: InstallJob.Phase, progress: Float? = null, message: String? = null) =
            InstallJob(base.key, base.name, phase, progress, message)
        assertEquals(ButtonView.Progress("Preparing", null, cancellable = true), appButton(base, job(InstallJob.Phase.Preparing), absent))
        assertEquals(ButtonView.Progress("Downloading", 0.25f, cancellable = true), appButton(base, job(InstallJob.Phase.Downloading, 0.25f), absent))
        assertEquals(ButtonView.Progress("Installing", null, cancellable = true), appButton(base, job(InstallJob.Phase.Installing), absent))
        assertEquals(
            ButtonView.Install("Retry", "No space", error = true),
            appButton(base, job(InstallJob.Phase.Failed, message = "No space"), absent),
        )
        assertEquals(
            ButtonView.Remove("app.morphe.android.youtube", "Another YouTube is installed"),
            appButton(base, job(InstallJob.Phase.Conflict, message = "Another YouTube is installed"), present),
        )
        assertEquals(
            ButtonView.AllowInstalls("Allow Kiwi to install apps, then try again."),
            appButton(base, job(InstallJob.Phase.NeedsPermission), absent),
        )
    }

    @Test
    fun mapsKiwisOwnUpdate() {
        assertEquals(ButtonView.Install("Update"), selfUpdateButton(null))
        assertEquals(
            ButtonView.Progress("Downloading", 0.5f, cancellable = true),
            selfUpdateButton(InstallJob(InstallManager.SELF, "Kiwi", InstallJob.Phase.Downloading, progress = 0.5f)),
        )
        assertEquals(
            ButtonView.Install("Retry", "Couldn't install Kiwi.", error = true),
            selfUpdateButton(InstallJob(InstallManager.SELF, "Kiwi", InstallJob.Phase.Failed, message = "Couldn't install Kiwi.")),
        )
        assertEquals(
            ButtonView.AllowInstalls("Allow Kiwi to install apps, then try again."),
            selfUpdateButton(InstallJob(InstallManager.SELF, "Kiwi", InstallJob.Phase.NeedsPermission)),
        )
    }
}
