package io.github.digitalpals.kiwi.work

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import io.github.digitalpals.kiwi.MainActivity
import io.github.digitalpals.kiwi.R

/** The app's notifications: available updates and installs that need a tap. */
object Notifications {
    private const val CHANNEL_UPDATES = "updates"
    private const val CHANNEL_INSTALLS = "installs"
    private const val TAG_CONFIRM = "confirm"
    private const val TAG_SELF = "self-update"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_UPDATES, "Updates", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "New versions of your apps"
            },
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_INSTALLS, "Installs", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Installs waiting for you to confirm them"
            },
        )
    }

    /** Whether notifications can be shown (the permission on Android 13+ and the app's switch). */
    fun allowed(context: Context): Boolean {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        return NotificationManagerCompat.from(context).areNotificationsEnabled()
    }

    fun showUpdates(context: Context, notices: List<UpdateNotice>) {
        for (notice in notices) {
            val notification = NotificationCompat.Builder(context, CHANNEL_UPDATES)
                .setSmallIcon(R.drawable.ic_stat_kiwi)
                .setContentTitle(notice.title)
                .setContentText(notice.body)
                .setContentIntent(openMyApps(context))
                .setAutoCancel(true)
                .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
                .build()
            post(context, notice.tag, 0, notification)
        }
    }

    fun showSelfUpdate(context: Context, versionName: String) {
        val notification = NotificationCompat.Builder(context, CHANNEL_UPDATES)
            .setSmallIcon(R.drawable.ic_stat_kiwi)
            .setContentTitle("Kiwi update available")
            .setContentText("Version $versionName of this app is ready to install")
            .setContentIntent(openMyApps(context))
            .setAutoCancel(true)
            .build()
        post(context, TAG_SELF, 0, notification)
    }

    /** An install that needs the user's confirmation while Kiwi is in the background. */
    fun showConfirmation(context: Context, sessionId: Int, appName: String, confirm: Intent) {
        val intent = PendingIntent.getActivity(
            context,
            sessionId,
            confirm,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL_INSTALLS)
            .setSmallIcon(R.drawable.ic_stat_kiwi)
            .setContentTitle("Install $appName")
            .setContentText("Tap to finish installing $appName")
            .setContentIntent(intent)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        post(context, TAG_CONFIRM, sessionId, notification)
    }

    fun cancelConfirmation(context: Context, sessionId: Int) {
        NotificationManagerCompat.from(context).cancel(TAG_CONFIRM, sessionId)
    }

    private fun post(context: Context, tag: String, id: Int, notification: android.app.Notification) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        NotificationManagerCompat.from(context).notify(tag, id, notification)
    }

    private fun openMyApps(context: Context): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .setAction(MainActivity.ACTION_OPEN_MY_APPS)
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }
}
