package io.github.digitalpals.kiwi.ui.components

import android.graphics.BlurMaskFilter
import android.os.Build
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import io.github.digitalpals.kiwi.R
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Tint
import io.github.digitalpals.kiwi.ui.VersionChange
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.SectionTitleStyle
import io.github.digitalpals.kiwi.ui.theme.kiwi
import kotlin.math.roundToInt

/** The colour of the Kiwi logo's flesh, for the Kiwi app icon's glow. */
val KiwiGreen = Color(0xFF5F9A1E)

/** An app's colour as a Compose colour: its tint, else the palette colour of its name. */
fun appColor(name: String, tint: String?): Color = Color(Tint.argb(Tint.appColor(name, tint)))

/**
 * A soft coloured shadow under a rounded shape, like the web app's tinted icon glow
 * (`drop-shadow(0 5px 6px tint 42%)`). Android draws blurred shapes from 9 (API 28) on.
 */
fun Modifier.glow(color: Color, blur: Dp, offsetY: Dp, corner: Dp): Modifier = drawBehind {
    if (Build.VERSION.SDK_INT < 28 || color.alpha == 0f) return@drawBehind
    drawIntoCanvas { canvas ->
        val paint = Paint().asFrameworkPaint().apply {
            isAntiAlias = true
            this.color = color.toArgb()
            maskFilter = BlurMaskFilter(blur.toPx(), BlurMaskFilter.Blur.NORMAL)
        }
        val dy = offsetY.toPx()
        canvas.nativeCanvas.drawRoundRect(0f, dy, size.width, size.height + dy, corner.toPx(), corner.toPx(), paint)
    }
}

/** An app's icon with its tinted glow; the first letter on the tint while it loads (or without one). */
@Composable
fun AppIcon(
    name: String,
    iconUrl: String?,
    tint: String?,
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    glow: Boolean = true,
    shadow: Color? = null,
) {
    val color = appColor(name, tint)
    val corner = size * 0.23f
    val shape = RoundedCornerShape(corner)
    Box(
        modifier = modifier
            .size(size)
            .then(
                if (glow) Modifier.glow(shadow ?: color.copy(alpha = 0.42f), size * 0.12f, size * 0.09f, corner) else Modifier,
            )
            .clip(shape)
            .background(color),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = name.take(1).uppercase(),
            color = Color.White,
            style = if (size >= 64.dp) MaterialTheme.typography.displaySmall else MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.Medium,
        )
        if (iconUrl != null) {
            AsyncImage(model = iconUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
        }
    }
}

/** Kiwi's own app icon (the logo on its tile), with a green glow. */
@Composable
fun KiwiAppIcon(modifier: Modifier = Modifier, size: Dp = 48.dp, glow: Boolean = true) {
    val corner = size * 0.23f
    Image(
        painter = painterResource(R.drawable.kiwi_tile),
        contentDescription = null,
        modifier = modifier
            .size(size)
            .then(if (glow) Modifier.glow(KiwiGreen.copy(alpha = 0.42f), size * 0.12f, size * 0.09f, corner) else Modifier)
            .clip(RoundedCornerShape(corner)),
    )
}

/** What the app's button does when tapped. */
class ButtonActions(
    val install: () -> Unit,
    val open: (String) -> Unit,
    val remove: (String) -> Unit,
    val allowInstalls: () -> Unit,
    /** Stops this app's own install (an unanswered confirmation, a long download). */
    val cancel: () -> Unit,
) {
    companion object {
        val None = ButtonActions({}, {}, {}, {}, {})
    }
}

/** A filled button: only for one-tap Install and Update. */
@Composable
fun FilledAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, small: Boolean = true, enabled: Boolean = true) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.height(if (small) 36.dp else 40.dp),
        contentPadding = PaddingValues(horizontal = if (small) 18.dp else 24.dp),
    ) { Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1) }
}

/** A tonal button: everything that isn't a one-tap install. */
@Composable
fun TonalAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, small: Boolean = true, enabled: Boolean = true) {
    FilledTonalButton(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.height(if (small) 36.dp else 40.dp),
        contentPadding = PaddingValues(horizontal = if (small) 18.dp else 24.dp),
    ) { Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1) }
}

/** The text of a progress button: "62%" while downloading, else what is happening. */
fun progressText(view: ButtonView.Progress): String =
    view.fraction?.let { "${(it * 100).roundToInt()}%" } ?: view.label

