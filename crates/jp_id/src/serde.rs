use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use time::UtcDateTime;

use crate::NANOSECONDS_PER_DECISECOND;

/// Serialize an `UtcDateTime` as its Unix timestamp with deciseconds.
pub fn serialize<S: Serializer>(
    datetime: &UtcDateTime,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let timestamp = datetime.unix_timestamp_nanos() / i128::from(NANOSECONDS_PER_DECISECOND);
    timestamp.serialize(serializer)
}

/// Deserialize an `UtcDateTime` from its Unix timestamp with deciseconds.
pub fn deserialize<'a, D: Deserializer<'a>>(
    deserializer: D,
) -> std::result::Result<UtcDateTime, D::Error> {
    let num = i128::deserialize(deserializer)?;
    UtcDateTime::from_unix_timestamp_nanos(num * i128::from(NANOSECONDS_PER_DECISECOND))
        .map_err(de::Error::custom)
}
