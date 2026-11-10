package io.github.digitalpals.kiwi.ui

import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToLong

/**
 * App colours, as in the web app (web/src/lib/tint.ts): the source's tint, or a colour picked
 * from the name when a source has none. Evergreen stays the colour of actions. Colours are
 * 0xRRGGBB longs, so the rules stay plain Kotlin and are tested on the JVM.
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

    /** The colours behind an app's page and Fresh card (see [heroColors]). */
    data class Hero(val base: Long, val deep: Long)

    /**
     * The tint, darkened until white text on it reads at 4.5:1, and a deeper shade for the far
     * end of the gradient.
     */
    fun heroColors(rgb: Long): Hero {
        var base = channels(rgb)
        var steps = 0
        while (steps < 20 && 1.05 / (luminance(base) + 0.05) < 4.5) {
            base = darken(base, 0.06)
            steps++
        }
        return Hero(pack(base), pack(darken(base, 0.45)))
    }

    /** WCAG contrast of white text on this colour. */
    fun contrastWithWhite(rgb: Long): Double = 1.05 / (luminance(channels(rgb)) + 0.05)

    /**
     * The tint when it is colourful enough to mark a progress ring or wash a card in both light
     * and dark mode; greys, near-whites and near-blacks give null (the accent is used instead).
     */
    fun vividTint(tint: String?): Long? {
        val parsed = parse(tint) ?: return null
        val (r, g, b) = channels(parsed).map { it / 255 }
        val high = max(r, max(g, b))
        val low = min(r, min(g, b))
        val lightness = (high + low) / 2
        val saturation = if (high == low) 0.0 else (high - low) / (1 - abs(2 * lightness - 1))
        return if (saturation >= 0.3 && lightness >= 0.2 && lightness <= 0.75) parsed else null
    }

    private fun channels(rgb: Long): DoubleArray =
        doubleArrayOf(((rgb shr 16) and 0xff).toDouble(), ((rgb shr 8) and 0xff).toDouble(), (rgb and 0xff).toDouble())

    private fun pack(color: DoubleArray): Long =
        (color[0].roundToLong() shl 16) or (color[1].roundToLong() shl 8) or color[2].roundToLong()

    private fun darken(color: DoubleArray, amount: Double): DoubleArray =
        DoubleArray(3) { color[it] * (1 - amount) }

    private fun luminance(color: DoubleArray): Double {
        fun linear(value: Double): Double {
            val c = value / 255
            return if (c <= 0.04045) c / 12.92 else ((c + 0.055) / 1.055).pow(2.4)
        }
        return 0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
    }
}
