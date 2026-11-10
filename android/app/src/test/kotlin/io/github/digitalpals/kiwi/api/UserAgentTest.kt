package io.github.digitalpals.kiwi.api

import org.junit.Assert.assertEquals
import org.junit.Test

class UserAgentTest {
    @Test
    fun followsTheContractFormat() {
        assertEquals(
            "Kiwi-Android/0.1.0 (1; Android 16; SDK 36; Google Pixel 9)",
            UserAgent.build("0.1.0", 1, "16", 36, "Google", "Pixel 9"),
        )
    }

    @Test
    fun doesNotRepeatTheManufacturer() {
        assertEquals(
            "Kiwi-Android/1.2 (12; Android 15; SDK 35; OnePlus CPH2581)",
            UserAgent.build("1.2", 12, "15", 35, "OnePlus", "OnePlus CPH2581"),
        )
        assertEquals("samsung SM-S921B", UserAgent.deviceName("samsung", "SM-S921B"))
    }

    @Test
    fun keepsTheHeaderParseableAndAscii() {
        assertEquals(
            "Kiwi-Android/1.0-beta (2; Android 17; SDK 37; Acme Phone X Pro 5G)",
            UserAgent.build("1.0 beta", 2, "17", 37, "Acme", "Phone (X); Pro 5G"),
        )
        assertEquals(
            "Kiwi-Android/0 (1; Android ; SDK 26; Android)",
            UserAgent.build("", 1, "", 26, "", ""),
        )
    }
}
