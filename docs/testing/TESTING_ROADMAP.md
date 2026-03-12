# OJS Rust SDK - Testing Roadmap & Coverage Map

## Executive Summary

The OJS Rust SDK has **3,707 lines of test code** across **15 test files** covering core functionality well, but with significant gaps in worker job processing, middleware execution, and optional features.

**Current Status**: 
- HTTP Transport: ✅ Well-tested (705 lines, 21 tests)
- Worker Core: ❌ Minimally tested (53 lines, 3 tests)
- Middleware: ⚠️ Registration only (202 lines, 6 tests)
- Features: ❌ Not tested (encryption, tracing, OTel, Lambda)

---

## Test File Overview

### Comprehensive Coverage (✅)
| File | Lines | Focus | Status |
|------|-------|-------|--------|
| **http_test.rs** | 705 | HTTP client, enqueue, batch, auth | ✅ Well-tested |
| **job_test.rs** | 346 | Job data, args, state | ✅ Complete |
| **testing_test.rs** | 315 | Testing utilities | ✅ Complete |
| **integration_test.rs** | 331 | End-to-end scenarios | ⚠️ Some gaps |
| **error_test.rs** | 237 | Error types, rate limits | ✅ Complete |
| **queue_test.rs** | 267 | Queue ops, stats, health | ✅ Complete |
| **rate_limiter_test.rs** | 263 | Backoff, retries | ✅ Complete |

### Partial Coverage (⚠️)
| File | Lines | Focus | Status |
|------|-------|-------|--------|
| **worker_integration_test.rs** | 312 | Worker with mock server | ⚠️ **Only 4 tests** |
| **middleware_test.rs** | 202 | Middleware system | ⚠️ **Registration only** |
| **schema_test.rs** | 168 | Schema validation | ✅ Complete |
| **events_test.rs** | 199 | Event types | ✅ Complete |

### Minimal Coverage (❌)
| File | Lines | Focus | Status |
|------|-------|-------|--------|
| **worker_test.rs** | 53 | Worker builder | ❌ **Only 3 tests** |
| **benchmark_test.rs** | 105 | Performance | Basic only |
| **client_test.rs** | 84 | Client builder | Basic only |
| **proptest_test.rs** | 120 | Property tests | Limited scope |

---

## Coverage Heat Map

### By Component

```
Component                Test Coverage      Recommendation
─────────────────────────────────────────────────────────────
HTTP Transport           ████████████░░░░░  ✅ Good
Job Data Structures      ████████████░░░░░  ✅ Good
Error Handling           ████████████░░░░░  ✅ Good
Queue Operations         ████████████░░░░░  ✅ Good
Rate Limiting            ████████████░░░░░  ✅ Good
Schema Validation        ████████████░░░░░  ✅ Good
Client Enqueue           ████████░░░░░░░░░  ✅ Good
Worker Builder           ████░░░░░░░░░░░░░  ⚠️ Needs work
Middleware Execution     ░░░░░░░░░░░░░░░░░  ❌ CRITICAL
Worker Processing        ░░░░░░░░░░░░░░░░░  ❌ CRITICAL
Graceful Shutdown        ░░░░░░░░░░░░░░░░░  ❌ CRITICAL
Concurrency              ░░░░░░░░░░░░░░░░░  ❌ CRITICAL
Optional Features        ░░░░░░░░░░░░░░░░░  ❌ NONE
```

### By Scenario

```
Scenario                           Coverage    Priority
──────────────────────────────────────────────────────────
Simple job enqueue                 ✅ ████░    DONE
Batch enqueue                      ✅ ████░    DONE
Error responses                    ✅ ████░    DONE
Rate limiting                      ✅ ████░    DONE
Get job details                    ✅ ████░    DONE
Worker builder config              ⚠️ ██░░    P2
Handler registration               ⚠️ ██░░    P1
Handler execution                  ❌ ░░░░    P1 URGENT
Middleware registration            ⚠️ ██░░    P2
Middleware execution               ❌ ░░░░    P1 URGENT
Job processing                     ❌ ░░░░    P1 URGENT
Concurrent processing              ❌ ░░░░    P1 URGENT
Worker state machine               ❌ ░░░░    P1 URGENT
Graceful shutdown                  ❌ ░░░░    P1 URGENT
Heartbeat loop                     ❌ ░░░░    P2
Signal handling                    ❌ ░░░░    P2
Error handling paths               ⚠️ ██░░    P2
Timeout scenarios                  ❌ ░░░░    P3
Malformed responses                ❌ ░░░░    P3
Encryption middleware              ❌ ░░░░    P4
Tracing middleware                 ❌ ░░░░    P4
OTel integration                   ❌ ░░░░    P4
Lambda integration                 ❌ ░░░░    P4
```

---

## Detailed Gap Analysis

### Priority 1: Critical Worker Tests (URGENT)

**Status**: 0% coverage | **Impact**: Core functionality
**Location**: Expand `tests/worker_advanced_test.rs` (create new file)
**Effort**: 20-25 tests, ~2000 lines

#### Missing Test Cases

