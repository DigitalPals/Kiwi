package io.github.digitalpals.kiwi.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import java.time.Instant
import java.time.ZoneOffset

class FormatTest {
    private val utc = ZoneOffset.UTC
    private val now = Instant.parse("2026-11-10T09:41:00Z")

    @Test
    fun formatsSizes() {
        assertEquals("900 bytes", Format.bytes(900))
        assertEquals("48 KB", Format.bytes(48_200))
        assertEquals("48.2 MB", Format.bytes(48_200_000))
        assertEquals("156 MB", Format.bytes(156_000_000))
        assertEquals("1.20 GB", Format.bytes(1_200_000_000))
    }

    @Test
    fun saysWhenKiwiWasChecked() {
        val at = now.toEpochMilli()
        assertEquals("Checked just now", Format.checkedAgo(at - 20_000, at, utc))
        assertEquals("Checked a minute ago", Format.checkedAgo(at - 70_000, at, utc))
        assertEquals("Checked 4 minutes ago", Format.checkedAgo(at - 4 * 60_000, at, utc))
        assertEquals("Checked an hour ago", Format.checkedAgo(at - 61 * 60_000, at, utc))
        assertEquals("Checked 3 hours ago", Format.checkedAgo(at - 3 * 3_600_000, at, utc))
    }

    @Test
    fun saysWhenTheNextCheckIs() {
        assertEquals("Next check at 15:00", Format.nextCheck(Instant.parse("2026-11-10T15:00:00Z"), now, utc))
        assertEquals("Next check tomorrow at 03:00", Format.nextCheck(Instant.parse("2026-11-11T03:00:00Z"), now, utc))
        assertEquals("Next check Nov 14 at 08:30", Format.nextCheck(Instant.parse("2026-11-14T08:30:00Z"), now, utc))
        assertEquals("Next check soon", Format.nextCheck(Instant.parse("2026-11-10T09:00:00Z"), now, utc))
    }

    @Test
    fun formatsDays() {
        assertEquals("Nov 8", Format.day(Instant.parse("2026-11-08T12:00:00Z"), utc, now))
        assertEquals("Dec 24, 2025", Format.day(Instant.parse("2025-12-24T12:00:00Z"), utc, now))
    }
}
