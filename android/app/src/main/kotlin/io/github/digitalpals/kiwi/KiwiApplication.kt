package io.github.digitalpals.kiwi

import android.app.Application
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import io.github.digitalpals.kiwi.util.ForegroundTracker
import io.github.digitalpals.kiwi.work.Notifications
import io.github.digitalpals.kiwi.work.UpdateWorker
import kotlinx.coroutines.launch

class KiwiApplication : Application(), SingletonImageLoader.Factory {
    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
        ForegroundTracker.register(this)
        Notifications.createChannels(this)
        container.packageReporter.watch()
        container.scope.launch {
            if (container.api() != null) UpdateWorker.schedule(this@KiwiApplication)
        }
    }

    /** App icons load through the app's HTTP client (and its User-Agent). */
    override fun newImageLoader(context: PlatformContext): ImageLoader = ImageLoader.Builder(context)
        .components { add(OkHttpNetworkFetcherFactory(callFactory = { container.httpClient })) }
        .build()
}
