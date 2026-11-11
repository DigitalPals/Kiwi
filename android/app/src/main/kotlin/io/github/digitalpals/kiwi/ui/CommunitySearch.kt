package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunitySearchPage
import io.github.digitalpals.kiwi.api.KiwiApi
import io.github.digitalpals.kiwi.api.userMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * Searching the community catalogue (`/api/community/search`): the query is sent [debounceMs]
 * after the last keystroke (a newer query cancels an older request), and results come in pages
 * of [pageSize] as the list is scrolled ([loadMore]). An empty query lists every app by installs.
 */
class CommunitySearch(
    private val scope: CoroutineScope,
    private val fetch: suspend (query: String, offset: Int, limit: Int) -> CommunitySearchPage,
    private val pageSize: Int = KiwiApi.COMMUNITY_PAGE,
    private val debounceMs: Long = DEBOUNCE_MS,
) {
    data class State(
        val query: String = "",
        /** The matches loaded so far (for [query] once [loaded]; the previous query's while it loads). */
        val results: List<CommunityApp> = emptyList(),
        /** How many apps match in all. */
        val total: Int = 0,
        /** The first page is on its way. */
        val loading: Boolean = false,
        /** A first page of [query] arrived. */
        val loaded: Boolean = false,
        /** The first page failed. */
        val error: String? = null,
        val loadingMore: Boolean = false,
        /** The next page failed (the results so far stay). */
        val moreError: String? = null,
    ) {
        val canLoadMore: Boolean get() = loaded && !loading && results.size < total && !loadingMore && moreError == null
        val empty: Boolean get() = loaded && !loading && error == null && results.isEmpty()
    }

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    private var first: Job? = null
    private var more: Job? = null

    /** Loads the first page of the current query when nothing is loaded or loading (opening the search). */
    fun open() {
        val now = _state.value
        if (now.loaded || first?.isActive == true) return
        search(now.query, wait = false)
    }

    fun setQuery(query: String) {
        if (query == _state.value.query) return
        _state.update { it.copy(query = query) }
        search(query, wait = true)
    }

    /** Loads the first page again after it failed, or the next one after that failed. */
    fun retry() {
        val now = _state.value
        if (now.moreError != null) {
            _state.update { it.copy(moreError = null) }
            loadMore()
        } else {
            search(now.query, wait = false)
        }
    }

    /** The next page, when there is one and nothing is loading. */
    fun loadMore() {
        val now = _state.value
        if (!now.canLoadMore || first?.isActive == true) return
        val query = now.query
        val offset = now.results.size
        _state.update { it.copy(loadingMore = true) }
        more = scope.launch {
            val page = try {
                fetch(query, offset, pageSize)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                _state.update { if (it.query != query) it else it.copy(loadingMore = false, moreError = error.userMessage()) }
                return@launch
            }
            _state.update {
                if (it.query != query) return@update it
                val results = (it.results + page.results).distinctBy { app -> app.id }
                // An empty page ends the list, whatever the total says.
                val total = if (page.results.isEmpty()) results.size else page.total.coerceAtLeast(results.size)
                it.copy(results = results, total = total, loadingMore = false)
            }
        }
    }

    private fun search(query: String, wait: Boolean) {
        first?.cancel()
        more?.cancel()
        _state.update { it.copy(loadingMore = false, moreError = null) }
        first = scope.launch {
            if (wait) delay(debounceMs)
            _state.update { it.copy(loading = true, error = null) }
            val page = try {
                fetch(query, 0, pageSize)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                _state.update { if (it.query != query) it else it.copy(loading = false, loaded = false, error = error.userMessage()) }
                return@launch
            }
            _state.update {
                if (it.query != query) return@update it
                val results = page.results.distinctBy { app -> app.id }
                it.copy(
                    results = results,
                    total = if (page.results.isEmpty()) 0 else page.total.coerceAtLeast(results.size),
                    loading = false,
                    loaded = true,
                    error = null,
                )
            }
        }
    }

    companion object {
        const val DEBOUNCE_MS = 300L
    }
}
