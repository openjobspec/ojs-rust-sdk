use ojs::{RegisterSchemaRequest, Schema, SchemaDetail};
use serde_json::json;

// ---------------------------------------------------------------------------
// Schema tests
// ---------------------------------------------------------------------------

#[test]
fn test_schema_serialization_roundtrip() {
    let schema = Schema {
        uri: "urn:ojs:schema:email.send".into(),
        schema_type: "json-schema".into(),
        version: "1.0.0".into(),
        created_at: "2025-01-01T00:00:00Z".into(),
    };

    let json_str = serde_json::to_string(&schema).unwrap();
    let deserialized: Schema = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.uri, "urn:ojs:schema:email.send");
    assert_eq!(deserialized.schema_type, "json-schema");
    assert_eq!(deserialized.version, "1.0.0");
    assert_eq!(deserialized.created_at, "2025-01-01T00:00:00Z");
}

#[test]
fn test_schema_deserialization_from_json() {
    let json = json!({
        "uri": "urn:ojs:schema:report.generate",
        "type": "json-schema",
        "version": "2.0.0",
        "created_at": "2025-06-15T10:30:00Z"
    });

    let schema: Schema = serde_json::from_value(json).unwrap();
    assert_eq!(schema.uri, "urn:ojs:schema:report.generate");
    assert_eq!(schema.schema_type, "json-schema");
    assert_eq!(schema.version, "2.0.0");
}

// ---------------------------------------------------------------------------
// SchemaDetail tests
// ---------------------------------------------------------------------------

#[test]
fn test_schema_detail_with_all_fields() {
    let json = json!({
        "uri": "urn:ojs:schema:email.send",
        "type": "json-schema",
        "version": "1.0.0",
        "created_at": "2025-01-01T00:00:00Z",
        "schema": {
            "type": "object",
            "properties": {
                "to": {"type": "string", "format": "email"},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            },
            "required": ["to", "subject"]
        }
    });

    let detail: SchemaDetail = serde_json::from_value(json).unwrap();
    assert_eq!(detail.uri, "urn:ojs:schema:email.send");
    assert_eq!(detail.schema_type, "json-schema");
    assert_eq!(detail.version, "1.0.0");
    assert!(detail.schema.is_object());
    assert!(detail.schema["properties"]["to"].is_object());
    assert_eq!(detail.schema["required"][0], "to");
    assert_eq!(detail.schema["required"][1], "subject");
}

#[test]
fn test_schema_detail_serialization_roundtrip() {
    let detail = SchemaDetail {
        uri: "urn:ojs:schema:test".into(),
        schema_type: "json-schema".into(),
        version: "0.1.0".into(),
        created_at: "2025-01-01T00:00:00Z".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            }
        }),
    };

    let json_str = serde_json::to_string(&detail).unwrap();
    let roundtrip: SchemaDetail = serde_json::from_str(&json_str).unwrap();

    assert_eq!(roundtrip.uri, "urn:ojs:schema:test");
    assert_eq!(roundtrip.version, "0.1.0");
    assert!(roundtrip.schema["properties"]["id"].is_object());
}

#[test]
fn test_schema_detail_with_minimal_schema() {
    let json = json!({
        "uri": "urn:ojs:schema:minimal",
        "type": "json-schema",
        "version": "1.0.0",
        "created_at": "2025-01-01T00:00:00Z",
        "schema": {"type": "object"}
    });

    let detail: SchemaDetail = serde_json::from_value(json).unwrap();
    assert_eq!(detail.schema["type"], "object");
}

// ---------------------------------------------------------------------------
// RegisterSchemaRequest tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_schema_request_serialization() {
    let req = RegisterSchemaRequest {
        uri: "urn:ojs:schema:payment.process".into(),
        schema_type: "json-schema".into(),
        version: "1.0.0".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "amount": {"type": "number"},
                "currency": {"type": "string"}
            },
            "required": ["amount", "currency"]
        }),
    };

    let json_str = serde_json::to_string(&req).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(value["uri"], "urn:ojs:schema:payment.process");
    assert_eq!(value["type"], "json-schema");
    assert_eq!(value["version"], "1.0.0");
    assert!(value["schema"]["properties"]["amount"].is_object());
}

#[test]
fn test_register_schema_request_with_complex_schema() {
    let req = RegisterSchemaRequest {
        uri: "urn:ojs:schema:order.create".into(),
        schema_type: "json-schema".into(),
        version: "2.0.0".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sku": {"type": "string"},
                            "quantity": {"type": "integer", "minimum": 1}
                        }
                    }
                },
                "customer_id": {"type": "string"}
            },
            "required": ["items", "customer_id"]
        }),
    };

    let json_str = serde_json::to_string(&req).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(value["schema"]["properties"]["items"]["type"], "array");
}
