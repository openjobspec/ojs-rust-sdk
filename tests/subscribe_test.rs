//! Integration tests for the SSE `subscribe()` client end-to-end against a
//! real (mocked) HTTP server, complementing the parser-level unit tests in
//! `src/subscribe.rs`.
#![cfg(feature = "reqwest-transport")]

use ojs::subscribe::{subscribe, SubscribeOptions};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

enum SequenceResponse {
    Status(u16),
    Stream {
        status: u16,
        body: &'static str,
        content_type: &'static str,
    },
}

struct SequenceSseResponder {
    call_count: AtomicUsize,
    responses: Vec<SequenceResponse>,
}

impl SequenceSseResponder {
    fn new(responses: Vec<SequenceResponse>) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            responses,
        }
    }
}

impl Respond for SequenceSseResponder {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        let response = self.responses.get(idx);
        match response {
            Some(SequenceResponse::Status(status)) => ResponseTemplate::new(*status),
            Some(SequenceResponse::Stream {
                status,
                body,
                content_type,
            }) => {
                ResponseTemplate::new(*status).set_body_raw(body.as_bytes().to_vec(), content_type)
            }
            None => ResponseTemplate::new(500),
        }
    }
}

struct LastEventIdResponder {
    call_count: AtomicUsize,
    observed: Arc<Mutex<Vec<Option<String>>>>,
}

impl Respond for LastEventIdResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(
                request
                    .headers
                    .get("last-event-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
            );

        match self.call_count.fetch_add(1, Ordering::SeqCst) {
            0 => ResponseTemplate::new(200).set_body_raw(
                b"id: retained\r\ndata: first\r\n\r\nid:\r\n\r\n".to_vec(),
                "text/event-stream",
            ),
            _ => ResponseTemplate::new(204),
        }
    }
}

#[derive(Default)]
struct SilentServerState {
    connections: AtomicUsize,
    client_closed: AtomicBool,
    client_closed_notify: Notify,
}

impl SilentServerState {
    async fn wait_for_client_close(&self) {
        loop {
            let notified = self.client_closed_notify.notified();
            if self.client_closed.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn mark_client_closed(&self) {
        self.client_closed.store(true, Ordering::SeqCst);
        self.client_closed_notify.notify_waiters();
    }
}

async fn read_http_request(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = socket.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    Ok(())
}

async fn start_silent_stream_server(
) -> (String, Arc<SilentServerState>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = Arc::new(SilentServerState::default());
    let server_state = state.clone();

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let state = server_state.clone();
            state.connections.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                if read_http_request(&mut socket).await.is_err() {
                    return;
                }
                if socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .is_err()
                {
                    return;
                }

                let mut byte = [0u8; 1];
                if matches!(socket.read(&mut byte).await, Ok(0) | Err(_)) {
                    state.mark_client_closed();
                }
            });
        }
    });

    (format!("http://{address}"), state, handle)
}

#[derive(Default)]
struct ReconnectServerState {
    connections: AtomicUsize,
    second_request_started: AtomicBool,
    second_request_started_notify: Notify,
    second_client_closed: AtomicBool,
    second_client_closed_notify: Notify,
}

