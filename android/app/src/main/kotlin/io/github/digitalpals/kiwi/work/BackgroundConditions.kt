package io.github.digitalpals.kiwi.work

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build

/**
 * When the background worker may install updates: on an unmetered network (Wi-Fi, Ethernet)
 * with a battery that isn't low, the conditions of WorkManager's `UNMETERED` and
 * `BATTERY_NOT_LOW`. Checking for updates and notifying them need neither, so they are checked
 * here, at the install step, rather than as constraints of the work.
 */
object BackgroundConditions {
    /** A battery at or below this level (percent) is low, unless it is charging. */
    const val LOW_BATTERY_PERCENT = 15

    /** What [unmetered] needs to know about a network. */
    data class Link(
        val vpn: Boolean,
        val notMetered: Boolean,
        /** Connected to the internet, as Android validated it. */
        val validated: Boolean,
    )

    fun met(context: Context): Boolean = unmetered(context) && batteryNotLow(context)

    fun unmetered(context: Context): Boolean {
        val manager = context.getSystemService(ConnectivityManager::class.java) ?: return false
        val active = manager.activeNetwork?.let(manager::getNetworkCapabilities)?.let(::link)

        @Suppress("DEPRECATION") // The replacement (a NetworkCallback) can't answer synchronously.
        val all = manager.allNetworks.mapNotNull { manager.getNetworkCapabilities(it)?.let(::link) }
        return unmetered(active, all)
    }

    /**
     * Whether the phone is on an unmetered network. Tailscale is a VPN, so it is usually the
     * active network; a VPN that calls itself metered whatever carries it still counts as
     * unmetered while a validated unmetered network (Wi-Fi), which Android prefers to mobile
     * data, is connected under it.
     */
    fun unmetered(active: Link?, all: List<Link>): Boolean {
        if (active == null) return false
        if (active.notMetered) return true
        return active.vpn && all.any { !it.vpn && it.notMetered && it.validated }
    }

    fun batteryNotLow(context: Context): Boolean {
        val battery = context.getSystemService(BatteryManager::class.java) ?: return true
        val percent = battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY).takeIf { it in 0..100 }
        return batteryNotLow(percent, battery.isCharging)
    }

    /** Charging, above [LOW_BATTERY_PERCENT], or unknown (no battery). */
    fun batteryNotLow(percent: Int?, charging: Boolean): Boolean =
        charging || percent == null || percent > LOW_BATTERY_PERCENT

    private fun link(capabilities: NetworkCapabilities) = Link(
        vpn = capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
        notMetered = capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) ||
            (Build.VERSION.SDK_INT >= 30 && capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_TEMPORARILY_NOT_METERED)),
        validated = capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
    )
}
