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

use reqwest::Client;
use std::error::Error;
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
    /// SSE channel (e.g., "job:<id>", "queue:<name>").
    pub channel: String,
    /// Bearer auth token (optional).
    pub auth: Option<String>,
}

/// Subscribe to an SSE event stream from the OJS server.
///
/// Returns a receiver channel that yields events as they arrive.
/// Drop the receiver to disconnect.
pub async fn subscribe(
    opts: SubscribeOptions,
) -> Result<mpsc::Receiver<SseEvent>, Box<dyn Error + Send + Sync>> {
    let url = format!(
        "{}/ojs/v1/events/stream?channel={}",
        opts.url.trim_end_matches('/'),
        urlencoding::encode(&opts.channel)
    );

    let client = Client::new();
    let mut req = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");

    if let Some(ref token) = opts.auth {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let response = req.send().await?;

    if !response.status().is_success() {
        return Err(format!("SSE connection failed: {}", response.status()).into());
    }

    let (tx, rx) = mpsc::channel(64);

    tokio::spawn(async move {
        let mut event_type = String::new();
        let mut event_id = String::new();
        let mut event_data = String::new();

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => break,
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    if !event_data.is_empty() {
                        let evt = SseEvent {
                            id: std::mem::take(&mut event_id),
                            event_type: if event_type.is_empty() {
                                "message".to_string()
                            } else {
                                std::mem::take(&mut event_type)
                            },
                            data: std::mem::take(&mut event_data),
                        };
                        if tx.send(evt).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                    event_type.clear();
                } else if let Some(val) = line.strip_prefix("event: ") {
                    event_type = val.to_string();
                } else if let Some(val) = line.strip_prefix("id: ") {
                    event_id = val.to_string();
                } else if let Some(val) = line.strip_prefix("data: ") {
                    event_data = val.to_string();
                }
            }
        }
    });

    Ok(rx)
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
