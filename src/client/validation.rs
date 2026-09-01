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
