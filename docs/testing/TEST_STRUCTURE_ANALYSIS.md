# OJS Rust SDK Test Structure Analysis

## Project Overview
- **SDK Name**: OJS (Open Job Spec) Rust SDK
- **Version**: 0.2.0
- **Rust Edition**: 2021
- **Min Rust Version**: 1.75

---

## Cargo.toml Configuration

### Key Dependencies
- **Core**: `tokio` (async runtime), `serde`/`serde_json`, `thiserror` (error handling)
- **HTTP**: `reqwest` (optional, feature: `reqwest-transport`)
- **Crypto**: `aes-gcm`, `base64` (optional, feature: `encryption`)
- **Tracing**: `tracing` (structured logging)
- **Telemetry**: `opentelemetry` (optional, feature: `otel-middleware`)
- **Serverless**: `lambda_runtime`, `aws_lambda_events` (optional, feature: `serverless-lambda`)

### Features
```
default = ["reqwest-transport"]
reqwest-transport     # HTTP client transport
testing               # Testing utilities
tracing-middleware   # Tracing middleware
common-middleware    # Built-in middleware (logging, timeout, metrics)
otel-middleware      # OpenTelemetry integration
serverless-lambda    # AWS Lambda support
encryption           # AES-256-GCM encryption
```

### Dev Dependencies
- `tokio` (full features)
- `wiremock` (HTTP mocking)
- `proptest` (property testing)

---

## Module Structure (`src/lib.rs`)

### Core Modules
| Module | Purpose | Exports |
|--------|---------|---------|
| `client` | Job enqueueing | `Client`, `ClientBuilder`, `EnqueueBuilder` |
| `worker` | Job processing | `Worker`, `WorkerBuilder`, `JobContext`, `WorkerState` |
| `job` | Job data structures | `Job`, `JobState`, `UniquePolicy` |
| `errors` | Error types | `OjsError`, `ServerError`, `JobError`, `Result` |
| `middleware` | Middleware system | `Middleware`, `Next`, `FnMiddleware`, `BoxFuture` |
| `retry` | Retry policies | `RetryPolicy`, `OnExhaustion` |
| `queue` | Queue operations | `Queue`, `QueueStats`, `Manifest` |
| `schema` | Schema validation | `Schema`, `RegisterSchemaRequest` |
| `events` | Event types | `Event` |

### Optional Modules
- `testing` (feature: testing)
- `tracing_mw` (feature: tracing-middleware)
- `encryption` (feature: encryption)
- `middleware_common` (feature: common-middleware)
- `otel` (feature: otel-middleware)
- `serverless` (feature: serverless-lambda)

---

## Error Handling (`src/errors.rs`)

### Error Types Hierarchy
```
OjsError (main enum):
├── Server(Box<ServerError>)           # Server returned error
├── Transport(String)                  # HTTP transport error
├── Serialization(String)              # JSON serialization error
├── Handler(String)                    # Retryable handler error
├── NonRetryable(String)               # Non-retryable handler error
├── Builder(String)                    # Configuration error
├── NoHandler(String)                  # No handler registered
└── WorkerShutdown                     # Worker was shut down
```

### ServerError Structure
```rust
pub struct ServerError {
    pub code: String,                  // Machine-readable code (ERR_*)
    pub message: String,               // Human-readable message
    pub retryable: bool,
    pub details: Option<HashMap<...>>,
    pub request_id: Option<String>,
    pub http_status: u16,
    pub retry_after: Option<Duration>, // From Retry-After header
    pub rate_limit: Option<RateLimitInfo>, // Rate limit metadata
}
```

### Error Codes (from OJS spec)
- `ERR_HANDLER_ERROR`
- `ERR_TIMEOUT`
- `ERR_CANCELLED`
- `ERR_INVALID_PAYLOAD`
- `ERR_RATE_LIMITED`
- `ERR_DUPLICATE`
- `ERR_QUEUE_PAUSED`
- `ERR_SCHEMA_VALIDATION`
- And 13 more...

