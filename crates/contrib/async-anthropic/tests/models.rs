use async_anthropic::{
    Client,
    errors::{AnthropicError, ApiError},
    types::{GetModelResponse, ListModelsResponse, ModelCapabilities},
};
use async_trait::async_trait;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[async_trait]
pub trait MockApp {
    async fn setup() -> MockServer;
}

struct TestSetup;

#[async_trait]
impl MockApp for TestSetup {
    async fn setup() -> MockServer {
        MockServer::start().await
    }
}

#[tokio::test]
async fn test_successful_list_models_request() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock successful list models response
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ListModelsResponse {
                data: vec![],
                first_id: Some("model_1".to_string()),
                has_more: false,
                last_id: Some("model_2".to_string()),
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .api_key(secret_key)
        .base_url(server.uri())
        .build()
        .unwrap();

    let result = client.models().list().await.unwrap();
    assert_eq!(result.first_id, Some("model_1".to_string()));
}

#[tokio::test]
async fn test_successful_get_model_request() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock successful get model response
    Mock::given(method("GET"))
        .and(path("/v1/models/model-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&GetModelResponse {
            created_at: "2023-10-10T00:00:00Z".to_string(),
            display_name: "Test Model".to_string(),
            id: "model-id".to_string(),
            model_type: "test-type".to_string(),
            max_input_tokens: 200_000,
            max_tokens: 8192,
            capabilities: ModelCapabilities::default(),
        }))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .api_key(secret_key)
        .base_url(server.uri())
        .build()
        .unwrap();

    let result = client.models().get("model-id").await.unwrap();
    assert_eq!(result.id, "model-id");
}

#[tokio::test]
async fn test_error_handling_bad_request() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock 400 Bad Request response
    Mock::given(method("GET"))
        .and(path("/v1/models/model-id"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "Bad request"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap();

    let result = client.models().get("model-id").await;

    assert!(result.is_err());
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::Api(ApiError { error_type, .. }) if error_type == "invalid_request_error"
        ),
        "actual: {:?}",
        &result
    );
}

#[tokio::test]
async fn test_error_handling_unauthorized() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock 401 Unauthorized response
    Mock::given(method("GET"))
        .and(path("/v1/models/model-id"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "Unauthorized"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap();

    let result = client.models().get("model-id").await;

    assert!(result.is_err());
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::Auth {
                status: 401,
                error: ApiError { error_type, .. },
            } if error_type == "authentication_error"
        ),
        "actual: {:?}",
        &result
    );
}
