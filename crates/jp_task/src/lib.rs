mod handler;
pub mod task;

use std::error::Error;

use async_trait::async_trait;
pub use handler::TaskHandler;
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;

/// An asynchronous task that runs in the background and syncs its state with
/// the workspace upon completion.
#[async_trait]
pub trait Task: Send + 'static {
    fn name(&self) -> &'static str;

    /// Run the task in the background.
    async fn run(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<Box<dyn Task>, Box<dyn Error + Send + Sync>>;

    /// Sync the results of the task with the workspace.
    ///
    /// This is called after the task has completed.
    #[expect(unused_variables)]
    async fn sync(
        self: Box<Self>,
        ctx: &mut Workspace,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}
