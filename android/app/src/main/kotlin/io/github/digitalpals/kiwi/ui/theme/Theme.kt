package io.github.digitalpals.kiwi.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// Evergreen, the web app's accent (web/src/styles.css). The final design of the screens is
// still to come; until then the app is plain Material 3 in Kiwi's accent.
val EvergreenLight = Color(0xFF1E6B4D)
val EvergreenDark = Color(0xFF5BBF94)

private val LightColors = lightColorScheme(
    primary = EvergreenLight,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFCDEBDC),
    onPrimaryContainer = Color(0xFF00210F),
    secondary = EvergreenLight,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFDCEDE4),
    onSecondaryContainer = Color(0xFF10251B),
)

private val DarkColors = darkColorScheme(
    primary = EvergreenDark,
    onPrimary = Color(0xFF03150D),
    primaryContainer = Color(0xFF13402E),
    onPrimaryContainer = Color(0xFFBFEBD5),
    secondary = EvergreenDark,
    onSecondary = Color(0xFF03150D),
    secondaryContainer = Color(0xFF1F3A2E),
    onSecondaryContainer = Color(0xFFD3EBDF),
)

@Composable
fun KiwiTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) DarkColors else LightColors, content = content)
}
