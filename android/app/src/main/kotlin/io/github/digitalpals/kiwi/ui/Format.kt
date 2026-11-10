package io.github.digitalpals.kiwi.ui

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

/** Words for sizes, times and versions, shared by the screens. Plain Kotlin, tested on the JVM. */
object Format {
    /** "48.2 MB", as the web app shows sizes; whole megabytes from 100 MB ("156 MB"). */
    fun bytes(bytes: Long): String = when {
        bytes < 1_000 -> "$bytes bytes"
        bytes < 1_000_000 -> "${Math.round(bytes / 1e3)} KB"
        bytes < 100_000_000 -> String.format(Locale.US, "%.1f MB", bytes / 1e6)
        bytes < 1_000_000_000 -> String.format(Locale.US, "%.0f MB", bytes / 1e6)
        else -> String.format(Locale.US, "%.2f GB", bytes / 1e9)
    }

    /** "Checked just now", "Checked 4 minutes ago", "Checked 3 hours ago", "Checked Nov 8". */
    fun checkedAgo(checkedAt: Long, now: Long, zone: ZoneId = ZoneId.systemDefault()): String {
        val minutes = ((now - checkedAt) / 60_000).coerceAtLeast(0)
        return when {
            minutes < 1 -> "Checked just now"
            minutes == 1L -> "Checked a minute ago"
            minutes < 60 -> "Checked $minutes minutes ago"
            minutes < 120 -> "Checked an hour ago"
            minutes < 24 * 60 -> "Checked ${minutes / 60} hours ago"
            else -> "Checked ${day(Instant.ofEpochMilli(checkedAt), zone)}"
        }
    }

    /** "Nov 8" (this year's dates) or "Nov 8, 2025". */
    fun day(instant: Instant, zone: ZoneId = ZoneId.systemDefault(), now: Instant = Instant.now()): String {
        val date = instant.atZone(zone)
        val pattern = if (date.year == now.atZone(zone).year) "MMM d" else "MMM d, yyyy"
        return DateTimeFormatter.ofPattern(pattern, Locale.US).format(date)
    }

    /**
     * When the next automatic check is: "at 15:00" today, "tomorrow at 03:00", or a date.
     * Kiwi's times are UTC; the phone shows them in its own zone.
     */
    fun nextCheck(at: Instant, now: Instant, zone: ZoneId = ZoneId.systemDefault()): String {
        val time = DateTimeFormatter.ofPattern("HH:mm", Locale.US).format(at.atZone(zone))
        val days = ChronoUnit.DAYS.between(now.atZone(zone).toLocalDate(), at.atZone(zone).toLocalDate())
        return when {
            !at.isAfter(now) -> "Next check soon"
            days == 0L -> "Next check at $time"
            days == 1L -> "Next check tomorrow at $time"
            else -> "Next check ${day(at, zone, now)} at $time"
        }
    }
}
