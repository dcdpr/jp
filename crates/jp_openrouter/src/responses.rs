//! Openrouter Responses API types.
//!
//! This module contains request types for making requests to the Openrouter
//! Responses API, as well as streaming event types for consuming Server-Sent
//! Events from streaming responses.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Service tier for request priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier {
    #[default]
    Auto,
    Default,
    Flex,
    Priority,
    Scale,
}

/// Truncation strategy for long inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Truncation {
    Auto,
    Disabled,
}

/// Data collection preference for providers.
///
/// If no available model provider meets the requirement, your request will
/// return an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataCollection {
    /// Allow providers which store user data non-transiently and may train on
    /// it.
    Allow,

    /// Use only providers which do not collect user data.
    Deny,
}

/// Provider sorting strategy for request routing.
///
/// The sorting strategy to use for this request, if "order" is not specified.
/// When set, no load balancing is performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSort {
    Price,
    Throughput,
    Latency,
}

/// Quantization levels for filtering providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantization {
    Int4,
    Int8,
    Fp4,
    Fp6,
    Fp8,
    Fp16,
    Bf16,
    Fp32,
    Unknown,
}

/// Reasoning effort level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    High,
    Medium,
    Low,
    Minimal,
    None,
}

/// Reasoning summary verbosity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummaryVerbosity {
    Auto,
    Concise,
    Detailed,
}

/// Response text verbosity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseTextVerbosity {
    High,
    Low,
    Medium,
}

/// Size of the search context for web search tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchContextSize {
    Low,
    Medium,
    High,
}

/// PDF parsing engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PdfEngine {
    MistralOcr,
    PdfText,
    Native,
}

/// Web search engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchEngine {
    Native,
    Exa,
}

/// Includable response content options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponsesIncludable {
    #[serde(rename = "file_search_call.results")]
    FileSearchCallResults,
    #[serde(rename = "message.input_image.image_url")]
    MessageInputImageImageUrl,
    #[serde(rename = "computer_call_output.output.image_url")]
    ComputerCallOutputOutputImageUrl,
    #[serde(rename = "reasoning.encrypted_content")]
    ReasoningEncryptedContent,
    #[serde(rename = "code_interpreter_call.outputs")]
    CodeInterpreterCallOutputs,
}

/// Image detail level for image inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Auto,
    High,
    Low,
}

mod strings {
    crate::named_unit_variant!(auto);
    crate::named_unit_variant!(none);
    crate::named_unit_variant!(required);
}

/// Tool choice union - specifies how the model should use tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Force a specific function call.
    Function { name: String },

    /// Force web search tool.
    WebSearchPreview,

    /// Force web search tool (2025-03-11 version).
    #[serde(rename = "web_search_preview_2025_03_11")]
    WebSearchPreview20250311,

    /// Call zero, one, or multiple tools, at the discretion of the LLM.
    #[serde(untagged, with = "strings::auto")]
    Auto,

    /// Force the LLM not to call any tools, even if any are available.
    #[serde(untagged, with = "strings::none")]
    None,

    /// Force the LLM to call at least one tool.
    #[serde(untagged, with = "strings::required")]
    Required,
}

/// Response format configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseFormat {
    /// Plain text response format.
    #[serde(rename = "text")]
    Text,
    /// JSON object response format.
    #[serde(rename = "json_object")]
    JsonObject,
    /// JSON schema response format with structured output.
    #[serde(rename = "json_schema")]
    JsonSchema {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        schema: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
}

/// Text output configuration including format and verbosity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseText {
    /// Text response format configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ResponseFormat>,
    /// Verbosity level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<ResponseTextVerbosity>,
}

/// Configuration for reasoning mode in the response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// The effort level for reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
    /// Summary verbosity level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummaryVerbosity>,
}

/// User location for web search context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchUserLocation {
    /// Always "approximate".
    #[serde(rename = "type")]
    pub type_: WebSearchUserLocationType,
    /// City name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// Country name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Region/state name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Timezone string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchUserLocationType {
    Approximate,
}

