# ojs - Open Job Spec SDK for Rust

[![CI](https://github.com/openjobspec/ojs-rust-sdk/actions/workflows/ci.yml/badge.svg)](https://github.com/openjobspec/ojs-rust-sdk/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/ojs.svg)](https://crates.io/crates/ojs)
[![docs.rs](https://docs.rs/ojs/badge.svg)](https://docs.rs/ojs)
[![License](https://img.shields.io/crates/l/ojs.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.75-blue.svg)](https://blog.rust-lang.org/2023/12/28/Rust-1.75.0.html)

The official Rust SDK for the [Open Job Spec](https://openjobspec.org) (OJS) protocol. OJS is a language-agnostic specification for background job processing, providing interoperability across languages and backends.

## Features

- **Async-first** - Built on `tokio` for high-performance async I/O
- **Type-safe** - Strong typing with `serde` serialization/deserialization
- **Middleware** - Tower-inspired middleware chain for cross-cutting concerns (logging, tracing, metrics)
- **Workflows** - Chain, group, and batch workflow primitives
- **Builder pattern** - Ergonomic client and worker configuration
- **Full OJS compliance** - Implements OJS v1.0.0-rc.1 specification

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
ojs = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

## Quick Start

### Enqueuing Jobs (Producer)

```rust
use ojs::{Client, RetryPolicy};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> ojs::Result<()> {
    let client = Client::builder()
        .url("http://localhost:8080")
        .build()?;

    // Simple enqueue
    let job = client
        .enqueue("email.send", json!({"to": "user@example.com"}))
        .await?;

    // Enqueue with options
    let job = client
        .enqueue("report.generate", json!({"id": 42}))
        .queue("reports")
        .delay(Duration::from_secs(300))
        .retry(RetryPolicy::new().max_attempts(5))
        .send()
        .await?;

    Ok(())
}
```

### Processing Jobs (Consumer)

```rust
use ojs::{Worker, JobContext};
use serde_json::json;

#[tokio::main]
async fn main() -> ojs::Result<()> {
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .queues(vec!["default", "email"])
        .concurrency(10)
        .build()?;

    worker.register("email.send", |ctx: JobContext| async move {
        let to: String = ctx.job.arg("to")?;
        // send the email...
        Ok(json!({"message_id": "msg_123"}))
    }).await;

    worker.start().await?;
    Ok(())
}
```

### Workflows

```rust
use ojs::{Client, chain, group, batch, Step, BatchCallbacks};
use serde_json::json;

#[tokio::main]
async fn main() -> ojs::Result<()> {
    let client = Client::builder()
        .url("http://localhost:8080")
        .build()?;

    // Chain: sequential execution (A -> B -> C)
    let workflow = client.create_workflow(
        chain(vec![
            Step::new("data.fetch", json!({"url": "https://api.example.com"})),
            Step::new("data.transform", json!({"format": "csv"})),
            Step::new("data.notify", json!({"channel": "slack"})),
        ]).name("ETL Pipeline")
    ).await?;

    // Group: parallel execution
    let workflow = client.create_workflow(
        group(vec![
            Step::new("export.csv", json!({"id": 1})),
            Step::new("export.pdf", json!({"id": 1})),
        ])
    ).await?;

    // Batch: parallel with callbacks
    let workflow = client.create_workflow(
        batch(
            BatchCallbacks::new()
                .on_complete(Step::new("report", json!({}))),
            vec![
                Step::new("email.send", json!({"to": "a@b.com"})),
                Step::new("email.send", json!({"to": "c@d.com"})),
            ],
        )
    ).await?;

    Ok(())
}
```

### Middleware

```rust
use ojs::{Worker, Middleware, Next, JobContext, BoxFuture, HandlerResult};

struct LoggingMiddleware;

impl Middleware for LoggingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = next.run(ctx).await;
            println!("Job processed in {:?}", start.elapsed());
            result
        })
    }
}

#[tokio::main]
async fn main() -> ojs::Result<()> {
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .build()?;

    worker.use_middleware("logging", LoggingMiddleware).await;
    // register handlers...
    worker.start().await
}
```

## API Reference

### Client

| Method | Description |
|--------|-------------|
| `enqueue(type, args)` | Enqueue a single job |
| `enqueue_batch(requests)` | Atomically enqueue multiple jobs |
| `get_job(id)` | Get job details |
| `cancel_job(id)` | Cancel a job |
| `create_workflow(def)` | Create a workflow |
| `get_workflow(id)` | Get workflow status |
| `cancel_workflow(id)` | Cancel a workflow |
| `list_queues()` | List all queues |
| `get_queue_stats(name)` | Get queue statistics |
| `pause_queue(name)` | Pause a queue |
| `resume_queue(name)` | Resume a queue |
| `list_dead_letter_jobs(...)` | List dead letter jobs |
| `retry_dead_letter_job(id)` | Retry a dead letter job |
| `list_cron_jobs()` | List cron jobs |
| `register_cron_job(req)` | Register a cron job |
| `health()` | Server health check |
| `manifest()` | Server conformance manifest |

### Worker

| Method | Description |
|--------|-------------|
| `register(type, handler)` | Register a job handler |
| `use_middleware(name, mw)` | Add middleware |
| `start()` | Start processing (blocks until shutdown) |
| `state()` | Get current worker state |
| `id()` | Get worker ID |

## MSRV

The minimum supported Rust version is **1.75**.

## License

Apache-2.0
