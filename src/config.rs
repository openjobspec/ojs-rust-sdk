use std::collections::HashMap;
use std::time::Duration;

/// Shared connection configuration for OJS clients and workers.
///
/// Use this to share connection settings between a [`Client`](crate::Client)
/// and a [`Worker`](crate::Worker) connecting to the same server.
///
/// # Example
///
/// ```rust
/// use ojs::ConnectionConfig;
///
/// let config = ConnectionConfig::new("http://localhost:8080")
///     .auth_token("my-token")
///     .header("X-Tenant-Id", "tenant-42")
///     .timeout(std::time::Duration::from_secs(10));
///
/// let client = ojs::Client::builder()
///     .connection(config.clone())
///     .build()
///     .unwrap();
///
/// let worker = ojs::Worker::builder()
///     .connection(config)
///     .queues(vec!["default"])
///     .build()
///     .unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// OJS server URL.
    pub url: String,
    /// Authentication bearer token.
    pub auth_token: Option<String>,
    /// Custom HTTP headers.
    pub headers: HashMap<String, String>,
    /// Request timeout.
    pub timeout: Option<Duration>,
}

impl ConnectionConfig {
    /// Create a new connection config with the given server URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            headers: HashMap::new(),
            timeout: None,
        }
    }

    /// Set the authentication bearer token.
    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Add a custom HTTP header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the request timeout. Defaults to 30 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}