/// Filters for web search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchFilters {
    /// Allowed domains for search results.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_domains: Vec<String>,
}

/// Web search tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchTool {
    /// The type of tool.
    #[serde(rename = "type")]
    pub type_: WebSearchToolType,
    /// Size of the search context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_context_size: Option<SearchContextSize>,
    /// User location for search context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_location: Option<WebSearchUserLocation>,
    /// Domain filters for search results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<WebSearchFilters>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebSearchToolType {
    #[serde(rename = "web_search_preview")]
    WebSearchPreview,
    #[serde(rename = "web_search_preview_2025_03_11")]
    WebSearchPreview20250311,
    #[serde(rename = "web_search")]
    WebSearch,
    #[serde(rename = "web_search_2025_08_26")]
    WebSearch20250826,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self {
            type_: WebSearchToolType::WebSearchPreview,
            search_context_size: None,
            user_location: None,
            filters: None,
        }
    }
}

/// Function tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionToolDefinition {
    /// Name of the function.
    pub name: String,
    /// Description of what the function does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether to enforce strict schema validation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// JSON Schema for the function parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// Tool union type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Tool {
    /// Function tool.
    #[serde(rename = "function")]
    Function {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<serde_json::Value>,
    },
    /// Web search preview tool.
    #[serde(rename = "web_search_preview")]
    WebSearchPreview {
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<SearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<WebSearchUserLocation>,
    },
    /// Web search preview tool (2025-03-11 version).
    #[serde(rename = "web_search_preview_2025_03_11")]
    WebSearchPreview20250311 {
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<SearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<WebSearchUserLocation>,
    },
    /// Web search tool.
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<SearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<WebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<WebSearchFilters>,
    },
    /// Web search tool (2025-08-26 version).
    #[serde(rename = "web_search_2025_08_26")]
    WebSearch20250826 {
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<SearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<WebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<WebSearchFilters>,
    },
}

/// Maximum price configuration for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MaxPrice {
    /// Max price per million prompt tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<serde_json::Value>,
    /// Max price per million completion tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<serde_json::Value>,
    /// Max price per image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<serde_json::Value>,
    /// Max price per audio second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<serde_json::Value>,
    /// Max price per request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
}

/// Provider routing configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Provider {
    /// Whether to allow backup providers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    /// Whether to filter providers to those supporting all parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    /// Data collection preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<DataCollection>,
    /// Whether to restrict to Zero Data Retention endpoints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    /// Whether to restrict to models that allow text distillation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforce_distillable_text: Option<bool>,
    /// Ordered list of preferred providers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// List of allowed providers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// List of providers to ignore.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// Filter by quantization levels.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quantizations: Vec<Quantization>,
    /// Sorting strategy for provider selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<ProviderSort>,
    /// Maximum price configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<MaxPrice>,
}

/// PDF configuration for file parser plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfConfig {
    /// PDF parsing engine to use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<PdfEngine>,
}

/// Plugin union type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "id")]
pub enum Plugin {
    /// File parser plugin.
    #[serde(rename = "file-parser")]
    FileParser {
        #[serde(skip_serializing_if = "Option::is_none")]
        max_files: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pdf: Option<PdfConfig>,
    },
    /// Web plugin.
    #[serde(rename = "web")]
    Web {
        #[serde(skip_serializing_if = "Option::is_none")]
        max_results: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        engine: Option<WebSearchEngine>,
    },
    /// Moderation plugin.
    #[serde(rename = "moderation")]
    Moderation,
}

/// Prompt template reference with variables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    /// The prompt template ID.
    pub id: String,
    /// Variables to substitute into the template.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, PromptVariable>,
}

/// Prompt variable - either a structured type or a plain string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptVariable {
    /// Plain string value.
    InputText { text: String },

    /// Image input.
    InputImage {
        detail: ImageDetail,
        #[serde(skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
    },
    /// File input.
    InputFile {
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
    },

    /// Plain string value.
    #[serde(untagged)]
    String(String),
}

/// Role for input messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputMessageRole {
    User,
    Assistant,
    System,
    Developer,
}