---

## Worker (`src/worker.rs`) - ~700+ lines

### WorkerState Enum
```rust
pub enum WorkerState {
    Running = 0,        // Fetching and processing jobs
    Quiet = 1,         // No new jobs, finishing active ones
    Terminate = 2,     // Shutting down
}
```

### JobContext (passed to handlers)
```rust
pub struct JobContext {
    pub job: Job,
    pub attempt: u32,                      // 1-indexed attempt number
    pub queue: String,
    pub workflow_id: Option<String>,
    pub parent_results: Option<HashMap<String, Value>>,
    
    // Internal
    transport: DynTransport,
    worker_id: String,
}

impl JobContext {
    pub async fn heartbeat(&self) -> Result<()>  // Extend visibility timeout
}
```

### WorkerBuilder
Key configuration:
```rust
pub struct WorkerBuilder {
    url: Option<String>,
    queues: Vec<String>,                   // Default: ["default"]
    concurrency: usize,                    // Default: 10
    grace_period: Duration,                // Default: 25s
    heartbeat_interval: Duration,          // Default: 5s
    poll_interval: Duration,               // Default: 1s
    labels: Vec<String>,
    auth_token: Option<String>,
    headers: HashMap<String, String>,
    timeout: Option<Duration>,
    retry_config: Option<RetryConfig>,
    #[cfg(feature = "reqwest-transport")]
    http_client: Option<reqwest::Client>,
}
```

### Worker Main Methods
```rust
pub async fn register<F, Fut>(&self, job_type, handler)
    where F: Fn(JobContext) -> Fut, Fut: Future<Output = HandlerResult>

pub async fn register_typed<T, F, Fut>(&self, job_type, handler)
    // Auto-deserializes job args into type T

pub async fn use_middleware(&self, name, mw: impl Middleware)

pub async fn remove_middleware(&self, name)
pub async fn insert_middleware_before(&self, existing, name, mw)
pub async fn insert_middleware_after(&self, existing, name, mw)

pub async fn start(&self) -> Result<()>
    // Main entry point - blocks until shutdown

pub fn state(&self) -> WorkerState
pub fn id(&self) -> &str
```

### Worker Internals
```rust
pub struct Worker {
    transport: DynTransport,
    worker_id: String,
    queues: Vec<String>,
    concurrency: usize,
    grace_period: Duration,
    heartbeat_interval: Duration,
    poll_interval: Duration,
    labels: Vec<String>,
    handlers: Arc<RwLock<HashMap<String, HandlerFn>>>,
    middleware: Arc<RwLock<MiddlewareChain>>,
    state: Arc<AtomicU8>,
    active_count: Arc<AtomicI64>,
    active_jobs: Arc<RwLock<HashSet<String>>>,
}
```

---

## Middleware System (`src/middleware.rs`)

### Core Types
```rust
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type HandlerResult = Result<serde_json::Value, OjsError>;
pub type HandlerFn = Arc<dyn Fn(JobContext) -> BoxFuture<'static, HandlerResult> + Send + Sync>;

pub struct Next {
    // Represents the next handler in the middleware chain
    pub fn run(self, ctx: JobContext) -> BoxFuture<'static, HandlerResult>
}
```

### Middleware Trait
```rust
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult>;
}
```

### MiddlewareChain (internal)
```rust
pub struct MiddlewareChain {
    pub fn add(&mut self, name, mw)
    pub fn prepend(&mut self, name, mw)
    pub fn insert_before(&mut self, existing, name, mw)
    pub fn insert_after(&mut self, existing, name, mw)
    pub fn remove(&mut self, name)
    pub fn wrap(&self, handler) -> HandlerFn  // Build final handler
}
```

