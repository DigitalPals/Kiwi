package io.github.digitalpals.kiwi.ui.screens

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.digitalpals.kiwi.AppContainer
import io.github.digitalpals.kiwi.api.AppSummary
import io.github.digitalpals.kiwi.api.title
import io.github.digitalpals.kiwi.api.userMessage
import io.github.digitalpals.kiwi.api.KiwiException
import io.github.digitalpals.kiwi.install.InstallJob
import io.github.digitalpals.kiwi.ui.AppItem
import io.github.digitalpals.kiwi.ui.BrowseViewModel
import io.github.digitalpals.kiwi.ui.Load
import io.github.digitalpals.kiwi.ui.SystemActions
import io.github.digitalpals.kiwi.ui.appButton
import io.github.digitalpals.kiwi.ui.components.AppRow
import io.github.digitalpals.kiwi.ui.components.Loading
import io.github.digitalpals.kiwi.ui.components.MessagePane

/** Browse: the apps Kiwi offers this phone (`GET /api/apps`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowseScreen(container: AppContainer, model: BrowseViewModel, onOpenApp: (AppSummary) -> Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val jobs by container.installs.jobs.collectAsStateWithLifecycle()
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            value = query,
            onValueChange = { model.query.value = it },
            leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
            placeholder = { Text("Search") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { model.reload() }),
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp),
        )
        when (val load = state) {
            Load.Loading -> Loading()
            is Load.Failed -> LoadFailed(load.error, onRetry = model::reload)
            is Load.Ready -> PullToRefreshBox(isRefreshing = load.refreshing, onRefresh = model::reload) {
                if (load.value.isEmpty()) {
                    MessagePane("No Apps", "Kiwi has no apps for this phone yet. Turn on Kiwi Builds on your Kiwi to add Android apps.")
                } else {
                    AppList(container, load.value, jobs, onOpenApp)
                }
            }
        }
    }
}

@Composable
fun AppList(container: AppContainer, items: List<AppItem>, jobs: Map<String, InstallJob>, onOpenApp: (AppSummary) -> Unit) {
    val context = LocalContext.current
    LazyColumn(Modifier.fillMaxSize()) {
        items(items, key = { it.app.libraryKey }) { item ->
            AppListRow(container, item, jobs[item.app.key], onOpenApp, context)
        }
    }
}

@Composable
fun AppListRow(
    container: AppContainer,
    item: AppItem,
    job: InstallJob?,
    onOpenApp: (AppSummary) -> Unit,
    context: android.content.Context = LocalContext.current,
) {
    val app = item.app
    AppRow(
        name = app.name,
        subtitle = app.subtitle ?: app.developer ?: app.sourceName,
        iconUrl = app.iconUrl?.let(container::resolveUrl),
        tint = app.tintColor,
        view = appButton(app, job, item.local),
        actions = SystemActions.buttonActions(context, container, app),
        onClick = { onOpenApp(app) },
    )
}

/** A screen's data could not be loaded. */
@Composable
fun LoadFailed(error: Throwable, onRetry: () -> Unit) {
    val title = (error as? KiwiException)?.title() ?: "Something Went Wrong"
    MessagePane(title, error.userMessage(), primary = "Try Again" to onRetry)
}
