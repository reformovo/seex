use chrono::{DateTime, Utc};

use crate::StorageError;

pub fn timestamp_as_rfc3339(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339()
}

pub fn current_timestamp(field: &'static str) -> Result<DateTime<Utc>, StorageError> {
    timestamp_from_millis(field, Utc::now().timestamp_millis())
}

pub fn timestamp_from_millis(
    field: &'static str,
    millis: i64,
) -> Result<DateTime<Utc>, StorageError> {
    DateTime::from_timestamp_millis(millis).ok_or(StorageError::InvalidTimestamp { field, millis })
}
