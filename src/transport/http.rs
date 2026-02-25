use crate::errors::{ErrorResponse, OjsError, RateLimitInfo, ServerError};
use crate::rate_limiter::RetryConfig;
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
    retry_config: RetryConfig,
}

/// Configuration used to construct an HttpTransport from client settings.
#[derive(Default)]
pub(crate) struct TransportConfig {
    pub auth_token: Option<String>,
    pub headers: HashMap<String, String>,
    pub timeout: Option<std::time::Duration>,
    pub retry_config: Option<RetryConfig>,
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
            retry_config: config.retry_config.unwrap_or_default(),
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
            let retry_after = parse_retry_after_header(response.headers());
            let rate_limit = parse_rate_limit_headers(response.headers(), retry_after);
            let body = response.bytes().await?;
            return Err(parse_error_response(&body, status.as_u16(), retry_after, rate_limit));
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

    #[cfg(feature = "reqwest-transport")]
    fn build_request(
        &self,
        method: Method,
        url: &str,
        body: &Option<serde_json::Value>,
    ) -> reqwest::RequestBuilder {
        match method {
            Method::Get => self.client.get(url),
            Method::Post => {
                let r = self.client.post(url);
                if let Some(body) = body {
                    r.body(serde_json::to_vec(body).unwrap_or_default())
                } else {
                    r
                }
            }
            Method::Delete => self.client.delete(url),
            Method::Patch => {
                let r = self.client.patch(url);
                if let Some(body) = body {
                    r.body(serde_json::to_vec(body).unwrap_or_default())
                } else {
                    r
                }
            }
        }
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
            let max_attempts = if self.retry_config.enabled {
                self.retry_config.max_retries + 1
            } else {
                1
            };

            for attempt in 0..max_attempts {
                let req = self.build_request(method, &url, &body);
                match self.execute(req).await {
                    Ok(val) => return Ok(val),
                    Err(err) => {
                        let is_last = attempt + 1 >= max_attempts;
                        if is_last {
                            return Err(err);
                        }

                        // Only retry on 429 (rate limited) responses.
                        let retry_after = match &err {
                            OjsError::Server(ref server_err) if server_err.http_status == 429 => {
                                server_err.retry_after
                            }
                            _ => return Err(err),
                        };

                        let delay = self.retry_config.compute_backoff(attempt, retry_after);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_attempts,
                            delay_ms = delay.as_millis() as u64,
                            "rate limited, retrying after backoff"
                        );
                        // Cancel-safe: dropping this future (e.g. when the caller's
                        // task is cancelled) immediately cancels the sleep.
                        tokio::time::sleep(delay).await;
                    }
                }
            }

            unreachable!("retry loop should have returned")
        })
    }
}

// ---------------------------------------------------------------------------
// Error response parsing
// ---------------------------------------------------------------------------

fn parse_error_response(
    body: &[u8],
    status_code: u16,
    retry_after: Option<std::time::Duration>,
    rate_limit: Option<RateLimitInfo>,
) -> OjsError {
    if let Ok(err_resp) = serde_json::from_slice::<ErrorResponse>(body) {
        let mut server_err = err_resp.error.into_server_error(status_code);
        server_err.retry_after = retry_after;
        server_err.rate_limit = rate_limit;
        OjsError::Server(Box::new(server_err))
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
            retry_after,
            rate_limit,
        }))
    }
}

/// Parse the `Retry-After` header value into a `Duration`.
#[cfg(feature = "reqwest-transport")]
fn parse_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let raw = headers.get("Retry-After")?.to_str().ok()?;
    let seconds: f64 = raw.parse().ok()?;
    Some(std::time::Duration::from_secs_f64(seconds))
}

/// Parse rate limit headers into a `RateLimitInfo`.
#[cfg(feature = "reqwest-transport")]
fn parse_rate_limit_headers(
    headers: &reqwest::header::HeaderMap,
    retry_after: Option<std::time::Duration>,
) -> Option<RateLimitInfo> {
    let limit = headers.get("X-RateLimit-Limit")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let remaining = headers.get("X-RateLimit-Remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let reset = headers.get("X-RateLimit-Reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());

    if limit.is_none() && remaining.is_none() && reset.is_none() && retry_after.is_none() {
        return None;
    }

    Some(RateLimitInfo {
        limit,
        remaining,
        reset,
        retry_after,
    })
}

