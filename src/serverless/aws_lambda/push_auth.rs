//! HTTP push delivery authentication: [`PushAuthConfig`] and constant-time
//! HMAC-SHA256 verification of the `X-OJS-Timestamp`/`X-OJS-Signature`
//! headers.
//!
//! `LambdaHandler::handle_http`/`handle_http_raw` accept any POST body with
//! no authentication at all -- appropriate only when the caller
//! authenticates push delivery upstream (e.g. via an API Gateway
//! authorizer). Any publicly reachable endpoint (the module's own docs
//! describe wiring this up behind a Lambda Function URL) MUST verify the
//! request actually came from the configured OJS backend before decoding
//! or executing it, since otherwise an unauthenticated caller on the
//! public internet can invoke any registered job handler with
//! attacker-controlled arguments. [`LambdaHandler::handle_http_authenticated`](super::LambdaHandler::handle_http_authenticated)
//! and [`LambdaHandler::handle_http_raw_authenticated`](super::LambdaHandler::handle_http_raw_authenticated)
//! use this module's (crate-private) verification function to provide
//! that check before any dispatch happens.
//!
//! The scheme (header names, signed-message format, freshness window, and
//! size limits) intentionally matches the Go SDK's `serverless` package
//! byte-for-byte so a single OJS backend implementation can sign push
//! requests once for every SDK.

use super::ServerlessError;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::time::Duration;

pub const PUSH_TIMESTAMP_HEADER: &str = "X-OJS-Timestamp";

/// Header carrying one or more comma-separated `sha256=<hex>` signatures.
pub const PUSH_SIGNATURE_HEADER: &str = "X-OJS-Signature";

/// Header carrying the unique delivery-attempt identity for push delivery.
pub const PUSH_DELIVERY_ID_HEADER: &str = "X-OJS-Delivery-ID";

/// Header carrying the job identity for push delivery.
pub const PUSH_JOB_ID_HEADER: &str = "X-OJS-Job-ID";

/// Default permitted clock skew (past or future) for a signed push request.
pub const DEFAULT_PUSH_FRESHNESS_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Minimum accepted signing-secret length.
///
/// HMAC accepts keys of any length, but short human-chosen values do not
/// provide adequate brute-force resistance. Production secrets must contain
/// at least 32 bytes (256 bits) of randomly generated key material.
pub const MIN_PUSH_SIGNING_SECRET_BYTES: usize = 32;

const MAX_PUSH_TIMESTAMP_HEADER_BYTES: usize = 32;
const MAX_PUSH_SIGNATURE_HEADER_BYTES: usize = 8 * 1024;
const MAX_PUSH_SIGNATURES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// Configuration for authenticating HTTP push delivery requests.
///
/// Fails closed by default: [`LambdaHandler::handle_http_authenticated`](super::LambdaHandler::handle_http_authenticated)
/// rejects every request unless at least one signing secret is configured,
/// or [`allow_insecure_unsigned_for_local_development`](Self::allow_insecure_unsigned_for_local_development)
/// is explicitly set.
///
/// [`Debug`] is implemented manually (rather than derived) so signing-secret
/// bytes are never rendered into logs or panic messages. Only the number of
/// configured secrets, the freshness window, and the insecure-unsigned flag
/// are shown.
#[derive(Clone)]
pub struct PushAuthConfig {
    signing_secrets: Vec<Vec<u8>>,
    freshness_window: Duration,
    allow_insecure_unsigned: bool,
}

impl std::fmt::Debug for PushAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushAuthConfig")
            // Never render secret bytes; expose only how many are configured.
            .field("signing_secret_count", &self.signing_secrets.len())
            .field("freshness_window", &self.freshness_window)
            .field("allow_insecure_unsigned", &self.allow_insecure_unsigned)
            .finish()
    }
}

impl Default for PushAuthConfig {
    fn default() -> Self {
        Self {
            signing_secrets: Vec::new(),
            freshness_window: DEFAULT_PUSH_FRESHNESS_WINDOW,
            allow_insecure_unsigned: false,
        }
    }
}

impl PushAuthConfig {
    /// Create a new, unsigned-by-default configuration. Call
    /// [`with_signing_secret`](Self::with_signing_secret) at least once
    /// before using it to authenticate real traffic.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an accepted signing secret.
    ///
    /// Configuring more than one supports zero-downtime secret rotation:
    /// requests signed with either the old or the new secret verify
    /// successfully until the old one is removed. Every configured secret
    /// must contain at least [`MIN_PUSH_SIGNING_SECRET_BYTES`] bytes; legacy
    /// call sites using this infallible method remain source-compatible, but
    /// an invalid value poisons the complete configuration and is rejected by
    /// [`validate`](Self::validate), [`LambdaHandler::try_with_push_auth`](super::LambdaHandler::try_with_push_auth),
    /// and request verification.
    ///
    /// New code should prefer [`try_with_signing_secret`](Self::try_with_signing_secret)
    /// to receive the configuration error immediately.
    pub fn with_signing_secret(mut self, secret: impl Into<Vec<u8>>) -> Self {
        self.signing_secrets.push(secret.into());
        self
    }

