package io.github.digitalpals.kiwi.ui

import android.app.Application
import android.content.res.Configuration
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.RoborazziComposeOptions
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.RoborazziTaskType
import com.github.takahirom.roborazzi.captureRoboImage
import com.github.takahirom.roborazzi.size
import com.github.takahirom.roborazzi.uiMode
import io.github.digitalpals.kiwi.R
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.screens.AppPageContent
import io.github.digitalpals.kiwi.ui.screens.BrowseContent
import io.github.digitalpals.kiwi.ui.screens.BuildsActions
import io.github.digitalpals.kiwi.ui.screens.BuildsContent
import io.github.digitalpals.kiwi.ui.screens.CommunityAppSheetContent
import io.github.digitalpals.kiwi.ui.screens.CommunityCandidatesSheetContent
import io.github.digitalpals.kiwi.ui.screens.CommunityInstallSheetContent
import io.github.digitalpals.kiwi.ui.screens.CommunitySearchContent
import io.github.digitalpals.kiwi.ui.screens.ConnectContent
import io.github.digitalpals.kiwi.ui.screens.EditBuildSheetContent
import io.github.digitalpals.kiwi.ui.screens.InstallBuildSheetContent
import io.github.digitalpals.kiwi.ui.screens.MyAppsContent
import io.github.digitalpals.kiwi.ui.screens.PhoneState
import io.github.digitalpals.kiwi.ui.screens.SettingsActions
import io.github.digitalpals.kiwi.ui.screens.SettingsContent
import io.github.digitalpals.kiwi.ui.screens.SheetPreviewFrame
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * Renders the screens of the approved mockup (C1–C6) on the JVM, in light and dark, with the
 * preview data, to `app/build/outputs/roborazzi/`. They are records to compare with the mockup,
 * not golden images: the test fails only when a screen can't be composed.
 *
 * Robolectric runs SDK 35 here: SDK 36 needs JVM flags on JDK 21 that the build doesn't set.
 */
@OptIn(ExperimentalRoborazziApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], application = Application::class, qualifiers = "w360dp-h780dp-xxhdpi")
class ScreenshotTest {
    private fun shot(name: String, dark: Boolean, height: Int = 780, content: @Composable () -> Unit) {
        captureRoboImage(
            file = File("build/outputs/roborazzi/$name-${if (dark) "dark" else "light"}.png"),
            roborazziOptions = RoborazziOptions(taskType = RoborazziTaskType.Record),
            roborazziComposeOptions = RoborazziComposeOptions {
                size(widthDp = 360, heightDp = height)
                uiMode(if (dark) Configuration.UI_MODE_NIGHT_YES else Configuration.UI_MODE_NIGHT_NO)
            },
        ) {
            KiwiTheme(darkTheme = dark) { content() }
        }
    }

    /** A screen above the navigation bar, as on the phone. */
    @Composable
    private fun Phone(tab: KiwiTab?, content: @Composable () -> Unit) {
        Scaffold(
            bottomBar = { KiwiNavigationBar(tab, badge = updateBadge(PreviewData.me)) {} },
            contentWindowInsets = WindowInsets(0),
        ) { padding ->
            Box(Modifier.padding(padding)) { content() }
        }
    }

    private fun both(name: String, height: Int = 780, content: @Composable (dark: Boolean) -> Unit) {
        shot(name, dark = false, height) { content(false) }
        shot(name, dark = true, height) { content(true) }
    }

    @Test
    fun c1Connect() {
        shot("c1-connect", dark = false) {
            ConnectContent(ConnectViewModel.State("https://kiwi.example.ts.net"), false, {}, {}, null, {})
        }
        shot("c1-connect", dark = true) {
            ConnectContent(
                ConnectViewModel.State(
                    "https://kiwi.example.ts.net",
                    problem = ConnectViewModel.Problem("Can't Reach Kiwi", "Can't reach Kiwi at kiwi.example.ts.net. Is Tailscale on?", network = true),
                ),
                false, {}, {}, {}, {},
            )
        }
    }

    @Test
    fun c2Browse() = both("c2-browse") { dark ->
        Phone(KiwiTab.Browse) {
            BrowseContent(
                Load.Ready(PreviewData.browse),
                "",
                {},
                PreviewData.freshKeys,
                if (dark) PreviewData.downloading else emptyMap(),
                AppActions(),
                {}, {}, {},
            )
        }
    }

    @Test
    fun c3AppPage() = both("c3-app") { dark ->
        Phone(KiwiTab.Browse) {
            AppPageContent(PreviewData.youTubePage, if (dark) null else PreviewData.youTubeInstalling, AppActions(), {}, {}, {}, {})
        }
    }

