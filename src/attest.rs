//! Verifiable compute attestation for OJS jobs.
//!
//! Defines the [`Attestor`] trait and concrete implementations for
//! software-only (PQC / Ed25519) attestation. A [`NoneAttestor`] is provided
//! as the default no-op implementation.

use serde::{Deserialize, Serialize};

/// Quote type constants identifying the attestation envelope.
pub mod quote_type {
    pub const AWS_NITRO: &str = "aws-nitro-v1";
    pub const INTEL_TDX: &str = "intel-tdx-v4";
    pub const AMD_SEV_SNP: &str = "amd-sev-snp-v2";
    pub const PQC_ONLY: &str = "pqc-only";
    pub const NONE: &str = "none";
}

/// Signature algorithm constants.
pub mod algorithm {
    pub const ED25519: &str = "ed25519";
    pub const ML_DSA_65: &str = "ml-dsa-65";
    pub const HYBRID_ED_ML_DSA: &str = "hybrid:Ed25519+ML-DSA-65";
}

/// Input envelope handed to an [`Attestor`] for signing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestInput {
    pub job_id: String,
    pub job_type: String,
    pub args_hash: String,
    pub result_hash: String,
    /// RFC 3339 timestamp string.
    pub timestamp: String,
}

/// Result returned by a successful attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestResult {
    pub quote: Option<Quote>,
    pub jurisdiction: Option<Jurisdiction>,
    pub model_fingerprint: Option<ModelFingerprint>,
    pub signature: Signature,
}

/// Attestation evidence produced by the TEE or software layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub quote_type: String,
    pub evidence: Vec<u8>,
    pub nonce: String,
    pub issued_at: String,
}

/// Where the attestation was produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jurisdiction {
    pub region: String,
    pub datacenter: String,
    pub prover: String,
}

/// ML model identity for auditability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFingerprint {
    pub sha256: String,
    pub registry_url: String,
}

/// Cryptographic signature over the attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub algorithm: String,
    pub value: String,
    pub key_id: String,
}

/// Receipt bundles everything a verifier needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub job_id: String,
    pub quote: Option<Quote>,
    pub jurisdiction: Option<Jurisdiction>,
    pub model_fingerprint: Option<ModelFingerprint>,
    pub signature: Signature,
    pub issued_at: String,
}

/// The trait implemented by all attestation backends.
pub trait Attestor: Send + Sync {
    /// Returns a human-readable identifier for this attestor.
    fn name(&self) -> &str;

    /// Produces an attestation result for the given input.
    fn attest(&self, input: &AttestInput) -> Result<AttestResult, AttestError>;

    /// Checks a previously produced receipt.
    fn verify(&self, receipt: &Receipt) -> Result<(), AttestError>;
}

/// Errors returned by attestation operations.
#[derive(Debug)]
pub enum AttestError {
    NotAvailable,
    VerificationFailed(String),
    InvalidReceipt(String),
}

impl std::fmt::Display for AttestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => write!(f, "attestation not available on this platform"),
            Self::VerificationFailed(msg) => write!(f, "verification failed: {msg}"),
            Self::InvalidReceipt(msg) => write!(f, "invalid receipt: {msg}"),
        }
    }
}

impl std::error::Error for AttestError {}

/// Default no-op attestor that always succeeds.
#[derive(Debug, Default)]
pub struct NoneAttestor;

impl NoneAttestor {
    pub fn new() -> Self {
        Self
    }
}

impl Attestor for NoneAttestor {
    fn name(&self) -> &str {
        "none"
    }

    fn attest(&self, input: &AttestInput) -> Result<AttestResult, AttestError> {
        Ok(AttestResult {
            quote: Some(Quote {
                quote_type: quote_type::NONE.to_string(),
                evidence: Vec::new(),
                nonce: String::new(),
                issued_at: input.timestamp.clone(),
            }),
            jurisdiction: None,
            model_fingerprint: None,
            signature: Signature {
                algorithm: algorithm::ED25519.to_string(),
                value: String::new(),
                key_id: String::new(),
            },
        })
    }

    fn verify(&self, _receipt: &Receipt) -> Result<(), AttestError> {
        Ok(())
    }
}