    /// Add and immediately validate an accepted signing secret.
    ///
    /// # Errors
    ///
    /// Returns [`ServerlessError::NonRetryable`] when the secret contains
    /// fewer than [`MIN_PUSH_SIGNING_SECRET_BYTES`] bytes.
    pub fn try_with_signing_secret(
        mut self,
        secret: impl Into<Vec<u8>>,
    ) -> Result<Self, ServerlessError> {
        let secret = secret.into();
        validate_signing_secret(&secret)?;
        self.signing_secrets.push(secret);
        Ok(self)
    }

    /// Read, validate, and add a signing secret from an environment variable.
    ///
    /// This is intended for Lambda configuration where secret material is
    /// injected by the deployment platform. Missing/non-Unicode variables,
    /// empty values, and values shorter than
    /// [`MIN_PUSH_SIGNING_SECRET_BYTES`] fail closed.
    pub fn try_with_signing_secret_from_env(
        self,
        variable: impl AsRef<str>,
    ) -> Result<Self, ServerlessError> {
        let variable = variable.as_ref().trim();
        if variable.is_empty() {
            return Err(ServerlessError::NonRetryable(
                "push authentication configuration is invalid: environment variable name must not be empty".into(),
            ));
        }
        let secret = std::env::var(variable).map_err(|_| {
            ServerlessError::NonRetryable(format!(
                "push authentication configuration is invalid: environment variable {variable} is not set or is not valid Unicode"
            ))
        })?;
        self.try_with_signing_secret(secret.into_bytes())
    }

    /// Set the permitted clock skew for the signed timestamp. Defaults to
    /// [`DEFAULT_PUSH_FRESHNESS_WINDOW`] (5 minutes).
    pub fn with_freshness_window(mut self, window: Duration) -> Self {
        self.freshness_window = window;
        self
    }

    /// Explicitly disable push authentication.
    ///
    /// Intended only for local development against a backend that does not
    /// yet sign push requests. A production deployment should always
    /// configure at least one signing secret instead of calling this.
    pub fn allow_insecure_unsigned_for_local_development(mut self) -> Self {
        self.allow_insecure_unsigned = true;
        self
    }

    /// Validate the complete push-authentication configuration.
    ///
    /// Secret rotation fails closed as a unit: one empty/short member rejects
    /// the entire list rather than silently ignoring it and creating a
    /// deployment-dependent accepted-key set.
    pub fn validate(&self) -> Result<(), ServerlessError> {
        for secret in &self.signing_secrets {
            validate_signing_secret(secret)?;
        }
        if self.freshness_window.is_zero() {
            return Err(ServerlessError::NonRetryable(
                "push authentication configuration is invalid: freshness_window must be greater than zero".into(),
            ));
        }
        if self.signing_secrets.is_empty() && !self.allow_insecure_unsigned {
            return Err(ServerlessError::NonRetryable(
                "push authentication is not configured".into(),
            ));
        }
        Ok(())
    }
}