1. **Handler Execution** (5 tests)
   ```
   - Handler receives correct job context
   - Handler return value is properly formatted
   - Handler errors trigger NACK
   - Handler success triggers ACK
   - Handler timeout is enforced
   ```

2. **Middleware Chain** (5 tests)
   ```
   - Middleware invoked with handler
   - Middleware ordering during execution
   - Middleware error propagation
   - Middleware can short-circuit handler
   - Middleware can modify context
   ```

3. **Job Processing Loop** (5 tests)
   ```
   - Worker fetches jobs from correct queue
   - Jobs processed in correct order
   - Active job count is tracked
   - Concurrent jobs processed up to concurrency limit
   - Jobs processed correctly after nack/retry
   ```

4. **Worker Lifecycle** (5 tests)
   ```
   - State transitions: Running → Quiet → Terminate
   - Graceful shutdown with grace period
   - Grace period waits for active jobs
   - Jobs exceeding grace period are terminated
   - Shutdown signal (Ctrl+C) is handled
   ```

5. **Heartbeat & State** (5 tests)
   ```
   - Heartbeat loop sends active jobs
   - Heartbeat interval is respected
   - Server can direct state changes via heartbeat
   - State is properly updated from heartbeat
   - Worker handles heartbeat failures
   ```

### Priority 2: Middleware & Integration (Important)

**Status**: 20% coverage | **Impact**: Feature completeness
**Location**: Expand `tests/middleware_test.rs`, new `tests/integration_advanced_test.rs`
**Effort**: 10-15 tests, ~1500 lines

#### Missing Test Cases

1. **Middleware Implementation** (3 tests)
   - FnMiddleware with state capture
   - Middleware with side effects
   - Error handling in middleware

2. **Integration Flows** (4 tests)
   - Enqueue → Fetch → Process → ACK
   - Error path with retries
   - Batch processing
   - Workflow integration

3. **Handler Registration** (4 tests)
   - `register_typed()` with deserialization
   - Multiple handlers for different types
   - Handler replacement/overwriting
   - Invalid handler signatures

### Priority 3: Edge Cases & Robustness

**Status**: 0% coverage | **Impact**: Reliability
**Location**: New `tests/worker_edge_cases_test.rs`
**Effort**: 10-15 tests, ~1500 lines

#### Missing Test Cases

1. **Malformed Responses** (3 tests)
   - Invalid JSON in job response
   - Missing required fields
   - Invalid field types

2. **Network Failures** (3 tests)
   - Connection timeout during fetch
   - Connection timeout during ack
   - Partial response handling

3. **Timeout Scenarios** (3 tests)
   - Handler execution timeout
   - Long-running job detection
   - Timeout with active retries

4. **Large Payloads** (3 tests)
   - Very large job args
   - Large handler response
   - Rate limiting on large requests

### Priority 4: Optional Features

**Status**: 0% coverage | **Impact**: Feature adoption
**Location**: New test files per feature
**Effort**: 15-20 tests, ~2000 lines

#### Missing Test Files

1. **tests/encryption_middleware_test.rs** (5 tests)
   - Encryption/decryption
   - Key rotation
   - Invalid keys
   - Compatibility

2. **tests/tracing_middleware_test.rs** (3 tests)
   - Span creation
   - Event logging
   - Error tracing

3. **tests/common_middleware_test.rs** (5 tests)
   - Logging middleware
   - Metrics middleware
   - Timeout middleware
   - Combined middleware

4. **tests/otel_integration_test.rs** (4 tests)
   - OpenTelemetry spans
   - Metrics export
   - Trace context

5. **tests/lambda_integration_test.rs** (5 tests)
   - Lambda handler invocation
   - Event parsing
   - Response format
   - Error handling

---

## Test Implementation Order

### Phase 1: Foundation (Week 1-2)
Priority: CRITICAL - These block other testing

- [ ] Handler execution with job context
- [ ] Middleware chain execution
- [ ] Worker state machine
- [ ] Graceful shutdown

**Files to modify/create**:
- `tests/worker_advanced_test.rs` (new)
- Expand `tests/middleware_test.rs`

**Expected**: 10-12 tests, ~1500 lines

### Phase 2: Integration (Week 2-3)
Priority: HIGH - Full feature testing

- [ ] Concurrent job processing
- [ ] Heartbeat loop operation
- [ ] Full enqueue → process → ack flow
- [ ] Error paths with retries
- [ ] Handler typed registration

**Files to modify/create**:
- Expand `tests/worker_integration_test.rs`
- `tests/integration_advanced_test.rs` (new)

**Expected**: 10-12 tests, ~1500 lines

### Phase 3: Robustness (Week 3-4)
Priority: MEDIUM - Edge cases

- [ ] Malformed responses
- [ ] Network failures
- [ ] Timeout handling
- [ ] Large payloads
- [ ] Empty responses

**Files to modify/create**:
- `tests/worker_edge_cases_test.rs` (new)

**Expected**: 10-12 tests, ~1500 lines

### Phase 4: Features (Week 4-5)
Priority: LOW - Optional features

- [ ] Encryption middleware
- [ ] Tracing middleware
- [ ] Common middleware
- [ ] OpenTelemetry
- [ ] AWS Lambda

