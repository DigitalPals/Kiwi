package io.github.digitalpals.kiwi.api

import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.net.URI
import java.net.URLDecoder

/** The Kiwi address the user typed or a `kiwi://connect` link carried. */
object ServerUrl {
    sealed interface Parsed {
        data class Ok(val url: HttpUrl) : Parsed
        data class Invalid(val message: String) : Parsed
    }

    const val SCHEME = "kiwi"
    const val CONNECT_HOST = "connect"

    /**
     * Turns what the user typed into the server's root URL: `https://<host>[:port]/`. A name
     * without a scheme gets `https://`. Kiwi is served by `tailscale serve` over HTTPS, so
     * plain HTTP is refused, and so is anything but the address itself (credentials).
     * A path, query or fragment (e.g. a copied link to an app page) is dropped.
     */
    fun parse(input: String): Parsed {
        val text = input.trim()
        if (text.isEmpty()) return Parsed.Invalid("Enter your Kiwi's address.")
        val withScheme = if (text.contains("://")) text else "https://$text"
        val scheme = withScheme.substringBefore("://").lowercase()
        if (scheme == "http") {
            return Parsed.Invalid("Kiwi uses HTTPS. Use the https:// address that tailscale serve shows.")
        }
        if (scheme != "https") return Parsed.Invalid("Enter an https:// address.")
        val url = withScheme.toHttpUrlOrNull() ?: return Parsed.Invalid("That's not a valid address.")
        if (url.username.isNotEmpty() || url.password.isNotEmpty()) {
            return Parsed.Invalid("The address can't contain a user name or password.")
        }
        if (!url.host.contains('.') && url.host != "localhost") {
            return Parsed.Invalid("Use the full name, like https://mac.example.ts.net.")
        }
        val root = url.newBuilder().encodedPath("/").query(null).fragment(null).build()
        return Parsed.Ok(root)
    }

    /**
     * The `url` of a `kiwi://connect?url=<https url>` link, or null when the link is not one.
     * The caller still validates it with [parse] and asks the user before connecting.
     */
    fun fromConnectLink(link: String): String? {
        val uri = runCatching { URI(link) }.getOrNull() ?: return null
        if (!uri.scheme.equals(SCHEME, ignoreCase = true)) return null
        if (!uri.host.equals(CONNECT_HOST, ignoreCase = true)) return null
        val query = uri.rawQuery ?: return null
        return query.split('&')
            .map { it.substringBefore('=') to it.substringAfter('=', "") }
            .firstOrNull { it.first == "url" }
            // The Charset overload needs API 33.
            ?.let { runCatching { URLDecoder.decode(it.second, "UTF-8") }.getOrNull() }
            ?.takeIf { it.isNotBlank() }
    }

    /** The address as shown to the user: "https://mac.example.ts.net". */
    fun display(url: HttpUrl): String = url.toString().removeSuffix("/")
}
