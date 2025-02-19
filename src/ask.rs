use anyhow::Result;
use log::info;

use crate::chat;
use crate::config::Config;
use crate::openrouter::Client;

pub async fn process_question(client: &Client, config: &Config, question: &str) -> Result<()> {
    chat::stdout(client, config, question).await
}
