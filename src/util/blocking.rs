//! Run DuckDB-bound or otherwise blocking work off the async runtime.
//!
//! Both transports and detached maintenance workflows map a lost task
//! (`JoinError`) onto the same `AppError::Db` shape, so the mapping lives here.

use std::future::Future;

use crate::error::AppError;

fn join_error(operation: &str, error: tokio::task::JoinError) -> AppError {
    AppError::db(format!("{operation} task failed: {error}"))
}

/// Run synchronous `work` on the blocking pool and surface its result.
pub async fn run_blocking<T, F>(operation: &'static str, work: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| join_error(operation, error))?
}

/// Drive `work` as an owned task that survives a cancelled caller.
///
/// Dropping the returned future only detaches the waiter; the workflow keeps
/// running to completion, which is what guarded corpus mutations require.
pub async fn run_detached<T, Fut>(operation: &'static str, work: Fut) -> Result<T, AppError>
where
    T: Send + 'static,
    Fut: Future<Output = Result<T, AppError>> + Send + 'static,
{
    tokio::spawn(work)
        .await
        .map_err(|error| join_error(operation, error))?
}
