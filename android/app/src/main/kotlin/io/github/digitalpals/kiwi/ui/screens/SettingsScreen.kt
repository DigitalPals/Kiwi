package io.github.digitalpals.kiwi.ui.screens

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
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
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.BuildConfig
import io.github.digitalpals.kiwi.R
import io.github.digitalpals.kiwi.api.AndroidApp
import io.github.digitalpals.kiwi.api.Me
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.SELF_UPDATE_KEY
import io.github.digitalpals.kiwi.ui.SessionViewModel
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.components.ActionButton
import io.github.digitalpals.kiwi.ui.components.ButtonActions
import io.github.digitalpals.kiwi.ui.components.GroupRow
import io.github.digitalpals.kiwi.ui.components.IconCircle
import io.github.digitalpals.kiwi.ui.components.KiwiAppIcon
import io.github.digitalpals.kiwi.ui.components.KiwiSwitch
import io.github.digitalpals.kiwi.ui.components.KiwiTopBar
import io.github.digitalpals.kiwi.ui.components.ListGroup
import io.github.digitalpals.kiwi.ui.components.SectionLabel
import io.github.digitalpals.kiwi.ui.components.TonalAction
import io.github.digitalpals.kiwi.ui.components.glow
import io.github.digitalpals.kiwi.ui.preview.PreviewData
import io.github.digitalpals.kiwi.ui.selfUpdateButton
import io.github.digitalpals.kiwi.ui.selfUpdateOffer
import io.github.digitalpals.kiwi.ui.theme.KiwiIcons
import io.github.digitalpals.kiwi.ui.theme.KiwiTheme
import io.github.digitalpals.kiwi.ui.theme.kiwi

/** What Settings shows about this phone and its permissions. */
data class PhoneState(
    /** "Google Pixel 9a". */
    val name: String,
    /** "Android 16". */
    val system: String,
    val autoUpdate: Boolean,
    val notifications: Boolean,
    val canInstall: Boolean,
)

/** What Settings can do. */
class SettingsActions(
    val autoUpdate: (Boolean) -> Unit = {},
    val notifications: () -> Unit = {},
    val allowInstalls: () -> Unit = {},
    val selfUpdate: ButtonActions = ButtonActions.None,
    val showSetup: () -> Unit = {},
    val disconnect: () -> Unit = {},
)

/** Settings: the server, this phone, automatic updates, notifications and the Kiwi app itself. */
@Composable
fun SettingsScreen(container: AppContainer, session: SessionViewModel, me: Me, serverUrl: String, onShowSetup: () -> Unit) {
    val context = LocalContext.current
    val autoUpdate by container.settings.autoUpdate.collectAsStateWithLifecycle(initialValue = true)
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    // Re-read the permissions whenever the screen comes back from Android's settings.
    var resumed by remember { mutableIntStateOf(0) }
    LifecycleResumeEffect(Unit) {
        resumed++
        onPauseOrDispose {}
    }
    val canInstall = remember(resumed) { container.sessionInstaller.canInstall() }
    var notifications by remember(resumed) { mutableStateOf(notificationsGranted(context)) }
    val requestNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        notifications = granted
        // Android doesn't ask again once the permission was refused: its settings can still allow it.
        if (!granted) SystemActions.notificationSettings(context)
    }
    val offer = selfUpdateOffer(me)
    SettingsContent(
        me = me,
        serverUrl = serverUrl,
        phone = PhoneState(
            name = phoneName(),
            system = "Android ${Build.VERSION.RELEASE}",
            autoUpdate = autoUpdate,
            notifications = notifications,
            canInstall = canInstall,
        ),
        selfOffer = offer,
        selfJob = jobs[SELF_UPDATE_KEY],
        actions = SettingsActions(
            autoUpdate = { session.setAutoUpdate(it) },
            notifications = {
                if (!notifications && Build.VERSION.SDK_INT >= 33) {
                    requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
                } else {
                    SystemActions.notificationSettings(context)
                }
            },
            allowInstalls = { SystemActions.allowInstalls(context) },
            selfUpdate = ButtonActions(
                install = { offer?.let(container.installs::startSelfUpdate) },
                open = {},
                remove = {},
                allowInstalls = {
                    container.installs.clear(SELF_UPDATE_KEY)
                    SystemActions.allowInstalls(context)
                },
                cancel = { container.installs.cancel(SELF_UPDATE_KEY) },
            ),
            showSetup = onShowSetup,
            disconnect = { session.disconnect() },
        ),
    )
}

