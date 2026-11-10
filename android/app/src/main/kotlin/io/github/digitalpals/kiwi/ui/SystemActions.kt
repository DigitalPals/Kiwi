package io.github.digitalpals.kiwi.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.widget.Toast
import androidx.core.net.toUri
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.ui.components.ButtonActions

/** What the app buttons and rows of a screen do. Previews use the defaults (nothing). */
class AppActions(
    val install: (AppSummary) -> Unit = {},
    val open: (String) -> Unit = {},
    val remove: (AppSummary, String) -> Unit = { _, _ -> },
    val allowInstalls: (AppSummary) -> Unit = {},
    val cancel: (AppSummary) -> Unit = {},
    /** Opens the app's page. */
    val show: (AppSummary) -> Unit = {},
) {
    /** The button actions for [app]. */
    fun buttons(app: AppSummary): ButtonActions = ButtonActions(
        install = { install(app) },
        open = open,
        remove = { remove(app, it) },
        allowInstalls = { allowInstalls(app) },
        cancel = { cancel(app) },
    )
}

/** Android screens the app sends the user to. */
object SystemActions {
    /** Tailscale's Android app (the tailnet Kiwi is on). */
    private const val TAILSCALE = "com.tailscale.ipn"

    fun openApp(context: Context, packageName: String) {
        val intent = context.packageManager.getLaunchIntentForPackage(packageName)
        if (intent == null) {
            Toast.makeText(context, "This app can't be opened from here.", Toast.LENGTH_SHORT).show()
            return
        }
        start(context, intent)
    }

    /** Opens Tailscale, or its download page when it isn't installed. */
    fun openTailscale(context: Context) {
        val intent = context.packageManager.getLaunchIntentForPackage(TAILSCALE)
            ?: Intent(Intent.ACTION_VIEW, "https://tailscale.com/download/android".toUri())
        start(context, intent)
    }

    /** Android's own uninstall confirmation (needs `REQUEST_DELETE_PACKAGES`). */
    fun uninstall(context: Context, packageName: String) {
        start(context, Intent(Intent.ACTION_DELETE, "package:$packageName".toUri()))
    }

    /** Android's App info page of a package. */
    fun appInfo(context: Context, packageName: String) {
        start(context, Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, "package:$packageName".toUri()))
    }

    /** The "Install unknown apps" switch for this app. */
    fun allowInstalls(context: Context) {
        start(context, Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, "package:${context.packageName}".toUri()))
    }

    /** Android's notification settings of this app (its channels: updates, builds, installs). */
    fun notificationSettings(context: Context) {
        start(context, Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
    }

    private fun start(context: Context, intent: Intent) {
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(context, "Android can't open this here.", Toast.LENGTH_SHORT).show()
        }
    }

    /** The actions of app buttons, backed by the install manager. */
    fun appActions(context: Context, container: AppContainer, show: (AppSummary) -> Unit): AppActions = AppActions(
        install = { container.installs.start(it) },
        open = { openApp(context, it) },
        remove = { app, packageName ->
            container.installs.clear(app.key)
            uninstall(context, packageName)
        },
        allowInstalls = { app ->
            container.installs.clear(app.key)
            allowInstalls(context)
        },
        cancel = { container.installs.cancel(it.key) },
        show = show,
    )
}