    @Test
    fun c4MyApps() = both("c4-my-apps") { dark ->
        Phone(KiwiTab.MyApps) {
            MyAppsContent(
                Load.Ready(PreviewData.myApps),
                if (dark) PreviewData.downloading else emptyMap(),
                AppActions(),
                "Checked 4 minutes ago",
                {}, {}, { ButtonActions.None }, {},
            )
        }
    }

    @Test
    fun c5Settings() = both("c5-settings") { dark ->
        Phone(KiwiTab.Settings) {
            SettingsContent(
                PreviewData.me,
                PreviewData.SERVER,
                PhoneState("Google Pixel 9a", "Android 16", autoUpdate = true, notifications = !dark, canInstall = !dark),
                PreviewData.kiwiOffer,
                null,
                SettingsActions(),
            )
        }
    }

    @Test
    fun c6Builds() = both("c6-builds") { _ ->
        Phone(KiwiTab.Builds) {
            BuildsContent(
                Load.Ready(PreviewData.buildsContent),
                emptySet(),
                notifyBuilds = true,
                now = PreviewData.NOW,
                actions = BuildsActions(),
            )
        }
    }

    @Test
    fun c6BuildsFirstRunAndInProgress() {
        both("c6-builds-first-run") {
            Phone(KiwiTab.Builds) {
                BuildsContent(Load.Ready(PreviewData.buildsFirstRun), emptySet(), true, PreviewData.NOW, BuildsActions())
            }
        }
        both("c6-builds-in-progress") {
            Phone(KiwiTab.Builds) {
                BuildsContent(
                    Load.Ready(PreviewData.buildsInProgress),
                    emptySet(),
                    true,
                    PreviewData.NOW,
                    BuildsActions(),
                    pending = PreviewData.pendingInstalls,
                )
            }
        }
    }

    @Test
    fun c6BuildsOffAndSheets() {
        both("c6-builds-off") {
            Phone(KiwiTab.Builds) {
                BuildsContent(Load.Ready(BuildsViewModel.Content(off = true, apps = KiwiBuilds.OFF_APPS)), emptySet(), true, PreviewData.NOW, BuildsActions())
            }
        }
        both("c6-install-sheet") {
            SheetPreviewFrame {
                InstallBuildSheetContent(PreviewData.redditToInstall, "#FF4500", PreviewData.redditBlocker, false, false, {}, {}, {})
            }
        }
        both("c6-app-sheet") {
            SheetPreviewFrame { EditBuildSheetContent(PreviewData.youTubeRecipe, "#FF0000", false, {}, {}, {}, {}) }
        }
    }

    /** The community section (step 4), tall enough to show all of it below Kiwi Builds. */
    @Test
    fun c6Community() {
        both("c6-community", height = 2300) {
            Phone(KiwiTab.Builds) {
                BuildsContent(
                    Load.Ready(PreviewData.buildsCommunity),
                    emptySet(),
                    true,
                    PreviewData.NOW,
                    BuildsActions(),
                    pending = PreviewData.communityPending,
                )
            }
        }
        both("c6-community-mac", height = 1900) {
            Phone(KiwiTab.Builds) { BuildsContent(Load.Ready(PreviewData.buildsCommunityMac), emptySet(), true, PreviewData.NOW, BuildsActions()) }
        }
        both("c6-community-loading", height = 1100) {
            Phone(KiwiTab.Builds) { BuildsContent(Load.Ready(PreviewData.buildsCommunityLoading), emptySet(), true, PreviewData.NOW, BuildsActions()) }
        }
        both("c6-community-error", height = 1100) {
            Phone(KiwiTab.Builds) { BuildsContent(Load.Ready(PreviewData.buildsCommunityError), emptySet(), true, PreviewData.NOW, BuildsActions()) }
        }
    }