### FnMiddleware (closure wrapper)
```rust
pub struct FnMiddleware<F> {
    f: F
}

impl<F, Fut> FnMiddleware<F>
where F: Fn(JobContext, Next) -> Fut,
      Fut: Future<Output = HandlerResult>
{
    pub fn new(f: F) -> Self
}
```

---

## Test Files Structure

### Test Files Inventory (15 test files)
```
tests/
├── benchmark_test.rs           (105 lines) - Performance benchmarks
├── client_test.rs              (84 lines)  - Client builder tests
├── error_test.rs               (237 lines) - Error parsing & handling
├── events_test.rs              (199 lines) - Event serialization
├── http_test.rs                (705 lines) - ⭐ MOST COMPREHENSIVE - HTTP client tests
├── integration_test.rs          (331 lines) - End-to-end scenarios
├── job_test.rs                 (346 lines) - Job data structure tests
├── middleware_test.rs           (202 lines) - Middleware system tests
├── proptest_test.rs            (120 lines) - Property-based tests
├── queue_test.rs               (267 lines) - Queue operations
├── rate_limiter_test.rs        (263 lines) - Rate limiting & retries
├── schema_test.rs              (168 lines) - Schema validation
├── testing_test.rs             (315 lines) - Testing utilities
├── worker_integration_test.rs   (312 lines) - Worker with mock server
├── worker_test.rs              (53 lines)  - Worker basics (MINIMAL)
└── Total: 3,707 lines
```

---

## Testing Patterns & Infrastructure

### HTTP Mocking (`wiremock`)
```rust
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path, header};

// Setup mock server
let server = MockServer::start().await;

// Create mock expectation
Mock::given(method("POST"))
    .and(path("/ojs/v1/jobs"))
    .and(header("Authorization", "Bearer token"))
    .respond_with(ResponseTemplate::new(201).set_body_json(response))
    .expect(1)  // Verify called exactly once
    .mount(&server)
    .await;

// Use server.uri() in client config
```

### Test Helpers
- `FetchResponder`: Custom responder that changes response after first call
- `TestMiddleware`: Helper middleware for verification
- `job_response()`: Factory for mock job responses
- `test_worker()`: Helper to create minimal worker

### Common Test Patterns

#### 1. HTTP Client Tests (`http_test.rs` - 705 lines)
- **21 test cases** covering:
  - Simple enqueue
  - Enqueue with options (queue, priority, retry, tags)
  - Auth token handling
  - Batch enqueue
  - Get job details
  - Update job state
  - Error responses
  - Rate limiting
  - Schema validation
  - Retry behavior
  - Custom headers
  - Timeout handling

#### 2. Worker Integration Tests (`worker_integration_test.rs` - 312 lines)
- **4 test cases** for main scenarios:
  - `test_worker_processes_and_acks_job()` - Success path
  - `test_worker_nacks_on_handler_error()` - Error path
  - Job heartbeats
  - Custom responders for stateful behavior

#### 3. Middleware Tests (`middleware_test.rs` - 202 lines)
- **6 test cases** (mostly registration/no-op tests):
  - Single middleware wrapping
  - Multiple middleware ordering
  - Passthrough middleware
  - Short-circuit middleware
  - FnMiddleware creation from closure
  - Side effects verification

#### 4. Worker Tests (`worker_test.rs` - 53 lines)
- **3 test cases** (MINIMAL):
  - Builder requires URL
  - Builder with defaults
  - Builder with all options
  - ⚠️ **No handler execution tests**
  - ⚠️ **No actual job processing tests**

#### 5. Error Tests (`error_test.rs` - 237 lines)
- Parsing server error responses
- Error code handling
- Rate limit parsing
- Retry-After header extraction
- Error serialization

#### 6. Job Tests (`job_test.rs` - 346 lines)
- Job structure validation
- Argument extraction
- State transitions
- Unique policy handling
- Workflow integration

#### 7. Rate Limiter Tests (`rate_limiter_test.rs` - 263 lines)
- Exponential backoff calculation
- Retry config variations
- Rate limit header parsing
- Disabled retry behavior

