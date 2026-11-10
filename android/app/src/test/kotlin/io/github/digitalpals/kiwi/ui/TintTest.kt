package io.github.digitalpals.kiwi.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Expected values come from the web app's web/src/lib/tint.ts. */
class TintTest {
    @Test
    fun parsesSixDigitHexColours() {
        assertEquals(0xff0000L, Tint.parse("#FF0000"))
        assertEquals(0x1e6b4dL, Tint.parse("1e6b4d"))
        assertNull(Tint.parse("#fff"))
        assertNull(Tint.parse("red"))
        assertNull(Tint.parse(null))
    }

    @Test
    fun picksTheSamePaletteColourAsTheWebApp() {
        // web/src/lib/tint.ts: hash = (hash * 31 + codePoint) >>> 0 over the name.
        // "YouTube" hashes to 671954723 there, and 671954723 % 8 = 3.
        assertEquals(Tint.PALETTE[3], Tint.appColor("YouTube", null))
        assertEquals(0x1e9e6aL, Tint.appColor("YouTube", null))
        assertEquals(0xc27c0eL, Tint.appColor("Reddit", null))
        assertEquals(0x2b7bffL, Tint.appColor("Kiwi", null))
        assertEquals(0xabcdefL, Tint.appColor("YouTube", "#abcdef"))
        assertEquals(0xff000000L or 0x1e6b4dL, Tint.argb(0x1e6b4dL))
    }

    @Test
    fun darkensHeroColoursUntilWhiteTextReads() {
        assertEquals(Tint.Hero(0xe10000, 0x7c0000), Tint.heroColors(0xff0000))
        assertEquals(Tint.Hero(0xd43900, 0x742000), Tint.heroColors(0xff4500))
        assertEquals(Tint.Hero(0x4f8019, 0x2b460e), Tint.heroColors(0x5f9a1e))
        assertEquals(Tint.Hero(0x81771e, 0x474110), Tint.heroColors(0xffeb3b))
        // Evergreen already reads: only the deep end changes.
        assertEquals(Tint.Hero(0x1e6b4d, 0x113b2a), Tint.heroColors(0x1e6b4d))
    }

    @Test
    fun measuresContrastWithWhite() {
        assertEquals(3.9985, Tint.contrastWithWhite(0xff0000), 0.0001)
        assertEquals(6.4310, Tint.contrastWithWhite(0x1e6b4d), 0.0001)
        assertEquals(1.2210, Tint.contrastWithWhite(0xffeb3b), 0.0001)
    }

    @Test
    fun keepsOnlyVividTints() {
        assertEquals(0xff0000L, Tint.vividTint("#ff0000"))
        assertEquals(0x1e6b4dL, Tint.vividTint("#1e6b4d"))
        assertEquals(0xffeb3bL, Tint.vividTint("#ffeb3b"))
        assertNull(Tint.vividTint("#808080"))
        assertNull(Tint.vividTint("#fafafa"))
        assertNull(Tint.vividTint("#101010"))
        assertNull(Tint.vividTint(null))
    }
}
