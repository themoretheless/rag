//! Terminal audit row shared by every maintenance workflow.
//!
//! `apply`, `refresh`, `compress` and `organize` each finish with one `MAINT`
//! ops-log row named `<op>` or `<op>_dry_run` whose payload is the full report.
//! Whether a failed audit write fails the run is the caller's visible choice:
//! most propagate with `?`, `compress` reports `TERMINAL_AUDIT_FAILED` instead.

use serde::Serialize;

use crate::db::Store;
use crate::error::Result;
use crate::models::OpsLogEntry;

/// Append the run summary and return the written row (id and seq).
pub(crate) fn record_run<R: Serialize>(
    store: &Store,
    base_op: &str,
    dry_run: bool,
    entity_kind: &str,
    message: String,
    agent: Option<String>,
    report: &R,
) -> Result<OpsLogEntry> {
    let op = if dry_run {
        format!("{base_op}_dry_run")
    } else {
        base_op.to_string()
    };
    let payload = serde_json::to_string(report).unwrap_or_else(|_| "{}".into());
    store.append_ops_log(
        &OpsLogEntry::new(op, "MAINT", message)
            .entity(None, entity_kind)
            .payload(payload)
            .agent(agent),
    )
}
