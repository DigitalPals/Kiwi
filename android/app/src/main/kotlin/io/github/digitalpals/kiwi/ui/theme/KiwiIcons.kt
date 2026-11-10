package io.github.digitalpals.kiwi.ui.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.unit.dp

/** Selected navigation icons fill their shapes at 22 % (initialised before [KiwiIcons]). */
private val SELECTED_FILL = Color.Black.copy(alpha = 0.22f)

/**
 * The line icons of Kiwi's design (24 × 24, 2-unit strokes with round ends), drawn black and
 * tinted by `Icon`. Navigation icons have a `selected` variant whose shapes are filled at 22 %.
 */
object KiwiIcons {
    private fun browse(selected: Boolean) = icon("browse", selected) {
        rect(3f, 3f, 7.5f, 7.5f, 2f)
        rect(13.5f, 3f, 7.5f, 7.5f, 2f)
        rect(3f, 13.5f, 7.5f, 7.5f, 2f)
        rect(13.5f, 13.5f, 7.5f, 7.5f, 2f)
    }

    private fun myApps(selected: Boolean) = icon("my-apps", selected) {
        line("M12 2.8 2.5 7.6 12 12.4l9.5-4.8L12 2.8Z")
        line("m2.5 16.4 9.5 4.8 9.5-4.8M2.5 12l9.5 4.8 9.5-4.8", fillable = false)
    }

    private fun builds(selected: Boolean) = icon("builds", selected) {
        line(
            "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9" +
                "a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9l-3.8 3.8Z",
        )
    }

    private fun settings(selected: Boolean) = icon("settings", selected) {
        circle(12f, 12f, 3f)
        line(
            "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 " +
                "1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06" +
                "a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09" +
                "A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 " +
                "1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33" +
                "l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 " +
                "0 4h-.09a1.65 1.65 0 0 0-1.51 1Z",
            fillable = false,
        )
    }

    val Browse = browse(false)
    val BrowseSelected = browse(true)
    val MyApps = myApps(false)
    val MyAppsSelected = myApps(true)
    val Builds = builds(false)
    val BuildsSelected = builds(true)
    val Settings = settings(false)
    val SettingsSelected = settings(true)

    val Search = icon("search") {
        circle(11f, 11f, 7f)
        line("m20 20-3.9-3.9")
    }
    val Back = icon("back") { line("M20 12H4.5M11 5l-7 7 7 7") }
    val Chevron = icon("chevron") { line("m9 5 7 7-7 7") }
    val Plus = icon("plus") { line("M12 5v14M5 12h14") }
    val Close = icon("close") { line("M18 6 6 18M6 6l12 12") }
    val Check = icon("check") { line("m5 12.5 4.5 4.5L19 7.5") }
    val Arrow = icon("arrow") { line("M5 12h14M13 6l6 6-6 6") }
    val Phone = icon("phone") {
        rect(6.5f, 2.5f, 11f, 19f, 2.5f)
        line("M10.5 18.5h3")
    }
    val Bell = icon("bell") {
        line("M18 8.5a6 6 0 1 0-12 0c0 7-3 9-3 9h18s-3-2-3-9")
        line("M13.7 21a2 2 0 0 1-3.4 0")
    }
    val More = icon("more") {
        dot(12f, 5.5f, 1.7f)
        dot(12f, 12f, 1.7f)
        dot(12f, 18.5f, 1.7f)
    }
    val Warning = icon("warning") {
        line("M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z")
        line("M12 9v4M12 17h.01")
    }
    val Refresh = icon("refresh") {
        line("M20.5 4.5v5.5H15")
        line("M20.2 14.5A8.5 8.5 0 1 1 18.1 6l2.4 2.4")
    }
    val Clock = icon("clock") {
        circle(12f, 12f, 9.5f)
        line("M12 7v5l3 2")
    }
    val Network = icon("network") {
        circle(5.5f, 12f, 2.5f)
        circle(18.5f, 5.5f, 2.5f)
        circle(18.5f, 18.5f, 2.5f)
        line("m7.8 10.8 8.4-4.1M7.8 13.2l8.4 4.1")
    }
    val CheckCircle = icon("check-circle") {
        circle(12f, 12f, 9.5f)
        line("m7.8 12.3 2.9 2.9 5.5-5.8")
    }
    val MinusCircle = icon("minus-circle") {
        circle(12f, 12f, 9.5f)
        line("M8 12h8")
    }
    val Wrench = builds(false)
    val Download = icon("download") { line("M12 4v11M7.5 10.5 12 15l4.5-4.5M4 15v3.5A1.5 1.5 0 0 0 5.5 20h13a1.5 1.5 0 0 0 1.5-1.5V15") }
    val Tune = icon("tune") {
        line("M4 7h10M18 7h2M4 17h2M10 17h10")
        circle(16f, 7f, 2.2f)
        circle(8f, 17f, 2.2f)
    }
    val Info = icon("info") {
        circle(12f, 12f, 9.5f)
        line("M12 11v5M12 7.6h.01")
    }
    val Shield = icon("shield") {
        line("M12 2.8 4.5 5.6v6c0 4.6 3.1 8.2 7.5 9.6 4.4-1.4 7.5-5 7.5-9.6v-6L12 2.8Z")
        line("m8.8 12 2.3 2.3 4.3-4.6")
    }

    private class Scope(val builder: ImageVector.Builder, val selected: Boolean) {
        fun line(pathData: String, fillable: Boolean = true) {
            builder.addPath(
                pathData = addPathNodes(pathData),
                fill = if (selected && fillable) SolidColor(SELECTED_FILL) else null,
                stroke = SolidColor(Color.Black),
                strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
        }

        fun circle(cx: Float, cy: Float, r: Float) =
            line("M${cx - r},${cy}a$r,$r 0 1,0 ${2 * r},0a$r,$r 0 1,0 ${-2 * r},0Z")

        fun rect(x: Float, y: Float, w: Float, h: Float, r: Float) = line(
            "M${x + r},${y}h${w - 2 * r}a$r,$r 0 0 1 $r,${r}v${h - 2 * r}a$r,$r 0 0 1 ${-r},${r}" +
                "h${-(w - 2 * r)}a$r,$r 0 0 1 ${-r},${-r}v${-(h - 2 * r)}a$r,$r 0 0 1 $r,${-r}Z",
        )

        /** A filled dot (no stroke). */
        fun dot(cx: Float, cy: Float, r: Float) {
            builder.addPath(
                pathData = addPathNodes("M${cx - r},${cy}a$r,$r 0 1,0 ${2 * r},0a$r,$r 0 1,0 ${-2 * r},0Z"),
                fill = SolidColor(Color.Black),
            )
        }
    }

    private fun icon(name: String, selected: Boolean = false, block: Scope.() -> Unit): ImageVector {
        val builder = ImageVector.Builder(
            name = if (selected) "$name-selected" else name,
            defaultWidth = 24.dp,
            defaultHeight = 24.dp,
            viewportWidth = 24f,
            viewportHeight = 24f,
        )
        Scope(builder, selected).block()
        return builder.build()
    }
}
