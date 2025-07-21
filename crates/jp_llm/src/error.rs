pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OpenRouter error: {0}")]
    OpenRouter(#[from] jp_openrouter::Error),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::Error),

    #[error("Missing environment variable: {0}")]
    MissingEnv(String),

    #[error("Invalid URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("Invalid response received: {0}")]
    InvalidResponse(String),

    #[error("OpenAI client error: {0}")]
    OpenaiClient(#[from] openai_responses::CreateError),

    #[error("OpenAI event error: {0}")]
    OpenaiEvent(Box<reqwest_eventsource::Error>),

    #[error("OpenAI response error: {0:?}")]
    OpenaiResponse(openai_responses::types::response::Error),

    #[error("OpenAI status code error: {:?} - {}", .status_code, .response)]
    OpenaiStatusCode {
        status_code: reqwest::StatusCode,
        response: String,
    },

    #[error("Gemini error: {0}")]
    Gemini(gemini_client_rs::GeminiError),

    #[error("Ollama error: {0}")]
    Ollama(#[from] ollama_rs::error::OllamaError),

    #[error("Missing structured data in response")]
    MissingStructuredData,

    #[error("Unknown model: {0}")]
    UnknownModel(String),

    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Anthropic error: {0}")]
    Anthropic(#[from] async_anthropic::errors::AnthropicError),

    #[error("Anthropic request builder error: {0}")]
    AnthropicRequestBuilder(#[from] async_anthropic::types::CreateMessagesRequestBuilderError),

    #[error("request rate limited (retry after {} seconds)", retry_after.unwrap_or_default())]
    RateLimit { retry_after: Option<u64> },
}

impl From<gemini_client_rs::GeminiError> for Error {
    fn from(error: gemini_client_rs::GeminiError) -> Self {
        use gemini_client_rs::GeminiError;

        dbg!(&error);

        match &error {
            GeminiError::Api(api) if api.get("status").is_some_and(|v| v.as_u64() == Some(404)) => {
                if let Some(model) = api.pointer("/message/error/message").and_then(|v| {
                    v.as_str().and_then(|s| {
                        s.contains("Call ListModels").then(|| {
                            s.split('/')
                                .nth(1)
                                .and_then(|v| v.split(' ').next())
                                .unwrap_or("unknown")
                        })
                    })
                }) {
                    return Self::UnknownModel(model.to_owned());
                }
                Self::Gemini(error)
            }
            _ => Self::Gemini(error),
        }
    }
}

impl From<openai_responses::types::response::Error> for Error {
    fn from(error: openai_responses::types::response::Error) -> Self {
        Self::OpenaiResponse(error)
    }
}

#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}
