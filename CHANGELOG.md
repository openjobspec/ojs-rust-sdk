# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.0 (2026-02-18)


### Features

* **client:** add OJS client with builder and enqueue API ([4c13e51](https://github.com/openjobspec/ojs-rust-sdk/commit/4c13e51e8d5934fc0819594da50f90ac0eec932b))
* **client:** add request timeout configuration ([8732c25](https://github.com/openjobspec/ojs-rust-sdk/commit/8732c258c6d981317e64f800618a900d6277b83b))
* **config:** add ConnectionConfig for shared client/worker settings ([85fad0a](https://github.com/openjobspec/ojs-rust-sdk/commit/85fad0a6d480000f36b6189f8d4db583ce1af835))
* **errors:** add OJS error types and error codes ([53e2db8](https://github.com/openjobspec/ojs-rust-sdk/commit/53e2db892c185568add3c0d4b4f799c2ce2c4eb3))
* **errors:** add RateLimitInfo struct and retry-after metadata to ServerError ([5b77323](https://github.com/openjobspec/ojs-rust-sdk/commit/5b773231083203b241debfe624391f58e072b34b))
* **events:** add CloudEvents-inspired event types ([f615be4](https://github.com/openjobspec/ojs-rust-sdk/commit/f615be42ae265f7e06aa550d3a240fd3312d2aec))
* **http:** parse rate limit and retry-after response headers ([5146bb6](https://github.com/openjobspec/ojs-rust-sdk/commit/5146bb6534a217db7085991589d849a75d2211fc))
* **job:** add job types, state machine, and wire format ([241f27c](https://github.com/openjobspec/ojs-rust-sdk/commit/241f27cfb35b1bce3bde8e0a62c4ff36f88f809e))
* **middleware:** add common-middleware feature with logging, metrics, and timeout ([265f401](https://github.com/openjobspec/ojs-rust-sdk/commit/265f4012167c0844e3a5943f52442eef725625be))
* **middleware:** add tower-inspired async middleware chain ([b7a07ce](https://github.com/openjobspec/ojs-rust-sdk/commit/b7a07cebfa395c771d700b53aa8c340aa63e3d48))
* **otel:** add OpenTelemetry tracing and metrics middleware ([6d3e248](https://github.com/openjobspec/ojs-rust-sdk/commit/6d3e24820bd8b8613ea0f08e3a0c1fda43f457cc))
* **otel:** add OpenTelemetry tracing middleware ([3cc5282](https://github.com/openjobspec/ojs-rust-sdk/commit/3cc528277a0db90c138f2331dcb7b807e20f23e8))
* **queue:** add queue, cron, health, and manifest types ([470df98](https://github.com/openjobspec/ojs-rust-sdk/commit/470df98e735eeab8a47eecae1edc0ed3c4ecaa04))
* **retry:** add retry policy types with builder pattern ([2c83e09](https://github.com/openjobspec/ojs-rust-sdk/commit/2c83e09a6560859b34d8bdd4ac70aebd60fa7621))
* **schema:** add schema types and client operations ([7f62be4](https://github.com/openjobspec/ojs-rust-sdk/commit/7f62be4c8fce089f395051330c8b065882edde05))
* **testing:** add fake store module with assertions and drain ([001900a](https://github.com/openjobspec/ojs-rust-sdk/commit/001900a87007d8e2e1d32432fc6b269daacd2998))
* **testing:** add JobBuilder, feature-gate testing module, and add worker integration tests ([e00a0c6](https://github.com/openjobspec/ojs-rust-sdk/commit/e00a0c611a433ac50a44ad3ca52143c3663d3310))
* **transport:** add HTTP transport layer with reqwest ([4a8bf87](https://github.com/openjobspec/ojs-rust-sdk/commit/4a8bf877b667d2dc7caf241e54cdb916c58aae9a))
* **worker:** add concurrent job worker with graceful shutdown ([38eda72](https://github.com/openjobspec/ojs-rust-sdk/commit/38eda7240f42e4bc73985d371c6cf14ada7eb495))
* **worker:** add register_typed for compile-time arg safety ([531005b](https://github.com/openjobspec/ojs-rust-sdk/commit/531005b6baa3b2527f1cbb66e154838f6c98798e))
* **workflow:** add chain, group, batch workflow primitives ([226c39d](https://github.com/openjobspec/ojs-rust-sdk/commit/226c39d6335a2eeb762341b58453963adebfa77f))


### Bug Fixes

* **clippy:** use derive(Default) and remove redundant format macros ([81e24b1](https://github.com/openjobspec/ojs-rust-sdk/commit/81e24b16445f9f92b7e1a0188965b1277ea1d7c8))
* **errors:** box ServerError to reduce Result stack size ([e142555](https://github.com/openjobspec/ojs-rust-sdk/commit/e14255574a5db82b1d46b3852895c9c7e383d4a1))

## [Unreleased]

### Added
- Client with builder pattern for enqueuing, cancelling, and retrieving jobs
- Batch enqueue support for atomic multi-job submission
- Worker with concurrent job processing and graceful shutdown
- Tower-inspired async middleware chain (logging, tracing, metrics)
- Workflow primitives: `chain` (sequential), `group` (parallel), `batch` (fan-out/fan-in)
- Queue management: list, pause, resume, stats
- Dead letter job operations: list, retry, discard
- Cron job registration and management
- Health check and server manifest endpoints
- Retry policy configuration with backoff strategies
- Unique/deduplication job policies
- Custom HTTP headers and auth token support
- `Transport` trait abstraction for testability
- URL encoding for path segments and query parameters
- Wiremock-based integration test suite
- GitHub Actions CI workflow (fmt, clippy, test, doc)
