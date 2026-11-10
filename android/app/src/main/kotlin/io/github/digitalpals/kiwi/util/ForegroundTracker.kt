package io.github.digitalpals.kiwi.util

import android.app.Activity
import android.app.Application
import android.os.Bundle

/**
 * Whether one of the app's activities is started. Android only lets an app open its install
 * confirmation while it is visible; in the background a notification offers it instead.
 */
object ForegroundTracker : Application.ActivityLifecycleCallbacks {
    @Volatile
    private var started = 0

    val isForeground: Boolean get() = started > 0

    fun register(application: Application) {
        application.registerActivityLifecycleCallbacks(this)
    }

    override fun onActivityStarted(activity: Activity) {
        started++
    }

    override fun onActivityStopped(activity: Activity) {
        started = (started - 1).coerceAtLeast(0)
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit
}
