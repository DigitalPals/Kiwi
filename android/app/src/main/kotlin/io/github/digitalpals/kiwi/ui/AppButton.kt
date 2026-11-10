package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.Action
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.Platforms
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.work.UpdateNotices

/** What an app's main button shows and does. */
sealed interface ButtonView {
    /** Starts the install flow (prepare when needed, download, install). */
    data class Install(val label: String, val caption: String? = null, val error: Boolean = false) : ButtonView

    /** Opens the installed app. */
    data class Open(val packageName: String) : ButtonView

    /** Opens Android's uninstall dialog for a copy with another signature. */
    data class Remove(val packageName: String, val caption: String) : ButtonView

    /** Opens the "Install unknown apps" setting for this app. */
    data class AllowInstalls(val caption: String) : ButtonView

    /** Work in progress; [cancellable] when it is this app's own job (it offers Cancel). */
    data class Progress(val label: String, val fraction: Float?, val cancellable: Boolean = false) : ButtonView

    data class Disabled(val label: String, val caption: String? = null) : ButtonView
}

/**
 * What this phone knows about the app's package; [silentUpdate] when Android lets Kiwi update it
 * without asking (Kiwi is its installer of record or update owner).
 */
data class LocalPackage(val installed: Boolean, val launchable: Boolean, val silentUpdate: Boolean = false)

/**
 * The button for [app] from the server's state, this app's own [job] for it, and whether the
 * package is on the phone. The phone is the truth about what is installed: Kiwi can only
 * guess from downloads until the app has reported its packages.
 */
fun appButton(app: AppSummary, job: InstallJob?, local: LocalPackage): ButtonView {
    if (job != null) {
        when (job.phase) {
            InstallJob.Phase.Preparing -> return ButtonView.Progress("Preparing", null, cancellable = true)
            InstallJob.Phase.Downloading -> return ButtonView.Progress("Downloading", job.progress, cancellable = true)
            InstallJob.Phase.Installing -> return ButtonView.Progress("Installing", null, cancellable = true)
            InstallJob.Phase.NeedsPermission ->
                return ButtonView.AllowInstalls("Allow Kiwi to install apps, then try again.")
            InstallJob.Phase.Conflict ->
                return ButtonView.Remove(app.packageName, job.message ?: foreignCaption(app))
            InstallJob.Phase.Failed ->
                return ButtonView.Install("Retry", job.message ?: "Installing failed", error = true)
        }
    }
    val state = app.state
    if (app.platform != Platforms.ANDROID) {
        return ButtonView.Disabled("Get", app.unavailable ?: "Made for iPhone and iPad")
    }
    if (state.foreignInstall && local.installed) {
        return ButtonView.Remove(app.packageName, foreignCaption(app))
    }
    val updateLabel = if (state.installed != null && local.installed) "Update" else "Install"
    return when (state.action) {
        Action.Get -> ButtonView.Install("Get")
        Action.Install -> ButtonView.Install("Install")
        Action.Update -> ButtonView.Install(
            "Update",
            caption = state.installed?.let { installed -> state.target?.let { UpdateNotices.versionChange(installed, it) } },
        )
        Action.Preparing -> ButtonView.Progress(
            state.build?.step ?: "Preparing",
            state.build?.progress?.toFloat()?.coerceIn(0f, 1f),
        )
        // A download Kiwi saw start without this app: offer it again.
        Action.Installing -> ButtonView.Install(updateLabel)
        Action.Open -> when {
            local.installed && local.launchable -> ButtonView.Open(app.packageName)
            local.installed -> ButtonView.Disabled("Installed")
            // Kiwi counts a finished download as an install; the phone says it isn't there.
            app.unavailable == null && state.target != null -> ButtonView.Install("Install")
            else -> ButtonView.Disabled("Installed")
        }
        Action.Retry -> ButtonView.Install("Retry", state.build?.error ?: "Preparing failed", error = true)
        Action.Unavailable -> ButtonView.Disabled("Get", app.unavailable ?: state.target?.incompatibility)
        Action.Unknown -> ButtonView.Disabled("Get", app.unavailable)
    }
}

/** The button of Kiwi's own update (`Me.android_app`), from its install job. */
fun selfUpdateButton(job: InstallJob?): ButtonView = when {
    job == null -> ButtonView.Install("Update")
    job.phase == InstallJob.Phase.Preparing -> ButtonView.Progress("Preparing", null, cancellable = true)
    job.phase == InstallJob.Phase.Downloading -> ButtonView.Progress("Downloading", job.progress, cancellable = true)
    job.phase == InstallJob.Phase.Installing -> ButtonView.Progress("Installing", null, cancellable = true)
    job.phase == InstallJob.Phase.NeedsPermission -> ButtonView.AllowInstalls("Allow Kiwi to install apps, then try again.")
    else -> ButtonView.Install("Retry", job.message ?: "Updating Kiwi failed", error = true)
}

/** "Remove the other YouTube first": the package is installed with another signature. */
fun foreignCaption(app: AppSummary): String = "Remove the other ${app.name} first"
