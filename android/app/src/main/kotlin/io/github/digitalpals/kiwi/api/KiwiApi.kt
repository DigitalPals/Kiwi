package io.github.digitalpals.kiwi.api

import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Kiwi's JSON API at [baseUrl] (the server's root, e.g. `https://mac.example.ts.net/`).
 *
 * Kiwi knows the phone by its Tailscale address, so there is no login: every request simply
 * goes over the tailnet. Mutating requests carry `X-Kiwi: 1` and a JSON body, as the server
 * requires (docs/api.md, "Conventions"). Redirects are not followed: Kiwi never sends one, and
 * the phone's requests (its package report included) must not end up at another host.
 */
class KiwiApi(
    val baseUrl: HttpUrl,
    client: OkHttpClient,
    private val json: Json = KiwiJson,
) {
    private val client: OkHttpClient = client.newBuilder()
        .callTimeout(REQUEST_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        .followRedirects(false)
        .followSslRedirects(false)
        .build()

    /**
     * `GET /api/health`: is this a Kiwi? Kiwi's own errors (`421 bad_host` from its host check,
     * which covers this route too) pass through, so the user learns what is wrong; any other
     * answer means the address is not a Kiwi.
     */
    suspend fun health(): Health {
        val health = try {
            send(get("api/health"), Health.serializer())
        } catch (error: KiwiException.Http) {
            if (error.isFromKiwi) throw error
            throw KiwiException.BadResponse(NOT_KIWI, error)
        } catch (error: KiwiException.BadResponse) {
            throw KiwiException.BadResponse(NOT_KIWI, error)
        }
        if (!health.ok) throw KiwiException.BadResponse(NOT_KIWI)
        return health
    }

    suspend fun me(): Me = send(get("api/me"), Me.serializer())

    /** `PATCH /api/me` with only the given fields, e.g. `{"onboarded": true}`. */
    suspend fun patchMe(fields: JsonObject): Me = send(mutate("PATCH", "api/me", fields.toString()), Me.serializer())

    /**
     * `GET /api/apps`: Browse's apps for this device, optionally matching [query]. With [hidden]
     * also the apps hidden from Browse and those made for the other platform.
     */
    suspend fun apps(query: String? = null, hidden: Boolean = false): List<AppSummary> {
        val url = url("api/apps").newBuilder().apply {
            if (!query.isNullOrBlank()) addQueryParameter("q", query.trim())
            if (hidden) addQueryParameter("hidden", "1")
        }.build()
        return send(Request.Builder().url(url).build(), ListSerializer(AppSummary.serializer()))
    }

    suspend fun app(sourceId: String, bundleId: String): AppDetail =
        send(Request.Builder().url(appUrl(sourceId, bundleId)).build(), AppDetail.serializer())

    /** Asks Kiwi to prepare a build (Android builds are ready at once). Returns the build. */
    suspend fun prepare(sourceId: String, bundleId: String, request: PrepareRequest = PrepareRequest()): Build {
        val url = appUrl(sourceId, bundleId).newBuilder().addPathSegment("prepare").build()
        val body = json.encodeToString(PrepareRequest.serializer(), request)
        return send(mutate("POST", url, body), Build.serializer())
    }

    suspend fun build(id: String): Build {
        val url = url("api/builds").newBuilder().addPathSegment(id).build()
        return send(Request.Builder().url(url).build(), Build.serializer())
    }

    /** `GET /api/library`: the apps installed on this device, one row per install. */
    suspend fun library(): List<AppSummary> = send(get("api/library"), ListSerializer(AppSummary.serializer()))

    /** `GET /api/updates`: installed apps with a newer release. */
    suspend fun updates(): List<AppSummary> = send(get("api/updates"), ListSerializer(AppSummary.serializer()))

    suspend fun settings(): Settings = send(get("api/settings"), Settings.serializer())

    suspend fun patchSettings(fields: JsonObject): Settings =
        send(mutate("PATCH", "api/settings", fields.toString()), Settings.serializer())

    /** `PUT /api/me/packages`: the packages installed on this phone, for Kiwi to reconcile. */
    suspend fun putPackages(report: PackagesReport) {
        val body = json.encodeToString(PackagesReport.serializer(), report)
        send<Unit>(mutate("PUT", "api/me/packages", body), null)
    }

    /** `GET /api/automation`: every app Kiwi Builds can build. `409 builds_off` while it is off. */
    suspend fun automation(): AutomationOverview = send(get("api/automation"), AutomationOverview.serializer())

    /** `POST /api/automation/:app/add`: adds the app with [preferences] and queues its first build. */
    suspend fun addAutomation(app: String, preferences: AutomationPreferences): AutomationState =
        send(mutate("POST", automationUrl(app, "add"), preferencesJson(preferences)), AutomationState.serializer())

    /** `PATCH /api/automation/:app`: replaces the app's preferences. */
    suspend fun updateAutomation(app: String, preferences: AutomationPreferences): AutomationState =
        send(mutate("PATCH", automationUrl(app), preferencesJson(preferences)), AutomationState.serializer())

    /** `DELETE /api/automation/:app`: removes the app from Kiwi Builds. */
    suspend fun removeAutomation(app: String): AutomationState =
        send(mutate("DELETE", automationUrl(app), "{}"), AutomationState.serializer())

    /** `POST /api/automation/:app/check`: looks for a new version now; [force] rebuilds. */
    suspend fun checkAutomation(app: String, force: Boolean = false): AutomationState {
        val body = buildJsonObject { put("force", JsonPrimitive(force)) }.toString()
        return send(mutate("POST", automationUrl(app, "check"), body), AutomationState.serializer())
    }

    /** `POST /api/automation/:app/cancel`: cancels the app's queued or running job. */
    suspend fun cancelAutomation(app: String): AutomationState =
        send(mutate("POST", automationUrl(app, "cancel"), "{}"), AutomationState.serializer())

    /** An absolute URL for a path or URL from the API (icons may be `/api/icons/<sha>.png`). */
    fun resolve(url: String): HttpUrl? = baseUrl.resolve(url)

    /** Whether [url] points at this Kiwi; downloads are only taken from Kiwi itself. */
    fun isOwnUrl(url: HttpUrl): Boolean =
        url.scheme == baseUrl.scheme && url.host == baseUrl.host && url.port == baseUrl.port

    private fun url(path: String): HttpUrl = baseUrl.resolve(path) ?: error("Bad path $path")

    private fun appUrl(sourceId: String, bundleId: String): HttpUrl =
        url("api/apps").newBuilder().addPathSegment(sourceId).addPathSegment(bundleId).build()

    private fun automationUrl(app: String, action: String? = null): HttpUrl =
        url("api/automation").newBuilder().addPathSegment(app).apply { if (action != null) addPathSegment(action) }.build()

    private fun get(path: String): Request = Request.Builder().url(url(path)).build()

    private fun mutate(method: String, path: String, body: String): Request = mutate(method, url(path), body)

    private fun mutate(method: String, url: HttpUrl, body: String): Request {
        val content: RequestBody = body.toRequestBody(JSON)
        return Request.Builder()
            .url(url)
            .header("X-Kiwi", "1")
            .method(method, content)
            .build()
    }

    private suspend fun <T> send(request: Request, deserializer: DeserializationStrategy<T>?): T {
        val response = try {
            client.newCall(request.newBuilder().header("Accept", "application/json").build()).await()
        } catch (error: SSLException) {
            // Tailscale being off shows up as a DNS or connect failure; a bad certificate
            // means the name is not the one `tailscale serve` has a certificate for.
            throw KiwiException.BadResponse("The certificate of ${baseUrl.host} isn't valid. Check the address.", error)
        } catch (error: IOException) {
            throw KiwiException.Network(baseUrl.host, error)
        }
        response.use {
            val text = try {
                it.body.string()
            } catch (error: IOException) {
                throw KiwiException.Network(baseUrl.host, error)
            }
            if (!it.isSuccessful) throw errorFrom(it.code, text)
            @Suppress("UNCHECKED_CAST")
            if (deserializer == null) return Unit as T
            return try {
                json.decodeFromString(deserializer, text)
            } catch (error: SerializationException) {
                throw KiwiException.BadResponse(UNEXPECTED, error)
            } catch (error: IllegalArgumentException) {
                throw KiwiException.BadResponse(UNEXPECTED, error)
            }
        }
    }

    private fun errorFrom(status: Int, text: String): KiwiException.Http {
        val body = runCatching { json.decodeFromString(ErrorBody.serializer(), text) }.getOrNull()
        val message = body?.error?.message?.takeIf { it.isNotBlank() } ?: "Kiwi answered with an error ($status)."
        return KiwiException.Http(status, body?.error?.code?.takeIf { it.isNotBlank() } ?: KiwiException.UNKNOWN, message)
    }

    companion object {
        /**
         * Preferences as the server wants them: every field, and `name` only when there is one
         * (Kiwi rejects a name for apps without a choice).
         */
        fun preferencesJson(preferences: AutomationPreferences): String = buildJsonObject {
            put("enabled", JsonPrimitive(preferences.enabled))
            put("interval_minutes", JsonPrimitive(preferences.intervalMinutes))
            put("tweaks", JsonArray(preferences.tweaks.map(::JsonPrimitive)))
            preferences.name?.let { put("name", JsonPrimitive(it)) }
        }.toString()

        const val REQUEST_TIMEOUT_SECONDS = 30L
        private val JSON = "application/json".toMediaType()
        private const val NOT_KIWI = "This doesn't look like a Kiwi server. Check the address."
        private const val UNEXPECTED = "Kiwi sent an answer this app doesn't understand. Update Kiwi and this app."
    }
}

/** Runs the call without blocking a thread; cancelling the coroutine cancels the call. */
internal suspend fun Call.await(): Response = suspendCancellableCoroutine { continuation ->
    continuation.invokeOnCancellation { cancel() }
    enqueue(object : Callback {
        override fun onFailure(call: Call, e: IOException) {
            continuation.resumeWithException(e)
        }

        override fun onResponse(call: Call, response: Response) {
            continuation.resume(response) { _, value, _ -> value.close() }
        }
    })
}
