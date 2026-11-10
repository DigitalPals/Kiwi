package io.github.digitalpals.kiwi.api

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ServerUrlTest {
    private fun ok(input: String): String {
        val parsed = ServerUrl.parse(input)
        assertTrue("$input: $parsed", parsed is ServerUrl.Parsed.Ok)
        return (parsed as ServerUrl.Parsed.Ok).url.toString()
    }

    private fun invalid(input: String): String {
        val parsed = ServerUrl.parse(input)
        assertTrue("$input: $parsed", parsed is ServerUrl.Parsed.Invalid)
        return (parsed as ServerUrl.Parsed.Invalid).message
    }

    @Test
    fun normalisesToTheServerRoot() {
        assertEquals("https://mac.example.ts.net/", ok("https://mac.example.ts.net"))
        assertEquals("https://mac.example.ts.net/", ok("  mac.example.ts.net  "))
        assertEquals("https://mac.example.ts.net/", ok("HTTPS://Mac.Example.ts.net/apps/kiwi-youtube?x=1#top"))
        assertEquals("https://mac.example.ts.net:8443/", ok("https://mac.example.ts.net:8443/"))
    }

    @Test
    fun refusesWhatIsNotAnHttpsAddress() {
        assertTrue(invalid("http://mac.example.ts.net").contains("HTTPS"))
        invalid("")
        invalid("ftp://mac.example.ts.net")
        invalid("https://user:pass@mac.example.ts.net")
        invalid("mac")
        invalid("https://")
    }

    @Test
    fun readsConnectLinks() {
        assertEquals(
            "https://mac.example.ts.net",
            ServerUrl.fromConnectLink("kiwi://connect?url=https%3A%2F%2Fmac.example.ts.net"),
        )
        assertEquals(
            "https://mac.example.ts.net/",
            ServerUrl.fromConnectLink("kiwi://connect?x=1&url=https://mac.example.ts.net/"),
        )
        assertNull(ServerUrl.fromConnectLink("kiwi://other?url=https%3A%2F%2Fmac.example.ts.net"))
        assertNull(ServerUrl.fromConnectLink("https://connect?url=https%3A%2F%2Fmac.example.ts.net"))
        assertNull(ServerUrl.fromConnectLink("kiwi://connect"))
        assertNull(ServerUrl.fromConnectLink("kiwi://connect?url="))
    }
}
