package io.github.digitalpals.kiwi.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

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
        assertEquals(0xabcdefL, Tint.appColor("YouTube", "#abcdef"))
        assertEquals(0xff000000L or 0x1e6b4dL, Tint.argb(0x1e6b4dL))
    }
}