**Files to modify/create**:
- `tests/encryption_middleware_test.rs` (new)
- `tests/tracing_middleware_test.rs` (new)
- `tests/common_middleware_test.rs` (new)
- `tests/otel_integration_test.rs` (new)
- `tests/lambda_integration_test.rs` (new)

**Expected**: 20-25 tests, ~2500+ lines

---

## Metrics & Success Criteria

### Coverage Goals

| Component | Current | Target | Status |
|-----------|---------|--------|--------|
| Worker | 10% | 95% | ❌ Need 18 tests |
| Middleware | 30% | 90% | ❌ Need 8 tests |
| Handlers | 0% | 95% | ❌ Need 10 tests |
| Lifecycle | 0% | 90% | ❌ Need 8 tests |
| Error Paths | 40% | 95% | ⚠️ Need 5 tests |
| Features | 0% | 50% | ❌ Need 15 tests |

### Test Count Goals

| Category | Current | Target | Gap |
|----------|---------|--------|-----|
| Total Tests | ~50 | 100+ | +50 tests |
| Worker Tests | 7 | 30 | +23 tests |
| Integration Tests | 4 | 15 | +11 tests |
| Edge Case Tests | 0 | 15 | +15 tests |
| Feature Tests | 0 | 20 | +20 tests |

### Code Coverage Goals

- **Overall**: 70% → 90% (+20%)
- **Worker module**: 40% → 95% (+55%)
- **Middleware module**: 50% → 90% (+40%)
- **Integration paths**: 30% → 85% (+55%)

---

## Testing Best Practices

### Structure for New Tests

```rust
#[tokio::test]
async fn test_worker_<feature>_<scenario>() {
    // 1. Setup: Create mock server
    let server = MockServer::start().await;
    
    // 2. Configure: Mount expected endpoints
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(/* response */)
        .expect(n)
        .mount(&server)
        .await;
    
    // 3. Create: Build worker/client
    let worker = Worker::builder()
        .url(server.uri())
        .concurrency(N)
        .build()
        .unwrap();
    
    // 4. Register: Add handlers/middleware
    worker.register("job.type", |ctx| async move {
        // handler logic
        Ok(json!({}))
    }).await;
    
    // 5. Execute: Run with timeout
    tokio::time::timeout(
        Duration::from_secs(1),
        worker.start()
    ).await.ok();
    
    // 6. Verify: Assert expectations
    assert_eq!(counter.load(Ordering::SeqCst), expected);
}
```

### Naming Convention

```
test_worker_<feature>_<scenario>
├── feature: job_processing, middleware, state, heartbeat, etc.
└── scenario: success, error, timeout, concurrent, etc.

Examples:
- test_worker_job_processing_executes_handler
- test_worker_middleware_chains_execution
- test_worker_state_transitions_running_to_quiet
- test_worker_graceful_shutdown_respects_grace_period
```

### Helper Functions to Extract

```rust
fn new_mock_server_with_worker() -> (MockServer, Worker)
async fn register_test_handler(worker: &Worker, job_type: &str)
fn job_response_for(id: &str, job_type: &str) -> Value
struct MockFetchResponder { ... }
struct OrderTrackingMiddleware { ... }
```

---

## Quick Reference

### Files to Modify

1. **tests/worker_test.rs** (+20 tests)
   - Expand from 53 to 300+ lines
   - Add handler execution tests
   - Add state machine tests

2. **tests/middleware_test.rs** (+5 tests)
   - Expand from 202 to 350+ lines
   - Add execution tests
   - Add error propagation tests

3. **tests/worker_integration_test.rs** (+8 tests)
   - Expand from 312 to 600+ lines
   - Add concurrent job tests
   - Add full flow tests

### Files to Create

1. **tests/worker_advanced_test.rs** (NEW)
   - Handler execution scenarios
   - Middleware chain execution
   - State transitions

2. **tests/integration_advanced_test.rs** (NEW)
   - Full enqueue → process flow
   - Batch processing
   - Error scenarios

3. **tests/worker_edge_cases_test.rs** (NEW)
   - Malformed responses
   - Network failures
   - Timeout scenarios

4. **tests/encryption_middleware_test.rs** (NEW)
5. **tests/tracing_middleware_test.rs** (NEW)
6. **tests/common_middleware_test.rs** (NEW)
7. **tests/otel_integration_test.rs** (NEW)
8. **tests/lambda_integration_test.rs** (NEW)

---

## Success Metrics

After implementing all recommended tests:

- ✅ Total test count: 50 → 100+
- ✅ Test file count: 15 → 23
- ✅ Test code lines: 3,707 → 6,500+
- ✅ Code coverage: ~70% → 90%+
- ✅ Worker coverage: ~10% → 95%+
- ✅ Feature coverage: 0% → 50%+

---

## Next Steps

1. **Review** this roadmap with the team
2. **Prioritize** based on business needs (recommend Phase 1-2 first)
3. **Create** branch for test development
4. **Implement** tests in recommended order
5. **Run** `cargo test --all` to verify
6. **Monitor** code coverage improvements

---

**Start with Priority 1**: Worker job processing tests are the foundation for all other testing.

