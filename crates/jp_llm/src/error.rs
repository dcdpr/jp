pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Missing environment variable: {0}")]
    MissingEnv(String),

    #[error("OpenRouter error: {0}")]
    OpenRouter(#[from] jp_openrouter::Error),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::Error),

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

    #[error("Missing structured data in response")]
    MissingStructuredData,

    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),
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
