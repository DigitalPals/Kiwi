package io.github.digitalpals.kiwi.api

/** Everything that can go wrong talking to Kiwi, with a message the UI can show as is. */
sealed class KiwiException(message: String, cause: Throwable? = null) : Exception(message, cause) {

    /** Kiwi could not be reached at all: Tailscale is off, the Mac is asleep, or the name is wrong. */
    class Network(val host: String, cause: Throwable?) :
        KiwiException("Can't reach Kiwi at $host. Is Tailscale on?", cause)

    /**
     * Kiwi answered with an error (`{ error: { code, message } }`); [code] is [UNKNOWN] when the
     * answer had no such body.
     */
    class Http(val status: Int, val code: String, message: String) : KiwiException(message) {
        /** The answer was one of Kiwi's JSON errors, not just a status. */
        val isFromKiwi: Boolean get() = code != UNKNOWN

        /** This device belongs to someone other than the owner (403). */
        val isNotOwner: Boolean get() = code == NOT_OWNER

        /** Tailscale could not tell Kiwi which device this is (503). */
        val isDeviceUnidentified: Boolean get() = code == DEVICE_UNIDENTIFIED

        /** Kiwi Builds is turned off in Kiwi's settings (409 from `/api/automation`). */
        val isBuildsOff: Boolean get() = code == BUILDS_OFF

        /** The request clashes with work already on its way (409), e.g. a build of the app. */
        val isConflict: Boolean get() = code == CONFLICT
    }

    /** The answer was not what Kiwi sends: probably not a Kiwi server, or a proxy's error page. */
    class BadResponse(message: String, cause: Throwable? = null) : KiwiException(message, cause)

    companion object {
        /** [Http.code] of an error answer without Kiwi's JSON body. */
        const val UNKNOWN = "http"
        const val NOT_OWNER = "not_owner"
        const val DEVICE_UNIDENTIFIED = "device_unidentified"
        const val BAD_HOST = "bad_host"
        const val BUILDS_OFF = "builds_off"
        const val CONFLICT = "conflict"
    }
}

/** A title for errors that replace a whole screen (connecting, or the first `/api/me`). */
fun KiwiException.title(): String = when {
    this is KiwiException.Http && isNotOwner -> "Not Your Kiwi"
    this is KiwiException.Http && isDeviceUnidentified -> "Who's This?"
    this is KiwiException.Network -> "Can't Reach Kiwi"
    else -> "Something Went Wrong"
}

/** Any thrown value as a message for the UI. */
fun Throwable.userMessage(): String = when (this) {
    is KiwiException -> message ?: "Something went wrong."
    else -> message?.takeIf { it.isNotBlank() } ?: "Something went wrong."
}
