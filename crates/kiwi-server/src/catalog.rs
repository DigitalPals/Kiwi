//! In-memory index of the sources' catalogs, mirrored from the `sources` table.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use kiwi_source::App;
use serde::Serialize;

use crate::db::{AppPrefs, BrowseMode, SourceRow, Time};

#[derive(Default)]
pub struct Catalog {
    state: RwLock<CatalogState>,
}

/// The lookup indexes and Browse order are rebuilt only when sources change. App refs
/// retain the same source snapshot as the indexes, including during a concurrent refresh.
#[derive(Default)]
struct CatalogState {
    sources: Vec<Arc<SourceRow>>,
    source_indices: HashMap<String, usize>,
    app_indices: Vec<HashMap<String, usize>>,
    ordered_apps: Vec<(usize, usize)>,
}

impl CatalogState {
    fn reindex(&mut self) {
        self.sources.sort_by_key(|r| (r.position, r.added_at));
        self.source_indices.clear();
        self.app_indices.clear();
        self.ordered_apps.clear();
        for (source_index, source) in self.sources.iter().enumerate() {
            self.source_indices.insert(source.id.clone(), source_index);
            let mut apps = HashMap::new();
            for (app_index, app) in source.source.catalog.apps.iter().enumerate() {
                // Preserve the first-match behavior even for duplicate bundle ids.
                apps.entry(app.bundle_id.clone()).or_insert(app_index);
                if source.enabled {
                    self.ordered_apps.push((source_index, app_index));
                }
            }
            self.app_indices.push(apps);
        }
        self.ordered_apps.sort_by_cached_key(|&(source, app)| {
            let row = &self.sources[source];
            (
                row.source.catalog.apps[app].name.to_lowercase(),
                row.position,
            )
        });
    }
}

/// `Source` in api.md.
#[derive(Debug, Serialize)]
pub struct SourceJson {
    pub id: String,
    /// `feed`, `ipa` (an IPA link or download page) or `local_build`.
    pub kind: &'static str,
    pub url: String,
    /// The IPA an `ipa` source's app comes from now.
    pub ipa_url: Option<String>,
    pub name: String,
    pub subtitle: Option<String>,
    pub icon_url: Option<String>,
    pub enabled: bool,
    pub app_count: usize,
    pub browse: BrowseMode,
    /// Apps Browse shows.
    pub shown_count: usize,
    /// Apps that appeared since the source was switched to picking apps, not yet chosen.
    pub new_count: usize,
    pub added_at: Time,
    pub last_attempt_at: Option<Time>,
    pub last_success_at: Option<Time>,
    pub error: Option<String>,
    pub diagnostics: Vec<String>,
}

impl SourceJson {
    /// `prefs`: the choices made for apps ([`crate::db::Db::app_prefs`]).
    pub fn new(row: &SourceRow, prefs: &HashMap<(String, String), AppPrefs>) -> Self {
        let catalog = &row.source.catalog;
        let choices = || {
            catalog
                .apps
                .iter()
                .map(|a| prefs.get(&(row.id.clone(), a.bundle_id.clone())))
        };
        Self {
            id: row.id.clone(),
            kind: if row.id == crate::automation::SOURCE_ID {
                "local_build"
            } else if row.source.link.is_some() {
                "ipa"
            } else {
                "feed"
            },
            ipa_url: row.source.link.as_ref().map(|l| l.ipa.to_string()),
            url: row.url.clone(),
            name: catalog.name.clone(),
            subtitle: catalog.subtitle.clone(),
            icon_url: catalog
                .icon_url
                .as_ref()
                .map(|u| crate::linked::client_icon(u.as_str())),
            enabled: row.enabled,
            app_count: catalog.apps.len(),
            browse: row.browse,
            shown_count: choices().filter(|p| row.shows(*p)).count(),
            new_count: choices().filter(|p| row.is_new(*p)).count(),
            added_at: row.added_at,
            last_attempt_at: row.last_attempt_at,
            last_success_at: row.source.last_success,
            error: row.source.error.clone(),
            diagnostics: catalog.diagnostics.clone(),
        }
    }
}

/// An app with the source it came from.
#[derive(Clone)]
pub struct CatalogApp {
    pub source: Arc<SourceRow>,
    pub index: usize,
}

impl CatalogApp {
    pub fn app(&self) -> &App {
        &self.source.source.catalog.apps[self.index]
    }
}

impl Catalog {
    pub fn load(&self, rows: Vec<SourceRow>) {
        let mut next = CatalogState {
            sources: rows.into_iter().map(Arc::new).collect(),
            ..Default::default()
        };
        next.reindex();
        *self.state.write().unwrap_or_else(|e| e.into_inner()) = next;
    }

