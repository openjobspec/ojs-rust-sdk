//! Encryption middleware for transparent job argument encryption/decryption.
//!
//! This module provides AES-256-GCM encryption for job arguments, enabling
//! at-rest encryption of sensitive data flowing through an OJS pipeline.
//!
//! # Architecture
//!
//! - **Enqueue side**: Use [`encrypt_job`] to encrypt a job's args before sending.
//! - **Worker side**: Add [`EncryptionMiddleware`] to the worker middleware chain;
//!   it transparently decrypts args before the handler sees them.
//!
//! # Example
//!
//! ```rust,no_run
//! use ojs::encryption::{EncryptionMiddleware, StaticKeyProvider, EncryptionCodec, encrypt_job};
//! use ojs::{Worker, Client};
//! use serde_json::json;
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() -> ojs::Result<()> {
//! let key = b"an-example-very-secret-key-12345"; // 32 bytes
//! let keys = Arc::new(StaticKeyProvider::new("k1", key.to_vec()));
//! let codec = Arc::new(EncryptionCodec::new());
//!
//! // Enqueue side
//! let client = Client::builder().url("http://localhost:8080").build()?;
//! let mut job_args = json!([{"ssn": "123-45-6789"}]);
//! // In practice, build a Job and call encrypt_job before enqueuing.
//!
//! // Worker side — middleware auto-decrypts before handler
//! let worker = Worker::builder()
//!     .url("http://localhost:8080")
//!     .queues(vec!["default"])
//!     .build()?;
//! worker.use_middleware("encryption", EncryptionMiddleware::new(codec, keys)).await;
//! # Ok(())
//! # }
//! ```

use crate::errors::OjsError;
use crate::job::Job;
use crate::middleware::{BoxFuture, HandlerResult, Middleware, Next};
use crate::worker::JobContext;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;

use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Key provider
// ---------------------------------------------------------------------------

/// Provides encryption keys by ID, supporting key rotation.
///
/// Implementations return the raw key bytes for a given key identifier and
/// declare which key ID should be used for new encryptions.
pub trait KeyProvider: Send + Sync + 'static {
    /// Look up a key by its identifier. Returns `None` if the key is unknown.
    fn get_key(&self, key_id: &str) -> Option<Vec<u8>>;

    /// The key ID to use when encrypting new data.
    fn current_key_id(&self) -> String;
}

// ---------------------------------------------------------------------------
// Static key provider
// ---------------------------------------------------------------------------

/// A simple in-memory [`KeyProvider`] backed by a `HashMap`.
///
/// Suitable for applications that load keys from configuration at startup.
pub struct StaticKeyProvider {
    keys: HashMap<String, Vec<u8>>,
    current: String,
}

impl StaticKeyProvider {
    /// Create a provider with a single key that is also the current key.
    pub fn new(key_id: impl Into<String>, key: Vec<u8>) -> Self {
        let id = key_id.into();
        let mut keys = HashMap::new();
        keys.insert(id.clone(), key);
        Self {
            keys,
            current: id,
        }
    }

    /// Add an additional key (e.g., a rotated-out key still needed for decryption).
    pub fn add_key(&mut self, key_id: impl Into<String>, key: Vec<u8>) {
        self.keys.insert(key_id.into(), key);
    }
}

impl KeyProvider for StaticKeyProvider {
    fn get_key(&self, key_id: &str) -> Option<Vec<u8>> {
        self.keys.get(key_id).cloned()
    }

    fn current_key_id(&self) -> String {
        self.current.clone()
    }
}

// ---------------------------------------------------------------------------
// Encryption codec
// ---------------------------------------------------------------------------

const NONCE_SIZE: usize = 12;

/// AES-256-GCM encryption codec.
///
/// Each `encrypt` call generates a random 12-byte nonce and prepends it to the
/// ciphertext.  The `decrypt` call splits the nonce back off before decrypting.
pub struct EncryptionCodec {
    _private: (),
}

