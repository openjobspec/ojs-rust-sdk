use serde::{Deserialize, Serialize};

/// A registered schema (summary, without the schema body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    /// Schema URI identifier.
    pub uri: String,
    /// Schema type (e.g., "json-schema").
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Schema version.
    pub version: String,
    /// When the schema was created.
    pub created_at: String,
}

/// A registered schema with the full schema body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDetail {
    /// Schema URI identifier.
    pub uri: String,
    /// Schema type (e.g., "json-schema").
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Schema version.
    pub version: String,
    /// When the schema was created.
    pub created_at: String,
    /// The schema definition.
    pub schema: serde_json::Value,
}

/// Request body for registering a schema.
#[derive(Debug, Serialize)]
pub struct RegisterSchemaRequest {
    /// Schema URI identifier.
    pub uri: String,
    /// Schema type (e.g., "json-schema").
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Schema version.
    pub version: String,
    /// The schema definition.
    pub schema: serde_json::Value,
}

/// Response wrapper for schema list endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct SchemasResponse {
    pub schemas: Vec<Schema>,
}
