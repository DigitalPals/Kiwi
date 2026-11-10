package io.github.digitalpals.kiwi

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import io.github.digitalpals.kiwi.api.ServerUrl
import io.github.digitalpals.kiwi.ui.KiwiRoot
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import kotlinx.coroutines.flow.MutableStateFlow

/** Something the app was opened for: a `kiwi://connect` link or a notification. */
sealed interface LaunchRequest {
    data class Connect(val url: String) : LaunchRequest
    data object MyApps : LaunchRequest
    data object Builds : LaunchRequest

    /** An app's page, e.g. a Kiwi Builds app whose build is ready. */
    data class App(val sourceId: String, val bundleId: String, val name: String) : LaunchRequest
}

class MainActivity : ComponentActivity() {
    private val requests = MutableStateFlow<LaunchRequest?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) handle(intent)
        val container = (application as KiwiApplication).container
        setContent {
            KiwiTheme {
                KiwiRoot(container, requests, onRequestHandled = { requests.value = null })
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent?) {
        when (intent?.action) {
            ACTION_OPEN_MY_APPS -> requests.value = LaunchRequest.MyApps
            ACTION_OPEN_BUILDS -> requests.value = LaunchRequest.Builds
            ACTION_OPEN_APP -> {
                val source = intent.getStringExtra(EXTRA_SOURCE)
                val bundle = intent.getStringExtra(EXTRA_BUNDLE)
                if (source != null && bundle != null) {
                    requests.value = LaunchRequest.App(source, bundle, intent.getStringExtra(EXTRA_NAME).orEmpty())
                }
            }
            Intent.ACTION_VIEW -> intent.dataString?.let(ServerUrl::fromConnectLink)?.let {
                requests.value = LaunchRequest.Connect(it)
            }
        }
    }

    companion object {
        const val ACTION_OPEN_MY_APPS = "io.github.digitalpals.kiwi.OPEN_MY_APPS"
        const val ACTION_OPEN_BUILDS = "io.github.digitalpals.kiwi.OPEN_BUILDS"
        const val ACTION_OPEN_APP = "io.github.digitalpals.kiwi.OPEN_APP"
        const val EXTRA_SOURCE = "source_id"
        const val EXTRA_BUNDLE = "bundle_id"
        const val EXTRA_NAME = "name"
    }
}
