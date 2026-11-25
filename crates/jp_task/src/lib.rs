mod handler;
pub mod task;

use std::error::Error;

use futures::future::BoxFuture;
pub use handler::TaskHandler;
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;

/// An asynchronous task that runs in the background and syncs its state with
/// the workspace upon completion.
///
/// Implement this trait, writing `run` and `sync` as `async fn`.
/// The [`TaskHandler`] holds tasks as `Box<dyn DynTask>`, which every `Task`
/// implements.
pub trait Task: Send + 'static {
    fn name(&self) -> &'static str;

    /// Run the task in the background.
    fn run(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<Box<dyn DynTask>, BoxedError>> + Send;

    /// Sync the results of the task with the workspace.
    ///
    /// This is called after the task has completed.
    #[expect(unused_variables)]
    fn sync(
        self: Box<Self>,
        ctx: &mut Workspace,
    ) -> impl Future<Output = Result<(), BoxedError>> + Send {
        async { Ok(()) }
    }
}

/// A [`Task`] behind a pointer: `Box<dyn DynTask>`.
///
/// Every `Task` implements this trait, and each method does what the `Task`
/// method of the same name does, returning a boxed future.
/// Implement [`Task`], not this trait.
pub trait DynTask: Send + 'static {
    fn name(&self) -> &'static str;

    fn run(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<Box<dyn DynTask>, BoxedError>>;

    fn sync(self: Box<Self>, ctx: &mut Workspace) -> BoxFuture<'_, Result<(), BoxedError>>;
}

impl<T: Task> DynTask for T {
    fn name(&self) -> &'static str {
        Task::name(self)
    }

    fn run(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<Box<dyn DynTask>, BoxedError>> {
        Box::pin(Task::run(self, cancel))
    }

    fn sync(self: Box<Self>, ctx: &mut Workspace) -> BoxFuture<'_, Result<(), BoxedError>> {
        Box::pin(Task::sync(self, ctx))
    }
}

type BoxedError = Box<dyn Error + Send + Sync>;