/// Content item for input messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContentPart {
    /// Text input.
    InputText { text: String },

    /// Image input.
    InputImage {
        detail: ImageDetail,
        #[serde(skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
    },

    /// Audio input.
    InputAudio { data: String, format: String },

    /// File input.
    InputFile {
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
    },
}

/// Message content - either a string or array of content parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content.
    Text(String),

    /// Array of content parts.
    Parts(Vec<InputContentPart>),
}

/// Status for tool calls and items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Incomplete,
}

/// Input item for array-style input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    /// Input message item.
    Message {
        role: InputMessageRole,
        content: MessageContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },

    /// Function call item (from assistant).
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Function call output (user providing result).
    FunctionCallOutput {
        call_id: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },
    /// File search call item.
    FileSearchCall {
        id: String,
        queries: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<serde_json::Value>,
    },
    /// Web search call item.
    WebSearchCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },
    /// Reasoning item.
    Reasoning {
        id: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        summary: Vec<ReasoningSummaryText>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },
    /// Image generation call item.
    ImageGenerationCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
}

/// Input for a response request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Input {
    /// Simple text input.
    Text(String),
    /// Array of input items.
    Items(Vec<InputItem>),
}

/// Request schema for Responses endpoint.
///
/// This is the main request type for making requests to Openrouter's Responses API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenResponsesRequest {
    /// The model to use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Alternative models to try if the primary is unavailable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Input for the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Input>,
    /// System instructions for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Metadata key-value pairs.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Tools available to the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// How the model should use tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Whether to allow parallel tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Text output configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ResponseText>,
    /// Reasoning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// Maximum output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Top-p sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Top-k sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Prompt cache key for caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// Previous response ID for continuation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Prompt template reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Prompt>,
    /// Content to include in the response.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<ResponsesIncludable>,
    /// Whether to run in background mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Safety identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,
    /// Whether to store the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// Service tier for request priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    /// Truncation strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    /// Whether to stream the response.
    #[serde(default)]
    pub stream: bool,
    /// Provider routing configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    /// Plugins to enable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<Plugin>,
    /// User identifier for abuse tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl OpenResponsesRequest {
    /// Create a new request with the given model and input.
    #[must_use]
    pub fn new(model: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
            input: Some(Input::Text(input.into())),
            models: vec![],
            instructions: None,
            metadata: HashMap::new(),
            tools: vec![],
            tool_choice: None,
            parallel_tool_calls: None,
            text: None,
            reasoning: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            prompt_cache_key: None,
            previous_response_id: None,
            prompt: None,
            include: vec![],
            background: None,
            safety_identifier: None,
            store: None,
            service_tier: None,
            truncation: None,
            stream: false,
            provider: None,
            plugins: vec![],
            user: None,
        }
    }

    /// Set the input.
    #[must_use]
    pub fn with_input(mut self, input: Input) -> Self {
        self.input = Some(input);
        self
    }

    /// Set the model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the temperature.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set the maximum output tokens.
    #[must_use]
    pub fn with_max_output_tokens(mut self, max_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_tokens);
        self
    }

    /// Set the tools.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Enable streaming.
    #[must_use]
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Set system instructions.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Set provider routing preferences.
    #[must_use]
    pub fn with_provider(mut self, provider: Provider) -> Self {
        self.provider = Some(provider);
        self
    }
}

/// Log probability information for a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogProbs {
    /// The log probability of this token.
    pub logprob: f64,
    /// The token string.
    pub token: String,
    /// Top log probabilities for alternative tokens.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_logprobs: Vec<TopLogProbs>,
}

/// Top log probability entry for an alternative token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopLogProbs {
    /// The log probability.
    pub logprob: f64,
    /// The token string.
    pub token: String,
}

/// Union of annotation types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Annotation {
    /// URL citation annotation.
    #[serde(rename = "url_citation")]
    UrlCitation {
        start_index: u32,
        end_index: u32,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// File citation annotation.
    #[serde(rename = "file_citation")]
    FileCitation {
        start_index: u32,
        end_index: u32,
        file_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
    },
    /// File path annotation.
    #[serde(rename = "file_path")]
    FilePath {
        start_index: u32,
        end_index: u32,
        file_id: String,
    },
}

