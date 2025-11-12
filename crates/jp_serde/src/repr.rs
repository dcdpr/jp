pub mod base64_string {
    use std::fmt::Display;

    use base64::{Engine, engine::general_purpose::URL_SAFE};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, T: AsRef<str>>(
        item: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE.encode(item.as_ref().as_bytes()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: TryFrom<String, Error: Display>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        T::try_from(
            String::from_utf8(
                URL_SAFE
                    .decode(String::deserialize(deserializer)?)
                    .map_err(serde::de::Error::custom)?,
            )
            .map_err(serde::de::Error::custom)?,
        )
        .map_err(serde::de::Error::custom)
    }
}

pub mod base64_json_map {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    pub fn serialize<S>(map: &Map<String, Value>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded: Map<String, Value> = map
            .iter()
            .map(|(k, v)| (k.clone(), encode_strings(v)))
            .collect();
        encoded.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Map<String, Value>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        map.iter()
            .map(|(k, v)| decode_strings(v).map(|decoded| (k.clone(), decoded)))
            .collect::<Result<_, _>>()
            .map_err(serde::de::Error::custom)
    }

    fn encode_strings(value: &Value) -> Value {
        match value {
            Value::String(s) => STANDARD.encode(s).into(),
            Value::Array(arr) => arr.iter().map(encode_strings).collect(),
            Value::Object(obj) => obj
                .iter()
                .map(|(k, v)| (k.clone(), encode_strings(v)))
                .collect(),
            _ => value.clone(),
        }
    }

    fn decode_strings(value: &Value) -> Result<Value, String> {
        match value {
            Value::String(s) => {
                let decoded = STANDARD
                    .decode(s)
                    .map_err(|e| format!("Base64 decode error: {e}"))?;

                String::from_utf8(decoded)
                    .map_err(|e| format!("UTF-8 decode error: {e}"))
                    .map(Value::String)
            }
            Value::Array(arr) => arr
                .iter()
                .map(decode_strings)
                .collect::<Result<_, _>>()
                .map(Value::Array),
            Value::Object(obj) => obj
                .iter()
                .map(|(k, v)| decode_strings(v).map(|decoded| (k.clone(), decoded)))
                .collect(),
            _ => Ok(value.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value, json};

    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestString(#[serde(with = "base64_string")] String);

    #[test]
    fn test_base64_string() {
        let serialized = serde_json::to_string(&TestString("hello world".to_string())).unwrap();
        assert_eq!(serialized, r#""aGVsbG8gd29ybGQ=""#);

        let deserialized: TestString = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.0, "hello world");
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestJsonMap(#[serde(with = "base64_json_map")] Map<String, Value>);

    #[test]
    fn test_base64_json_map() {
        let serialized = serde_json::to_value(TestJsonMap(
            json!({"key": "value"}).as_object().unwrap().clone(),
        ))
        .unwrap();

        assert_eq!(serialized, json!({"key": "dmFsdWU="}));

        let deserialized: TestJsonMap = serde_json::from_value(serialized).unwrap();
        assert_eq!(
            deserialized.0,
            json!({"key": "value"}).as_object().unwrap().clone()
        );
    }
}
