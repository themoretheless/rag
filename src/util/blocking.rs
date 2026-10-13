//! Run DuckDB-bound or otherwise blocking work off the async runtime.
//!
//! Both transports and detached maintenance workflows map a lost task
//! (`JoinError`) onto the same `AppError::Db` shape, so the mapping lives here.

use std::future::Future;

use crate::error::AppError;

fn join_error(operation: &str, error: tokio::task::JoinError) -> AppError {
    AppError::db(format!("{operation} task failed: {error}"))
}

/// Run short synchronous store work from an async handler without stalling
/// the runtime.
///
/// On the multi-thread runtime the current worker is handed over to blocking
/// mode (`block_in_place`), so tasks queued on it move to other workers while
/// DuckDB runs or waits for the store mutex. Unlike [`run_blocking`] it needs
/// no `'static` captures, so handlers keep borrowing their state. On a
/// current-thread runtime (tests) or outside a runtime the work runs inline.
pub fn blocking_section<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
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

#[cfg(test)]
mod tests {
    use super::blocking_section;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_section_hands_off_the_worker_on_the_multi_thread_runtime() {
        // A task spawned before the section must still make progress while the
        // section blocks: block_in_place moves it to another worker.
        let (tx, rx) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            tx.send(()).unwrap();
        });
        let value = blocking_section(|| {
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("spawned task ran while the section blocked");
            7
        });
        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn blocking_section_runs_inline_on_the_current_thread_runtime() {
        assert_eq!(blocking_section(|| 3), 3);
    }

    #[test]
    fn blocking_section_runs_inline_outside_a_runtime() {
        assert_eq!(blocking_section(|| "ok"), "ok");
    }
}
