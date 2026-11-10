package io.github.digitalpals.kiwi

import android.app.Application
import android.os.Build
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.UserAgent
import io.github.digitalpals.kiwi.data.SettingsStore
import io.github.digitalpals.kiwi.install.ApkDownloader
import io.github.digitalpals.kiwi.install.InstallManager
import io.github.digitalpals.kiwi.install.SessionInstaller
import io.github.digitalpals.kiwi.packages.PackageReporter
import io.github.digitalpals.kiwi.packages.PackageScanner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/** The app's long-lived objects, created once in [KiwiApplication]. */
class AppContainer(val application: Application) {
    /** Work that outlives a screen: installs and package reports. */
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val settings = SettingsStore(application)

    val userAgent: String = UserAgent.build(
        versionName = BuildConfig.VERSION_NAME,
        versionCode = BuildConfig.VERSION_CODE.toLong(),
        release = Build.VERSION.RELEASE ?: "",
        sdkInt = Build.VERSION.SDK_INT,
        manufacturer = Build.MANUFACTURER ?: "",
        model = Build.MODEL ?: "",
    )

    /** Hosts the user connected (or is connecting) to: they get the full User-Agent. */
    private val kiwiHosts: MutableSet<String> = ConcurrentHashMap.newKeySet()

    /**
     * One HTTP client (and connection pool) for the API, downloads and icons. Requests to Kiwi
     * carry the full User-Agent (Kiwi reads the Android release from it); icon hosts only learn
     * the app name. The API and the downloader don't follow redirects (see KiwiApi); icons may.
     */
    val httpClient: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .addInterceptor(Interceptor { chain ->
            val request = chain.request()
            val agent = if (request.url.host in kiwiHosts) userAgent else "${UserAgent.PRODUCT}/${BuildConfig.VERSION_NAME}"
            chain.proceed(request.newBuilder().header("User-Agent", agent).build())
        })
        .build()

    val packageScanner = PackageScanner(application)
    val sessionInstaller = SessionInstaller(application)

    val packageReporter = PackageReporter(packageScanner, scope, api = { api() }, ownPackage = application.packageName)

    val installs = InstallManager(
        scope = scope,
        api = { api() },
        downloader = ApkDownloader(httpClient),
        installer = sessionInstaller,
        packages = packageScanner,
        report = { packageReporter.report() },
        cacheDir = application.cacheDir,
        ownPackage = application.packageName,
    )

    @Volatile
    private var cached: KiwiApi? = null

    /** The API of the saved server, or null before connecting. */
    suspend fun api(): KiwiApi? {
        val url = settings.serverUrl.first()?.toHttpUrlOrNull() ?: return null
        return remember(apiFor(url))
    }

    /** The API at [url]. Its requests carry the full User-Agent. */
    @Synchronized
    fun apiFor(url: HttpUrl): KiwiApi {
        cached?.takeIf { it.baseUrl == url }?.let { return it }
        kiwiHosts += url.host
        return KiwiApi(url, httpClient)
    }

    /** The API of the saved server, remembered for [resolveUrl]. */
    private fun remember(api: KiwiApi): KiwiApi = api.also { cached = it }

    /** An absolute URL for an icon or link from the API of the saved server. */
    fun resolveUrl(url: String): String? = cached?.resolve(url)?.toString()
}