/// Union of content part types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    /// Text output content part.
    #[serde(rename = "output_text")]
    OutputText {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annotations: Vec<Annotation>,
    },
    /// Reasoning text content part.
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    /// Refusal content part.
    #[serde(rename = "refusal")]
    Refusal { refusal: String },
}

/// Reasoning summary text part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningSummaryText {
    /// The summary text.
    pub text: String,
}

/// Union of output item types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    /// Output message item.
    Message {
        id: String,
        role: String,
        content: Vec<ContentPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Function call output item.
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// File search call output item.
    FileSearchCall {
        id: String,
        queries: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<serde_json::Value>,
    },

    /// Reasoning output item.
    Reasoning {
        id: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        summary: Vec<ReasoningSummaryText>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Web search call output item.
    WebSearchCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Image generation call output item.
    ImageGenerationCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
}

/// Input tokens details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTokensDetails {
    /// Number of cached tokens.
    pub cached_tokens: u32,
}

/// Output tokens details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// Number of reasoning tokens.
    pub reasoning_tokens: u32,
}

/// Cost details breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostDetails {
    /// Total upstream inference cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_inference_cost: Option<f64>,
    /// Input cost.
    pub upstream_inference_input_cost: f64,
    /// Output cost.
    pub upstream_inference_output_cost: f64,
}

/// Token usage information for the response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Number of input tokens.
    pub input_tokens: u32,
    /// Details about input tokens.
    pub input_tokens_details: InputTokensDetails,
    /// Number of output tokens.
    pub output_tokens: u32,
    /// Details about output tokens.
    pub output_tokens_details: OutputTokensDetails,
    /// Total tokens used.
    pub total_tokens: u32,
    /// Total cost of the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Whether request used Bring Your Own Key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_byok: Option<bool>,
    /// Detailed cost breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_details: Option<CostDetails>,
}

/// Error information from the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseError {
    /// Error code.
    pub code: Option<String>,
    /// Error message.
    pub message: String,
    /// Parameter that caused the error.
    pub param: Option<String>,
}

/// Details about why a response is incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncompleteDetails {
    /// The reason the response is incomplete.
    pub reason: String,
}

/// Response status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Incomplete,
    Failed,
    Queued,
}

/// Complete non-streaming response from the Responses API.
///
/// This is embedded in lifecycle events like `response.created`, `response.completed`, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Unique identifier for the response.
    pub id: String,
    /// Object type, always "response".
    pub object: String,
    /// Unix timestamp of creation.
    pub created_at: u64,
    /// The model used.
    pub model: String,
    /// Response status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ResponseStatus>,
    /// Output items.
    #[serde(default)]
    pub output: Vec<OutputItem>,
    /// User identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Concatenated output text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    /// Error information if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    /// Details if response is incomplete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_details: Option<IncompleteDetails>,
    /// Token usage information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Temperature used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Top-p value used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Maximum output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Previous response ID if continuing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Metadata key-value pairs.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Tools available in the request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    /// Tool choice setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// Whether parallel tool calls are enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

