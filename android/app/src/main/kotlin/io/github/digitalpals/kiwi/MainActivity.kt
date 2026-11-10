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
            Intent.ACTION_VIEW -> intent.dataString?.let(ServerUrl::fromConnectLink)?.let {
                requests.value = LaunchRequest.Connect(it)
            }
        }
    }

    companion object {
        const val ACTION_OPEN_MY_APPS = "io.github.digitalpals.kiwi.OPEN_MY_APPS"
    }
}
