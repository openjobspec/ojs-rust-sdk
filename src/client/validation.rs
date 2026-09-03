//! Job-type and queue-name validation, applied before any request reaches
//! the transport layer.
//!
//! This is a self-contained actor with no dependency on [`crate::client::Client`]/
//! [`crate::client::ClientBuilder`]/transport: it is used both by
//! [`crate::client::EnqueueBuilder::send`] and by
//! [`crate::workflow::WorkflowDefinition::validate`] (via the crate-private
//! re-exports below), so each step/job of a workflow is checked with
//! exactly the same rules as a directly-enqueued job.

use crate::errors::OjsError;
use crate::workflow::EnqueueOption;

/// Canonical maximum job-type length, measured in UTF-8 *bytes* (not Unicode
/// scalar values). Per `ojs-payload-limits.md` (PL-004) and the
/// `JOB_TYPE_TOO_LONG` error, a job type MUST NOT exceed 255 bytes when
/// encoded as UTF-8.
const MAX_TYPE_BYTES: usize = 255;

/// Canonical maximum queue-name length, measured in UTF-8 *bytes* (not
/// Unicode scalar values). Per `ojs-payload-limits.md` (PL-003), the
/// `QUEUE_NAME_TOO_LONG` error, and the security pattern
/// `^[a-zA-Z0-9_.-]{1,255}$` (SEC-010), a queue name MUST NOT exceed 255
/// bytes when encoded as UTF-8.
const MAX_QUEUE_NAME_BYTES: usize = 255;

pub(crate) fn validate_job_type(job_type: &str) -> crate::Result<()> {
    if job_type.is_empty() {
        return Err(OjsError::Builder("job type must not be empty".into()));
    }
    // Length is bounded by UTF-8 *byte* count (`str::len()`), not scalar
    // count, matching the canonical 255-byte limit, and is checked before
    // the pattern so an oversized (possibly multibyte) value is rejected on
    // length grounds regardless of its characters.
    if job_type.len() > MAX_TYPE_BYTES {
        return Err(OjsError::Builder(format!(
            "job type must not exceed {} bytes, got {}",
            MAX_TYPE_BYTES,
            job_type.len()
        )));
    }
    let valid = job_type.split('.').all(|segment| {
        !segment.is_empty()
            && segment.starts_with(|c: char| c.is_ascii_lowercase())
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    });
    if !valid {
        return Err(OjsError::Builder(format!(
            "invalid job type {:?}: each segment must match [a-z][a-z0-9_]*",
            job_type
        )));
    }
    Ok(())
}

pub(crate) fn validate_queue_name(queue: &str) -> crate::Result<()> {
    if queue.is_empty() {
        return Err(OjsError::Builder("queue name must not be empty".into()));
    }
    // Length is bounded by UTF-8 *byte* count (`str::len()`), not scalar
    // count, matching the canonical 255-byte limit (SEC-010 / PL-003). This
    // byte-based check runs before pattern validation so an oversized value
    // -- including a multibyte string whose scalar count is <= 255 but whose
    // encoded byte length exceeds it -- is always rejected on length grounds.
    if queue.len() > MAX_QUEUE_NAME_BYTES {
        return Err(OjsError::Builder(format!(
            "queue name must not exceed {} bytes, got {}",
            MAX_QUEUE_NAME_BYTES,
            queue.len()
        )));
    }
    let first = queue.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(OjsError::Builder(format!(
            "invalid queue name {:?}: must start with lowercase alphanumeric",
            queue
        )));
    }
    let last = queue.as_bytes()[queue.len() - 1];
    if !(last.is_ascii_lowercase() || last.is_ascii_digit()) {
        return Err(OjsError::Builder(format!(
            "invalid queue name {:?}: must end with lowercase alphanumeric",
            queue
        )));
    }
    let bytes = queue.as_bytes();
    for i in 0..bytes.len() {
        let c = bytes[i] as char;
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') {
            return Err(OjsError::Builder(format!(
                "invalid queue name {:?}: must contain only lowercase alphanumeric, hyphens, and dots",
                queue
            )));
        }
        if (c == '-' || c == '.') && i + 1 < bytes.len() {
            let next = bytes[i + 1] as char;
            if next == '-' || next == '.' {
                return Err(OjsError::Builder(format!(
                    "invalid queue name {:?}: must not contain consecutive separators",
                    queue
                )));
            }
        }
    }
    Ok(())
}

