package io.github.digitalpals.kiwi.api

/**
 * The app's User-Agent, which tells Kiwi that the request comes from the Android app and which
 * Android release the phone runs:
 * `Kiwi-Android/<versionName> (<versionCode>; Android <release>; SDK <int>; <manufacturer> <model>)`.
 */
object UserAgent {
    const val PRODUCT = "Kiwi-Android"

    fun build(
        versionName: String,
        versionCode: Long,
        release: String,
        sdkInt: Int,
        manufacturer: String,
        model: String,
    ): String {
        val device = deviceName(manufacturer, model).ifEmpty { "Android" }
        return "$PRODUCT/${token(versionName)} ($versionCode; Android ${clean(release)}; SDK $sdkInt; $device)"
    }

    /** "Google Pixel 9"; a model that already starts with the manufacturer is kept as it is. */
    fun deviceName(manufacturer: String, model: String): String {
        val maker = clean(manufacturer)
        val name = clean(model)
        return when {
            maker.isEmpty() -> name
            name.isEmpty() -> maker
            name.startsWith(maker, ignoreCase = true) -> name
            else -> "$maker $name"
        }
    }

    /** The product token without anything a client would read as a separator. */
    private fun token(value: String): String = clean(value).replace(' ', '-').ifEmpty { "0" }

    /**
     * Header values must be printable ASCII, and the comment's `;` and parentheses separate
     * fields, so anything else becomes a space.
     */
    internal fun clean(value: String): String = value
        .map { if (it in ' '..'~' && it != '(' && it != ')' && it != ';') it else ' ' }
        .joinToString("")
        .trim()
        .replace(Regex(" +"), " ")
}
