use crate::{
    Client,
    client::StreamResponse,
    errors::AnthropicError,
    types::{CreateMessagesRequest, CreateMessagesResponse, MessagesStreamEvent},
};

pub const DEFAULT_MAX_TOKENS: i32 = 2048;

#[derive(Debug, Clone)]
pub struct Messages<'c> {
    client: &'c Client,
}

impl Messages<'_> {
    #[must_use]
    pub fn new(client: &Client) -> Messages<'_> {
        Messages { client }
    }

    #[tracing::instrument(skip_all)]
    pub async fn create(
        &self,
        request: impl Into<CreateMessagesRequest>,
    ) -> Result<CreateMessagesResponse, AnthropicError> {
        let mut request = request.into();
        request.stream = false;

        // Betas ride in the `anthropic-beta` header, not the body.
        let betas = std::mem::take(&mut request.betas);

        self.client.post("/v1/messages", request, &betas).await
    }

    /// Stream a message completion.
    ///
    /// The returned [`StreamResponse`] carries the response's quota headers
    /// alongside its events, so a caller can read the subscription's usage
    /// state from a successful request rather than waiting for a rejection.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is rejected before streaming begins,
    /// which is where a spent quota or a refused credential surfaces.
    #[tracing::instrument(skip_all)]
    pub async fn create_stream(
        &self,
        request: impl Into<CreateMessagesRequest>,
    ) -> Result<StreamResponse<MessagesStreamEvent>, AnthropicError> {
        let mut request = request.into();
        request.stream = true;

        // Betas ride in the `anthropic-beta` header, not the body.
        let betas = std::mem::take(&mut request.betas);

        self.client
            .post_stream(
                "/v1/messages",
                request,
                [
                    "ping",
                    "message_start",
                    "message_delta",
                    "message_stop",
                    "content_block_start",
                    "content_block_delta",
                    "content_block_stop",
                ],
                &betas,
            )
            .await
    }
}