fn validate_signing_secret(secret: &[u8]) -> Result<(), ServerlessError> {
    if secret.len() < MIN_PUSH_SIGNING_SECRET_BYTES {
        return Err(ServerlessError::NonRetryable(format!(
            "push authentication configuration is invalid: every signing secret must contain at least {MIN_PUSH_SIGNING_SECRET_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn find_header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn parse_push_timestamp(value: &str) -> Result<i64, ServerlessError> {
    if value.len() > MAX_PUSH_TIMESTAMP_HEADER_BYTES {
        return Err(ServerlessError::NonRetryable(
            "push authentication header is too large".into(),
        ));
    }
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ServerlessError::NonRetryable(
            "invalid push authentication: malformed X-OJS-Timestamp".into(),
        ));
    }
    value.parse::<i64>().map_err(|_| {
        ServerlessError::NonRetryable(
            "invalid push authentication: malformed X-OJS-Timestamp".into(),
        )
    })
}

fn decode_hex_sha256(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// Parse one or more raw `X-OJS-Signature` header values (each possibly
/// containing multiple comma-separated `sha256=<hex>` entries) into decoded
/// signature bytes, bounding both total input size and signature count.
fn parse_push_signatures(headers: &[&str]) -> Result<Vec<[u8; 32]>, ServerlessError> {
    if headers.is_empty() {
        return Err(ServerlessError::NonRetryable(
            "invalid push authentication: missing X-OJS-Signature".into(),
        ));
    }

    let mut total_bytes = 0usize;
    let mut signatures = Vec::new();
    for header in headers {
        total_bytes += header.len();
        if total_bytes > MAX_PUSH_SIGNATURE_HEADER_BYTES {
            return Err(ServerlessError::NonRetryable(
                "push authentication header is too large".into(),
            ));
        }
        for part in header.split(',') {
            if signatures.len() >= MAX_PUSH_SIGNATURES {
                return Err(ServerlessError::NonRetryable(
                    "push authentication header is too large".into(),
                ));
            }
            let part = part.trim();
            let hex_part = part.strip_prefix("sha256=").ok_or_else(|| {
                ServerlessError::NonRetryable(
                    "invalid push authentication: malformed X-OJS-Signature".into(),
                )
            })?;
            let decoded = decode_hex_sha256(hex_part).ok_or_else(|| {
                ServerlessError::NonRetryable(
                    "invalid push authentication: malformed X-OJS-Signature".into(),
                )
            })?;
            signatures.push(decoded);
        }
    }

    if signatures.is_empty() {
        return Err(ServerlessError::NonRetryable(
            "invalid push authentication: missing X-OJS-Signature".into(),
        ));
    }
    Ok(signatures)
}

/// Verify a push delivery request's `X-OJS-Timestamp`/`X-OJS-Signature`
/// headers against `config`, in constant time per candidate signature.
///
/// Returns how long the delivery ID must remain in the replay cache so a
/// request with an accepted future-skewed timestamp cannot be replayed while
/// its signature is still inside the inclusive freshness window.
pub(crate) fn authenticate_push(
    config: &PushAuthConfig,
    timestamp_header: Option<&str>,
    signature_headers: &[&str],
    body: &[u8],
    now: Duration,
) -> Result<Duration, ServerlessError> {
    config.validate()?;
    if config.allow_insecure_unsigned {
        return Ok(config.freshness_window);
    }

    let timestamp_header = timestamp_header.ok_or_else(|| {
        ServerlessError::NonRetryable("invalid push authentication: missing X-OJS-Timestamp".into())
    })?;
    let timestamp_secs = parse_push_timestamp(timestamp_header)?;

    // Saturate rather than wrap: both values are Unix-second counts that
    // will not realistically reach `i64::MAX`, but a saturating
    // conversion is both clippy-clean and correct at the boundary either
    // way (an out-of-range timestamp simply fails the freshness check).
    let now_secs = i64::try_from(now.as_secs()).unwrap_or(i64::MAX);
    let window_secs = i64::try_from(config.freshness_window.as_secs()).unwrap_or(i64::MAX);
    if (timestamp_secs - now_secs).abs() > window_secs {
        return Err(ServerlessError::NonRetryable(
            "invalid push authentication: timestamp outside the allowed freshness window".into(),
        ));
    }

    let signatures = parse_push_signatures(signature_headers)?;

    // Signed message: `"{timestamp}.{body}"`, using the *raw* timestamp
    // header string (not a re-serialized number) so the exact signed bytes
    // are unambiguous.
    let mut signed_message = Vec::with_capacity(timestamp_header.len() + 1 + body.len());
    signed_message.extend_from_slice(timestamp_header.as_bytes());
    signed_message.push(b'.');
    signed_message.extend_from_slice(body);

    for secret in &config.signing_secrets {
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts a key of any length");
        mac.update(&signed_message);
        for signature in &signatures {
            // `Mac::verify_slice` performs a constant-time comparison and
            // consumes the MAC state, so each candidate signature needs
            // its own clone of the (already-updated) instance rather than
            // recomputing the HMAC over the body again.
            if mac.clone().verify_slice(signature).is_ok() {
                let valid_until = i128::from(timestamp_secs)
                    .saturating_add(i128::from(config.freshness_window.as_secs()));
                let remaining_inclusive = valid_until
                    .saturating_sub(i128::from(now.as_secs()))
                    .saturating_add(1);
                let replay_ttl_secs = u64::try_from(remaining_inclusive.max(1)).unwrap_or(u64::MAX);
                return Ok(Duration::from_secs(replay_ttl_secs));
            }
        }
    }

    Err(ServerlessError::NonRetryable(
        "invalid push authentication: signature verification failed".into(),
    ))
}

pub(crate) fn unix_timestamp_now() -> Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}
