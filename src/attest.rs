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
    fn name(&self) -> &'static str {
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

    /// The key identifier this attestor was configured with.
    ///
    /// Retained for introspection/logging even though [`attest`](Attestor::attest)
    /// and [`verify`](Attestor::verify) do not yet perform real signing with it.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

impl Attestor for PqcOnlyAttestor {
    fn name(&self) -> &'static str {
        "pqc-only"
    }

    /// Always returns `Err(AttestError::NotAvailable)`.
    ///
    /// `PqcOnlyAttestor` does not yet perform real Ed25519/ML-DSA signing:
    /// no signing dependency is wired into this crate. An earlier version
    /// of this method returned `Ok` with an *empty* signature and a
    /// non-cryptographic digest as the "evidence" -- a fabricated,
    /// always-successful receipt that looked legitimate but proved
    /// nothing. Failing honestly here is safer than shipping a receipt
    /// that cannot back up its own claims: callers that check for `Ok`
    /// before trusting a receipt cannot be misled into treating a fake
    /// attestation as real. Construct a real signing-backed `Attestor` (or
    /// use [`NoneAttestor`] to explicitly opt out of attestation) instead
    /// of relying on this type for anything security-sensitive.
    fn attest(&self, _input: &AttestInput) -> Result<AttestResult, AttestError> {
        Err(AttestError::NotAvailable)
    }

    /// Always returns `Err(AttestError::VerificationFailed(_))`.
    ///
    /// See [`attest`](Self::attest): without real signing, there is no
    /// signature this method could meaningfully check, so it must not
    /// report any receipt as valid (the previous structure-only check
    /// accepted every receipt that merely included a quote, regardless of
    /// its contents).
    fn verify(&self, _receipt: &Receipt) -> Result<(), AttestError> {
        Err(AttestError::VerificationFailed(
            "pqc-only attestor has no signing key material and cannot verify receipts".into(),
        ))
    }
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
    fn test_pqc_attestor_honestly_fails_attest() {
        // PqcOnlyAttestor has no real signing key material wired in (see
        // its `attest`/`verify` doc comments); it must fail honestly rather
        // than fabricate a fake-successful receipt with an empty signature.
        let a = PqcOnlyAttestor::new("key-1");
        assert_eq!(a.name(), "pqc-only");
        assert_eq!(a.key_id(), "key-1");
        let input = AttestInput {
            job_id: "test-456".into(),
            job_type: "ml.train".into(),
            args_hash: "sha256:abc".into(),
            result_hash: "sha256:def".into(),
            timestamp: "2024-01-15T12:00:00Z".into(),
        };
        let err = a.attest(&input).unwrap_err();
        assert!(matches!(err, AttestError::NotAvailable));
    }

    #[test]
    fn test_pqc_attestor_honestly_fails_verify_even_with_a_quote() {
        // A structurally well-formed receipt (with a quote) must still be
        // rejected: without real signing there is nothing to check the
        // signature against, so accepting it would be trust theater. This
        // replaces the previous structure-only check, which accepted any
        // receipt as long as it merely included a quote.
        let a = PqcOnlyAttestor::new("key-1");
        let receipt = Receipt {
            job_id: "test".into(),
            quote: Some(Quote {
                quote_type: quote_type::PQC_ONLY.to_string(),
                evidence: vec![1, 2, 3],
                nonce: "deadbeef".into(),
                issued_at: "2024-01-01T00:00:00Z".into(),
            }),
            jurisdiction: None,
            model_fingerprint: None,
            signature: Signature {
                algorithm: algorithm::ED25519.into(),
                value: String::new(),
                key_id: "key-1".into(),
            },
            issued_at: "2024-01-01T00:00:00Z".into(),
        };
        let err = a.verify(&receipt).unwrap_err();
        assert!(matches!(err, AttestError::VerificationFailed(_)));
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
