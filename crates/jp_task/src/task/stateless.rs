use std::error::Error;

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::Task;

pub struct StatelessTask(BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>);

impl StatelessTask {
    pub fn new(
        future: impl Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send + Sync + 'static,
    ) -> Self {
        Self(Box::pin(future))
    }
}

#[async_trait]
impl Task for StatelessTask {
    fn name(&self) -> &'static str {
        "stateless"
    }

    async fn start(
        mut self: Box<Self>,
        token: CancellationToken,
    ) -> Result<Box<dyn Task>, Box<dyn Error + Send + Sync>> {
        tokio::select! {
            () = token.cancelled() => {}
            v = &mut self.0 => v?
        };

        Ok(self)
    }
}
