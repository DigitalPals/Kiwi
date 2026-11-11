package io.github.digitalpals.kiwi.data

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.longPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.kiwiPreferences: DataStore<Preferences> by preferencesDataStore(name = "kiwi")

/** What the app remembers on the phone. Everything else lives on the Kiwi server. */
class SettingsStore(context: Context) {
    private val store = context.applicationContext.kiwiPreferences

    /** The server's root URL ("https://mac.example.ts.net/"), or null before connecting. */
    val serverUrl: Flow<String?> = store.data.map { it[SERVER_URL] }.distinctUntilChanged()

    /** Install updates in the background where Android allows it without asking. */
    val autoUpdate: Flow<Boolean> = store.data.map { it[AUTO_UPDATE] ?: true }.distinctUntilChanged()

    /** Post a notification when an update is available. */
    val notifyUpdates: Flow<Boolean> = store.data.map { it[NOTIFY_UPDATES] ?: true }.distinctUntilChanged()

    /** Post a notification when a Kiwi Builds build finishes or fails. */
    val notifyBuilds: Flow<Boolean> = store.data.map { it[NOTIFY_BUILDS] ?: true }.distinctUntilChanged()

    /** When the app last checked Kiwi for updates (milliseconds since the epoch), or null. */
    val lastChecked: Flow<Long?> = store.data.map { it[LAST_CHECKED] }.distinctUntilChanged()

    /**
     * Kiwi Builds apps (by recipe id) to install once their build is ready: "Build and install"
     * saves the wish here so it survives the app being closed (see BuildInstalls).
     */
    val pendingInstalls: Flow<Set<String>> = store.data.map { it[PENDING_INSTALLS] ?: emptySet() }.distinctUntilChanged()

    /** Whether the app has asked for the notification permission once already. */
    val askedNotifications: Flow<Boolean> = store.data.map { it[ASKED_NOTIFICATIONS] ?: false }

    suspend fun setServerUrl(url: String?) {
        store.edit {
            val changed = it[SERVER_URL] != url
            if (url == null) it.remove(SERVER_URL) else it[SERVER_URL] = url
            if (changed) {
                // Notifications already sent and the last check belong to the old server.
                it.remove(NOTIFIED)
                it.remove(NOTIFIED_BUILDS)
                it.remove(LAST_CHECKED)
                it.remove(PENDING_INSTALLS)
            }
        }
    }

    suspend fun setAutoUpdate(enabled: Boolean) {
        store.edit { it[AUTO_UPDATE] = enabled }
    }

    suspend fun setNotifyUpdates(enabled: Boolean) {
        store.edit { it[NOTIFY_UPDATES] = enabled }
    }

    suspend fun setNotifyBuilds(enabled: Boolean) {
        store.edit { it[NOTIFY_BUILDS] = enabled }
    }

    suspend fun setLastChecked(millis: Long) {
        store.edit { it[LAST_CHECKED] = millis }
    }

    suspend fun addPendingInstall(app: String) {
        store.edit { it[PENDING_INSTALLS] = (it[PENDING_INSTALLS] ?: emptySet()) + app }
    }

    suspend fun removePendingInstall(app: String) {
        store.edit { it[PENDING_INSTALLS] = (it[PENDING_INSTALLS] ?: emptySet()) - app }
    }

    suspend fun setAskedNotifications() {
        store.edit { it[ASKED_NOTIFICATIONS] = true }
    }

    /** Keys of the updates already notified (see `UpdateNotices.key`). */
    suspend fun notified(): Set<String> = store.data.first()[NOTIFIED] ?: emptySet()

    suspend fun setNotified(keys: Set<String>) {
        store.edit { it[NOTIFIED] = keys }
    }

    /** Ids of the finished Kiwi Builds jobs already seen, or null before the first look. */
    suspend fun notifiedBuilds(): Set<String>? = store.data.first()[NOTIFIED_BUILDS]

    suspend fun setNotifiedBuilds(ids: Set<String>) {
        store.edit { it[NOTIFIED_BUILDS] = ids }
    }

    private companion object {
        val SERVER_URL = stringPreferencesKey("server_url")
        val AUTO_UPDATE = booleanPreferencesKey("auto_update")
        val NOTIFY_UPDATES = booleanPreferencesKey("notify_updates")
        val ASKED_NOTIFICATIONS = booleanPreferencesKey("asked_notifications")
        val NOTIFIED = stringSetPreferencesKey("notified_updates")
        val NOTIFY_BUILDS = booleanPreferencesKey("notify_builds")
        val NOTIFIED_BUILDS = stringSetPreferencesKey("notified_builds")
        val LAST_CHECKED = longPreferencesKey("last_checked")
        val PENDING_INSTALLS = stringSetPreferencesKey("pending_build_installs")
    }
}
