pub(crate) mod http;

pub(crate) use self::http::HttpTransport;

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// HTTP method for transport requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
    Patch,
}

/// An abstract transport layer for communicating with an OJS server.
///
/// This trait is object-safe and uses `Pin<Box<dyn Future>>` for async support.
/// The default implementation uses reqwest (enabled via the `reqwest-transport` feature).
///
/// Implement this trait to provide custom transports (e.g., in-memory for
/// testing, gRPC, or other HTTP clients).
///
/// # Example
///
/// ```rust,no_run
/// use ojs::transport::{Transport, Method};
/// use std::fmt::Debug;
/// use std::pin::Pin;
///
/// #[derive(Debug)]
/// struct MyTransport;
///
/// impl Transport for MyTransport {
///     fn request(
///         &self,
///         method: Method,
///         path: &str,
///         body: Option<serde_json::Value>,
///         raw_path: bool,
///     ) -> Pin<Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>> {
///         Box::pin(async move {
///             // your implementation here
///             Ok(None)
///         })
///     }
/// }
/// ```
pub trait Transport: Send + Sync + Debug {
    /// Send a request with the given method, path, and optional JSON body.
    ///
    /// - `path` is relative to the OJS base path (e.g., `/jobs`, `/workers/fetch`).
    /// - `raw_path` indicates the path should not be prefixed with `/ojs/v1`.
    /// - Returns `Ok(Some(value))` for responses with a body, `Ok(None)` for empty responses.
    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        raw_path: bool,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Option<serde_json::Value>>> + Send + '_>>;
}

/// A cloneable, type-erased transport handle.
pub type DynTransport = Arc<dyn Transport>;

// ---------------------------------------------------------------------------
// Typed helper functions for working with DynTransport
// ---------------------------------------------------------------------------

pub(crate) async fn transport_get<T: serde::de::DeserializeOwned>(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<T> {
    let value = transport.request(Method::Get, path, None, false).await?;
    deserialize_response(value)
}

pub(crate) async fn transport_get_raw<T: serde::de::DeserializeOwned>(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<T> {
    let value = transport.request(Method::Get, path, None, true).await?;
    deserialize_response(value)
}

pub(crate) async fn transport_post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    transport: &DynTransport,
    path: &str,
    body: &B,
) -> crate::Result<T> {
    let body = serde_json::to_value(body)?;
    let value = transport
        .request(Method::Post, path, Some(body), false)
        .await?;
    deserialize_response(value)
}

pub(crate) async fn transport_post_no_response<B: serde::Serialize>(
    transport: &DynTransport,
    path: &str,
    body: &B,
) -> crate::Result<()> {
    let body = serde_json::to_value(body)?;
    transport
        .request(Method::Post, path, Some(body), false)
        .await?;
    Ok(())
}

pub(crate) async fn transport_post_empty<T: serde::de::DeserializeOwned>(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<T> {
    let value = transport.request(Method::Post, path, None, false).await?;
    deserialize_response(value)
}

pub(crate) async fn transport_post_empty_no_response(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<()> {
    transport.request(Method::Post, path, None, false).await?;
    Ok(())
}

pub(crate) async fn transport_delete<T: serde::de::DeserializeOwned>(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<T> {
    let value = transport.request(Method::Delete, path, None, false).await?;
    deserialize_response(value)
}

pub(crate) async fn transport_delete_no_response(
    transport: &DynTransport,
    path: &str,
) -> crate::Result<()> {
    transport.request(Method::Delete, path, None, false).await?;
    Ok(())
}

fn deserialize_response<T: serde::de::DeserializeOwned>(
    value: Option<serde_json::Value>,
) -> crate::Result<T> {
    match value {
        Some(v) => serde_json::from_value(v).map_err(|e| {
            crate::OjsError::Serialization(format!("failed to parse response: {}", e))
        }),
        None => Err(crate::OjsError::Serialization(
            "empty response body".to_string(),
        )),
    }
}
