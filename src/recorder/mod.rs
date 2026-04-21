//! OJS Rust SDK Recorder — captures execution traces for job handlers.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Source code location for a trace entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceMap {
    pub git_sha: String,
    pub file_path: String,
    pub line: u32,
}

/// A single recorded function call.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceEntry {
    pub func_name: String,
    pub args: String,
    pub result: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_map: Option<SourceMap>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Captures execution traces for a single job handler invocation.
pub struct Recorder {
    entries: Vec<TraceEntry>,
}

impl Recorder {
    /// Create a new empty Recorder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Record a successful function call.
    pub fn record_call(&mut self, func_name: &str, args: &str, result: &str, duration_ms: u64) {
        self.entries.push(TraceEntry {
            func_name: func_name.to_string(),
            args: args.to_string(),
            result: result.to_string(),
            duration_ms,
            source_map: None,
            timestamp: chrono_now(),
            error: None,
        });
    }

    /// Record a failed function call.
    pub fn record_error(&mut self, func_name: &str, args: &str, error: &str, duration_ms: u64) {
        self.entries.push(TraceEntry {
            func_name: func_name.to_string(),
            args: args.to_string(),
            result: String::new(),
            duration_ms,
            source_map: None,
            timestamp: chrono_now(),
            error: Some(error.to_string()),
        });
    }

    /// Attach source location to the most recent trace entry.
    pub fn attach_source_map(&mut self, git_sha: &str, file_path: &str, line: u32) {
        if let Some(entry) = self.entries.last_mut() {
            entry.source_map = Some(SourceMap {
                git_sha: git_sha.to_string(),
                file_path: file_path.to_string(),
                line,
            });
        }
    }

    /// Return a copy of all recorded trace entries.
    pub fn trace(&self) -> Vec<TraceEntry> {
        self.entries.clone()
    }

    /// Number of recorded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the recorder is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all recorded entries.
    pub fn reset(&mut self) {
        self.entries.clear();
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

/// ISO-8601 timestamp using std::time (no chrono dependency).
fn chrono_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Convert epoch seconds to date/time components.
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since 1970-01-01 → (year, month, day) via civil calendar algorithm.
    let (year, month, day) = days_to_date(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Converts days since Unix epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's date library (public domain).
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_call() {
        let mut r = Recorder::new();
        r.record_call("do_work", r#"["a"]"#, r#""ok""#, 42);
        assert_eq!(r.len(), 1);
        let t = &r.trace()[0];
        assert_eq!(t.func_name, "do_work");
        assert_eq!(t.duration_ms, 42);
        assert!(t.error.is_none());
        assert!(!t.timestamp.starts_with("1970"), "timestamp should be current, not epoch");
        assert!(t.timestamp.ends_with('Z'), "timestamp should be UTC");
    }

    #[test]
    fn test_attach_source_map() {
        let mut r = Recorder::new();
        r.record_call("fn", "", "", 1);
        r.attach_source_map("abc", "main.rs", 10);
        let sm = r.trace()[0].source_map.as_ref().unwrap();
        assert_eq!(sm.git_sha, "abc");
        assert_eq!(sm.line, 10);
    }

    #[test]
    fn test_reset() {
        let mut r = Recorder::new();
        r.record_call("fn", "", "", 1);
        r.reset();
        assert!(r.is_empty());
    }
}
