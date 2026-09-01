//! Event and body wire types for OJS jobs delivered to serverless
//! functions: [`JobEvent`] plus the SQS, HTTP push, and direct-invocation
//! envelope shapes. Pure data (deserialize/serialize only); no dispatch
//! logic lives here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// An OJS job delivered to a serverless function.
///
/// This is a simplified job envelope containing only the fields relevant
/// for serverless processing. It is deserialized from SQS message bodies,
/// HTTP push delivery requests, or direct invocation events.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEvent {
    /// Unique job identifier (UUIDv7).
    pub id: String,

    /// Dot-namespaced job type (e.g., `email.send`).
    #[serde(rename = "type")]
    pub job_type: String,

    /// Target queue name.
    #[serde(default = "default_queue")]
    pub queue: String,

    /// Positional job arguments.
    #[serde(default = "default_args")]
    pub args: serde_json::Value,

    /// Current attempt number.
    #[serde(default = "default_attempt")]
    pub attempt: u32,

    /// Extensible metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,

    /// Job priority.
    #[serde(default)]
    pub priority: i32,
}

fn default_queue() -> String {
    "default".to_string()
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Array(vec![])
}

fn default_attempt() -> u32 {
    1
}

/// SQS event containing one or more messages.
///
/// This mirrors the AWS SQS event structure. When using the
/// `aws_lambda_events` crate, you can use its `SqsEvent` type directly
/// and call [`LambdaHandler::handle_sqs_records`] with the records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsEvent {
    /// The SQS message records.
    #[serde(rename = "Records", default)]
    pub records: Vec<SqsMessage>,
}

/// A single SQS message containing an OJS job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsMessage {
    /// Unique SQS message identifier.
    #[serde(rename = "messageId", default)]
    pub message_id: String,

    /// The message body (JSON-encoded OJS job).
    #[serde(default)]
    pub body: String,

    /// SQS message attributes.
    #[serde(default)]
    pub attributes: HashMap<String, String>,

    /// The receipt handle for deleting the message.
    #[serde(rename = "receiptHandle", default)]
    pub receipt_handle: String,
}

/// Response format for SQS batch item failures.
///
/// Returning failed message IDs tells SQS to retry only those messages.
/// See: <https://docs.aws.amazon.com/lambda/latest/dg/with-sqs.html>
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SqsBatchResponse {
    /// List of failed message identifiers.
    #[serde(rename = "batchItemFailures")]
    pub batch_item_failures: Vec<BatchItemFailure>,
}

/// Identifies a single failed message in an SQS batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemFailure {
    /// The SQS message ID of the failed item.
    #[serde(rename = "itemIdentifier")]
    pub item_identifier: String,
}

/// HTTP push delivery request body from an OJS server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushDeliveryRequest {
    /// The job to process.
    pub job: JobEvent,

    /// Identifier of the push worker registration.
    #[serde(default)]
    pub worker_id: String,

    /// Unique delivery identifier for idempotency.
    #[serde(default)]
    pub delivery_id: String,
}

/// HTTP push delivery response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushDeliveryResponse {
    /// Processing result: `"completed"` or `"failed"`.
    pub status: String,

    /// Result data from successful processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error information if processing failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PushError>,
}

/// Describes a job processing failure in push delivery responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushError {
    /// Machine-readable error code.
    pub code: String,

    /// Human-readable error description.
    pub message: String,

    /// Whether the job should be retried.
    pub retryable: bool,
}

/// Response from direct Lambda invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectResponse {
    /// Processing result: `"completed"` or `"failed"`.
    pub status: String,

    /// The job ID that was processed.
    pub job_id: String,

    /// Error message if processing failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_job_event_deserialization() {
        let raw = r#"{"id":"j1","type":"test","queue":"q","args":[{"key":"value"}],"attempt":1,"priority":5}"#;
        let job: JobEvent = serde_json::from_str(raw).unwrap();

        assert_eq!(job.id, "j1");
        assert_eq!(job.job_type, "test");
        assert_eq!(job.queue, "q");
        assert_eq!(job.priority, 5);
        assert_eq!(job.attempt, 1);
    }

    #[test]
    fn test_job_event_defaults() {
        let raw = r#"{"id":"j1","type":"test"}"#;
        let job: JobEvent = serde_json::from_str(raw).unwrap();

        assert_eq!(job.queue, "default");
        assert_eq!(job.attempt, 1);
        assert_eq!(job.priority, 0);
        assert_eq!(job.args, json!([]));
    }
}
