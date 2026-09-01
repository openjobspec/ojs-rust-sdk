use crate::errors::OjsError;

const MAX_TYPE_LENGTH: usize = 255;
const MAX_QUEUE_LENGTH: usize = 128;

pub(crate) fn validate_job_type(job_type: &str) -> crate::Result<()> {
    if job_type.is_empty() {
        return Err(OjsError::Builder("job type must not be empty".into()));
    }
    if job_type.len() > MAX_TYPE_LENGTH {
        return Err(OjsError::Builder(format!(
            "job type must not exceed {} characters, got {}",
            MAX_TYPE_LENGTH,
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
    if queue.len() > MAX_QUEUE_LENGTH {
        return Err(OjsError::Builder(format!(
            "queue name must not exceed {} characters, got {}",
            MAX_QUEUE_LENGTH,
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
    fn rejects_too_long() {
        let long = "a".repeat(129);
        assert!(validate_queue_name(&long).is_err());
    }

    #[test]
    fn accepts_max_length() {
        let exact = "a".repeat(128);
        assert!(validate_queue_name(&exact).is_ok());
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