impl EncryptionCodec {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Encrypt `plaintext` with AES-256-GCM using the given 32-byte key.
    ///
    /// Returns `nonce (12 bytes) || ciphertext`.
    pub fn encrypt(&self, plaintext: &[u8], key: &[u8]) -> Result<Vec<u8>, OjsError> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| OjsError::Handler(format!("encryption failed: {}", e)))?;

        let mut out = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt data previously produced by [`encrypt`](Self::encrypt).
    ///
    /// Expects `nonce (12 bytes) || ciphertext`.
    pub fn decrypt(&self, data: &[u8], key: &[u8]) -> Result<Vec<u8>, OjsError> {
        if data.len() < NONCE_SIZE {
            return Err(OjsError::Handler(
                "encrypted data too short to contain nonce".into(),
            ));
        }

        let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
        let key = Key::<Aes256Gcm>::from_slice(key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| OjsError::Handler(format!("decryption failed: {}", e)))
    }
}

impl Default for EncryptionCodec {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Meta-key constants (OJS spec-canonical names)
// ---------------------------------------------------------------------------

const META_CODEC_ENCODINGS: &str = "ojs.codec.encodings";
const META_CODEC_KEY_ID: &str = "ojs.codec.key_id";
const ENCODING_BINARY_ENCRYPTED: &str = "binary/encrypted";

// Legacy meta keys for backward compatibility on decrypt
const LEGACY_META_ENCRYPTED: &str = "_encrypted";
const LEGACY_META_KEY_ID: &str = "_encryption_key_id";

// ---------------------------------------------------------------------------
// Encryption middleware (worker / decrypt side)
// ---------------------------------------------------------------------------

/// Worker middleware that transparently decrypts job arguments.
///
/// When the job's `meta` contains `ojs.codec.encodings` with `"binary/encrypted"`,
/// the middleware:
/// 1. Reads `ojs.codec.key_id` from meta to select the decryption key.
/// 2. Base64-decodes and AES-256-GCM-decrypts the job's `args`.
/// 3. Replaces `args` with the decrypted JSON value before calling the next handler.
///
/// Also supports legacy `_encrypted` / `_encryption_key_id` meta keys for
/// backward compatibility. Jobs without encryption markers pass through unchanged.
pub struct EncryptionMiddleware {
    codec: Arc<EncryptionCodec>,
    keys: Arc<dyn KeyProvider>,
}

impl EncryptionMiddleware {
    pub fn new(codec: Arc<EncryptionCodec>, keys: Arc<dyn KeyProvider>) -> Self {
        Self { codec, keys }
    }
}

impl Middleware for EncryptionMiddleware {
    fn handle(&self, mut ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let codec = self.codec.clone();
        let keys = self.keys.clone();

        Box::pin(async move {
            // Check spec-canonical ojs.codec.encodings for "binary/encrypted"
            let is_encrypted = ctx
                .job
                .meta
                .as_ref()
                .and_then(|m| m.get(META_CODEC_ENCODINGS))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|e| e.as_str() == Some(ENCODING_BINARY_ENCRYPTED))
                })
                .unwrap_or(false)
                // Backward compat: check legacy _encrypted flag
                || ctx
                    .job
                    .meta
                    .as_ref()
                    .and_then(|m| m.get(LEGACY_META_ENCRYPTED))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

            if is_encrypted {
                let key_id = ctx
                    .job
                    .meta
                    .as_ref()
                    .and_then(|m| {
                        m.get(META_CODEC_KEY_ID)
                            .or_else(|| m.get(LEGACY_META_KEY_ID))
                    })
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        OjsError::Handler("encrypted job missing ojs.codec.key_id in meta".into())
                    })?;

                let key = keys.get_key(key_id).ok_or_else(|| {
                    OjsError::Handler(format!("unknown encryption key id: {}", key_id))
                })?;