/** A small progress ring, determinate when the fraction is known. */
@Composable
fun ProgressRing(fraction: Float?, color: Color, modifier: Modifier = Modifier, size: Dp = 18.dp) {
    val track = color.copy(alpha = 0.2f)
    if (fraction != null) {
        CircularProgressIndicator(
            progress = { fraction },
            modifier = modifier.size(size),
            color = color,
            trackColor = track,
            strokeWidth = 2.5.dp,
        )
    } else {
        CircularProgressIndicator(modifier = modifier.size(size), color = color, trackColor = track, strokeWidth = 2.5.dp)
    }
}

/** A button-sized pill with progress; tapping it cancels this app's own job. */
@Composable
fun ProgressPill(view: ButtonView.Progress, onCancel: () -> Unit, modifier: Modifier = Modifier) {
    val label = if (view.cancellable) "Cancel ${view.label}" else view.label
    Surface(
        onClick = onCancel,
        enabled = view.cancellable,
        shape = CircleShape,
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = modifier
            .height(36.dp)
            .widthIn(min = 76.dp)
            .semantics { contentDescription = label },
    ) {
        Row(
            Modifier.padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
        ) {
            ProgressRing(view.fraction, MaterialTheme.colorScheme.primary)
            Spacer(Modifier.width(8.dp))
            Text(progressText(view), style = MaterialTheme.typography.labelLarge, maxLines = 1)
        }
    }
}

/** An app's main button in a list: filled to install or update, tonal for the rest. */
@Composable
fun ActionButton(view: ButtonView, actions: ButtonActions, modifier: Modifier = Modifier) {
    when (view) {
        is ButtonView.Install -> FilledAction(view.label, actions.install, modifier)
        is ButtonView.Open -> TonalAction("Open", { actions.open(view.packageName) }, modifier)
        is ButtonView.Remove -> TonalAction("Remove", { actions.remove(view.packageName) }, modifier)
        is ButtonView.AllowInstalls -> TonalAction("Allow", actions.allowInstalls, modifier)
        is ButtonView.Progress -> ProgressPill(view, actions.cancel, modifier)
        is ButtonView.Disabled -> TonalAction(view.label, {}, modifier, enabled = false)
    }
}

/** The caption a button wants shown in its row (errors, reasons), or null. */
fun ButtonView.caption(): Pair<String, Boolean>? = when (this) {
    is ButtonView.Install -> caption?.takeIf { error }?.let { it to true }
    is ButtonView.Remove -> caption to true
    is ButtonView.AllowInstalls -> caption to false
    is ButtonView.Disabled -> caption?.let { it to false }
    else -> null
}

/** "9.29.52 → 9.30.53" with the new version emphasised. */
@Composable
fun versionChangeText(change: VersionChange): AnnotatedString {
    val strong = MaterialTheme.colorScheme.onSurface
    return buildAnnotatedString {
        append(change.from)
        append(" → ")
        withStyle(SpanStyle(fontWeight = FontWeight.Medium, color = strong)) { append(change.to) }
    }
}

/** A row of an app list: icon, name, a second line and the action button. */
@Composable
fun AppListRow(
    name: String,
    iconUrl: String?,
    tint: String?,
    subtitle: AnnotatedString?,
    view: ButtonView,
    actions: ButtonActions,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = 72.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        AppIcon(name, iconUrl, tint)
        Column(Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null && subtitle.isNotEmpty()) {
                Text(
                    subtitle,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            view.caption()?.let { (text, error) ->
                Text(
                    text,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        ActionButton(view, actions)
    }
}

/** A large list title ("Fresh this week", "Updates"), with an optional action at the end. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier, action: (@Composable RowScope.() -> Unit)? = null) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .padding(start = 20.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = SectionTitleStyle, modifier = Modifier.weight(1f))
        action?.invoke(this)
    }
}

/** A small section label in the accent ("This phone"), above a list group. */
@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 8.dp),
    )
}

/** Collects the rows of a [ListGroup]. */
class ListGroupScope {
    internal val rows = mutableListOf<@Composable () -> Unit>()

    fun row(content: @Composable () -> Unit) {
        rows += content
    }
}

/** Material 3's segmented list: cards 2 dp apart, round at the ends of the group. */
@Composable
fun ListGroup(modifier: Modifier = Modifier, content: ListGroupScope.() -> Unit) {
    val rows = ListGroupScope().apply(content).rows
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        rows.forEachIndexed { index, row ->
            val top = if (index == 0) 22.dp else 6.dp
            val bottom = if (index == rows.lastIndex) 22.dp else 6.dp
            Surface(
                shape = RoundedCornerShape(topStart = top, topEnd = top, bottomStart = bottom, bottomEnd = bottom),
                color = MaterialTheme.kiwi.card,
                modifier = Modifier.fillMaxWidth(),
            ) { row() }
        }
    }
}

