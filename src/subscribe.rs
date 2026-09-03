//! Server-Sent Events (SSE) subscription for real-time OJS job events.
//!
//! # Example
//!
//! ```no_run
//! use ojs::subscribe::{subscribe, SubscribeOptions};
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut stream = subscribe(SubscribeOptions {
//!         url: "http://localhost:8080".to_string(),
//!         channel: "queue:default".to_string(),
//!         auth: None,
//!     }).await.unwrap();
//!
//!     while let Some(event) = stream.recv().await {
//!         println!("Event: {} — {}", event.event_type, event.data);
//!     }
//! }
//! ```

use futures_util::StreamExt;
use reqwest::Client;
use std::error::Error;
use std::time::Duration;
use tokio::sync::mpsc;

/// A single SSE event from the OJS server.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// Event ID (for resume with Last-Event-ID).
    pub id: String,
    /// Event type (e.g., "job.state_changed").
    pub event_type: String,
    /// Raw event data (typically JSON).
    pub data: String,
}

/// Options for subscribing to an SSE stream.
pub struct SubscribeOptions {
    /// Base URL of the OJS server.
    pub url: String,
    /// SSE channel (e.g., `"job:<id>"`, `"queue:<name>"`).
    pub channel: String,
    /// Bearer auth token (optional).
    pub auth: Option<String>,
}

// ---------------------------------------------------------------------------
// Reconnect tuning
// ---------------------------------------------------------------------------

/// Maximum number of bytes buffered while waiting for a complete SSE line.
/// A misbehaving or malicious peer that never sends a newline would
/// otherwise grow this buffer without bound; exceeding this closes the
/// connection (and triggers a reconnect) instead.
const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MiB

const MIN_RECONNECT_BACKOFF: Duration = Duration::from_millis(250);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);

/// Subscribe to an SSE event stream from the OJS server.
///
/// Returns a receiver channel that yields events as they arrive. Drop the
/// receiver to disconnect and stop the background reconnect loop.
///
/// Only an HTTP `200 OK` response with `Content-Type: text/event-stream`
/// starts a stream. `204 No Content` is treated as a terminal "stop
/// reconnecting" response and returns a closed receiver immediately. A `4xx`
/// response (e.g. invalid channel or missing authorization) is treated as
/// permanently non-retryable. Transient connect failures (network errors and
/// `5xx` responses) are retried in the background with bounded exponential
/// backoff, resuming via `Last-Event-ID` when the server sent event IDs.
/// Dropping the returned receiver cancels idle body reads, reconnect sleeps,
/// and in-flight reconnect requests. Per the SSE parsing rules, an explicit
/// empty `id:` clears the stored value and the next reconnect omits the
/// `Last-Event-ID` header.
pub async fn subscribe(
    opts: SubscribeOptions,
) -> Result<mpsc::Receiver<SseEvent>, Box<dyn Error + Send + Sync>> {
    let base_url = opts.url.trim_end_matches('/').to_string();
    let channel = opts.channel;
    let auth = opts.auth;
    let client = Client::new();

    let (tx, rx) = mpsc::channel(64);

    let initial_response = match connect_once(&client, &base_url, &channel, auth.as_deref(), None)
        .await
    {
        Ok(ConnectOutcome::Stream(response)) => Some(response),
        Ok(ConnectOutcome::Closed) => {
            drop(tx);
            return Ok(rx);
        }
        Err(ConnectError::Permanent(err)) => return Err(err),
        Err(ConnectError::Retryable(err)) => {
            tracing::warn!(error = %err, "initial SSE connection failed transiently, retrying in the background");
            None
        }
    };

    tokio::spawn(async move {
        let mut last_event_id: Option<String> = None;
        let mut current = initial_response;

        loop {
            if tx.is_closed() {
                return;
            }

            if let Some(response) = current.take() {
                // `drain_stream` takes the response by value
                // (`bytes_stream()` consumes `self`) and reads it to
                // completion, reporting whether the caller (receiver) is
                // still around.
                if !drain_stream(response, &tx, &mut last_event_id).await {
                    return; // receiver dropped mid-stream
                }
            }

            match reconnect_until_stream(
                &client,
                &base_url,
                &channel,
                auth.as_deref(),
                last_event_id.as_deref(),
                &tx,
            )
            .await
            {
                Some(response) => {
                    current = Some(response);
                }
                None => return,
            }
        }
    });

    Ok(rx)
}

