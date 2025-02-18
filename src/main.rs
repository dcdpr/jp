use anyhow::{Context, Result};
use futures_util::StreamExt;
use log::{debug, error, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::Write;

#[derive(Debug, Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stop: Option<Vec<String>>,
    include_reasoning: bool,
}

#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct DeepSeekResponse {
    choices: Vec<DeepSeekChoice>,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct DeepSeekChoice {
    message: DeepSeekMessage,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct DeepSeekMessage {
    #[serde(default)]
    content: Option<String>,
    reasoning: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

struct LLMClient {
    http_client: Client,
    api_key: String,
}

impl LLMClient {
    fn new(api_key: String) -> Result<Self> {
        Ok(Self {
            http_client: Client::new(),
            api_key,
        })
    }

    async fn process_question(&self, question: &str) -> Result<()> {
        info!("Processing question: {}", question);

        // First get reasoning from DeepSeek (silently)
        let reasoning = self.query_deepseek(question).await?;
        debug!("Got reasoning: {}", reasoning);

        // Stream Claude's response to stdout
        self.stream_claude_response(&reasoning, question).await?;

        Ok(())
    }

    async fn query_deepseek(&self, question: &str) -> Result<String> {
        let request = DeepSeekRequest {
            model: "deepseek/deepseek-r1".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are a helpful AI assistant. Process each request thoughtfully and methodically.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: question.to_string(),
                },
            ],
            stop: Some(vec!["</think>".to_string()]),
            include_reasoning: true,
        };

        let request_builder = self
            .http_client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        debug!(
            "Request headers: {:#?}",
            request_builder
                .try_clone()
                .unwrap()
                .build()
                .unwrap()
                .headers()
        );

        let request_json = serde_json::to_string_pretty(&request).unwrap();
        debug!("Request body: {}", request_json);

        let response = request_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to DeepSeek")?;

        let response_text = response.text().await?;
        debug!("Raw response from DeepSeek: {}", response_text);

        let response: DeepSeekResponse =
            serde_json::from_str(&response_text).context("Failed to parse DeepSeek response")?;

        debug!("Parsed DeepSeek response: {:#?}", response);

        // Get the reasoning field, which is what we want when include_reasoning is true
        let first_choice = response.choices.first().context("No choices in response")?;

        debug!("First choice: {:#?}", first_choice);

        if let Some(reasoning) = &first_choice.message.reasoning {
            Ok(reasoning.clone())
        } else if let Some(content) = &first_choice.message.content {
            debug!("No reasoning field found, falling back to content");
            Ok(content.clone())
        } else {
            error!("Neither reasoning nor content found in response");
            Err(anyhow::anyhow!("No response content available"))
        }
    }

    async fn stream_claude_response(&self, reasoning: &str, question: &str) -> Result<()> {
        info!("Starting Claude streaming response");

        // Create combined prompt
        let combined_prompt = format!("{}\n\n{}", reasoning, question);
        debug!("Combined prompt for Claude: {}", combined_prompt);

        let request = ClaudeRequest {
            model: "anthropic/claude-3.5-sonnet".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: combined_prompt,
            }],
            stream: true,
        };

        let response = self
            .http_client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to start streaming")?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Error reading chunk")?;

            // Convert chunk to string and split into SSE messages
            let chunk_str = String::from_utf8_lossy(&chunk);
            for line in chunk_str.lines() {
                if line.starts_with("data: ") {
                    let data = &line[6..];

                    // Skip "[DONE]" message
                    if data == "[DONE]" {
                        println!();
                        continue;
                    }

                    // Parse the message
                    if let Ok(response) = serde_json::from_str::<StreamResponse>(data) {
                        if let Some(content) = response.choices[0].delta.content.as_ref() {
                            print!("{}", content);
                            std::io::stdout().flush().unwrap();
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();

    // Load environment variables
    dotenv::dotenv().ok();

    let api_key = env::var("OPENROUTER_API_KEY")
        .context("OPENROUTER_API_KEY environment variable must be set")?;

    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <question>", args[0]);
        std::process::exit(1);
    }

    let question = &args[1];
    let client = LLMClient::new(api_key)?;

    // Process the question and stream response
    client.process_question(question).await?;

    Ok(())
}
