package io.github.digitalpals.kiwi.ui.screens

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.api.AppDetail
import io.github.digitalpals.kiwi.api.AutomationState
import io.github.digitalpals.kiwi.api.Release
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppActions
import io.github.digitalpals.kiwi.ui.AppPage
import io.github.digitalpals.kiwi.ui.AppViewModel
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Format
import io.github.digitalpals.kiwi.ui.Fresh
import io.github.digitalpals.kiwi.ui.KiwiBuilds
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.Tint
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.components.AppIcon
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.components.ListGroup
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.ProgressRing
import io.github.digitalpals.kiwi.ui.components.SectionTitle
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.WarningCard
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi
import io.github.digitalpals.kiwi.work.UpdateNotices
import kotlin.math.roundToInt

/** An app's page: the app in its colour, its button, facts, patches, description and versions. */
@Composable
fun AppScreen(container: AppContainer, model: AppViewModel, actions: AppActions, onBack: () -> Unit, onEditBuild: (String) -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    val context = LocalContext.current
    when (val load = state) {
        Load.Loading -> Loading()
        is Load.Failed -> LoadFailed(load.error, onRetry = { model.reload() })
        is Load.Ready -> AppPageContent(
            page = load.value,
            job = jobs[load.value.detail.summary.key],
            actions = actions,
            onBack = onBack,
            onEditBuild = onEditBuild,
            onAppInfo = { SystemActions.appInfo(context, it) },
            onUninstall = { SystemActions.uninstall(context, it) },
        )
    }
}

/** C3: the top in the app's colour with a white button that fills while Kiwi installs. */
@Composable
fun AppPageContent(
    page: AppPage,
    job: InstallJob?,
    actions: AppActions,
    onBack: () -> Unit,
    onEditBuild: (String) -> Unit,
    onAppInfo: (String) -> Unit,
    onUninstall: (String) -> Unit,
) {
    val app = page.detail
    val summary = app.summary
    val view = appButton(summary, job, page.local)
    val buttons = actions.buttons(summary)
    val hero = Tint.heroColors(Tint.appColor(app.name, app.tintColor))
    val base = Color(Tint.argb(hero.base))
    val deep = Color(Tint.argb(hero.deep))
    val background = MaterialTheme.colorScheme.background
    LightStatusBarIcons()
    Column(
        Modifier
            .fillMaxSize()
            .background(background)
            .verticalScroll(rememberScrollState())
            .padding(bottom = 24.dp),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .drawBehind { heroBackground(base, deep, background, fade = 96.dp.toPx()) },
        ) {
            Column(Modifier.statusBarsPadding()) {
                HeroBar(page, onBack, onEditBuild, onAppInfo, onUninstall)
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    AppIcon(app.name, app.iconUrl, app.tintColor, size = 92.dp, shadow = Color.Black.copy(alpha = 0.4f))
                    Text(
                        app.name,
                        color = Color.White,
                        fontSize = 30.sp,
                        lineHeight = 36.sp,
                        fontWeight = FontWeight.Medium,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(top = 16.dp, bottom = 8.dp),
                    )
                    page.recipe?.let { PatchedChip(KiwiBuilds.patcherName(it.tweak)) }
                    Text(
                        heroLine(app, page.recipe),
                        color = Color.White.copy(alpha = 0.8f),
                        fontSize = 13.5.sp,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                    HeroButton(view, base, buttons, app.name, Modifier.padding(top = 20.dp, bottom = 10.dp))
                    heroNote(view, summary.state.installed != null)?.let {
                        Text(it, color = Color.White.copy(alpha = 0.88f), fontSize = 13.sp, textAlign = TextAlign.Center)
                    }
                }
                Spacer(Modifier.height(22.dp))
                Facts(app, Modifier.padding(horizontal = 16.dp))
            }
        }
        if (app.state.foreignInstall && page.local.installed) {
            WarningCard(
                "${app.name} can't be installed",
                "Another ${app.name} is installed with a different signature. Remove it first, then install Kiwi's.",
                Modifier.padding(top = 14.dp),
                action = "Uninstall it",
                onAction = { buttons.remove(summary.packageName) },
            )
        }
        app.state.replaces?.let {
            Text(
                "Replaces ${it.name} from ${it.sourceName}",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 14.dp),
            )
        }
        page.recipe?.let { Patches(it, onEdit = { onEditBuild(it.id) }) }
        if (!app.description.isNullOrBlank()) {
            SectionTitle("About")
            Text(
                app.description,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 20.dp),
            )
        }
        if (app.releases.isNotEmpty()) {
            SectionTitle("Versions")
            ListGroup {
                app.releases.take(MAX_RELEASES).forEach { release -> row { ReleaseRow(release) } }
            }
        }
    }
}

