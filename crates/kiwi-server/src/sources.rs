//! Adding, refreshing, enabling and removing sources (database and in-memory catalog).

use std::sync::Arc;

use chrono::Utc;

use crate::{
    Kiwi,
    db::{BrowseMode, SourceRow},
    error::{ApiError, ApiResult},
};

pub async fn add(kiwi: &Kiwi, input: &str) -> ApiResult<SourceRow> {
    let input = input.trim();
    if input.is_empty() || input.len() > 2048 {
        return Err(ApiError::bad_request("Enter the source's URL."));
    }
    let cancel = kiwi.cancel.child_token();
    let url = kiwi
        .sources
        .canonical_url(input, &cancel)
        .await
        .map_err(ApiError::source)?;
    let conflict = || ApiError::conflict("This source is already added.");
    if kiwi
        .catalog
        .source(&kiwi_source::source_key(&url))
        .is_some()
    {
        return Err(conflict());
    }
    let source = match kiwi
        .sources
        .find(url, &cancel)
        .await
        .map_err(ApiError::source)?
    {
        kiwi_source::Found::Source(source) => source,
        kiwi_source::Found::Ipa { ipa, page } => {
            // Checked before the download, which can take minutes.
            if kiwi
                .catalog
                .source(&crate::linked::key(&ipa, page.as_ref()))
                .is_some()
            {
                return Err(conflict());
            }
            crate::linked::build(kiwi, ipa, page.as_ref(), None, &cancel)
                .await
                .map_err(ApiError::source)?
        }
    };
    if kiwi.catalog.source(&source.key).is_some() {
        return Err(conflict());
    }
    let now = Utc::now();
    let row = SourceRow {
        id: source.key.clone(),
        url: source.url.to_string(),
        enabled: true,
        position: kiwi.catalog.next_position(),
        added_at: now,
        last_attempt_at: Some(now),
        browse: BrowseMode::All,
        picked_at: None,
        source,
    };
    kiwi.db.save_source(&row)?;
    record_apps(kiwi, &row);
    kiwi.catalog.upsert(row.clone());
    tracing::info!(source = %row.url, apps = row.source.catalog.apps.len(), "source added");
    kiwi.kick.notify_one();
    Ok(row)
}

/// Conditional refetch. A failure keeps the previous catalog and records the error. Only
/// the fetch results are written, so a source disabled during the fetch stays disabled.
pub async fn refresh(kiwi: &Kiwi, id: &str) -> ApiResult<SourceRow> {
    let row = kiwi
        .catalog
        .source(id)
        .ok_or_else(|| ApiError::not_found("Source not found."))?;
    if id == crate::automation::SOURCE_ID {
        return Ok((*row).clone());
    }
    let not_found = || ApiError::not_found("Source not found.");
    // Recorded first, so a fetch that crashes the server is not repeated right after the
    // restart.
    let started = Utc::now();
    if !kiwi.db.set_source_attempt(id, started)? {
        kiwi.catalog.remove(id);
        return Err(not_found());
    }
    kiwi.catalog
        .modify(id, |current| current.last_attempt_at = Some(started));
    let result = if row.source.link.is_some() {
        crate::linked::refresh(kiwi, &row.source, &kiwi.cancel).await
    } else {
        kiwi.sources.refresh(&row.source, &kiwi.cancel).await
    };
    let now = Utc::now();
    // The source may have been changed or removed meanwhile: apply the fetch to its
    // current state.
    let next = kiwi
        .catalog
        .modify(id, |current| {
            current.last_attempt_at = Some(now);
            match result {
                Ok(source) => current.source = source,
                Err(e) => {
                    tracing::warn!(source = %current.url, "refresh failed: {e}");
                    current.source.error = Some(e.to_string());
                }
            }
            current.source.enabled = current.enabled;
        })
        .ok_or_else(not_found)?;
    if !kiwi.db.save_source_fetch(id, now, &next.source)? {
        kiwi.catalog.remove(id);
        return Err(not_found());
    }
    record_apps(kiwi, &next);
    Ok(next)
}

/// Records when the source's apps were first seen, so apps added later can be told apart
/// (see [`BrowseMode::Picked`]). Failures only cost that distinction.
pub fn record_apps(kiwi: &Kiwi, row: &SourceRow) {
    let apps = &row.source.catalog.apps;
    if let Err(e) = kiwi.db.record_apps(
        &row.id,
        apps.iter().map(|a| a.bundle_id.as_str()),
        Utc::now(),
    ) {
        tracing::warn!(source = %row.url, "could not record the source's apps: {e}");
    }
}

/// Refreshes the given sources concurrently (kiwi-source limits parallel requests).
pub async fn refresh_many(kiwi: &Arc<Kiwi>, ids: Vec<String>) {
    let tasks = ids.into_iter().map(|id| {
        let kiwi = kiwi.clone();
        async move {
            if let Err(e) = refresh(&kiwi, &id).await {
                tracing::warn!("refresh of {id} failed: {e}");
            }
        }
    });
    futures_util::future::join_all(tasks).await;
    kiwi.kick.notify_one();
}

pub fn set_enabled(kiwi: &Kiwi, id: &str, enabled: bool) -> ApiResult<SourceRow> {
    let not_found = || ApiError::not_found("Source not found.");
    if !kiwi.db.set_source_enabled(id, enabled)? {
        return Err(not_found());
    }
    kiwi.catalog
        .modify(id, |row| {
            row.enabled = enabled;
            row.source.enabled = enabled;
        })
        .ok_or_else(not_found)
}

/// Sets which apps Browse shows by default. Switching to [`BrowseMode::Picked`] again
/// restarts what counts as a new app.
pub fn set_browse(kiwi: &Kiwi, id: &str, browse: BrowseMode) -> ApiResult<SourceRow> {
    let not_found = || ApiError::not_found("Source not found.");
    let current = kiwi.catalog.source(id).ok_or_else(not_found)?;
    if current.browse == browse {
        return Ok((*current).clone());
    }
    let picked_at = (browse == BrowseMode::Picked).then(Utc::now);
    if !kiwi.db.set_source_browse(id, browse, picked_at)? {
        return Err(not_found());
    }
    kiwi.catalog
        .modify(id, |row| {
            row.browse = browse;
            row.picked_at = picked_at;
        })
        .ok_or_else(not_found)
}

/// Removes a source; installs are kept (their apps become orphaned).
pub fn remove(kiwi: &Kiwi, id: &str) -> ApiResult<()> {
    if id == crate::automation::SOURCE_ID {
        return Err(ApiError::bad_request(
            "Kiwi Builds is built in. Hide its apps with the source switch, or turn off automatic builds in Settings.",
        ));
    }
    if !kiwi.db.delete_source(id)? {
        return Err(ApiError::not_found("Source not found."));
    }
    kiwi.catalog.remove(id);
    Ok(())
}