/// Union of all possible event types emitted during response streaming.
///
/// This enum represents all Server-Sent Events that can be received when streaming
/// a response from the Openrouter Responses API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated {
        response: Response,
        sequence_number: u32,
    },

    #[serde(rename = "response.in_progress")]
    ResponseInProgress {
        response: Response,
        sequence_number: u32,
    },

    #[serde(rename = "response.completed")]
    ResponseCompleted {
        response: Response,
        sequence_number: u32,
    },

    #[serde(rename = "response.incomplete")]
    ResponseIncomplete {
        response: Response,
        sequence_number: u32,
    },

    #[serde(rename = "response.failed")]
    ResponseFailed {
        response: Response,
        sequence_number: u32,
    },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: u32,
        item: OutputItem,
        sequence_number: u32,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: u32,
        item: OutputItem,
        sequence_number: u32,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
        sequence_number: u32,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
        sequence_number: u32,
    },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
        #[serde(default)]
        logprobs: Vec<LogProbs>,
        sequence_number: u32,
    },

    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
        #[serde(default)]
        logprobs: Vec<LogProbs>,
        sequence_number: u32,
    },

    #[serde(rename = "response.output_text.annotation.added")]
    OutputTextAnnotationAdded {
        item_id: String,
        output_index: u32,
        content_index: u32,
        annotation_index: u32,
        annotation: Annotation,
        sequence_number: u32,
    },

    #[serde(rename = "response.refusal.delta")]
    RefusalDelta {
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.refusal.done")]
    RefusalDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        refusal: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        item_id: String,
        output_index: u32,
        delta: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        item_id: String,
        output_index: u32,
        name: String,
        arguments: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning.delta")]
    ReasoningDelta {
        output_index: u32,
        item_id: String,
        content_index: u32,
        delta: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning.done")]
    ReasoningDone {
        output_index: u32,
        item_id: String,
        content_index: u32,
        text: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded {
        output_index: u32,
        item_id: String,
        summary_index: u32,
        part: ReasoningSummaryText,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        output_index: u32,
        item_id: String,
        summary_index: u32,
        part: ReasoningSummaryText,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        output_index: u32,
        item_id: String,
        summary_index: u32,
        delta: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone {
        output_index: u32,
        item_id: String,
        summary_index: u32,
        text: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.image_generation_call.in_progress")]
    ImageGenCallInProgress {
        output_index: u32,
        item_id: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.image_generation_call.generating")]
    ImageGenCallGenerating {
        output_index: u32,
        item_id: String,
        sequence_number: u32,
    },

    #[serde(rename = "response.image_generation_call.partial_image")]
    ImageGenCallPartialImage {
        output_index: u32,
        item_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        partial_image_b64: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        partial_image_index: Option<u32>,
        sequence_number: u32,
    },

    #[serde(rename = "response.image_generation_call.completed")]
    ImageGenCallCompleted {
        output_index: u32,
        item_id: String,
        sequence_number: u32,
    },

    #[serde(rename = "error")]
    Error {
        code: Option<String>,
        message: String,
        param: Option<String>,
        sequence_number: u32,
    },
}

impl StreamEvent {
    /// Returns the sequence number from any event variant.
    #[must_use]
    pub fn sequence_number(&self) -> u32 {
        use StreamEvent::*;

        match self {
            ResponseCreated {
                sequence_number, ..
            }
            | ResponseInProgress {
                sequence_number, ..
            }
            | ResponseCompleted {
                sequence_number, ..
            }
            | ResponseIncomplete {
                sequence_number, ..
            }
            | ResponseFailed {
                sequence_number, ..
            }
            | OutputItemAdded {
                sequence_number, ..
            }
            | OutputItemDone {
                sequence_number, ..
            }
            | ContentPartAdded {
                sequence_number, ..
            }
            | ContentPartDone {
                sequence_number, ..
            }
            | OutputTextDelta {
                sequence_number, ..
            }
            | OutputTextDone {
                sequence_number, ..
            }
            | OutputTextAnnotationAdded {
                sequence_number, ..
            }
            | RefusalDelta {
                sequence_number, ..
            }
            | RefusalDone {
                sequence_number, ..
            }
            | FunctionCallArgumentsDelta {
                sequence_number, ..
            }
            | FunctionCallArgumentsDone {
                sequence_number, ..
            }
            | ReasoningDelta {
                sequence_number, ..
            }
            | ReasoningDone {
                sequence_number, ..
            }
            | ReasoningSummaryPartAdded {
                sequence_number, ..
            }
            | ReasoningSummaryPartDone {
                sequence_number, ..
            }
            | ReasoningSummaryTextDelta {
                sequence_number, ..
            }
            | ReasoningSummaryTextDone {
                sequence_number, ..
            }
            | ImageGenCallInProgress {
                sequence_number, ..
            }
            | ImageGenCallGenerating {
                sequence_number, ..
            }
            | ImageGenCallPartialImage {
                sequence_number, ..
            }
            | ImageGenCallCompleted {
                sequence_number, ..
            }
            | Error {
                sequence_number, ..
            } => *sequence_number,
        }
    }

    /// Returns the event type as a string.
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            StreamEvent::ResponseCreated { .. } => "response.created",
            StreamEvent::ResponseInProgress { .. } => "response.in_progress",
            StreamEvent::ResponseCompleted { .. } => "response.completed",
            StreamEvent::ResponseIncomplete { .. } => "response.incomplete",
            StreamEvent::ResponseFailed { .. } => "response.failed",
            StreamEvent::OutputItemAdded { .. } => "response.output_item.added",
            StreamEvent::OutputItemDone { .. } => "response.output_item.done",
            StreamEvent::ContentPartAdded { .. } => "response.content_part.added",
            StreamEvent::ContentPartDone { .. } => "response.content_part.done",
            StreamEvent::OutputTextDelta { .. } => "response.output_text.delta",
            StreamEvent::OutputTextDone { .. } => "response.output_text.done",
            StreamEvent::OutputTextAnnotationAdded { .. } => {
                "response.output_text.annotation.added"
            }
            StreamEvent::RefusalDelta { .. } => "response.refusal.delta",
            StreamEvent::RefusalDone { .. } => "response.refusal.done",
            StreamEvent::FunctionCallArgumentsDelta { .. } => {
                "response.function_call_arguments.delta"
            }
            StreamEvent::FunctionCallArgumentsDone { .. } => {
                "response.function_call_arguments.done"
            }
            StreamEvent::ReasoningDelta { .. } => "response.reasoning.delta",
            StreamEvent::ReasoningDone { .. } => "response.reasoning.done",
            StreamEvent::ReasoningSummaryPartAdded { .. } => {
                "response.reasoning_summary_part.added"
            }
            StreamEvent::ReasoningSummaryPartDone { .. } => "response.reasoning_summary_part.done",
            StreamEvent::ReasoningSummaryTextDelta { .. } => {
                "response.reasoning_summary_text.delta"
            }
            StreamEvent::ReasoningSummaryTextDone { .. } => "response.reasoning_summary_text.done",
            StreamEvent::ImageGenCallInProgress { .. } => {
                "response.image_generation_call.in_progress"
            }
            StreamEvent::ImageGenCallGenerating { .. } => {
                "response.image_generation_call.generating"
            }
            StreamEvent::ImageGenCallPartialImage { .. } => {
                "response.image_generation_call.partial_image"
            }
            StreamEvent::ImageGenCallCompleted { .. } => "response.image_generation_call.completed",
            StreamEvent::Error { .. } => "error",
        }
    }

    /// Returns `true` if this is a terminal event (response completed, failed, or incomplete).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StreamEvent::ResponseCompleted { .. }
                | StreamEvent::ResponseFailed { .. }
                | StreamEvent::ResponseIncomplete { .. }
        )
    }

    /// Returns `true` if this is an error event.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            StreamEvent::Error { .. } | StreamEvent::ResponseFailed { .. }
        )
    }

    /// Extracts the text delta if this is an [`OutputTextDelta`](StreamEvent::OutputTextDelta) event.
    #[must_use]
    pub fn as_text_delta(&self) -> Option<&str> {
        match self {
            StreamEvent::OutputTextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn test_simple_request_serialization() {
        let request = OpenResponsesRequest::new("gpt-4", "Hello, world!");

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4"));
        assert!(json.contains("Hello, world!"));
    }

    #[test]
    fn test_request_with_tools() {
        let request = OpenResponsesRequest::new("gpt-4", "Search for Rust").with_tools(vec![
            Tool::WebSearchPreview {
                search_context_size: None,
                user_location: None,
            },
        ]);

        let json = serde_json::to_string_pretty(&request).unwrap();
        assert!(json.contains("web_search_preview"));
    }

    #[test]
    fn test_function_tool() {
        let tool = Tool::Function {
            name: "get_weather".to_string(),
            description: Some("Get the current weather".to_string()),
            strict: Some(true),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                },
                "required": ["location"]
            })),
        };

        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("get_weather"));
        assert!(json.contains("function"));
    }

    #[test]
    fn test_provider_config() {
        let provider = Provider {
            allow_fallbacks: Some(true),
            data_collection: Some(DataCollection::Deny),
            sort: Some(ProviderSort::Price),
            ..Default::default()
        };

        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("allow_fallbacks"));
        assert!(json.contains("deny"));
        assert!(json.contains("price"));
    }

    #[test]
    fn test_tool_choice_variants() {
        // Test mode variant
        let auto = ToolChoice::Auto;
        let json = serde_json::to_string(&auto).unwrap();
        assert_eq!(json, "\"auto\"");

        // Test function variant
        let func = ToolChoice::Function {
            name: "my_function".to_string(),
        };
        let json = serde_json::to_string(&func).unwrap();
        assert!(json.contains("my_function"));
        assert!(json.contains("function"));
    }

    #[test]
    fn test_text_delta_event_deserialization() {
        let json = r#"{
            "type": "response.output_text.delta",
            "output_index": 0,
            "item_id": "item_123",
            "content_index": 0,
            "delta": "Hello",
            "logprobs": [],
            "sequence_number": 1
        }"#;

        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, StreamEvent::OutputTextDelta { .. }));
        assert_eq!(event.as_text_delta(), Some("Hello"));
        assert_eq!(event.sequence_number(), 1);
    }

    #[test]
    fn test_response_completed_event() {
        let json = r#"{
            "type": "response.completed",
            "response": {
                "id": "resp_123",
                "object": "response",
                "created_at": 1234567890,
                "model": "gpt-4",
                "output": []
            },
            "sequence_number": 10
        }"#;

        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_terminal());
        assert!(!event.is_error());
        assert_eq!(event.event_type(), "response.completed");
    }

    #[test]
    fn test_error_event() {
        let json = r#"{
            "type": "error",
            "code": "rate_limit_exceeded",
            "message": "Too many requests",
            "param": null,
            "sequence_number": 5
        }"#;

        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_error());
        assert_eq!(event.event_type(), "error");
    }

    #[test]
    fn test_function_call_arguments_done() {
        let json = r#"{
            "type": "response.function_call_arguments.done",
            "item_id": "item_456",
            "output_index": 0,
            "name": "get_weather",
            "arguments": "{\"location\": \"London\"}",
            "sequence_number": 3
        }"#;

        let event: StreamEvent = serde_json::from_str(json).unwrap();
        match event {
            StreamEvent::FunctionCallArgumentsDone {
                name, arguments, ..
            } => {
                assert_eq!(name, "get_weather");
                assert!(arguments.contains("London"));
            }
            _ => panic!("Expected FunctionCallArgumentsDone"),
        }
    }

    #[test]
    fn test_response_format_variants() {
        let text = ResponseFormat::Text;
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));

        let json_obj = ResponseFormat::JsonObject;
        let json = serde_json::to_string(&json_obj).unwrap();
        assert!(json.contains("\"type\":\"json_object\""));

        let json_schema = ResponseFormat::JsonSchema {
            name: "person".to_string(),
            description: Some("A person".to_string()),
            schema: serde_json::json!({"type": "object"}),
            strict: Some(true),
        };
        let json = serde_json::to_string(&json_schema).unwrap();
        assert!(json.contains("\"type\":\"json_schema\""));
        assert!(json.contains("\"name\":\"person\""));
    }

    #[test]
    fn test_plugin_variants() {
        let moderation = Plugin::Moderation;
        let json = serde_json::to_string(&moderation).unwrap();
        assert!(json.contains("\"id\":\"moderation\""));

        let web = Plugin::Web {
            max_results: Some(10),
            search_prompt: None,
            engine: Some(WebSearchEngine::Exa),
        };
        let json = serde_json::to_string(&web).unwrap();
        assert!(json.contains("\"id\":\"web\""));
        assert!(json.contains("\"max_results\":10"));
    }
}