                // args is stored as a base64 string (single-element array wrapping it)
                let encoded = match &ctx.job.args {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Array(arr) if arr.len() == 1 => arr[0]
                        .as_str()
                        .ok_or_else(|| {
                            OjsError::Handler("encrypted args element is not a string".into())
                        })?
                        .to_string(),
                    _ => {
                        return Err(OjsError::Handler(
                            "encrypted args must be a base64 string or single-element array".into(),
                        ))
                    }
                };

                let raw = BASE64.decode(&encoded).map_err(|e| {
                    OjsError::Handler(format!("base64 decode of encrypted args failed: {}", e))
                })?;

                let plaintext = codec.decrypt(&raw, &key)?;

                let decrypted: serde_json::Value =
                    serde_json::from_slice(&plaintext).map_err(|e| {
                        OjsError::Handler(format!(
                            "failed to parse decrypted args as JSON: {}",
                            e
                        ))
                    })?;

                ctx.job.args = decrypted;

                // Remove encryption metadata so handlers don't see it
                if let Some(ref mut meta) = ctx.job.meta {
                    meta.remove(META_CODEC_ENCODINGS);
                    meta.remove(META_CODEC_KEY_ID);
                    meta.remove(LEGACY_META_ENCRYPTED);
                    meta.remove(LEGACY_META_KEY_ID);
                }
            }

            next.run(ctx).await
        })
    }
}

// ---------------------------------------------------------------------------
// Enqueue-side helper
// ---------------------------------------------------------------------------