/// Read `response`'s body as an SSE stream until it ends or errors,
/// forwarding parsed events to `tx` and tracking the last seen event ID for
/// resumption. Returns `false` if `tx`'s receiver was dropped (the caller
/// should stop entirely); `true` if the stream simply ended and the caller
/// should attempt to reconnect.
async fn drain_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<SseEvent>,
    last_event_id: &mut Option<String>,
) -> bool {
    let mut parser = SseParser::new();
    let mut stream = response.bytes_stream();

    loop {
        let next_chunk = tokio::select! {
            _ = tx.closed() => return false,
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next_chunk else {
            break;
        };
        let chunk = match chunk {
            Ok(c) => c,
            Err(_) => break,
        };

        match parser.feed(&chunk) {
            Ok(events) => {
                if let Some(id) = parser.take_last_event_id_update() {
                    *last_event_id = if id.is_empty() { None } else { Some(id) };
                }
                for evt in events {
                    if tx.send(evt).await.is_err() {
                        return false; // receiver dropped
                    }
                }
            }
            Err(SseParseError::BufferOverflow) => {
                tracing::warn!(
                    limit = MAX_LINE_BUFFER,
                    "SSE line buffer exceeded limit, reconnecting"
                );
                break;
            }
            Err(SseParseError::InvalidUtf8) => {
                // A complete field line was not valid UTF-8. Treat this as a
                // fatal stream error and reconnect (the same classification
                // used for buffer overflow) rather than forwarding
                // replacement-character-corrupted data to the subscriber.
                tracing::warn!("SSE stream produced invalid UTF-8, reconnecting");
                break;
            }
        }
    }

    true
}

fn reconnect_backoff(attempt: u32) -> Duration {
    let base_ms = MIN_RECONNECT_BACKOFF.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(1u64.checked_shl(attempt.min(20)).unwrap_or(u64::MAX));
    Duration::from_millis(exp_ms.min(MAX_RECONNECT_BACKOFF.as_millis() as u64))
}

enum ConnectError {
    Retryable(Box<dyn Error + Send + Sync>),
    Permanent(Box<dyn Error + Send + Sync>),
}

enum ConnectOutcome {
    Stream(reqwest::Response),
    Closed,
}

impl std::fmt::Debug for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Retryable(e) => write!(f, "ConnectError::Retryable({e})"),
            ConnectError::Permanent(e) => write!(f, "ConnectError::Permanent({e})"),
        }
    }
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Retryable(e) | ConnectError::Permanent(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

async fn connect_once(
    client: &Client,
    base_url: &str,
    channel: &str,
    auth: Option<&str>,
    last_event_id: Option<&str>,
) -> Result<ConnectOutcome, ConnectError> {
    let url = format!(
        "{}/ojs/v1/events/stream?channel={}",
        base_url,
        urlencoding::encode(channel)
    );

    let mut req = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");

    if let Some(token) = auth {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    if let Some(id) = last_event_id {
        req = req.header("Last-Event-ID", id);
    }

    let response = req.send().await.map_err(|e| {
        let is_builder = e.is_builder();
        classify_connect_error(Box::new(e), is_builder)
    })?;

    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(ConnectOutcome::Closed);
    }
    if status != reqwest::StatusCode::OK {
        let err: Box<dyn Error + Send + Sync> = format!("SSE connection failed: {status}").into();
        if status.is_client_error() {
            return Err(ConnectError::Permanent(err));
        }
        return Err(ConnectError::Retryable(err));
    }
    validate_event_stream_content_type(&response)?;

    Ok(ConnectOutcome::Stream(response))
}

async fn reconnect_until_stream(
    client: &Client,
    base_url: &str,
    channel: &str,
    auth: Option<&str>,
    last_event_id: Option<&str>,
    tx: &mpsc::Sender<SseEvent>,
) -> Option<reqwest::Response> {
    let mut attempt = 0u32;

    loop {
        if tx.is_closed() {
            return None;
        }

        let delay = reconnect_backoff(attempt);
        tracing::debug!(
            attempt = attempt + 1,
            delay_ms = delay.as_millis() as u64,
            "SSE stream unavailable, retrying"
        );
        tokio::select! {
            _ = tx.closed() => return None,
            _ = tokio::time::sleep(delay) => {}
        }
        attempt = attempt.saturating_add(1);

        let connect = tokio::select! {
            _ = tx.closed() => return None,
            result = connect_once(client, base_url, channel, auth, last_event_id) => result,
        };
        match connect {
            Ok(ConnectOutcome::Stream(response)) => return Some(response),
            Ok(ConnectOutcome::Closed) => return None,
            Err(ConnectError::Permanent(err)) => {
                tracing::warn!(
                    error = %err,
                    "SSE reconnect failed permanently, giving up"
                );
                return None;
            }
            Err(ConnectError::Retryable(err)) => {
                tracing::warn!(error = %err, "SSE reconnect failed transiently");
            }
        }
    }
}

fn classify_connect_error(
    err: Box<dyn Error + Send + Sync>,
    is_builder_error: bool,
) -> ConnectError {
    if is_builder_error {
        ConnectError::Permanent(err)
    } else {
        ConnectError::Retryable(err)
    }
}

fn validate_event_stream_content_type(response: &reqwest::Response) -> Result<(), ConnectError> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);

    let Some(content_type) = content_type else {
        return Err(ConnectError::Permanent(
            "SSE connection failed: missing Content-Type header".into(),
        ));
    };

    let media_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();

    if media_type.eq_ignore_ascii_case("text/event-stream") {
        Ok(())
    } else {
        Err(ConnectError::Permanent(
            format!(
                "SSE connection failed: expected Content-Type text/event-stream, got {content_type}"
            )
            .into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// SSE line/event parsing
// ---------------------------------------------------------------------------

/// Error returned by [`SseParser::feed`]: the caller should treat either
/// variant as a fatal stream error and reconnect rather than continuing.
#[derive(Debug)]
enum SseParseError {
    /// Buffering an incomplete line would exceed [`MAX_LINE_BUFFER`].
    BufferOverflow,
    /// A *complete* field line was not valid UTF-8.
    InvalidUtf8,
}

/// Incremental SSE parser: accumulates raw bytes across `feed()` calls and
/// yields complete events, handling `\n` and `\r\n` line endings and
/// bounding how much unterminated data it will buffer.
///
/// The buffer holds raw bytes (not a `String`) so that a multibyte UTF-8
/// sequence split across transport chunks is preserved intact: only
/// *complete* lines (terminated by a `\n` byte) are UTF-8 decoded, and a
/// UTF-8 lead/continuation byte can never be `0x0A`, so splitting on the
/// newline byte never bisects a character. Decoding is strict
/// ([`std::str::from_utf8`]): invalid bytes surface as
/// [`SseParseError::InvalidUtf8`] rather than being silently replaced with
/// U+FFFD.
struct SseParser {
    event_type: String,
    event_id: String,
    id_field_present: bool,
    event_data: String,
    buffer: Vec<u8>,
}

impl SseParser {
    fn new() -> Self {
        Self {
            event_type: String::new(),
            event_id: String::new(),
            id_field_present: false,
            event_data: String::new(),
            buffer: Vec::new(),
        }
    }

    /// Feed a chunk of raw transport bytes, returning any complete events.
    ///
    /// Returns `Err(SseParseError::BufferOverflow)` if buffering an
    /// incomplete line would exceed [`MAX_LINE_BUFFER`], or
    /// `Err(SseParseError::InvalidUtf8)` if a complete line is not valid
    /// UTF-8; the caller should treat either as a fatal stream error
    /// (reconnect) rather than continuing to buffer unbounded, potentially
    /// attacker-controlled data or forwarding corrupted text.
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseParseError> {
        self.buffer.extend_from_slice(chunk);

        let mut events = Vec::new();

        // Split complete lines on the raw newline *byte* (0x0A). Partial
        // multibyte sequences in an as-yet-unterminated line stay buffered as
        // raw bytes until the rest of the character arrives in a later chunk.
        while let Some(newline_pos) = self.buffer.iter().position(|&b| b == b'\n') {
            if newline_pos > MAX_LINE_BUFFER {
                return Err(SseParseError::BufferOverflow);
            }
            let mut line_bytes: Vec<u8> = self.buffer.drain(..=newline_pos).collect();
            // Remove the trailing '\n', then a trailing '\r' so CRLF-terminated
            // streams (common with many HTTP servers/proxies) don't leak '\r'
            // into field values.
            line_bytes.pop();
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            // Strictly decode the *complete* line; never lossily replace.
            let line = std::str::from_utf8(&line_bytes).map_err(|_| SseParseError::InvalidUtf8)?;

            if line.is_empty() {
                if !self.event_data.is_empty() {
                    events.push(SseEvent {
                        id: self.event_id.clone(),
                        event_type: if self.event_type.is_empty() {
                            "message".to_string()
                        } else {
                            std::mem::take(&mut self.event_type)
                        },
                        data: std::mem::take(&mut self.event_data),
                    });
                }
                self.event_type.clear();
            } else if let Some(val) = line.strip_prefix("event:") {
                self.event_type = strip_one_leading_space(val).to_string();
            } else if let Some(val) = line.strip_prefix("id:") {
                self.event_id = strip_one_leading_space(val).to_string();
                self.id_field_present = true;
            } else if let Some(val) = line.strip_prefix("data:") {
                let value = strip_one_leading_space(val);
                if self.event_data.is_empty() {
                    self.event_data = value.to_string();
                } else {
                    self.event_data.push('\n');
                    self.event_data.push_str(value);
                }
            }
            // Unrecognized fields (e.g. `retry:`, comments starting with
            // `:`) are intentionally ignored rather than erroring, per the
            // permissive WHATWG EventSource parsing model.
        }

        if self.buffer.len() > MAX_LINE_BUFFER {
            return Err(SseParseError::BufferOverflow);
        }

        Ok(events)
    }

    /// Return the most recent `id:` field parsed since the previous call.
    ///
    /// Presence is tracked separately from the value so an explicit empty
    /// `id:` produces `Some("")`, allowing callers to clear Last-Event-ID,
    /// while no ID field produces `None`.
    fn take_last_event_id_update(&mut self) -> Option<String> {
        if std::mem::take(&mut self.id_field_present) {
            Some(self.event_id.clone())
        } else {
            None
        }
    }
}

/// Per the SSE field-parsing algorithm, only a single leading space after
/// the colon is stripped -- not all leading whitespace -- so a data payload
/// that intentionally starts with extra spaces round-trips correctly.
fn strip_one_leading_space(s: &str) -> &str {
    s.strip_prefix(' ').unwrap_or(s)
}

/// Subscribe to events for a specific job.
pub async fn subscribe_job(
    url: &str,
    job_id: &str,
    auth: Option<String>,
) -> Result<mpsc::Receiver<SseEvent>, Box<dyn Error + Send + Sync>> {
    subscribe(SubscribeOptions {
        url: url.to_string(),
        channel: format!("job:{}", job_id),
        auth,
    })
    .await
}

/// Subscribe to events for all jobs in a queue.
pub async fn subscribe_queue(
    url: &str,
    queue: &str,
    auth: Option<String>,
) -> Result<mpsc::Receiver<SseEvent>, Box<dyn Error + Send + Sync>> {
    subscribe(SubscribeOptions {
        url: url.to_string(),
        channel: format!("queue:{}", queue),
        auth,
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_lf_terminated_event() {
        let mut parser = SseParser::new();
        let events = parser
            .feed(b"event: job.completed\nid: 42\ndata: {\"ok\":true}\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "job.completed");
        assert_eq!(events[0].id, "42");
        assert_eq!(events[0].data, "{\"ok\":true}");
    }

    #[test]
    fn test_parses_crlf_terminated_event_without_trailing_cr() {
        let mut parser = SseParser::new();
        let events = parser
            .feed(b"event: job.completed\r\nid: 42\r\ndata: {\"ok\":true}\r\n\r\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        // Before the fix, these would retain a trailing '\r'.
        assert_eq!(events[0].event_type, "job.completed");
        assert_eq!(events[0].id, "42");
        assert_eq!(events[0].data, "{\"ok\":true}");
        assert!(!events[0].id.contains('\r'));
        assert!(!events[0].event_type.contains('\r'));
        assert!(!events[0].data.contains('\r'));
    }

    #[test]
    fn test_defaults_to_message_event_type() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: hello\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
    }

    #[test]
    fn test_multi_line_data_joined_with_newline() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: line1\ndata: line2\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn test_feed_across_multiple_chunks() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"data: par").unwrap().is_empty());
        assert!(parser.feed(b"tial\n").unwrap().is_empty());
        let events = parser.feed(b"\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "partial");
    }

    #[test]
    fn test_fragmented_crlf_empty_id_clears_last_event_id() {
        let mut parser = SseParser::new();

        assert!(parser.feed(b"id: retained\r").unwrap().is_empty());
        let first = parser.feed(b"\ndata: first\r\n\r\nid:\r").unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, "retained");
        assert_eq!(
            parser.take_last_event_id_update().as_deref(),
            Some("retained")
        );

        assert!(parser.feed(b"\n\r\n").unwrap().is_empty());
        assert_eq!(
            parser.take_last_event_id_update().as_deref(),
            Some(""),
            "an explicit empty id field must be distinguishable from no id field"
        );

        let next = parser.feed(b"data: after reset\r\n\r\n").unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].id, "");
        assert_eq!(parser.take_last_event_id_update(), None);
    }

    #[test]
    fn test_only_one_leading_space_is_stripped() {
        let mut parser = SseParser::new();
        // Two leading spaces: only the first (the one directly after the
        // colon) is part of the field-parsing algorithm's strip rule.
        let events = parser.feed(b"data:  extra space\n\n").unwrap();
        assert_eq!(events[0].data, " extra space");
    }

    #[test]
    fn test_unbounded_buffer_growth_is_rejected() {
        let mut parser = SseParser::new();
        // Never send a newline: without a bound this would grow forever.
        let chunk = vec![b'x'; MAX_LINE_BUFFER + 1];
        assert!(parser.feed(&chunk).is_err());
    }

    #[test]
    fn test_reconnect_backoff_is_bounded_and_increasing() {
        let d0 = reconnect_backoff(0);
        let d1 = reconnect_backoff(1);
        let d_large = reconnect_backoff(63);
        assert!(d0 >= MIN_RECONNECT_BACKOFF);
        assert!(d1 >= d0);
        assert!(d_large <= MAX_RECONNECT_BACKOFF);
    }

    /// Feed a full byte sequence one byte at a time, returning all events.
    fn feed_one_byte_at_a_time(
        parser: &mut SseParser,
        bytes: &[u8],
    ) -> Result<Vec<SseEvent>, SseParseError> {
        let mut events = Vec::new();
        for &b in bytes {
            events.extend(parser.feed(&[b])?);
        }
        Ok(events)
    }

    #[test]
    fn test_one_byte_chunks_preserve_multibyte_event_id_and_data() {
        // Event type, id, and data all contain multibyte UTF-8 whose bytes
        // are split across single-byte transport chunks. Nothing may be
        // corrupted with U+FFFD.
        let mut parser = SseParser::new();
        let raw =
            "event: café.updated\nid: naïve-42\ndata: {\"emoji\":\"🚀\",\"city\":\"東京\"}\n\n";
        let events = feed_one_byte_at_a_time(&mut parser, raw.as_bytes()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "café.updated");
        assert_eq!(events[0].id, "naïve-42");
        assert_eq!(events[0].data, "{\"emoji\":\"🚀\",\"city\":\"東京\"}");
        assert!(!events[0].data.contains('\u{FFFD}'));
        assert!(!events[0].id.contains('\u{FFFD}'));
        assert!(!events[0].event_type.contains('\u{FFFD}'));
    }

    #[test]
    fn test_one_byte_chunks_with_crlf() {
        let mut parser = SseParser::new();
        let raw = "event: café\r\nid: 7\r\ndata: 日本語\r\n\r\n";
        let events = feed_one_byte_at_a_time(&mut parser, raw.as_bytes()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "café");
        assert_eq!(events[0].id, "7");
        assert_eq!(events[0].data, "日本語");
        assert!(!events[0].data.contains('\r'));
    }

    #[test]
    fn test_partial_multibyte_across_chunk_boundary_is_not_corrupted() {
        // Split the two bytes of 'é' (0xC3 0xA9) across two feed() calls.
        let mut parser = SseParser::new();
        assert!(parser.feed(b"data: caf").unwrap().is_empty());
        assert!(parser.feed(&[0xC3]).unwrap().is_empty()); // lead byte only
        assert!(parser.feed(&[0xA9]).unwrap().is_empty()); // continuation byte
        let events = parser.feed(b"\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "café");
        assert!(!events[0].data.contains('\u{FFFD}'));
    }

    #[test]
    fn test_invalid_utf8_in_complete_line_returns_controlled_error() {
        // A lone 0xFF is never valid UTF-8. Once the line is complete
        // (newline seen), strict decoding must surface a controlled error
        // rather than a panic or replacement-character corruption.
        let mut parser = SseParser::new();
        let mut bytes = b"data: ".to_vec();
        bytes.push(0xFF);
        bytes.push(0xFE);
        bytes.extend_from_slice(b"\n\n");
        let err = parser.feed(&bytes).unwrap_err();
        assert!(matches!(err, SseParseError::InvalidUtf8));
    }

    #[test]
    fn test_invalid_utf8_one_byte_chunks_returns_controlled_error() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"d").unwrap().is_empty());
        assert!(parser.feed(b"a").unwrap().is_empty());
        assert!(parser.feed(b"t").unwrap().is_empty());
        assert!(parser.feed(b"a").unwrap().is_empty());
        assert!(parser.feed(b":").unwrap().is_empty());
        assert!(parser.feed(b" ").unwrap().is_empty());
        assert!(parser.feed(&[0x80]).unwrap().is_empty()); // stray continuation
                                                           // Line still incomplete; error only fires once the line completes.
        let err = parser.feed(b"\n").unwrap_err();
        assert!(matches!(err, SseParseError::InvalidUtf8));
    }

    #[test]
    fn test_large_chunk_with_individually_bounded_lines_is_accepted() {
        let line = format!("ignored:{}\n", "x".repeat(MAX_LINE_BUFFER / 2));
        let chunk = format!("{line}{line}");
        assert!(chunk.len() > MAX_LINE_BUFFER);

        let mut parser = SseParser::new();
        assert!(parser.feed(chunk.as_bytes()).unwrap().is_empty());
    }
}