impl ReconnectServerState {
    async fn wait_for_second_request(&self) {
        loop {
            let notified = self.second_request_started_notify.notified();
            if self.second_request_started.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    async fn wait_for_second_client_close(&self) {
        loop {
            let notified = self.second_client_closed_notify.notified();
            if self.second_client_closed.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

async fn start_pending_reconnect_server() -> (
    String,
    Arc<ReconnectServerState>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = Arc::new(ReconnectServerState::default());
    let server_state = state.clone();

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let connection = server_state.connections.fetch_add(1, Ordering::SeqCst);
            let state = server_state.clone();
            tokio::spawn(async move {
                if read_http_request(&mut socket).await.is_err() {
                    return;
                }
                if connection == 0 {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    return;
                }

                state.second_request_started.store(true, Ordering::SeqCst);
                state.second_request_started_notify.notify_waiters();
                let mut byte = [0u8; 1];
                if matches!(socket.read(&mut byte).await, Ok(0) | Err(_)) {
                    state.second_client_closed.store(true, Ordering::SeqCst);
                    state.second_client_closed_notify.notify_waiters();
                }
            });
        }
    });

    (format!("http://{address}"), state, handle)
}

#[tokio::test]
async fn test_subscribe_receives_events_end_to_end() {
    let server = MockServer::start().await;

    let body = "event: job.completed\r\nid: 1\r\ndata: {\"job_id\":\"a\"}\r\n\r\n\
                data: {\"job_id\":\"b\"}\r\n\r\n";

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .and(query_param("channel", "queue:default"))
        .and(header("Accept", "text/event-stream"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive first event before timeout")
        .expect("channel should not be closed");
    assert_eq!(first.event_type, "job.completed");
    assert_eq!(first.id, "1");
    assert_eq!(first.data, r#"{"job_id":"a"}"#);

    let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive second event before timeout")
        .expect("channel should not be closed");
    // No explicit `event:` field on the second event: defaults to "message".
    assert_eq!(second.event_type, "message");
    assert_eq!(second.data, r#"{"job_id":"b"}"#);
}

#[tokio::test]
async fn test_subscribe_sends_bearer_auth_header() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .and(header("Authorization", "Bearer secret-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"data: hello\n\n".to_vec(), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: Some("secret-token".to_string()),
    })
    .await
    .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event before timeout")
        .expect("channel should not be closed");
    assert_eq!(evt.data, "hello");
}

#[tokio::test]
async fn test_subscribe_fails_immediately_on_non_retryable_status() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let result = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:unknown".to_string(),
        auth: None,
    })
    .await;

    assert!(
        result.is_err(),
        "an initial 404 must surface as an error rather than silently retrying forever"
    );
}

#[tokio::test]
async fn test_subscribe_returns_closed_receiver_on_initial_204() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .expect("204 should be treated as a terminal closed stream, not an error");

    let next = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("closed receiver should resolve promptly");
    assert!(
        next.is_none(),
        "204 should close the stream without reconnecting"
    );
}

#[tokio::test]
async fn test_subscribe_retries_initial_transient_status_in_background() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(SequenceSseResponder::new(vec![
            SequenceResponse::Status(503),
            SequenceResponse::Stream {
                status: 200,
                body: "data: hello\n\n",
                content_type: "text/event-stream",
            },
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .expect("transient initial statuses should retry in the background");

    let evt = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("background retry should reconnect before timeout")
        .expect("channel should deliver an event after the transient failure");
    assert_eq!(evt.data, "hello");
}

#[tokio::test]
async fn test_subscribe_rejects_non_event_stream_content_type() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(br#"{"ok":true}"#.to_vec(), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await;

    assert!(
        result.is_err(),
        "a 200 response without text/event-stream must be rejected"
    );
}

#[tokio::test]
async fn test_subscribe_stops_reconnecting_after_terminal_204_response() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(SequenceSseResponder::new(vec![
            SequenceResponse::Stream {
                status: 200,
                body: "data: first\n\n",
                content_type: "text/event-stream",
            },
            SequenceResponse::Status(204),
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive the first event before timeout")
        .expect("stream should initially produce an event");
    assert_eq!(evt.data, "first");

    let next = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("the terminal 204 reconnect response should close the receiver promptly");
    assert!(
        next.is_none(),
        "204 on reconnect must stop the reconnect loop instead of retrying forever"
    );
}

#[tokio::test]
async fn test_dropping_receiver_cancels_silent_stream_promptly_without_reconnect() {
    let (url, state, server) = start_silent_stream_server().await;
    let rx = subscribe(SubscribeOptions {
        url,
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();

    let dropped_at = tokio::time::Instant::now();
    drop(rx);
    tokio::time::timeout(Duration::from_millis(500), state.wait_for_client_close())
        .await
        .expect("dropping the receiver should cancel an idle body read promptly");
    assert!(dropped_at.elapsed() < Duration::from_millis(500));

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.connections.load(Ordering::SeqCst),
        1,
        "receiver cancellation must not start a reconnect"
    );
    server.abort();
}

#[tokio::test]
async fn test_dropping_receiver_during_backoff_prevents_reconnect() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(Vec::<u8>::new(), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();
    drop(rx);

    tokio::time::sleep(Duration::from_millis(400)).await;
    server.verify().await;
}

#[tokio::test]
async fn test_dropping_receiver_cancels_inflight_reconnect_request() {
    let (url, state, server) = start_pending_reconnect_server().await;
    let rx = subscribe(SubscribeOptions {
        url,
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), state.wait_for_second_request())
        .await
        .expect("the reconnect request should begin after the first empty stream");
    drop(rx);
    tokio::time::timeout(
        Duration::from_millis(500),
        state.wait_for_second_client_close(),
    )
    .await
    .expect("receiver drop should cancel the in-flight reconnect request");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.connections.load(Ordering::SeqCst),
        2,
        "cancelling an in-flight reconnect must stop the reconnect loop"
    );
    server.abort();
}

#[tokio::test]
async fn test_explicit_empty_id_clears_last_event_id_before_reconnect() {
    let server = MockServer::start().await;
    let observed = Arc::new(Mutex::new(Vec::new()));

    Mock::given(method("GET"))
        .and(path("/ojs/v1/events/stream"))
        .respond_with(LastEventIdResponder {
            call_count: AtomicUsize::new(0),
            observed: observed.clone(),
        })
        .expect(2)
        .mount(&server)
        .await;

    let mut rx = subscribe(SubscribeOptions {
        url: server.uri(),
        channel: "queue:default".to_string(),
        auth: None,
    })
    .await
    .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("first event should arrive")
        .expect("stream should remain open for the first event");
    assert_eq!(first.id, "retained");

    assert!(tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("terminal reconnect response should close the receiver")
        .is_none());
    let observed = observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(observed.as_slice(), [None, None]);
}
