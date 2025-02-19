use anyhow::Result;
use log::info;
use reqwest::Client;

use crate::anthropic;
use crate::deepseek;

pub struct LLMClient {
    pub http_client: Client,
    pub api_key: String,
}

// Make LLMClient safely shareable between threads
impl Clone for LLMClient {
    fn clone(&self) -> Self {
        Self {
            http_client: Client::new(),
            api_key: self.api_key.clone(),
        }
    }
}

impl LLMClient {
    pub fn new(api_key: String) -> Result<Self> {
        Ok(Self {
            http_client: Client::new(),
            api_key,
        })
    }

    pub async fn process_question(&self, question: &str) -> Result<()> {
        info!("Processing question: {}", question);

        // First get reasoning from DeepSeek (silently)
        let reasoning =
            deepseek::query_deepseek(&self.http_client, &self.api_key, question).await?;

        // Stream Claude's response to stdout
        anthropic::stream_claude_response(&self.http_client, &self.api_key, &reasoning, question)
            .await?;

        Ok(())
    }

    pub async fn query_deepseek(&self, question: &str) -> Result<String> {
        deepseek::query_deepseek(&self.http_client, &self.api_key, question).await
    }

    pub async fn get_streaming_response(
        &self,
        reasoning: &str,
        question: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<axum::response::sse::Event, axum::Error>>>
    {
        anthropic::get_streaming_response(&self.http_client, &self.api_key, reasoning, question)
            .await
    }

    pub async fn process_completion_request(
        &self,
        request: anthropic::ClaudeRequest,
    ) -> Result<(String, bool)> {
        anthropic::process_completion_request(&self.http_client, &self.api_key, request).await
    }
}
