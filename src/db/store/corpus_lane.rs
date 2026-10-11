//! Process-local corpus read/write coordination and FTS-finalization failure records.

use super::*;

impl Store {
    /// Process-local corpus-mutation lane shared by every store clone.
    ///
    /// Source synchronization and guarded bulk mutations own the write side
    /// through their derived-index refresh. Searches briefly
    /// own the read side, so a mutation cannot race past their idle check.
    pub(crate) fn corpus_mutation_lane(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.corpus_mutation_lane.clone()
    }

    /// Acquire the exclusive side of the corpus lane without waiting.
    ///
    /// A writer can be blocked by another corpus mutation or by an admitted
    /// guarded search. Keeping this diagnostic in the store prevents
    /// each transport workflow from inventing a stale source-sync-only reason.
    pub(crate) fn try_corpus_mutation_guard(&self, operation: &str) -> Result<CorpusMutationGuard> {
        self.corpus_mutation_lane
            .clone()
            .try_write_owned()
            .map_err(|_| {
                AppError::busy(format!(
                    "another exclusive corpus mutation or guarded search is active; retry {operation} after it completes"
                ))
            })
    }

    /// Acquire the non-exclusive side of the corpus-mutation lane without waiting.
    ///
    /// `None` means an exclusive corpus mutation is active. Keeping the guard
    /// alive prevents a new mutation from starting until the guarded search ends.
    pub(crate) fn try_corpus_idle_guard(&self) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        self.corpus_mutation_lane.clone().try_read_owned().ok()
    }

    /// Pass an existing exclusive lease to nested application services explicitly.
    /// Only clones of this returned store may reuse the lease; ordinary clones of
    /// the original store still see a busy corpus. A guard for another store is
    /// never accepted, even when the databases happen to use the same path.
    pub(crate) fn with_corpus_mutation_guard(&self, guard: CorpusMutationGuard) -> Result<Self> {
        if !Arc::ptr_eq(
            CorpusMutationGuard::rwlock(&guard),
            &self.corpus_mutation_lane,
        ) {
            return Err(AppError::config(
                "corpus mutation guard belongs to another store",
            ));
        }
        let mut scoped = self.clone();
        scoped.corpus_mutation_guard = Some(Arc::new(guard));
        Ok(scoped)
    }

    /// Enter a bulk mutation, or reuse the explicitly inherited exclusive lease.
    pub(crate) fn try_corpus_mutation_scope(&self, operation: &str) -> Result<Self> {
        if self.corpus_mutation_guard.is_some() {
            return Ok(self.clone());
        }
        self.with_corpus_mutation_guard(self.try_corpus_mutation_guard(operation)?)
    }

    /// Validate an embedding write while holding the corpus identity stable.
    /// New/empty stores may initialize their manifest; existing vectors without
    /// a manifest and incomplete/mismatched migrations remain fail-closed.
    pub(crate) fn try_embedding_write_guard(
        &self,
        config: &impl EmbeddingIdentity,
        operation: &str,
    ) -> Result<EmbeddingWriteGuard> {
        let guard = if let Some(exclusive) = &self.corpus_mutation_guard {
            EmbeddingWriteGuard {
                _shared: None,
                _exclusive: Some(exclusive.clone()),
            }
        } else {
            let shared = self.try_corpus_idle_guard().ok_or_else(|| {
                AppError::busy(format!(
                    "an exclusive corpus mutation is active; retry {operation} after it completes"
                ))
            })?;
            EmbeddingWriteGuard {
                _shared: Some(shared),
                _exclusive: None,
            }
        };
        self.ensure_embedding_manifest(config)?;
        self.require_embedding_manifest_match(config)?;
        Ok(guard)
    }

    /// Leave lexical state explicitly stale after a failed eager finalization.
    ///
    /// Bulk workflows call this only after their durable mutation has already
    /// committed. Advancing the generation is a safe false-positive: it may
    /// invalidate one vector snapshot and force one lexical rebuild, but it can
    /// never publish stale BM25 rows as current.
    pub(super) fn mark_fts_dirty_for_retry(&self) -> Result<()> {
        let conn = self.lock()?;
        crate::db::fts::mark_fts_dirty(&conn)?;
        Ok(())
    }

    /// Persist the fail-safe state and a sanitized terminal audit record after
    /// eager FTS finalization fails. The payload says whether preceding corpus
    /// work actually committed, including the legitimate no-op case.
    ///
    /// This method is intentionally safe to call from detached blocking work:
    /// even if the transport waiter disappears, operators can still distinguish
    /// committed corpus work, a no-op failure, and a clean terminal success.
    pub(crate) fn record_fts_finalization_failure(
        &self,
        operation: &str,
        durable_mutation_committed: bool,
        error: &str,
    ) -> FtsFinalizationFailure {
        let marker_error = self
            .mark_fts_dirty_for_retry()
            .err()
            .map(|marker_error| marker_error.to_string());
        let log_error = self
            .append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: Utc::now(),
                op: "fts_finalization_failed".into(),
                prefix: Some("FTS".into()),
                message: format!("{operation} FTS finalization failed"),
                entity_id: None,
                entity_kind: Some("corpus".into()),
                payload_json: serde_json::json!({
                    "operation": operation,
                    "code": "FTS_FINALIZATION_FAILED",
                    "durable_mutation_committed": durable_mutation_committed,
                    "retryable": true,
                    "dirty_marker_written": marker_error.is_none(),
                })
                .to_string(),
                agent_name: None,
            })
            .err()
            .map(|log_error| log_error.to_string());

        let mutation_state = if durable_mutation_committed {
            "durable corpus work was committed"
        } else {
            "no durable corpus mutation was detected"
        };
        let mut message = format!(
            "FTS_FINALIZATION_FAILED: {operation} final FTS refresh failed ({mutation_state}): {error}; retryable=true"
        );
        if let Some(marker_error) = &marker_error {
            message.push_str(&format!(
                "; additionally failed to mark FTS dirty for retry: {marker_error}"
            ));
        }
        if let Some(log_error) = &log_error {
            message.push_str(&format!(
                "; additionally failed to append terminal ops log: {log_error}"
            ));
        }
        FtsFinalizationFailure {
            message,
            dirty_marker_written: marker_error.is_none(),
            dirty_marker_error: marker_error,
            ops_log_error: log_error,
        }
    }
}
