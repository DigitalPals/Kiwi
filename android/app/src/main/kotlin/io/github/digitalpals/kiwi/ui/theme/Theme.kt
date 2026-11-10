package io.github.digitalpals.kiwi.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// Kiwi's Material 3 theme, seeded with Evergreen, the web app's accent (web/src/styles.css).
// There is no dynamic colour: the brand stays Evergreen and the colour comes from the apps.

val EvergreenLight = Color(0xFF1E6B4D)
val EvergreenDark = Color(0xFF5BBF94)

/** The tonal scheme of the approved mockup (light), filled in from Evergreen's tonal palettes. */
private val LightColors: ColorScheme = lightColorScheme(
    primary = EvergreenLight,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFB0F1CE),
    onPrimaryContainer = Color(0xFF002114),
    inversePrimary = EvergreenDark,
    secondary = Color(0xFF4D6357),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFCFE9D9),
    onSecondaryContainer = Color(0xFF0B2015),
    tertiary = Color(0xFF3D6373),
    onTertiary = Color.White,
    tertiaryContainer = Color(0xFFC1E8FB),
    onTertiaryContainer = Color(0xFF001F29),
    background = Color(0xFFF1F5F1),
    onBackground = Color(0xFF181D1A),
    surface = Color(0xFFF1F5F1),
    onSurface = Color(0xFF181D1A),
    surfaceVariant = Color(0xFFDCE5DD),
    onSurfaceVariant = Color(0xFF414943),
    surfaceTint = EvergreenLight,
    inverseSurface = Color(0xFF2D322E),
    inverseOnSurface = Color(0xFFEEF2ED),
    error = Color(0xFFBA1A1A),
    onError = Color.White,
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF410002),
    outline = Color(0xFF717972),
    outlineVariant = Color(0xFFC1C9C2),
    scrim = Color.Black,
    surfaceBright = Color(0xFFF7FBF6),
    surfaceDim = Color(0xFFD7DBD7),
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFECF1EC),
    surfaceContainer = Color(0xFFE7ECE7),
    surfaceContainerHigh = Color(0xFFE3E9E3),
    surfaceContainerHighest = Color(0xFFDDE3DE),
)

private val DarkColors: ColorScheme = darkColorScheme(
    primary = EvergreenDark,
    onPrimary = Color(0xFF003823),
    primaryContainer = Color(0xFF0B5139),
    onPrimaryContainer = Color(0xFFB0F1CE),
    inversePrimary = EvergreenLight,
    secondary = Color(0xFFB4CCBE),
    onSecondary = Color(0xFF1F352A),
    secondaryContainer = Color(0xFF354B40),
    onSecondaryContainer = Color(0xFFCFE9D9),
    tertiary = Color(0xFFA5CCDF),
    onTertiary = Color(0xFF073543),
    tertiaryContainer = Color(0xFF244C5B),
    onTertiaryContainer = Color(0xFFC1E8FB),
    background = Color(0xFF0F1511),
    onBackground = Color(0xFFDEE4DE),
    surface = Color(0xFF0F1511),
    onSurface = Color(0xFFDEE4DE),
    surfaceVariant = Color(0xFF404943),
    onSurfaceVariant = Color(0xFFC0C9C1),
    surfaceTint = EvergreenDark,
    inverseSurface = Color(0xFFDEE4DE),
    inverseOnSurface = Color(0xFF2C322E),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    outline = Color(0xFF8A938B),
    outlineVariant = Color(0xFF404943),
    scrim = Color.Black,
    surfaceBright = Color(0xFF343B36),
    surfaceDim = Color(0xFF0F1511),
    surfaceContainerLowest = Color(0xFF0A0F0C),
    surfaceContainerLow = Color(0xFF171D19),
    surfaceContainer = Color(0xFF1B211D),
    surfaceContainerHigh = Color(0xFF252B27),
    surfaceContainerHighest = Color(0xFF303632),
)

/** Colours Material 3 has no role for: list cards, the amber warning and the status dot. */
@Immutable
data class KiwiColors(
    val dark: Boolean,
    /** List groups and cards on the background (white in light mode). */
    val card: Color,
    val warningContainer: Color,
    val onWarningContainer: Color,
    val warningBody: Color,
    val warningIcon: Color,
    /** "Connected" and other good news. */
    val success: Color,
    /** How strongly an app's colour washes an update card. */
    val washStrength: Float,
)

private val LightKiwi = KiwiColors(
    dark = false,
    card = Color.White,
    warningContainer = Color(0xFFFFEFD5),
    onWarningContainer = Color(0xFF2B1700),
    warningBody = Color(0xFF5A3C14),
    warningIcon = Color(0xFF9A5700),
    success = Color(0xFF7EF0A8),
    washStrength = 0.13f,
)

private val DarkKiwi = KiwiColors(
    dark = true,
    card = Color(0xFF1B211D),
    warningContainer = Color(0xFF3B2C12),
    onWarningContainer = Color(0xFFFFDDB3),
    warningBody = Color(0xFFE8CBA2),
    warningIcon = Color(0xFFFFB95C),
    success = Color(0xFF7EF0A8),
    washStrength = 0.24f,
)

private val LocalKiwiColors = staticCompositionLocalOf { LightKiwi }

/** Kiwi's own colours next to the Material ones: `MaterialTheme.kiwi.card`. */
val MaterialTheme.kiwi: KiwiColors
    @Composable @ReadOnlyComposable
    get() = LocalKiwiColors.current

/** Material 3's type scale in Roboto (the system font), with the mockup's medium titles. */
private val KiwiTypography: Typography = Typography().let { base ->
    base.copy(
        headlineMedium = base.headlineMedium.copy(fontWeight = FontWeight.Medium),
        titleLarge = base.titleLarge.copy(fontWeight = FontWeight.Medium),
    )
}

/** Large titles of lists ("Fresh this week", "Updates"): between title medium and large. */
val SectionTitleStyle = TextStyle(fontSize = 18.sp, lineHeight = 24.sp, fontWeight = FontWeight.Medium)

private val KiwiShapes = Shapes(
    extraSmall = androidx.compose.foundation.shape.RoundedCornerShape(6.dp),
    small = androidx.compose.foundation.shape.RoundedCornerShape(8.dp),
    medium = androidx.compose.foundation.shape.RoundedCornerShape(16.dp),
    large = androidx.compose.foundation.shape.RoundedCornerShape(22.dp),
    extraLarge = androidx.compose.foundation.shape.RoundedCornerShape(28.dp),
)

@Composable
fun KiwiTheme(darkTheme: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalKiwiColors provides if (darkTheme) DarkKiwi else LightKiwi) {
        MaterialTheme(
            colorScheme = if (darkTheme) DarkColors else LightColors,
            typography = KiwiTypography,
            shapes = KiwiShapes,
            content = content,
        )
    }
}