/// Validate every enqueue option constraint enforced locally by this SDK.
///
/// Keeping this as one shared entry point prevents direct enqueue, workflow
/// defaults, workflow steps, and batch callbacks from drifting apart as new
/// option validation is added.
pub(crate) fn validate_enqueue_options(options: &[EnqueueOption]) -> crate::Result<()> {
    for option in options {
        if let EnqueueOption::Queue(queue) = option {
            validate_queue_name(queue)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod shared_path_tests {
    use super::{validate_enqueue_options, validate_job_type};
    use crate::workflow::EnqueueOption;

    #[test]
    fn job_type_length_is_byte_based() {
        // 255 ASCII bytes ok; segments still must match [a-z][a-z0-9_]*.
        let exact = "a".repeat(255);
        assert!(validate_job_type(&exact).is_ok());
        let over = "a".repeat(256);
        let err = validate_job_type(&over).unwrap_err().to_string();
        assert!(err.contains("255 bytes"), "got: {err}");
    }

    #[test]
    fn enqueue_options_path_enforces_byte_length() {
        // This is the shared entry point used by direct enqueue, workflow
        // step options, workflow-level defaults, and batch callbacks.
        let over = "a".repeat(256);
        let opts = vec![EnqueueOption::Queue(over)];
        let err = validate_enqueue_options(&opts).unwrap_err().to_string();
        assert!(
            err.contains("255 bytes"),
            "workflow/defaults path did not enforce 255-byte queue limit: {err}"
        );

        // A multibyte value exceeding 255 bytes is also rejected on length.
        let multibyte = "猫".repeat(200); // 600 bytes
        let opts = vec![EnqueueOption::Queue(multibyte)];
        let err = validate_enqueue_options(&opts).unwrap_err().to_string();
        assert!(err.contains("255 bytes"), "got: {err}");
    }
}

#[cfg(test)]
mod queue_validation_tests {
    use super::validate_queue_name;

    #[test]
    fn valid_queue_names() {
        assert!(validate_queue_name("default").is_ok());
        assert!(validate_queue_name("my-queue").is_ok());
        assert!(validate_queue_name("queue.v2").is_ok());
        assert!(validate_queue_name("email.send.priority").is_ok());
        assert!(validate_queue_name("q").is_ok());
        assert!(validate_queue_name("0-queue").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_queue_name("").is_err());
    }

    #[test]
    fn accepts_exactly_255_bytes() {
        // 255 ASCII bytes == 255 UTF-8 bytes: the canonical maximum.
        let exact = "a".repeat(255);
        assert_eq!(exact.len(), 255);
        assert!(validate_queue_name(&exact).is_ok());
    }

    #[test]
    fn rejects_256_bytes() {
        let over = "a".repeat(256);
        assert_eq!(over.len(), 256);
        let err = validate_queue_name(&over).unwrap_err().to_string();
        assert!(
            err.contains("255 bytes"),
            "expected byte-based length error, got: {err}"
        );
    }

    #[test]
    fn length_limit_is_measured_in_bytes_not_scalars() {
        // The canonical allowed pattern `^[a-zA-Z0-9_.-]{1,255}$` is
        // ASCII-only, so multibyte characters are never *accepted*. This test
        // documents the pragmatic interpretation: the byte-based length check
        // runs *before* pattern validation, so a multibyte string whose
        // Unicode scalar count is <= 255 but whose UTF-8 byte length exceeds
        // 255 is rejected specifically on length grounds.
        //
        // 100 * 'é' (U+00E9, 2 bytes each) = 100 scalars but 200 bytes: under
        // 255 on both axes, so it fails on *pattern* (non-ASCII), not length.
        let under_bytes = "é".repeat(100);
        assert_eq!(under_bytes.chars().count(), 100);
        assert_eq!(under_bytes.len(), 200);
        let err = validate_queue_name(&under_bytes).unwrap_err().to_string();
        assert!(
            !err.contains("255 bytes"),
            "expected a pattern error (not length) for a <=255-byte value: {err}"
        );

        // 200 * '猫' (U+732B, 3 bytes each) = 200 scalars but 600 bytes. A
        // *scalar*-based limit of 255 would wrongly accept the length; the
        // byte-based limit rejects it, and because length is checked first it
        // is reported as a length error rather than a pattern error.
        let over_bytes = "猫".repeat(200);
        assert_eq!(over_bytes.chars().count(), 200);
        assert_eq!(over_bytes.len(), 600);
        let err = validate_queue_name(&over_bytes).unwrap_err().to_string();
        assert!(
            err.contains("255 bytes"),
            "expected byte-based length error for a >255-byte multibyte value: {err}"
        );
    }

    #[test]
    fn rejects_uppercase() {
        assert!(validate_queue_name("MyQueue").is_err());
    }

    #[test]
    fn rejects_trailing_separator() {
        assert!(validate_queue_name("queue.").is_err());
        assert!(validate_queue_name("queue-").is_err());
    }

    #[test]
    fn rejects_leading_separator() {
        assert!(validate_queue_name(".queue").is_err());
        assert!(validate_queue_name("-queue").is_err());
    }

    #[test]
    fn rejects_consecutive_separators() {
        assert!(validate_queue_name("queue..name").is_err());
        assert!(validate_queue_name("queue--name").is_err());
        assert!(validate_queue_name("queue.-name").is_err());
        assert!(validate_queue_name("queue-.name").is_err());
    }

    #[test]
    fn rejects_special_characters() {
        assert!(validate_queue_name("queue@name").is_err());
        assert!(validate_queue_name("queue name").is_err());
    }
}