    @Test
    fun c6CommunitySheets() {
        both("c6-community-install-sheet", height = 1250) {
            SheetPreviewFrame {
                CommunityInstallSheetContent(
                    Community.target(PreviewData.discord), null, emptyList(), PreviewData.discordBlocker,
                    available = true, reason = null, notificationsAllowed = false, busy = false, onBuild = {}, onUninstall = {}, onNotificationSettings = {},
                )
            }
        }
        both("c6-community-install-sheet-mac", height = 1000) {
            SheetPreviewFrame {
                CommunityInstallSheetContent(
                    Community.target(PreviewData.discord), null, emptyList(), null,
                    available = false, reason = PreviewData.MAC_REASON, notificationsAllowed = true, busy = false, onBuild = {}, onUninstall = {}, onNotificationSettings = {},
                )
            }
        }
        both("c6-community-app-sheet", height = 1500) {
            SheetPreviewFrame { CommunityAppSheetContent(PreviewData.twitchCommunity, "#9146FF", true, false, {}, {}, {}, {}, {}) }
        }
        both("c6-community-app-sheet-failed", height = 900) {
            SheetPreviewFrame { CommunityAppSheetContent(PreviewData.xFailed, null, true, false, {}, {}, {}, {}, {}) }
        }
        both("c6-community-app-sheet-unavailable", height = 900) {
            SheetPreviewFrame { CommunityAppSheetContent(PreviewData.chessUnavailable, null, true, false, {}, {}, {}, {}, {}) }
        }
        both("c6-community-candidates", height = 1000) {
            SheetPreviewFrame {
                CommunityCandidatesSheetContent(
                    PreviewData.twitchCommunity, "#9146FF", Load.Ready(PreviewData.twitchCandidates), PreviewData.NOW, true, false, {}, {},
                )
            }
        }
    }

    @Test
    fun c6CommunitySearch() {
        both("c6-community-search") {
            SearchFrame { CommunitySearchContent(PreviewData.communitySearch, true, emptySet(), emptySet(), {}, {}, {}, {}, {}) }
        }
        both("c6-community-search-empty") {
            SearchFrame { CommunitySearchContent(PreviewData.communitySearchEmpty, true, emptySet(), emptySet(), {}, {}, {}, {}, {}) }
        }
        both("c6-community-search-failed") {
            SearchFrame { CommunitySearchContent(PreviewData.communitySearchFailed, false, emptySet(), emptySet(), {}, {}, {}, {}, {}) }
        }
    }

    /** The search sheet's background, with room at the top as the sheet leaves. */
    @Composable
    private fun SearchFrame(content: @Composable () -> Unit) {
        Surface(color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxSize()) {
            Box(Modifier.padding(top = 24.dp)) { content() }
        }
    }

    @Test
    fun launcherIcon() = both("launcher-icon") { dark ->
        Surface(color = MaterialTheme.colorScheme.background, modifier = Modifier.fillMaxSize()) {
            Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
                Text("Adaptive icon: circle, rounded square, themed (monochrome)", style = MaterialTheme.typography.bodyMedium)
                Row(horizontalArrangement = Arrangement.spacedBy(24.dp), verticalAlignment = Alignment.CenterVertically) {
                    AdaptiveIcon(CircleShape)
                    AdaptiveIcon(RoundedCornerShape(22.dp))
                    Box(
                        Modifier
                            .size(72.dp)
                            .clip(CircleShape)
                            .background(MaterialTheme.colorScheme.secondaryContainer),
                        contentAlignment = Alignment.Center,
                    ) {
                        Image(
                            painterResource(R.drawable.ic_launcher_monochrome),
                            contentDescription = null,
                            colorFilter = ColorFilter.tint(MaterialTheme.colorScheme.onSecondaryContainer),
                            modifier = Modifier.requiredSize(108.dp),
                        )
                    }
                }
                Text("In the app: tile and slice", style = MaterialTheme.typography.bodyMedium)
                Row(horizontalArrangement = Arrangement.spacedBy(24.dp), verticalAlignment = Alignment.CenterVertically) {
                    io.github.digitalpals.kiwi.ui.components.KiwiAppIcon(size = 68.dp)
                    Image(painterResource(R.drawable.kiwi_slice), contentDescription = null, modifier = Modifier.size(96.dp))
                    Image(
                        painterResource(R.drawable.ic_stat_kiwi),
                        contentDescription = null,
                        colorFilter = ColorFilter.tint(MaterialTheme.colorScheme.onSurface),
                        modifier = Modifier.size(24.dp),
                    )
                }
                Text(if (dark) "Dark" else "Light", style = MaterialTheme.typography.labelSmall)
            }
        }
    }

    /** The launcher's view of an adaptive icon: the middle 72 dp of the 108 dp layers, masked. */
    @Composable
    private fun AdaptiveIcon(shape: androidx.compose.ui.graphics.Shape) {
        Box(Modifier.size(72.dp).clip(shape), contentAlignment = Alignment.Center) {
            Image(painterResource(R.drawable.ic_launcher_background), contentDescription = null, modifier = Modifier.requiredSize(108.dp))
            Image(painterResource(R.drawable.ic_launcher_foreground), contentDescription = null, modifier = Modifier.requiredSize(108.dp))
        }
    }
}