/** "Google Pixel 9a": the maker (unless the model already names it) and the model. */
private fun phoneName(): String {
    val maker = Build.MANUFACTURER.orEmpty().replaceFirstChar { it.uppercase() }
    val model = Build.MODEL.orEmpty()
    return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
}

private fun notificationsGranted(context: Context): Boolean =
    Build.VERSION.SDK_INT < 33 ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

/** C5: the server card, this phone, and the Kiwi app. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsContent(
    me: Me,
    serverUrl: String,
    phone: PhoneState,
    selfOffer: AndroidApp?,
    selfJob: InstallJob?,
    actions: SettingsActions,
) {
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    var confirmDisconnect by remember { mutableStateOf(false) }
    Scaffold(
        topBar = { KiwiTopBar("Settings", scroll) },
        contentWindowInsets = WindowInsets(0),
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .nestedScroll(scroll.nestedScrollConnection)
                .verticalScroll(rememberScrollState())
                .padding(bottom = 24.dp),
        ) {
            ServerCard(
                host = serverUrl.removePrefix("https://").removeSuffix("/"),
                detail = serverLine(me),
            )
            SectionLabel("This phone")
            ListGroup {
                row {
                    GroupRow(
                        phone.name,
                        subtitle = AnnotatedString(phone.system),
                        leading = { IconCircle(KiwiIcons.Phone, Color(0xFF2F7CF6)) },
                    )
                }
                row {
                    GroupRow(
                        "Install updates automatically",
                        subtitle = AnnotatedString("Apps Kiwi installed · on Wi-Fi, battery not low"),
                        leading = { IconCircle(KiwiIcons.Refresh, Color(0xFF1E8E9E)) },
                        trailing = { KiwiSwitch(phone.autoUpdate, actions.autoUpdate) },
                        onClick = { actions.autoUpdate(!phone.autoUpdate) },
                    )
                }
                row {
                    GroupRow(
                        "Notifications",
                        subtitle = AnnotatedString(if (phone.notifications) "Updates and finished builds" else "Off · tap to allow them"),
                        leading = { IconCircle(KiwiIcons.Bell, Color(0xFFE5483C)) },
                        trailing = { Icon(KiwiIcons.Chevron, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                        onClick = actions.notifications,
                    )
                }
                if (!phone.canInstall) {
                    row {
                        GroupRow(
                            "Install apps",
                            subtitle = AnnotatedString("Allow Kiwi to install and update apps"),
                            leading = { IconCircle(KiwiIcons.Download, Color(0xFFC27C0E)) },
                            trailing = { TonalAction("Allow", actions.allowInstalls) },
                            onClick = actions.allowInstalls,
                        )
                    }
                }
            }
            SectionLabel("Kiwi app")
            ListGroup {
                row { KiwiAppRow(selfOffer, selfJob, actions.selfUpdate) }
                row { GroupRow("Show setup again", onClick = actions.showSetup) }
                row {
                    GroupRow(
                        "Disconnect",
                        subtitle = AnnotatedString("Your apps stay installed"),
                        titleColor = MaterialTheme.colorScheme.error,
                        onClick = { confirmDisconnect = true },
                    )
                }
            }
            Text(
                "Kiwi for Android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 20.dp),
            )
        }
    }
    if (confirmDisconnect) {
        AlertDialog(
            onDismissRequest = { confirmDisconnect = false },
            title = { Text("Disconnect from your Kiwi?") },
            text = { Text("Your apps stay installed. Connect again to get their updates.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDisconnect = false
                    actions.disconnect()
                }) { Text("Disconnect") }
            },
            dismissButton = { TextButton(onClick = { confirmDisconnect = false }) { Text("Cancel") } },
        )
    }
}

/** "Kiwi 0.3.0 · on a Mac": the server's version and where it runs. */
private fun serverLine(me: Me): String {
    val host = when (me.host?.os) {
        "macos" -> " · on a Mac"
        "linux" -> " · on Linux"
        else -> ""
    }
    return "Kiwi ${me.server.version}$host"
}

