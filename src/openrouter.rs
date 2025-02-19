use core::fmt;
use std::{collections::HashMap, future::Future};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use log::{debug, error};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::config::{Config, ModelConfig};

#[derive(Debug, Clone)]
pub struct Client {
    pub api_key: String,
    pub app_name: String,
    pub app_referrer: Option<String>,
    http_client: reqwest::Client,
}

impl Client {
    pub fn from_config(config: &Config) -> Result<Self> {
        let api_key = std::env::var(&config.openrouter.api_key_env).context(format!(
            "Missing API key from {}",
            &config.openrouter.api_key_env
        ))?;

        Ok(Self::new(
            api_key,
            config.openrouter.app_name.clone(),
            config.openrouter.app_referrer.clone(),
        ))
    }

    pub fn new(api_key: String, app_name: String, app_referrer: Option<String>) -> Self {
        Self {
            api_key,
            app_name,
            app_referrer,
            http_client: reqwest::Client::new(),
        }
    }

    pub fn request(
        &self,
        config: &ModelConfig,
        messages: Vec<ChatMessage>,
        stream: bool,
    ) -> Request {
        let mut request = Request {
            model: config.model().to_owned(),
            messages,
            stream: Some(stream),
            max_tokens: Some(config.max_tokens()),
            temperature: Some(config.temperature()),
            stop: None,
            include_reasoning: config.is_reasoning(),
            headers: self.headers(),
            http_client: self.http_client.clone(),
        };

        if config.model().starts_with("anthropic/") {
            request
                .headers
                .insert("anthropic-version", "2023-06-01".parse().unwrap());
            request.headers.insert(
                "anthropic-beta",
                "prompt-caching-2024-07-31".parse().unwrap(),
            );
        }

        if let ModelConfig::Reasoning {
            stop_word: Some(stop_word),
            ..
        } = config
        {
            request.stop = Some(vec![stop_word.to_string()]);
        }

        request
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::try_from(&HashMap::from([
            (
                "Authorization".to_owned(),
                format!("Bearer {}", self.api_key).parse().unwrap(),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("X-Title".to_owned(), self.app_name.to_owned()),
        ]))
        .unwrap();

        if let Some(referrer) = &self.app_referrer {
            headers.insert("HTTP-Referer", referrer.parse().unwrap());
        }

        headers
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    pub include_reasoning: bool,

    #[serde(skip)]
    pub headers: HeaderMap,

    #[serde(skip)]
    pub http_client: reqwest::Client,
}

impl Request {
    pub async fn send(self) -> Result<Response> {
        let builder = self
            .http_client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .headers(self.headers.clone());

        debug!("Sending request to model: {}", self.model);
        let response = builder
            .json(&self)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        // Check for successful response
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Could not read error response".to_string());
            error!(
                "OpenRouter API returned error: {} with body: {}",
                status, error_text
            );
            return Err(anyhow::anyhow!("API error: {}", status));
        }

        let response_text = response.text().await?;
        debug!("OpenRouter response: {}", response_text);

        let parsed_response: Response =
            serde_json::from_str(&response_text).context("Failed to parse OpenRouter response")?;

        Ok(parsed_response)
    }

    pub async fn stream<F, Fut>(&self, mut message_handler: F) -> Result<()>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let response = self
            .http_client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .headers(self.headers.clone())
            .json(&self)
            .send()
            .await
            .context("Failed to start streaming")?;

        // Check for non-success status
        if !response.status().is_success() {
            // TODO: Use a custom error type
            error!("Chat API returned error status: {}", response.status());
            return Ok(());
        }

        let mut stream = response.bytes_stream();
        let mut received_data = false;

        // Process the stream with error handling
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    // Convert chunk to string and split into SSE messages
                    let chunk_str = String::from_utf8_lossy(&chunk);
                    for line in chunk_str.lines() {
                        if !line.starts_with("data: ") {
                            continue;
                        }

                        received_data = true;
                        let data = &line[6..];

                        // Skip "[DONE]" message
                        if data == "[DONE]" {
                            continue;
                        }

                        // Parse the message with error handling
                        match serde_json::from_str::<StreamResponse>(data) {
                            Ok(response) => {
                                if let Some(content) = response
                                    .choices
                                    .first()
                                    .and_then(|choice| choice.delta.content.as_ref())
                                {
                                    message_handler(content.to_owned()).await?;
                                }
                            }
                            Err(e) => {
                                debug!("Failed to parse chunk: {} - Raw data: {}", e, data);
                                // Continue processing other chunks
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Error reading chunk: {}", e);
                    if !received_data {
                        println!("Error: Failed to receive data. Please try again later.");
                    }
                    break;
                }
            }
        }

        // If no data was received, show a generic error
        if !received_data {
            error!("No data received from stream");
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    #[expect(dead_code)]
    pub id: String,
    pub choices: Vec<Choice>,
    #[serde(flatten)]
    pub _extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: Message,
    #[expect(dead_code)]
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(flatten)]
    pub _extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub content: String,
    #[expect(dead_code)]
    pub role: Role,
    #[expect(dead_code)]
    pub reasoning: Option<String>,
    #[serde(flatten)]
    pub _extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct StreamResponse {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
    #[expect(dead_code)]
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    pub content: Option<String>,
}
