use serde_json::{Value, json};

use super::*;

#[test]
fn test_model_id_config_deserialize() {
    struct TestCase {
        data: Value,
        expected: PartialModelIdConfig,
    }

    let cases = vec![
        TestCase {
            data: json!({
                "provider": "ollama",
                "name": "bar",
            }),
            expected: PartialModelIdConfig {
                provider: Some(ProviderId::Ollama),
                name: "bar".parse().ok(),
            },
        },
        TestCase {
            data: json!("llamacpp/bar"),
            expected: PartialModelIdConfig {
                provider: Some(ProviderId::Llamacpp),
                name: "bar".parse().ok(),
            },
        },
    ];

    for TestCase { data, expected } in cases {
        let result = serde_json::from_value::<PartialModelIdConfig>(data);
        assert_eq!(result.unwrap(), expected);
    }
}