#### 8. Queue Tests (`queue_test.rs` - 267 lines)
- Queue information fetching
- Stats parsing
- Health status
- Pagination

#### 9. Schema Tests (`schema_test.rs` - 168 lines)
- Schema registration
- Validation
- Schema detail retrieval

---

## Coverage Analysis - What's Tested vs. What's Missing

### ✅ Well-Tested Areas
1. **HTTP Transport** (`http_test.rs`)
   - All HTTP methods and endpoints
   - Header handling
   - Response parsing
   - Error scenarios

2. **Error Handling** (`error_test.rs`)
   - All error types
   - Rate limit info
   - Retry-After parsing

3. **Job Data** (`job_test.rs`)
   - Serialization/deserialization
   - Field extraction
   - State management

4. **Client Enqueue** (`http_test.rs`)
   - Simple and batch operations
   - Options (queue, priority, retry, tags)
   - Auth tokens

5. **Middleware Chain** (unit tests in `middleware.rs`)
   - Chain construction
   - Ordering
   - Addition/removal

### ⚠️ Needs More Testing

#### 1. **Worker Job Processing** - CRITICAL GAPS
- `worker_test.rs` has only 53 lines, mostly builder tests
- No tests for:
  - ✗ Handler execution with actual job data
  - ✗ Middleware chain execution during job processing
  - ✗ Concurrent job processing (concurrency > 1)
  - ✗ State transitions (Running → Quiet → Terminate)
  - ✗ Grace period behavior on shutdown
  - ✗ Active job tracking
  - ✗ Fetch loop with backpressure

#### 2. **Worker Integration** - MINIMAL COVERAGE
- `worker_integration_test.rs` has only 4 tests:
  - ✓ Success ACK path
  - ✓ Failure NACK path
  - ✗ Heartbeat loop
  - ✗ Graceful shutdown
  - ✗ Quiet mode
  - ✗ Multiple job processing
  - ✗ Handler errors vs. non-retryable errors

#### 3. **Middleware Execution**
- `middleware_test.rs` tests chain construction but:
  - ✗ No tests for actual handler invocation with middleware
  - ✗ No tests for middleware error handling
  - ✗ No tests for middleware ordering during execution
  - ✗ No tests for FnMiddleware with closures capturing state

#### 4. **Handler Registration**
- ✗ No tests for `register_typed()` with type deserialization
- ✗ No tests for handler overwriting
- ✗ No tests for invalid handler return values

#### 5. **Worker Lifecycle**
- ✗ Signal handling (Ctrl+C)
- ✗ State transitions
- ✗ Grace period enforcement
- ✗ Timeout handling on job processing

#### 6. **Concurrent Operations**
- ✗ Multiple handlers processing concurrently
- ✗ Semaphore/concurrency limiting
- ✗ Active job count tracking

#### 7. **Edge Cases**
- ✗ Empty job fetch response
- ✗ Malformed job responses
- ✗ Connection failures during fetch
- ✗ Timeout during handler execution
- ✗ Very large job payloads

#### 8. **Optional Features**
- ✗ Encryption middleware not tested
- ✗ Tracing middleware not tested
- ✗ Common middleware (logging, metrics, timeout) not tested
- ✗ OpenTelemetry integration not tested
- ✗ AWS Lambda integration not tested

---

## Code Files Summary