/** White status bar icons while the page shows (its top is the app's dark colour). */
@Composable
private fun LightStatusBarIcons() {
    val view = LocalView.current
    DisposableEffect(view) {
        val window = view.context.findActivity()?.window ?: return@DisposableEffect onDispose {}
        val controller = WindowCompat.getInsetsController(window, view)
        val before = controller.isAppearanceLightStatusBars
        controller.isAppearanceLightStatusBars = false
        onDispose { controller.isAppearanceLightStatusBars = before }
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** "Private Kiwi build · signed with your key" for Kiwi Builds, else the developer or source. */
private fun heroLine(app: AppDetail, recipe: AutomationState?): String = when {
    recipe != null || app.sourceId == KiwiBuilds.SOURCE_ID -> "${app.developer ?: "Private Kiwi build"} · signed with your key"
    else -> app.developer ?: app.sourceName
}

/** The line under the hero button: what is happening, or why the button is as it is. */
private fun heroNote(view: ButtonView, installed: Boolean): String? = when (view) {
    is ButtonView.Progress -> "Kiwi installs it in the background"
    is ButtonView.Install -> view.caption ?: if (!installed) "Android asks you to confirm the first install" else null
    is ButtonView.Remove -> view.caption
    is ButtonView.AllowInstalls -> view.caption
    is ButtonView.Disabled -> view.caption
    is ButtonView.Open -> null
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.heroBackground(base: Color, deep: Color, page: Color, fade: Float) {
    drawRect(Brush.verticalGradient(listOf(base, deep)))
    // Light from the top left, depth from the right, as on the web app's app pages.
    drawRect(
        Brush.radialGradient(
            listOf(lerp(base, Color.White, 0.2f), Color.Transparent),
            center = Offset(size.width * 0.3f, size.height * 0.1f),
            radius = size.width * 0.8f,
        ),
    )
    drawRect(
        Brush.radialGradient(
            listOf(deep.copy(alpha = 0.8f), Color.Transparent),
            center = Offset(size.width, size.height * 0.35f),
            radius = size.width * 0.7f,
        ),
    )
    val start = (size.height - fade) / size.height
    drawRect(Brush.verticalGradient(start to Color.Transparent, 1f to page))
}

@Composable
private fun HeroBar(
    page: AppPage,
    onBack: () -> Unit,
    onEditBuild: (String) -> Unit,
    onAppInfo: (String) -> Unit,
    onUninstall: (String) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val packageName = page.detail.summary.packageName
    val installed = page.local.installed
    Row(
        Modifier
            .fillMaxWidth()
            .height(56.dp)
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) { Icon(KiwiIcons.Back, contentDescription = "Back", tint = Color.White) }
        Spacer(Modifier.weight(1f))
        if (installed || page.recipe != null) {
            Box {
                IconButton(onClick = { menu = true }) { Icon(KiwiIcons.More, contentDescription = "More", tint = Color.White) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    page.recipe?.let { recipe ->
                        DropdownMenuItem(text = { Text(if (recipe.community != null) "Patches" else "Patches and schedule") }, onClick = {
                            menu = false
                            onEditBuild(recipe.id)
                        })
                    }
                    if (installed) {
                        DropdownMenuItem(text = { Text("App info") }, onClick = {
                            menu = false
                            onAppInfo(packageName)
                        })
                        DropdownMenuItem(text = { Text("Uninstall") }, onClick = {
                            menu = false
                            onUninstall(packageName)
                        })
                    }
                }
            }
        }
    }
}

@Composable
private fun PatchedChip(patcher: String) {
    Row(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(Color.White.copy(alpha = 0.18f))
            .height(32.dp)
            .padding(start = 10.dp, end = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(KiwiIcons.Wrench, contentDescription = null, tint = Color.White, modifier = Modifier.size(16.dp))
        Text("Patched with $patcher", color = Color.White, style = MaterialTheme.typography.labelLarge)
    }
}

/** The white button on the app's colour; it fills from the left while Kiwi downloads. */
@Composable
private fun HeroButton(view: ButtonView, color: Color, actions: ButtonActions, name: String, modifier: Modifier = Modifier) {
    val (label, onClick) = when (view) {
        is ButtonView.Install -> view.label to actions.install
        is ButtonView.Open -> "Open" to { actions.open(view.packageName) }
        is ButtonView.Remove -> "Remove the other $name" to { actions.remove(view.packageName) }
        is ButtonView.AllowInstalls -> "Allow Kiwi to install apps" to actions.allowInstalls
        is ButtonView.Progress -> {
            val percent = view.fraction?.let { " ${(it * 100).roundToInt()}%" }.orEmpty()
            "${view.label}…$percent" to actions.cancel
        }
        is ButtonView.Disabled -> view.label to {}
    }
    val enabled = view !is ButtonView.Disabled && (view !is ButtonView.Progress || view.cancellable)
    Surface(
        onClick = onClick,
        enabled = enabled,
        shape = CircleShape,
        color = Color.White,
        contentColor = color,
        shadowElevation = 6.dp,
        modifier = modifier
            .fillMaxWidth()
            .height(52.dp),
    ) {
        Box(contentAlignment = Alignment.Center) {
            if (view is ButtonView.Progress && view.fraction != null) {
                Box(
                    Modifier
                        .align(Alignment.CenterStart)
                        .fillMaxHeight()
                        .fillMaxWidth(view.fraction)
                        .background(color.copy(alpha = 0.13f)),
                )
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (view is ButtonView.Progress) {
                    ProgressRing(view.fraction, color, size = 20.dp)
                    Spacer(Modifier.width(10.dp))
                }
                Text(
                    label,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Medium,
                    color = if (enabled || view is ButtonView.Progress) color else color.copy(alpha = 0.5f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** Version, size, and what the app needs (or when it came out), in a card. */
@Composable
private fun Facts(app: AppDetail, modifier: Modifier = Modifier) {
    val release = app.state.target ?: app.releases.firstOrNull()
    val facts = buildList {
        (release?.version ?: app.state.installed?.version)?.let { add(it to "Version") }
        release?.size?.let { add(Format.bytes(it) to "Size") }
        when {
            release?.minOs != null -> add("Android ${release.minOs}" to "Requires")
            else -> Fresh.parseDate(release?.date)?.let { add(Format.day(it) to "Released") }
        }
    }
    if (facts.isEmpty()) return
    Surface(
        shape = RoundedCornerShape(22.dp),
        color = MaterialTheme.kiwi.card,
        shadowElevation = 2.dp,
        modifier = modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(vertical = 14.dp).height(IntrinsicSize.Min)) {
            facts.forEachIndexed { index, (value, label) ->
                if (index > 0) VerticalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(value, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

/** The patches of a Kiwi Builds app: on (check) and off, and a way to change them. */
@Composable
private fun Patches(recipe: AutomationState, onEdit: () -> Unit) {
    val tweaks = KiwiBuilds.builtTweaks(recipe)
    val (on, total) = KiwiBuilds.patchCount(recipe, tweaks)
    SectionTitle("Patches", Modifier.padding(top = 8.dp)) { TonalAction("Change", onEdit) }
    Text(
        "${recipe.tweak} · $on of $total on",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 20.dp, end = 20.dp, bottom = 4.dp),
    )
    recipe.included.forEach { PatchRow(it.name, on = true) }
    recipe.availableTweaks.forEach { PatchRow(it.name, on = it.id in tweaks) }
}

@Composable
private fun PatchRow(name: String, on: Boolean) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
            .padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Icon(
            if (on) KiwiIcons.CheckCircle else KiwiIcons.MinusCircle,
            contentDescription = if (on) "On" else "Off",
            tint = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
            modifier = Modifier.size(22.dp),
        )
        Text(
            name,
            style = MaterialTheme.typography.bodyLarge,
            color = if (on) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ReleaseRow(release: Release) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
        Row {
            Text(UpdateNotices.formatVersion(release), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
            Fresh.parseDate(release.date)?.let {
                Text(Format.day(it), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        release.notes?.takeIf { it.isNotBlank() }?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 6,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
        if (!release.compatible && release.incompatibility != null) {
            Text(release.incompatibility, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
    }
}

private const val MAX_RELEASES = 10

@Preview(name = "C3 App page")
@Composable
private fun AppPagePreview() {
    KiwiTheme(darkTheme = false) {
        AppPageContent(PreviewData.youTubePage, PreviewData.youTubeInstalling, AppActions(), {}, {}, {}, {})
    }
}

@Preview(name = "C3 App page, dark")
@Composable
private fun AppPageDarkPreview() {
    KiwiTheme(darkTheme = true) {
        AppPageContent(PreviewData.youTubePage, null, AppActions(), {}, {}, {}, {})
    }
}
