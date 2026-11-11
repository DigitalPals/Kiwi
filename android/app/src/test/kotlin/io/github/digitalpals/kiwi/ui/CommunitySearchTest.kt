package io.github.digitalpals.kiwi.ui

import io.github.digitalpals.kiwi.api.CommunityApp
import io.github.digitalpals.kiwi.api.CommunitySearchPage
import io.github.digitalpals.kiwi.api.KiwiException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class CommunitySearchTest {
    /** A fake catalogue of [count] apps whose names contain the query. */
    private class Catalogue(val count: Int = 70) {
        val requests = mutableListOf<Triple<String, Int, Int>>()
        var failing: Exception? = null
        var gate: CompletableDeferred<Unit>? = null

        suspend fun fetch(query: String, offset: Int, limit: Int): CommunitySearchPage {
            requests += Triple(query, offset, limit)
            gate?.await()
            failing?.let { throw it }
            val all = (0 until count).map { CommunityApp("community-app$it", "app$it", "$query app $it") }
            return CommunitySearchPage(all.drop(offset).take(limit), count)
        }
    }

    private fun TestScope.search(catalogue: Catalogue) =
        CommunitySearch(this, catalogue::fetch, pageSize = 30, debounceMs = 300)

    @Test
    fun openingListsEveryAppAtOnceThenPagesThroughThem() = runTest {
        val catalogue = Catalogue()
        val search = search(catalogue)
        search.open()
        runCurrent()
        assertEquals(listOf(Triple("", 0, 30)), catalogue.requests)
        var state = search.state.value
        assertTrue(state.loaded)
        assertEquals(30, state.results.size)
        assertEquals(70, state.total)
        assertTrue(state.canLoadMore)
        // Opening again keeps what is loaded.
        search.open()
        runCurrent()
        assertEquals(1, catalogue.requests.size)

        search.loadMore()
        search.loadMore() // ignored: a page is on its way
        advanceUntilIdle()
        search.loadMore()
        advanceUntilIdle()
        state = search.state.value
        assertEquals(listOf(0, 30, 60), catalogue.requests.map { it.second })
        assertEquals(70, state.results.size)
        assertFalse(state.canLoadMore)
        search.loadMore()
        advanceUntilIdle()
        assertEquals(3, catalogue.requests.size)
    }

    @Test
    fun typingIsDebouncedAndANewerQueryWins() = runTest {
        val catalogue = Catalogue()
        val search = search(catalogue)
        search.setQuery("d")
        advanceTimeBy(100)
        search.setQuery("di")
        advanceTimeBy(100)
        search.setQuery("dis")
        assertEquals("dis", search.state.value.query)
        advanceTimeBy(299)
        runCurrent()
        assertTrue(catalogue.requests.isEmpty())
        advanceTimeBy(2)
        runCurrent()
        assertEquals(listOf("dis"), catalogue.requests.map { it.first })
        assertEquals("dis app 0", search.state.value.results.first().name)

        // A request still on its way is dropped when the query changes.
        catalogue.gate = CompletableDeferred()
        search.setQuery("disc")
        advanceTimeBy(301)
        runCurrent()
        assertTrue(search.state.value.loading)
        search.setQuery("disco")
        catalogue.gate = null
        advanceUntilIdle()
        assertEquals(listOf("dis", "disc", "disco"), catalogue.requests.map { it.first })
        assertEquals("disco app 0", search.state.value.results.first().name)
        assertFalse(search.state.value.loading)
        // Setting the same query again asks nothing.
        search.setQuery("disco")
        advanceUntilIdle()
        assertEquals(3, catalogue.requests.size)
    }

    @Test
    fun anEmptyResultAndErrorsCanBeRetried() = runTest {
        val catalogue = Catalogue(count = 0)
        val search = search(catalogue)
        search.open()
        advanceUntilIdle()
        assertTrue(search.state.value.empty)
        assertFalse(search.state.value.canLoadMore)

        val failing = Catalogue()
        failing.failing = KiwiException.BadResponse("Kiwi sent an answer this app doesn't understand.")
        val other = search(failing)
        other.setQuery("x")
        advanceUntilIdle()
        assertEquals("Kiwi sent an answer this app doesn't understand.", other.state.value.error)
        assertFalse(other.state.value.empty)
        failing.failing = null
        other.retry()
        advanceUntilIdle()
        assertNull(other.state.value.error)
        assertEquals(30, other.state.value.results.size)

        // A failed next page keeps the results; Try again loads it.
        failing.failing = KiwiException.BadResponse("Not now.")
        other.loadMore()
        advanceUntilIdle()
        assertEquals("Not now.", other.state.value.moreError)
        assertEquals(30, other.state.value.results.size)
        assertFalse(other.state.value.canLoadMore)
        failing.failing = null
        other.retry()
        advanceUntilIdle()
        assertNull(other.state.value.moreError)
        assertEquals(60, other.state.value.results.size)
    }

    @Test
    fun anEmptyPageEndsTheListAndDuplicatesAreDropped() = runTest {
        var calls = 0
        val search = CommunitySearch(this, { _, offset, _ ->
            calls++
            if (offset == 0) {
                CommunitySearchPage(listOf(CommunityApp("community-a", name = "A"), CommunityApp("community-a", name = "A")), total = 50)
            } else {
                CommunitySearchPage(emptyList(), total = 50)
            }
        })
        search.open()
        advanceUntilIdle()
        assertEquals(1, search.state.value.results.size)
        search.loadMore()
        advanceUntilIdle()
        assertEquals(1, search.state.value.total)
        assertFalse(search.state.value.canLoadMore)
        assertEquals(2, calls)
    }
}
