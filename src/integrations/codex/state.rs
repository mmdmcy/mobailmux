use crate::AppState;
use crate::Arc;
use crate::CODEX_INDEX_REFRESH_AFTER;
use crate::CodexIndex;
use crate::CodexModel;
use crate::Instant;
use crate::Utc;
use crate::fetch_codex_model_catalog;
use crate::load_codex_index;
use crate::persistence;

pub(crate) fn codex_index_snapshot(state: &Arc<AppState>) -> Option<CodexIndex> {
    let (snapshot, should_refresh) = {
        let mut cache = state.codex_index.lock().unwrap();
        let stale = cache
            .refreshed_at
            .is_none_or(|refreshed_at| refreshed_at.elapsed() >= CODEX_INDEX_REFRESH_AFTER);
        let should_refresh = stale && !cache.refreshing;
        if should_refresh {
            cache.refreshing = true;
        }
        (cache.snapshot.clone(), should_refresh)
    };
    if should_refresh {
        refresh_codex_index(state.clone());
    }
    snapshot
}

pub(crate) fn refresh_codex_index(state: Arc<AppState>) {
    {
        let mut cache = state.codex_index.lock().unwrap();
        cache.refreshing = true;
    }
    tokio::task::spawn_blocking(move || {
        let index = load_codex_index_for_state(&state);
        let mut cache = state.codex_index.lock().unwrap();
        cache.snapshot = Some(index);
        cache.refreshed_at = Some(Instant::now());
        cache.refreshing = false;
    });
}

pub(crate) fn codex_model_catalog_snapshot(state: &Arc<AppState>) -> Vec<CodexModel> {
    let (models, should_refresh) = {
        let cache = state.codex_models.lock().unwrap();
        let stale = cache
            .refreshed_at
            .is_none_or(|refreshed_at| refreshed_at.elapsed() >= CODEX_INDEX_REFRESH_AFTER);
        let should_refresh = stale && !cache.refreshing;
        (cache.models.clone(), should_refresh)
    };
    if should_refresh {
        refresh_codex_model_catalog(state.clone());
    }
    models
}

pub(crate) fn refresh_codex_model_catalog(state: Arc<AppState>) {
    {
        let mut cache = state.codex_models.lock().unwrap();
        if cache.refreshing {
            return;
        }
        cache.refreshing = true;
    }
    tokio::task::spawn_blocking(move || {
        let models = fetch_codex_model_catalog(&state.config);
        let mut cache = state.codex_models.lock().unwrap();
        cache.models = models;
        cache.refreshed_at = (!cache.models.is_empty()).then(Instant::now);
        cache.refreshing = false;
    });
}

pub(crate) fn refresh_codex_index_blocking(state: &Arc<AppState>) -> CodexIndex {
    let index = load_codex_index_for_state(state);
    let mut cache = state.codex_index.lock().unwrap();
    cache.snapshot = Some(index.clone());
    cache.refreshed_at = Some(Instant::now());
    cache.refreshing = false;
    index
}

pub(crate) fn load_codex_index_for_state(state: &Arc<AppState>) -> CodexIndex {
    let mut index = load_codex_index(&state.config);
    attach_codex_reset_credit_estimate(state, &mut index);
    index
}

pub(crate) fn attach_codex_reset_credit_estimate(state: &Arc<AppState>, index: &mut CodexIndex) {
    let Some(summary) = index
        .usage
        .as_mut()
        .and_then(|usage| usage.reset_credits.as_mut())
    else {
        return;
    };
    let db = state.db.lock().unwrap();
    if let Ok(estimate) =
        persistence::reset_ledger::reconcile(&db, summary.available_count, Utc::now())
    {
        summary.estimate = Some(estimate);
    }
}