/** A row of a list group: leading icon, title and detail, trailing control. */
@Composable
fun GroupRow(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: AnnotatedString? = null,
    subtitleColor: Color = MaterialTheme.colorScheme.onSurfaceVariant,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    titleColor: Color = MaterialTheme.colorScheme.onSurface,
    onClick: (() -> Unit)? = null,
    below: (@Composable ColumnScope.() -> Unit)? = null,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = if (subtitle == null && leading == null) 56.dp else 72.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        leading?.invoke()
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = titleColor)
            if (subtitle != null && subtitle.isNotEmpty()) {
                Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = subtitleColor)
            }
            below?.invoke(this)
        }
        trailing?.invoke()
    }
}

/** An icon (white unless [iconTint] says otherwise) on a coloured circle: settings rows. */
@Composable
fun IconCircle(icon: ImageVector, color: Color, modifier: Modifier = Modifier, size: Dp = 40.dp, iconTint: Color = Color.White) {
    Box(
        modifier
            .size(size)
            .clip(CircleShape)
            .background(color),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = iconTint, modifier = Modifier.size(size / 2))
    }
}

/** Material 3's switch with a check mark on the thumb while on. */
@Composable
fun KiwiSwitch(checked: Boolean, onCheckedChange: ((Boolean) -> Unit)?, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Switch(
        checked = checked,
        onCheckedChange = onCheckedChange,
        enabled = enabled,
        modifier = modifier,
        thumbContent = if (checked) {
            { Icon(KiwiIcons.Check, contentDescription = null, modifier = Modifier.size(SwitchDefaults.IconSize)) }
        } else {
            null
        },
    )
}

/** The amber card: something on the phone blocks an install. */
@Composable
fun WarningCard(title: String, body: String, modifier: Modifier = Modifier, action: String? = null, onAction: () -> Unit = {}) {
    val colors = MaterialTheme.kiwi
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = colors.warningContainer,
        contentColor = colors.onWarningContainer,
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp),
    ) {
        Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = if (action != null) 6.dp else 14.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(KiwiIcons.Warning, contentDescription = null, tint = colors.warningIcon, modifier = Modifier.size(22.dp))
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium)
                    Text(body, style = MaterialTheme.typography.bodyMedium, color = colors.warningBody, modifier = Modifier.padding(top = 2.dp))
                }
            }
            if (action != null) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    TextButton(onClick = onAction) { Text(action, color = colors.warningIcon) }
                }
            }
        }
    }
}

/** A whole-screen message: loading failed, nothing to show, or something to turn on. */
@Composable
fun MessagePane(
    title: String,
    message: String?,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    primary: Pair<String, () -> Unit>? = null,
    secondary: Pair<String, () -> Unit>? = null,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(horizontal = 32.dp, vertical = 24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (icon != null) {
            Box(
                Modifier
                    .size(72.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.secondaryContainer),
                contentAlignment = Alignment.Center,
            ) {
                Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSecondaryContainer, modifier = Modifier.size(32.dp))
            }
            Spacer(Modifier.height(20.dp))
        }
        Text(title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        if (!message.isNullOrBlank()) {
            Spacer(Modifier.height(8.dp))
            Text(
                message,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
        if (primary != null) {
            Spacer(Modifier.height(24.dp))
            TonalAction(primary.first, primary.second, small = false)
        }
        if (secondary != null) {
            Spacer(Modifier.height(4.dp))
            TextButton(onClick = secondary.second) { Text(secondary.first) }
        }
    }
}

@Composable
fun Loading(modifier: Modifier = Modifier) {
    Box(modifier = modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

/** A tab's top app bar: a large title on the background, tinted once the list scrolls. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun KiwiTopBar(
    title: String,
    scrollBehavior: TopAppBarScrollBehavior? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    TopAppBar(
        title = { Text(title, style = MaterialTheme.typography.headlineMedium, maxLines = 1) },
        actions = actions,
        scrollBehavior = scrollBehavior,
        colors = TopAppBarDefaults.topAppBarColors(
            containerColor = MaterialTheme.colorScheme.background,
            scrolledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
            actionIconContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
        ),
    )
}

/** A short line under a tab's title ("Checked 4 minutes ago"). */
@Composable
fun TitleSummary(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = modifier.padding(start = 20.dp, end = 20.dp, bottom = 4.dp),
    )
}

/** The snackbar of the main screens, for messages after actions. */
val LocalSnackbars = androidx.compose.runtime.staticCompositionLocalOf<androidx.compose.material3.SnackbarHostState?> { null }