/** The server, on Kiwi's dark green with the big kiwi slice. */
@Composable
private fun ServerCard(host: String, detail: String) {
    Box(
        Modifier
            .padding(start = 12.dp, end = 12.dp, top = 4.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(26.dp))
            .drawBehind {
                drawRect(
                    Brush.linearGradient(
                        0f to Color(0xFF14523A),
                        0.55f to Color(0xFF0E3A2B),
                        1f to Color(0xFF173D21),
                        start = Offset.Zero,
                        end = Offset(size.width, size.height),
                    ),
                )
                drawRect(
                    Brush.radialGradient(
                        0f to Color(0xFF3F8F3A),
                        0.6f to Color.Transparent,
                        center = Offset(size.width, size.height / 2),
                        radius = size.width * 0.9f,
                    ),
                )
            },
    ) {
        Image(
            painterResource(R.drawable.kiwi_slice),
            contentDescription = null,
            modifier = Modifier
                .align(Alignment.CenterEnd)
                .offset(x = 30.dp)
                .size(124.dp)
                .glow(Color.Black.copy(alpha = 0.4f), 18.dp, 10.dp, 62.dp),
        )
        Column(
            Modifier
                .padding(18.dp)
                .fillMaxWidth(0.72f),
        ) {
            Text(host, color = Color.White, fontSize = 20.sp, lineHeight = 26.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(detail, color = Color.White.copy(alpha = 0.78f), fontSize = 13.5.sp, lineHeight = 19.sp, modifier = Modifier.padding(top = 4.dp))
            Row(
                Modifier
                    .padding(top = 10.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .background(Color.White.copy(alpha = 0.15f))
                    .padding(start = 10.dp, end = 12.dp, top = 5.dp, bottom = 5.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(7.dp),
            ) {
                Box(
                    Modifier
                        .size(8.dp)
                        .clip(CircleShape)
                        .background(MaterialTheme.kiwi.success),
                )
                Text("Connected", color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}

/** The Kiwi app's own version and its update. */
@Composable
private fun KiwiAppRow(offer: AndroidApp?, job: InstallJob?, actions: ButtonActions) {
    val primary = MaterialTheme.colorScheme.primary
    val subtitle = if (offer != null) {
        buildAnnotatedString {
            withStyle(SpanStyle(color = primary, fontWeight = FontWeight.Medium)) {
                append("${BuildConfig.VERSION_NAME} · Update available")
            }
        }
    } else {
        AnnotatedString("${BuildConfig.VERSION_NAME} · Up to date")
    }
    GroupRow(
        "Kiwi app",
        subtitle = subtitle,
        leading = { KiwiAppIcon(size = 40.dp) },
        trailing = if (offer != null) {
            {
                // As in the mockup, Settings offers the update with a tonal button; My Apps has the filled one.
                when (val view = selfUpdateButton(job)) {
                    is ButtonView.Install -> TonalAction(view.label, actions.install)
                    else -> ActionButton(view, actions)
                }
            }
        } else {
            null
        },
    )
}

@Preview(name = "C5 Settings")
@Composable
private fun SettingsPreview() {
    KiwiTheme(darkTheme = false) {
        SettingsContent(
            PreviewData.me,
            PreviewData.SERVER,
            PhoneState("Google Pixel 9a", "Android 16", autoUpdate = true, notifications = true, canInstall = true),
            PreviewData.kiwiOffer,
            null,
            SettingsActions(),
        )
    }
}

@Preview(name = "C5 Settings, dark")
@Composable
private fun SettingsDarkPreview() {
    KiwiTheme(darkTheme = true) {
        SettingsContent(
            PreviewData.me,
            PreviewData.SERVER,
            PhoneState("Google Pixel 9a", "Android 16", autoUpdate = true, notifications = false, canInstall = false),
            PreviewData.kiwiOffer,
            null,
            SettingsActions(),
        )
    }
}