/// Encrypt a job's arguments in-place before enqueueing.
///
/// This serialises the current `args` to JSON, encrypts with AES-256-GCM,
/// base64-encodes the result, and stores it as a single-element string array.
/// It sets `ojs.codec.encodings` to `["binary/encrypted"]` and
/// `ojs.codec.key_id` in the job's `meta`.
///
/// The returned `Job` is ready to be sent to the server.
pub fn encrypt_job(
    codec: &EncryptionCodec,
    keys: &dyn KeyProvider,
    mut job: Job,
) -> Result<Job, OjsError> {
    let key_id = keys.current_key_id();
    let key = keys.get_key(&key_id).ok_or_else(|| {
        OjsError::Handler(format!("current encryption key not found: {}", key_id))
    })?;

    let plaintext = serde_json::to_vec(&job.args)
        .map_err(|e| OjsError::Handler(format!("failed to serialize args: {}", e)))?;

    let encrypted = codec.encrypt(&plaintext, &key)?;
    let encoded = BASE64.encode(&encrypted);

    // Store as single-element array with the base64 string
    job.args = serde_json::Value::Array(vec![serde_json::Value::String(encoded)]);

    // Set encryption metadata (OJS spec-canonical keys)
    let meta = job.meta.get_or_insert_with(HashMap::new);
    meta.insert(
        META_CODEC_ENCODINGS.to_string(),
        serde_json::Value::Array(vec![serde_json::Value::String(
            ENCODING_BINARY_ENCRYPTED.to_string(),
        )]),
    );
    meta.insert(
        META_CODEC_KEY_ID.to_string(),
        serde_json::Value::String(key_id),
    );

    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_key_provider() -> StaticKeyProvider {
        // 32-byte key for AES-256
        let key = b"0123456789abcdef0123456789abcdef".to_vec();
        StaticKeyProvider::new("test-key", key)
    }

    fn make_job(args: serde_json::Value) -> Job {
        Job {
            specversion: "1.0".into(),
            id: "job-1".into(),
            job_type: "test.encrypt".into(),
            queue: "default".into(),
            args,
            meta: None,
            priority: 0,
            timeout: None,
            scheduled_at: None,
            expires_at: None,
            retry: None,
            unique: None,
            schema: None,
            state: None,
            attempt: 0,
            max_attempts: None,
            created_at: None,
            enqueued_at: None,
            started_at: None,
            completed_at: None,
            error: None,
            result: None,
            tags: vec![],
            timeout_ms: None,
        }
    }

    #[test]
    fn test_codec_round_trip() {
        let codec = EncryptionCodec::new();
        let key = b"0123456789abcdef0123456789abcdef";
        let plaintext = b"hello, world";

        let encrypted = codec.encrypt(plaintext, key).unwrap();
        assert_ne!(encrypted, plaintext);
        assert!(encrypted.len() > NONCE_SIZE);

        let decrypted = codec.decrypt(&encrypted, key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_codec_decrypt_too_short() {
        let codec = EncryptionCodec::new();
        let key = b"0123456789abcdef0123456789abcdef";
        let result = codec.decrypt(&[0u8; 5], key);
        assert!(result.is_err());
    }

    #[test]
    fn test_codec_decrypt_wrong_key() {
        let codec = EncryptionCodec::new();
        let key1 = b"0123456789abcdef0123456789abcdef";
        let key2 = b"fedcba9876543210fedcba9876543210";

        let encrypted = codec.encrypt(b"secret", key1).unwrap();
        let result = codec.decrypt(&encrypted, key2);
        assert!(result.is_err());
    }

    #[test]
    fn test_static_key_provider() {
        let mut provider = StaticKeyProvider::new("k1", vec![1, 2, 3]);
        assert_eq!(provider.current_key_id(), "k1");
        assert_eq!(provider.get_key("k1"), Some(vec![1, 2, 3]));
        assert_eq!(provider.get_key("k2"), None);

        provider.add_key("k2", vec![4, 5, 6]);
        assert_eq!(provider.get_key("k2"), Some(vec![4, 5, 6]));
        assert_eq!(provider.current_key_id(), "k1");
    }

    #[test]
    fn test_encrypt_job_round_trip() {
        let codec = EncryptionCodec::new();
        let keys = make_key_provider();
        let original_args = json!([{"ssn": "123-45-6789", "name": "Alice"}]);

        let job = make_job(original_args.clone());
        let encrypted_job = encrypt_job(&codec, &keys, job).unwrap();

        // Meta should have spec-canonical encryption markers
        let meta = encrypted_job.meta.as_ref().unwrap();
        assert_eq!(
            meta.get(META_CODEC_ENCODINGS).unwrap(),
            &json!(["binary/encrypted"])
        );
        assert_eq!(meta.get(META_CODEC_KEY_ID).unwrap(), &json!("test-key"));

        // Args should be a base64 string, not the original JSON
        assert_ne!(encrypted_job.args, original_args);

        // Decrypt and verify
        let encoded = encrypted_job.args.as_array().unwrap()[0]
            .as_str()
            .unwrap();
        let raw = BASE64.decode(encoded).unwrap();
        let key = keys.get_key("test-key").unwrap();
        let plaintext = codec.decrypt(&raw, &key).unwrap();
        let recovered: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(recovered, original_args);
    }

    #[test]
    fn test_encrypt_job_preserves_existing_meta() {
        let codec = EncryptionCodec::new();
        let keys = make_key_provider();

        let mut job = make_job(json!(["test"]));
        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), json!("acme"));
        job.meta = Some(meta);

        let encrypted_job = encrypt_job(&codec, &keys, job).unwrap();
        let meta = encrypted_job.meta.as_ref().unwrap();
        assert_eq!(meta.get("tenant").unwrap(), &json!("acme"));
        assert_eq!(
            meta.get(META_CODEC_ENCODINGS).unwrap(),
            &json!(["binary/encrypted"])
        );
    }

    #[test]
    fn test_unique_nonces() {
        let codec = EncryptionCodec::new();
        let key = b"0123456789abcdef0123456789abcdef";
        let plaintext = b"same data";

        let enc1 = codec.encrypt(plaintext, key).unwrap();
        let enc2 = codec.encrypt(plaintext, key).unwrap();

        // Different nonces should produce different ciphertexts
        assert_ne!(enc1, enc2);

        // Both should decrypt to the same plaintext
        assert_eq!(
            codec.decrypt(&enc1, key).unwrap(),
            codec.decrypt(&enc2, key).unwrap()
        );
    }
}
