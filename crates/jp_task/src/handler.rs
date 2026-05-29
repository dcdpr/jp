use std::{error::Error, time::Duration};

use jp_workspace::Workspace;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::Task;

#[derive(Debug, Default)]
pub struct TaskHandler {
    tasks: JoinSet<Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>>,
    /// Soft-cancellation signal.
    /// Firing it asks each task's `run()` to return promptly; well-behaved
    /// tasks then proceed to their `sync()` phase under [`TaskHandler::sync`].
    /// Tasks that don't observe the token are force-aborted after the grace
    /// window.
    cancel_token: CancellationToken,
    /// Hard-cancellation signal.
    /// Firing it short-circuits both the soft wait and the grace window in
    /// [`TaskHandler::sync`], force-aborts the `JoinSet`, and skips the
    /// workspace-sync iteration entirely.
    /// Tasks that had completed their `run()` cleanly lose their pending
    /// workspace mutation.
    force_token: CancellationToken,
}

impl TaskHandler {
    /// Returns `true` if no tasks are currently live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Returns a clone of the soft-cancellation token.
    ///
    /// Cancelling the token signals every task's `run()` to stop.
    /// The `sync()` phase still runs for tasks that returned cleanly.
    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Returns a clone of the hard-cancellation token.
    ///
    /// Cancelling the token force-aborts the `JoinSet` and skips the
    /// workspace-sync iteration.
    /// Pending workspace mutations are dropped.
    #[must_use]
    pub fn force_token(&self) -> CancellationToken {
        self.force_token.clone()
    }

    pub fn spawn(&mut self, task: impl Task) {
        let name = task.name();
        debug!(name, "Spawning task.");
        let token = self.cancel_token.child_token();
        self.tasks.spawn(async move {
            let mut task = Box::new(task).run(token);
            let now = tokio::time::Instant::now();
            loop {
                jp_macro::select!(
                    biased,
                    tokio::time::sleep(Duration::from_millis(500)),
                    |_wake| {
                        trace!(name, elapsed_ms = %now.elapsed().as_millis(), "Task running...");
                    },
                    &mut task,
                    |v| {
                        debug!(name, elapsed_ms = %now.elapsed().as_millis(), "Task completed.");
                        return v;
                    }
                );
            }
        });
    }

    pub async fn sync(
        &mut self,
        workspace: &mut Workspace,
        timeout: Duration,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        let mut tasks: Vec<Box<dyn Task>> = Vec::new();
        self.wait_for_tasks(timeout, &mut tasks, false).await;

        // Grace window for stragglers that didn't observe the soft
        // cancellation signal.
        self.wait_for_tasks(Duration::from_secs(2), &mut tasks, true)
            .await;

        // Force quit: drop accumulated results without applying them.
        if self.force_token.is_cancelled() {
            warn!(
                count = tasks.len(),
                "Force-quit requested; skipping workspace sync for collected tasks."
            );
            return Ok(());
        }

        for task in tasks {
            if let Err(error) = task.sync(workspace).await {
                tracing::error!(%error, "Error syncing background task.");
            }
        }

        Ok(())
    }

    async fn wait_for_tasks(
        &mut self,
        timeout: Duration,
        tasks: &mut Vec<Box<dyn Task>>,
        shutdown: bool,
    ) {
        let timeout = tokio::time::sleep(timeout);
        tokio::pin!(timeout);

        loop {
            jp_macro::select!(
                biased,
                self.force_token.cancelled(),
                |_force| {
                    // Force quit: abort everything immediately. The
                    // grace pass observes the same signal and exits via
                    // the empty-JoinSet branch.
                    warn!("Force-quit requested. Aborting background tasks.");
                    self.tasks.shutdown().await;
                    break;
                },
                self.cancel_token.cancelled(),
                |_cancel| if (!shutdown) {
                    // Soft cancellation fired externally during the soft
                    // wait: stop waiting and let the grace pass collect
                    // any tasks still in flight.
                    break;
                },
                &mut timeout,
                |_wake| {
                    if shutdown {
                        warn!("Tasks did not respond to cancellation signal. Forcing shutdown.");
                        self.tasks.shutdown().await;
                    } else {
                        warn!(
                            "Task finalization timed out. Signalling cancellation to remaining \
                             tasks."
                        );
                        self.cancel_token.cancel();
                    }
                    break;
                },
                self.tasks.join_next(),
                |task| {
                    match task {
                        Some(task) => handle_task_completion(task, tasks),
                        None => break,
                    }
                },
            );
        }
    }
}

#[expect(clippy::type_complexity)]
fn handle_task_completion(
    result: Result<Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>, JoinError>,
    tasks: &mut Vec<Box<dyn Task>>,
) {
    match result {
        Ok(Ok(task)) => tasks.push(task),
        Ok(Err(error)) => tracing::error!(%error, "Background task failed."),
        Err(error) => tracing::error!(%error, "Error waiting for background task to complete."),
    }
}
