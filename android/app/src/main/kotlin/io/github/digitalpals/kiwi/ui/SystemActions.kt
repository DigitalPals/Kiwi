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

/** Android screens the app sends the user to. */
object SystemActions {
    fun openApp(context: Context, packageName: String) {
        val intent = context.packageManager.getLaunchIntentForPackage(packageName)
        if (intent == null) {
            Toast.makeText(context, "This app can't be opened from here.", Toast.LENGTH_SHORT).show()
            return
        }
        start(context, intent)
    }

    /** Android's own uninstall confirmation (needs `REQUEST_DELETE_PACKAGES`). */
    fun uninstall(context: Context, packageName: String) {
        start(context, Intent(Intent.ACTION_DELETE, "package:$packageName".toUri()))
    }

    /** The "Install unknown apps" switch for this app. */
    fun allowInstalls(context: Context) {
        start(context, Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, "package:${context.packageName}".toUri()))
    }

    private fun start(context: Context, intent: Intent) {
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(context, "Android can't open this here.", Toast.LENGTH_SHORT).show()
        }
    }

    /** The button actions for [app]. */
    fun buttonActions(context: Context, container: AppContainer, app: AppSummary): ButtonActions = ButtonActions(
        install = { container.installs.start(app) },
        open = { openApp(context, it) },
        remove = {
            container.installs.clear(app.key)
            uninstall(context, it)
        },
        allowInstalls = {
            container.installs.clear(app.key)
            allowInstalls(context)
        },
        cancel = { container.installs.cancel(app.key) },
    )
}
