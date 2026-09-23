use async_anthropic::{
    Client,
    errors::{AnthropicError, ApiError},
    types::{GetModelResponse, ListModelsResponse, ModelCapabilities},
};
use httpmock::{MockServer, prelude::GET};
use serde_json::json;

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .api_key("test_secret")
        .base_url(server.base_url())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_successful_list_models_request() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200).json_body_obj(&ListModelsResponse {
                data: vec![],
                first_id: Some("model_1".to_string()),
                has_more: false,
                last_id: Some("model_2".to_string()),
            });
        })
        .await;

    let result = client_for(&server).models().list().await.unwrap();

    mock.assert_calls_async(1).await;
    assert_eq!(result.first_id, Some("model_1".to_string()));
}

#[tokio::test]
async fn test_successful_get_model_request() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/models/model-id");
            then.status(200).json_body_obj(&GetModelResponse {
                created_at: "2023-10-10T00:00:00Z".to_string(),
                display_name: "Test Model".to_string(),
                id: "model-id".to_string(),
                model_type: "test-type".to_string(),
                max_input_tokens: 200_000,
                max_tokens: 8192,
                capabilities: ModelCapabilities::default(),
            });
        })
        .await;

    let result = client_for(&server).models().get("model-id").await.unwrap();

    mock.assert_calls_async(1).await;
    assert_eq!(result.id, "model-id");
}

#[tokio::test]
async fn test_error_handling_bad_request() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/models/model-id");
            then.status(400).json_body(json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "Bad request"
                }
            }));
        })
        .await;

    let result = client_for(&server).models().get("model-id").await;

    mock.assert_calls_async(1).await;
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
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/models/model-id");
            then.status(401).json_body(json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "Unauthorized"
                }
            }));
        })
        .await;

    let result = client_for(&server).models().get("model-id").await;

    mock.assert_calls_async(1).await;
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