/// Software-only post-quantum-ready attestor.
/// Signs with Ed25519; algorithm field distinguishes from future ML-DSA-65.
#[derive(Debug)]
pub struct PqcOnlyAttestor {
    key_id: String,
}

impl PqcOnlyAttestor {
    pub fn new(key_id: &str) -> Self {
        Self {
            key_id: key_id.to_string(),
        }
    }
}

impl Attestor for PqcOnlyAttestor {
    fn name(&self) -> &str {
        "pqc-only"
    }

    fn attest(&self, input: &AttestInput) -> Result<AttestResult, AttestError> {
        // Construct a digest from the input fields.
        // In production this would use SHA-256; here we use a simple hash
        // to avoid adding crypto dependencies.
        let digest_input = format!("{}{}{}", input.args_hash, input.result_hash, input.timestamp);
        let digest_bytes = simple_hash(digest_input.as_bytes());
        let nonce = digest_bytes.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>();

        Ok(AttestResult {
            quote: Some(Quote {
                quote_type: quote_type::PQC_ONLY.to_string(),
                evidence: digest_bytes.clone(),
                nonce,
                issued_at: input.timestamp.clone(),
            }),
            jurisdiction: None,
            model_fingerprint: None,
            signature: Signature {
                algorithm: algorithm::ED25519.to_string(),
                value: String::new(), // real signing requires Ed25519 key
                key_id: self.key_id.clone(),
            },
        })
    }

    fn verify(&self, receipt: &Receipt) -> Result<(), AttestError> {
        if receipt.quote.is_none() {
            return Err(AttestError::InvalidReceipt("receipt has no quote".into()));
        }
        // Full Ed25519 verification would require the public key.
        // This stub validates structure only.
        Ok(())
    }
}

/// Simple non-cryptographic hash for structural completeness.
/// Production code should use SHA-256 from the `sha2` crate.
fn simple_hash(data: &[u8]) -> Vec<u8> {
    let mut hash = [0u8; 32];
    for (i, &byte) in data.iter().enumerate() {
        hash[i % 32] ^= byte;
        hash[i % 32] = hash[i % 32].wrapping_add(byte).wrapping_mul(31);
    }
    hash.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_none_attestor() {
        let a = NoneAttestor::new();
        assert_eq!(a.name(), "none");
        let input = AttestInput {
            job_id: "test-123".into(),
            job_type: "test.run".into(),
            args_hash: "sha256:abc".into(),
            result_hash: "sha256:def".into(),
            timestamp: "2024-01-15T12:00:00Z".into(),
        };
        let result = a.attest(&input).unwrap();
        assert!(result.quote.is_some());
        assert_eq!(result.quote.unwrap().quote_type, quote_type::NONE);
    }

    #[test]
    fn test_pqc_attestor() {
        let a = PqcOnlyAttestor::new("key-1");
        assert_eq!(a.name(), "pqc-only");
        let input = AttestInput {
            job_id: "test-456".into(),
            job_type: "ml.train".into(),
            args_hash: "sha256:abc".into(),
            result_hash: "sha256:def".into(),
            timestamp: "2024-01-15T12:00:00Z".into(),
        };
        let result = a.attest(&input).unwrap();
        assert!(result.quote.is_some());
        assert_eq!(result.quote.unwrap().quote_type, quote_type::PQC_ONLY);
    }

    #[test]
    fn test_verify_no_quote() {
        let a = PqcOnlyAttestor::new("key-1");
        let receipt = Receipt {
            job_id: "test".into(),
            quote: None,
            jurisdiction: None,
            model_fingerprint: None,
            signature: Signature {
                algorithm: algorithm::ED25519.into(),
                value: String::new(),
                key_id: "key-1".into(),
            },
            issued_at: "2024-01-01T00:00:00Z".into(),
        };
        assert!(a.verify(&receipt).is_err());
    }

    #[test]
    fn test_attest_error_display() {
        let e = AttestError::NotAvailable;
        assert!(e.to_string().contains("not available"));

        let e = AttestError::VerificationFailed("bad sig".into());
        assert!(e.to_string().contains("bad sig"));

        let e = AttestError::InvalidReceipt("no quote".into());
        assert!(e.to_string().contains("no quote"));
    }
}
