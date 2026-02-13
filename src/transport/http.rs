use crate::errors::{ErrorResponse, OjsError, ServerError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;

const OJS_CONTENT_TYPE: &str = "application/openjobspec+json";
const OJS_VERSION: &str = "1.0.0-rc.1";
const BASE_PATH: &str = "/ojs/v1";

/// HTTP transport layer for communicating with an OJS server.
#[derive(Clone, Debug)]
pub(crate) struct HttpTransport {
    base_url: String,
    #[cfg(feature = "reqwest-transport")]
    client: reqwest::Client,
    auth_token: Option<String>,
    headers: HashMap<String, String>,
}

/// Configuration used to construct an HttpTransport from client settings.
pub(crate) struct TransportConfig {
    pub auth_token: Option<String>,
    pub headers: HashMap<String, String>,
    #[cfg(feature = "reqwest-transport")]
    pub http_client: Option<reqwest::Client>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            headers: HashMap::new(),
            #[cfg(feature = "reqwest-transport")]
            http_client: None,
        }
    }
}

impl HttpTransport {
    pub fn new(base_url: &str, config: TransportConfig) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();

        #[cfg(feature = "reqwest-transport")]
        let client = config.http_client.unwrap_or_else(reqwest::Client::new);

        Self {
            base_url,
            #[cfg(feature = "reqwest-transport")]
            client,
            auth_token: config.auth_token,
            headers: config.headers,
        }
    }

    /// Build the full URL for a given path (relative to `/ojs/v1`).
    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base_url, BASE_PATH, path)
    }

    /// Build the full URL for a path NOT under `/ojs/v1` (e.g., `/ojs/manifest`).
    fn url_raw(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // -----------------------------------------------------------------------
    // reqwest-based implementation
    // -----------------------------------------------------------------------

    #[cfg(feature = "reqwest-transport")]
    fn apply_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req
            .header("Content-Type", OJS_CONTENT_TYPE)
            .header("Accept", OJS_CONTENT_TYPE)
            .header("OJS-Version", OJS_VERSION);

        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        for (key, value) in &self.headers {
            req = req.header(key.as_str(), value.as_str());
        }

        req
    }

    #[cfg(feature = "reqwest-transport")]
    async fn do_request<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> crate::Result<T> {
        let req = self.apply_headers(req);
        let response = req.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.bytes().await?;
            return Err(parse_error_response(&body, status.as_u16()));
        }

        let body = response.bytes().await?;
        if body.is_empty() {
            // For responses that are expected to be empty, this will fail
            // unless T can be deserialized from empty. Callers that expect
            // empty responses should use `do_request_no_body` instead.
            return Err(OjsError::Serialization(
                "empty response body".to_string(),
            ));
        }

        serde_json::from_slice(&body).map_err(|e| {
            OjsError::Serialization(format!(
                "failed to parse response: {} (body: {})",
                e,
                String::from_utf8_lossy(&body)
            ))
        })
    }

    #[cfg(feature = "reqwest-transport")]
    async fn do_request_no_body(&self, req: reqwest::RequestBuilder) -> crate::Result<()> {
        let req = self.apply_headers(req);
        let response = req.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.bytes().await?;
            return Err(parse_error_response(&body, status.as_u16()));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public transport methods
    // -----------------------------------------------------------------------

    #[cfg(feature = "reqwest-transport")]
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> crate::Result<T> {
        let url = self.url(path);
        let req = self.client.get(&url);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn get_raw<T: DeserializeOwned>(&self, path: &str) -> crate::Result<T> {
        let url = self.url_raw(path);
        let req = self.client.get(&url);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> crate::Result<T> {
        let url = self.url(path);
        let req = self.client.post(&url).json(body);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn post_no_response<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> crate::Result<()> {
        let url = self.url(path);
        let req = self.client.post(&url).json(body);
        self.do_request_no_body(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> crate::Result<T> {
        let url = self.url(path);
        let req = self.client.post(&url);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn post_empty_no_response(&self, path: &str) -> crate::Result<()> {
        let url = self.url(path);
        let req = self.client.post(&url);
        self.do_request_no_body(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> crate::Result<T> {
        let url = self.url(path);
        let req = self.client.delete(&url);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    pub async fn delete_no_response(&self, path: &str) -> crate::Result<()> {
        let url = self.url(path);
        let req = self.client.delete(&url);
        self.do_request_no_body(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    #[allow(dead_code)]
    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> crate::Result<T> {
        let url = self.url(path);
        let req = self.client.patch(&url).json(body);
        self.do_request(req).await
    }

    #[cfg(feature = "reqwest-transport")]
    #[allow(dead_code)]
    pub async fn patch_no_response<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> crate::Result<()> {
        let url = self.url(path);
        let req = self.client.patch(&url).json(body);
        self.do_request_no_body(req).await
    }
}

// ---------------------------------------------------------------------------
// Error response parsing
// ---------------------------------------------------------------------------

fn parse_error_response(body: &[u8], status_code: u16) -> OjsError {
    if let Ok(err_resp) = serde_json::from_slice::<ErrorResponse>(body) {
        OjsError::Server(err_resp.error.into_server_error(status_code))
    } else {
        let message = String::from_utf8_lossy(body).to_string();
        OjsError::Server(ServerError {
            code: format!("http_{}", status_code),
            message: if message.is_empty() {
                format!("HTTP {}", status_code)
            } else {
                message
            },
            retryable: status_code == 429 || status_code >= 500,
            details: None,
            request_id: None,
            http_status: status_code,
        })
    }
}
