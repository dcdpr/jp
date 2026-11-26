use std::error::Error;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::{DynTask, Task};

pub struct StatelessTask(BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>);

impl StatelessTask {
    pub fn new(
        future: impl Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send + Sync + 'static,
    ) -> Self {
        Self(Box::pin(future))
    }
}

impl Task for StatelessTask {
    fn name(&self) -> &'static str {
        "stateless"
    }

    async fn run(
        mut self: Box<Self>,
        token: CancellationToken,
    ) -> Result<Box<dyn DynTask>, Box<dyn Error + Send + Sync>> {
        jp_macro::select!(token.cancelled(), |_cancel| {}, &mut self.0, |v| { v? });

        Ok(self)
    }
}
