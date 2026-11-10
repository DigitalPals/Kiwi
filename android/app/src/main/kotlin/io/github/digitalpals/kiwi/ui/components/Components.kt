package io.github.digitalpals.kiwi.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import io.github.digitalpals.kiwi.ui.ButtonView
import io.github.digitalpals.kiwi.ui.Tint

/** An app's icon on its tint, with the first letter while the icon loads (or without one). */
@Composable
fun AppIcon(name: String, iconUrl: String?, tint: String?, size: Dp = 48.dp) {
    val shape = RoundedCornerShape(size * 0.22f)
    Box(
        modifier = Modifier
            .size(size)
            .clip(shape)
            .background(Color(Tint.argb(Tint.appColor(name, tint)))),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = name.take(1).uppercase(),
            color = Color.White,
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.SemiBold,
        )
        if (iconUrl != null) {
            AsyncImage(model = iconUrl, contentDescription = null, modifier = Modifier.fillMaxSize())
        }
    }
}

/** What the app's button does when tapped. */
class ButtonActions(
    val install: () -> Unit,
    val open: (String) -> Unit,
    val remove: (String) -> Unit,
    val allowInstalls: () -> Unit,
    /** Stops this app's own install (an unanswered confirmation, a long download). */
    val cancel: () -> Unit,
)

/** An app's main button with its caption (an error, a version change, a reason). */
@Composable
fun ActionButton(view: ButtonView, actions: ButtonActions, modifier: Modifier = Modifier) {
    Column(modifier = modifier, horizontalAlignment = Alignment.End) {
        when (view) {
            is ButtonView.Install -> {
                Button(onClick = actions.install) { Text(view.label) }
                Caption(view.caption, error = view.error)
            }
            is ButtonView.Open -> FilledTonalButton(onClick = { actions.open(view.packageName) }) { Text("Open") }
            is ButtonView.Remove -> {
                OutlinedButton(onClick = { actions.remove(view.packageName) }) { Text("Remove Other") }
                Caption(view.caption, error = true)
            }
            is ButtonView.AllowInstalls -> {
                Button(onClick = actions.allowInstalls) { Text("Allow") }
                Caption(view.caption, error = false)
            }
            is ButtonView.Progress -> Column(horizontalAlignment = Alignment.End, modifier = Modifier.widthIn(min = 96.dp)) {
                if (view.fraction != null) {
                    LinearProgressIndicator(progress = { view.fraction }, modifier = Modifier.width(96.dp))
                } else {
                    LinearProgressIndicator(modifier = Modifier.width(96.dp))
                }
                Spacer(Modifier.height(4.dp))
                Text(view.label, style = MaterialTheme.typography.labelMedium)
                if (view.cancellable) {
                    TextButton(onClick = actions.cancel) { Text("Cancel") }
                }
            }
            is ButtonView.Disabled -> {
                OutlinedButton(onClick = {}, enabled = false) { Text(view.label) }
                Caption(view.caption, error = false)
            }
        }
    }
}

@Composable
private fun Caption(text: String?, error: Boolean) {
    if (text.isNullOrBlank()) return
    Text(
        text = text,
        style = MaterialTheme.typography.bodySmall,
        color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.End,
        modifier = Modifier.widthIn(max = 200.dp).padding(top = 2.dp),
    )
}

/** A row in an app list: icon, name and subtitle, and the action button. */
@Composable
fun AppRow(
    name: String,
    subtitle: String?,
    iconUrl: String?,
    tint: String?,
    view: ButtonView,
    actions: ButtonActions,
    onClick: () -> Unit,
    warning: String? = null,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AppIcon(name, iconUrl, tint)
        Column(
            modifier = Modifier
                .weight(1f)
                .padding(horizontal = 12.dp),
        ) {
            Text(name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (!subtitle.isNullOrBlank()) {
                Text(
                    subtitle,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (!warning.isNullOrBlank()) {
                Text(warning, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
        }
        ActionButton(view, actions)
    }
}

/** A whole-screen message: loading failed, nothing to show, or this device isn't allowed. */
@Composable
fun MessagePane(
    title: String,
    message: String?,
    modifier: Modifier = Modifier,
    primary: Pair<String, () -> Unit>? = null,
    secondary: Pair<String, () -> Unit>? = null,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
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
            Button(onClick = primary.second) { Text(primary.first) }
        }
        if (secondary != null) {
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

/** A section title in a list. */
@Composable
fun SectionHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 4.dp),
    )
}
