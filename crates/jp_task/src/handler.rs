use std::{error::Error, time::Duration};

use jp_workspace::Workspace;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::Task;

#[derive(Debug, Default)]
pub struct TaskHandler {
    tasks: JoinSet<Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>>,
    cancel_token: CancellationToken,
}

impl TaskHandler {
    pub fn spawn(&mut self, task: impl Task) {
        self.tasks
            .spawn(Box::new(task).start(self.cancel_token.child_token()));
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

        // Handle background tasks.
        loop {
            tokio::select! {
                biased;
                // Ask long-running tasks to stop.
                () = tokio::time::sleep(timeout) => {
                    warn!("Task finalization timed out. Signalling cancellation to remaining tasks.");
                    if !self.cancel_token.is_cancelled() {
                        self.cancel_token.cancel();
                    }
                    break;
                }
                Some(result) = self.tasks.join_next() => {
                    match result {
                        Ok(Ok(task)) => tasks.push(task),
                        Ok(Err(error)) => tracing::error!(%error, "Background task failed."),
                        Err(error) => tracing::error!(%error, "Error waiting for background task to complete."),
                    }
                }
                else => {
                    break;
                }
            }
        }

        // Force long-running tasks to stop if they aren't responding to the
        // cancellation signal.
        loop {
            tokio::select! {
                biased;
                () = tokio::time::sleep(Duration::from_secs(2)) => {
                    warn!("Tasks did not respond to cancellation signal. Forcing shutdown.");
                    self.tasks.shutdown().await;
                    break;
                }
                Some(result) = self.tasks.join_next() => {
                    match result {
                        Ok(Ok(task)) => tasks.push(task),
                        Ok(Err(error)) => tracing::error!(%error, "Background task failed."),
                        Err(error) => tracing::error!(%error, "Error waiting for background task to complete."),
                    }
                }
                else => {
                    break;
                }
            }
        }

        for task in tasks {
            if let Err(error) = task.sync(workspace).await {
                tracing::error!(%error, "Error syncing background task.");
            }
        }

        Ok(())
    }
}
