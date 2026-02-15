use crate::errors::{ErrorResponse, OjsError, ServerError};
use crate::transport::{Method, Transport};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

const OJS_CONTENT_TYPE: &str = "application/openjobspec+json";
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
#[derive(Default)]
pub(crate) struct TransportConfig {
    pub auth_token: Option<String>,
    pub headers: HashMap<String, String>,
    pub timeout: Option<std::time::Duration>,
    #[cfg(feature = "reqwest-transport")]
    pub http_client: Option<reqwest::Client>,
}

impl HttpTransport {
    pub fn new(base_url: &str, config: TransportConfig) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();

        #[cfg(feature = "reqwest-transport")]
        let client = config.http_client.unwrap_or_else(|| {
            let mut builder = reqwest::Client::builder();
            let timeout = config.timeout.unwrap_or(std::time::Duration::from_secs(30));
            builder = builder.timeout(timeout);
            builder.build().expect("failed to build reqwest client")
        });

        Self {
            base_url,
            #[cfg(feature = "reqwest-transport")]
            client,
            auth_token: config.auth_token,
            headers: config.headers,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base_url, BASE_PATH, path)
    }

    fn url_raw(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    #[cfg(feature = "reqwest-transport")]
    fn apply_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req
            .header("Content-Type", OJS_CONTENT_TYPE)
            .header("Accept", OJS_CONTENT_TYPE)
            .header("OJS-Version", crate::OJS_VERSION);

        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        for (key, value) in &self.headers {
            req = req.header(key.as_str(), value.as_str());
        }

        req
    }

    #[cfg(feature = "reqwest-transport")]
    async fn execute(
        &self,
        req: reqwest::RequestBuilder,
    ) -> crate::Result<Option<serde_json::Value>> {
        let req = self.apply_headers(req);
        let response = req.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.bytes().await?;
            return Err(parse_error_response(&body, status.as_u16()));
        }

        let body = response.bytes().await?;
        if body.is_empty() {
            return Ok(None);
        }

        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            OjsError::Serialization(format!(
                "failed to parse response: {} (body: {})",
                e,
                String::from_utf8_lossy(&body)
            ))
        })?;

        Ok(Some(value))
    }
}

#[cfg(feature = "reqwest-transport")]
impl Transport for HttpTransport {
    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        raw_path: bool,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Option<serde_json::Value>>> + Send + '_>> {
        let url = if raw_path {
            self.url_raw(path)
        } else {
            self.url(path)
        };

        Box::pin(async move {
            let req = match method {
                Method::Get => self.client.get(&url),
                Method::Post => {
                    let r = self.client.post(&url);
                    if let Some(body) = &body {
                        r.body(serde_json::to_vec(body).unwrap_or_default())
                    } else {
                        r
                    }
                }
                Method::Delete => self.client.delete(&url),
                Method::Patch => {
                    let r = self.client.patch(&url);
                    if let Some(body) = &body {
                        r.body(serde_json::to_vec(body).unwrap_or_default())
                    } else {
                        r
                    }
                }
            };

            self.execute(req).await
        })
    }
}

// ---------------------------------------------------------------------------
// Error response parsing
// ---------------------------------------------------------------------------

fn parse_error_response(body: &[u8], status_code: u16) -> OjsError {
    if let Ok(err_resp) = serde_json::from_slice::<ErrorResponse>(body) {
        OjsError::Server(Box::new(err_resp.error.into_server_error(status_code)))
    } else {
        let message = String::from_utf8_lossy(body).to_string();
        OjsError::Server(Box::new(ServerError {
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
        }))
    }
}
