package io.github.digitalpals.kiwi.ui

/**
 * App colours, as in the web app (web/src/lib/tint.ts): the source's tint, or a colour picked
 * from the name when a source has none. Evergreen stays the colour of actions.
 */
object Tint {
    /** Mid-tone colours that carry white text, for apps without a tint. */
    val PALETTE: List<Long> = listOf(0x2b7bff, 0x8a28f7, 0xe85d2c, 0x1e9e6a, 0xd6336c, 0x0f8fb0, 0xc27c0e, 0x5861bd)

    private val HEX = Regex("^#?[0-9a-fA-F]{6}$")

    /** A source's `tint_color` as 0xRRGGBB, or null when it isn't a 6-digit hex colour. */
    fun parse(tint: String?): Long? {
        if (tint == null || !HEX.matches(tint)) return null
        return tint.removePrefix("#").toLong(16)
    }

    /** The app's tint, else the same palette colour for the same name every time. */
    fun appColor(name: String, tint: String?): Long {
        parse(tint)?.let { return it }
        var hash = 0L
        var index = 0
        while (index < name.length) {
            val codePoint = name.codePointAt(index)
            hash = (hash * 31 + codePoint) and 0xffffffffL
            index += Character.charCount(codePoint)
        }
        return PALETTE[(hash % PALETTE.size).toInt()]
    }

    /** 0xAARRGGBB for Compose's `Color(Long)`. */
    fun argb(rgb: Long): Long = 0xff000000L or rgb
}
