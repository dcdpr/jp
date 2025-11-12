use std::{error::Error, time::Duration};

use jp_workspace::Workspace;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::Task;

#[derive(Debug, Default)]
pub struct TaskHandler {
    tasks: JoinSet<Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>>,
    cancel_token: CancellationToken,
}

impl TaskHandler {
    pub fn spawn(&mut self, task: impl Task) {
        let name = task.name();
        debug!(name, "Spawning task.");
        let mut task = Box::new(task).start(self.cancel_token.child_token());
        self.tasks.spawn(async move {
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

        // Force long-running tasks to stop if they aren't responding to the
        // cancellation signal.
        self.wait_for_tasks(Duration::from_secs(2), &mut tasks, true)
            .await;

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
                // Ask long-running tasks to stop.
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