    /// All sources by position.
    pub fn sources(&self) -> Vec<Arc<SourceRow>> {
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .sources
            .clone()
    }

    pub fn source(&self, id: &str) -> Option<Arc<SourceRow>> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        Some(state.sources[*state.source_indices.get(id)?].clone())
    }

    pub fn upsert(&self, row: SourceRow) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        match state.source_indices.get(&row.id).copied() {
            Some(index) => state.sources[index] = Arc::new(row),
            None => state.sources.push(Arc::new(row)),
        }
        state.reindex();
    }

    /// Changes a source in place (atomically with other catalog updates); `None` when it is
    /// not in the catalog.
    pub fn modify(&self, id: &str, f: impl FnOnce(&mut SourceRow)) -> Option<SourceRow> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        let index = *state.source_indices.get(id)?;
        let mut row = (*state.sources[index]).clone();
        f(&mut row);
        state.sources[index] = Arc::new(row.clone());
        state.reindex();
        Some(row)
    }

    pub fn remove(&self, id: &str) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.sources.retain(|s| s.id != id);
        state.reindex();
    }

    pub fn next_position(&self) -> i64 {
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .sources
            .iter()
            .map(|s| s.position + 1)
            .max()
            .unwrap_or(0)
    }

    /// An app of any source (enabled or not).
    pub fn app(&self, source_id: &str, bundle_id: &str) -> Option<CatalogApp> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        let source_index = *state.source_indices.get(source_id)?;
        let index = *state.app_indices[source_index].get(bundle_id)?;
        Some(CatalogApp {
            source: state.sources[source_index].clone(),
            index,
        })
    }

    /// Apps of enabled sources, optionally filtered by source and query, sorted by name.
    pub fn apps(&self, source: Option<&str>, query: Option<&str>) -> Vec<CatalogApp> {
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state
            .ordered_apps
            .iter()
            .filter_map(|&(source_index, index)| {
                let row = &state.sources[source_index];
                if source.is_some_and(|id| id != row.id)
                    || query.is_some_and(|q| !row.source.catalog.apps[index].matches(q))
                {
                    return None;
                }
                Some(CatalogApp {
                    source: row.clone(),
                    index,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{app, source_row};

    fn named(bundle: &str, name: &str) -> App {
        let mut a = app(&[("1", None)]);
        a.bundle_id = bundle.into();
        a.name = name.into();
        a
    }

    fn names(catalog: &Catalog) -> Vec<String> {
        catalog
            .apps(None, None)
            .iter()
            .map(|a| a.app().name.clone())
            .collect()
    }

    #[test]
    fn indexes_follow_all_catalog_changes_and_keep_old_snapshots_valid() {
        let catalog = Catalog::default();
        let mut first = source_row("first", vec![named("z", "Zulu"), named("a", "Alpha")]);
        first.position = 1;
        let second = source_row("second", vec![named("b", "beta")]);
        catalog.load(vec![first, second]);
        assert_eq!(names(&catalog), ["Alpha", "beta", "Zulu"]);
        let old = catalog.app("first", "a").unwrap();
        assert_eq!(catalog.apps(Some("first"), Some("  zUlU  ")).len(), 1);
        assert!(catalog.apps(Some("missing"), None).is_empty());
        assert_eq!(catalog.next_position(), 2);

        catalog.modify("first", |s| s.enabled = false).unwrap();
        assert_eq!(names(&catalog), ["beta"]);
        assert!(!catalog.app("first", "a").unwrap().source.enabled);
        assert!(old.source.enabled);

        catalog.upsert(source_row("first", vec![named("new", "Aardvark")]));
        assert!(catalog.app("first", "a").is_none());
        assert_eq!(catalog.app("first", "new").unwrap().app().name, "Aardvark");
        assert_eq!(old.app().name, "Alpha");
        assert_eq!(names(&catalog), ["Aardvark", "beta"]);
        catalog.modify("first", |s| s.source.catalog.apps[0].name = "Omega".into());
        assert_eq!(names(&catalog), ["beta", "Omega"]);
        catalog.remove("second");
        assert!(catalog.source("second").is_none());
        assert!(catalog.app("second", "b").is_none());
        assert_eq!(names(&catalog), ["Omega"]);
        catalog.load(Vec::new());
        assert!(catalog.apps(None, None).is_empty());
        assert!(catalog.source("first").is_none());
    }

    #[test]
    fn ordering_preserves_position_and_feed_order_for_equal_names() {
        let catalog = Catalog::default();
        let first = source_row("first", vec![named("a", "same"), named("b", "Same")]);
        let mut second = source_row("second", vec![named("c", "SAME")]);
        second.position = 1;
        catalog.load(vec![second, first]);
        let ids: Vec<_> = catalog
            .apps(None, None)
            .iter()
            .map(|a| a.app().bundle_id.clone())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }
}
