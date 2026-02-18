use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::NANOSECONDS_PER_DECISECOND;

/// Serialize a `DateTime<Utc>` as its Unix timestamp with deciseconds.
pub fn serialize<S: Serializer>(
    datetime: &DateTime<Utc>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let nanos = datetime.timestamp_nanos_opt().ok_or_else(|| {
        serde::ser::Error::custom("timestamp out of range for nanosecond precision")
    })?;
    let timestamp = nanos / i64::from(NANOSECONDS_PER_DECISECOND);
    timestamp.serialize(serializer)
}

/// Deserialize a `DateTime<Utc>` from its Unix timestamp with deciseconds.
pub fn deserialize<'a, D: Deserializer<'a>>(
    deserializer: D,
) -> std::result::Result<DateTime<Utc>, D::Error> {
    let num = i64::deserialize(deserializer)?;
    let nanos = num
        .checked_mul(i64::from(NANOSECONDS_PER_DECISECOND))
        .ok_or_else(|| de::Error::custom("decisecond timestamp overflow"))?;
    Ok(DateTime::from_timestamp_nanos(nanos))
}
