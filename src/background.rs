//! Background loops owned by the composition root.
//!
//! Auto-backup, FTS maintenance, auto-sync, the file watcher and the replica
//! sync worker all register here. They share one [`CancellationToken`]: each
//! loop checks it between iterations (never mid-write), and
//! [`BackgroundTasks::shutdown`] cancels the token and waits, bounded, for
//! every loop to return before the final checkpoint.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

enum Handle {
    Task(&'static str, tokio::task::JoinHandle<()>),
    Thread(&'static str, std::thread::JoinHandle<()>),
}

/// Registry of background loops with one shared cancellation token.
#[derive(Clone, Default)]
pub struct BackgroundTasks {
    token: CancellationToken,
    handles: Arc<Mutex<Vec<Handle>>>,
}

impl BackgroundTasks {
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared token; cancelled by [`Self::shutdown`].
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Spawn an async loop on the current runtime.
    pub fn spawn<F>(&self, name: &'static str, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        self.push(Handle::Task(name, handle));
    }

    /// Spawn a loop on a dedicated OS thread (blocking watch loops).
    pub fn spawn_thread<F>(&self, name: &'static str, work: F) -> std::io::Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        let handle = std::thread::Builder::new().name(name.into()).spawn(work)?;
        self.push(Handle::Thread(name, handle));
        Ok(())
    }

    fn push(&self, handle: Handle) {
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(handle);
    }

    /// Sleep for `interval` unless cancelled first. Returns `false` when the
    /// loop should stop.
    pub async fn pause(&self, interval: Duration) -> bool {
        tokio::select! {
            _ = self.token.cancelled() => false,
            _ = tokio::time::sleep(interval) => true,
        }
    }

    /// Cancel every loop and wait up to `timeout` for all of them to return.
    ///
    /// Returns the names of the loops still running at the deadline (also
    /// logged), so the caller knows what may still touch the store when it
    /// runs the final checkpoint.
    pub async fn shutdown(&self, timeout: Duration) -> Vec<&'static str> {
        self.token.cancel();
        let handles = std::mem::take(
            &mut *self
                .handles
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let deadline = tokio::time::Instant::now() + timeout;
        let mut still_running = Vec::new();
        for handle in handles {
            match handle {
                Handle::Task(name, task) => {
                    if tokio::time::timeout_at(deadline, task).await.is_err() {
                        still_running.push(name);
                    }
                }
                Handle::Thread(name, thread) => {
                    let joined = tokio::task::spawn_blocking(move || thread.join());
                    if tokio::time::timeout_at(deadline, joined).await.is_err() {
                        still_running.push(name);
                    }
                }
            }
        }
        if !still_running.is_empty() {
            tracing::warn!(
                tasks = ?still_running,
                timeout_ms = timeout.as_millis() as u64,
                "background tasks still running after the shutdown timeout"
            );
        }
        still_running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_stops_pausing_loops_and_threads() {
        let tasks = BackgroundTasks::new();
        let looping = tasks.clone();
        tasks.spawn("loop", async move {
            while looping.pause(Duration::from_secs(3600)).await {}
        });
        let token = tasks.token();
        tasks
            .spawn_thread("thread", move || {
                while !token.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .unwrap();
        let started = std::time::Instant::now();
        tasks.shutdown(Duration::from_secs(5)).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(tasks.is_cancelled());
    }

    #[tokio::test]
    async fn shutdown_reports_tasks_that_ignore_the_token() {
        let tasks = BackgroundTasks::new();
        tasks.spawn("stubborn", async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        let looping = tasks.clone();
        tasks.spawn("polite", async move {
            while looping.pause(Duration::from_secs(3600)).await {}
        });
        let still_running = tasks.shutdown(Duration::from_millis(50)).await;
        assert_eq!(still_running, vec!["stubborn"]);
    }
}
