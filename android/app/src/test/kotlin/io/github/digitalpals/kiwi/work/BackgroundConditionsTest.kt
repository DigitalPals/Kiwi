package io.github.digitalpals.kiwi.work

import io.github.digitalpals.kiwi.packages.SilentUpdates
import io.github.digitalpals.kiwi.work.BackgroundConditions.Link
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BackgroundConditionsTest {
    private val wifi = Link(vpn = false, notMetered = true, validated = true)
    private val mobile = Link(vpn = false, notMetered = false, validated = true)
    private val meteredVpn = Link(vpn = true, notMetered = false, validated = true)
    private val unmeteredVpn = Link(vpn = true, notMetered = true, validated = true)

    @Test
    fun installsOnlyOnUnmeteredNetworks() {
        assertTrue(BackgroundConditions.unmetered(wifi, listOf(wifi)))
        assertFalse(BackgroundConditions.unmetered(mobile, listOf(mobile)))
        assertFalse(BackgroundConditions.unmetered(null, listOf(wifi)))
        // Tailscale passes on what carries it...
        assertTrue(BackgroundConditions.unmetered(unmeteredVpn, listOf(unmeteredVpn, wifi)))
        // ...or calls itself metered: then the Wi-Fi under it counts, even with mobile data kept up.
        assertTrue(BackgroundConditions.unmetered(meteredVpn, listOf(meteredVpn, wifi, mobile)))
        assertFalse(BackgroundConditions.unmetered(meteredVpn, listOf(meteredVpn, mobile)))
        assertFalse(BackgroundConditions.unmetered(meteredVpn, listOf(meteredVpn, wifi.copy(validated = false))))
    }

    @Test
    fun installsOnlyWithEnoughBattery() {
        assertTrue(BackgroundConditions.batteryNotLow(80, charging = false))
        assertTrue(BackgroundConditions.batteryNotLow(16, charging = false))
        assertFalse(BackgroundConditions.batteryNotLow(15, charging = false))
        assertTrue(BackgroundConditions.batteryNotLow(5, charging = true))
        assertTrue(BackgroundConditions.batteryNotLow(null, charging = false))
    }

    @Test
    fun knowsWhatAndroidUpdatesSilently() {
        val own = "io.github.digitalpals.kiwi"
        val app = "app.morphe.android.youtube"
        // Installer of record (no update owner).
        assertTrue(SilentUpdates.allowed(own, app, installer = own, updateOwner = null))
        assertFalse(SilentUpdates.allowed(own, app, installer = "com.android.chrome", updateOwner = null))
        // Kiwi updating itself, even when the browser installed it.
        assertTrue(SilentUpdates.allowed(own, own, installer = "com.android.chrome", updateOwner = null))
        // Update ownership (Android 14+): only the owner.
        assertTrue(SilentUpdates.allowed(own, app, installer = "com.android.chrome", updateOwner = own))
        assertFalse(SilentUpdates.allowed(own, app, installer = own, updateOwner = "com.android.vending"))
        assertFalse(SilentUpdates.allowed(own, own, installer = own, updateOwner = "org.fdroid.fdroid"))
    }
}