### src/ Module Files
```
src/
├── lib.rs                  - Module exports & documentation
├── client.rs              - Enqueue client
├── config.rs              - Connection configuration
├── errors.rs              - Error types (✓ well-tested)
├── error_codes.rs         - Error constants
├── job.rs                 - Job data structures (✓ tested)
├── worker.rs              - Worker implementation (⚠️ minimally tested)
├── middleware.rs          - Middleware system (unit tests only)
├── middleware_common/     - Built-in middleware (no integration tests)
│   ├── mod.rs
│   ├── logging.rs
│   ├── metrics.rs
│   └── timeout.rs
├── queue.rs               - Queue operations (✓ tested)
├── retry.rs               - Retry policies
├── rate_limiter.rs        - Rate limiting (✓ tested)
├── schema.rs              - Schema validation (✓ tested)
├── events.rs              - Event types (✓ tested)
├── durable.rs
├── workflow.rs            - Workflow primitives
├── subscribe.rs           - Event subscription (SSE)
├── transport/             - Transport layer
│   ├── mod.rs
│   └── http.rs            - HTTP transport (✓ tested)
├── testing.rs             - Testing utilities (✓ tested)
├── tracing_mw.rs          - Tracing middleware (❌ no tests)
├── encryption.rs          - Encryption middleware (❌ no tests)
├── otel.rs                - OpenTelemetry middleware (❌ no tests)
├── serverless/            - Serverless adapters
│   ├── mod.rs
│   └── aws_lambda.rs      (❌ no tests)
└── durable.rs             - Durable execution (❌ no tests)
```

---

## Recommendations for New Tests

### Priority 1: Critical Worker Tests
1. **Worker Job Processing**
   - Test handler execution with actual jobs
   - Test middleware invocation chain
   - Test error handling (handler error vs. non-retryable)
   - Test concurrent job processing
   - Test graceful shutdown with grace period

2. **Worker Lifecycle**
   - Test state transitions (Running → Quiet → Terminate)
   - Test signal handling (Ctrl+C)
   - Test heartbeat loop
   - Test fetch-nack-retry cycle

3. **Handler Registration**
   - Test `register_typed()` with deserialization
   - Test multiple handlers for different job types
   - Test handler replacement

### Priority 2: Middleware & Integration
4. **Middleware Execution**
   - Test actual middleware wrapping during job processing
   - Test middleware error propagation
   - Test middleware side effects (logging, metrics)
   - Test FnMiddleware with state capturing

5. **Integration Scenarios**
   - Test full flow: enqueue → worker fetch → process → ack
   - Test error scenarios with retry
   - Test batch processing

### Priority 3: Edge Cases
6. **Robustness**
   - Test malformed job responses
   - Test network failures
   - Test timeout handling
   - Test large payloads
   - Test rate limiting with backoff

### Priority 4: Feature Coverage
7. **Optional Features**
   - Encryption middleware tests
   - Tracing middleware tests
   - Common middleware tests
   - OpenTelemetry integration tests
   - AWS Lambda integration tests

---

## Example Test Template for New Tests

```rust
// Example: Handler execution test
#[tokio::test]
async fn test_worker_executes_handler_with_context() {
    let server = MockServer::start().await;
    
    // Mock fetch
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(/* job response */)
        .mount(&server)
        .await;
    
    // Mock ack
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    
    let worker = Worker::builder()
        .url(server.uri())
        .build()
        .unwrap();
    
    // Register handler that captures context info
    let captured_context = Arc::new(Mutex::new(None));
    let captured_context_clone = captured_context.clone();
    
    worker.register("test.type", move |ctx: JobContext| {
        let ctx_clone = captured_context_clone.clone();
        async move {
            *ctx_clone.lock().unwrap() = Some((
                ctx.job.id.clone(),
                ctx.job.job_type.clone(),
                ctx.attempt,
            ));
            Ok(json!({"processed": true}))
        }
    }).await;
    
    // Run worker briefly
    tokio::time::timeout(
        Duration::from_secs(1),
        worker.start()
    ).await.ok();
    
    // Verify handler was called with correct context
    let captured = captured_context.lock().unwrap();
    assert!(captured.is_some());
    let (job_id, job_type, attempt) = captured.clone().unwrap();
    assert_eq!(job_id, "test-job-id");
    assert_eq!(job_type, "test.type");
    assert_eq!(attempt, 1);
}
```

