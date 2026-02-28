# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0](https://github.com/openjobspec/ojs-rust-sdk/compare/v0.1.0...v0.2.0) (2026-02-28)


### Features

* add batch enqueue support ([9996b35](https://github.com/openjobspec/ojs-rust-sdk/commit/9996b352753d65f4305125b5310d0d01691d57b5))
* add custom deserializer for job args ([9b0ac29](https://github.com/openjobspec/ojs-rust-sdk/commit/9b0ac29a24e26c553825119a3b74b3239c8adf8d))
* add retry backoff configuration ([e9ff0f6](https://github.com/openjobspec/ojs-rust-sdk/commit/e9ff0f6a28d1e9af06debae63d431d04f32a6618))
* add tokio task metrics for worker pool monitoring ([1164dea](https://github.com/openjobspec/ojs-rust-sdk/commit/1164dea74560cb8d08889ec798e1c75cd07e2a91))


### Bug Fixes

* handle timeout in worker polling loop ([61877de](https://github.com/openjobspec/ojs-rust-sdk/commit/61877de6baf681aa27a61a449fe88ac2bee4497b))
* resolve lifetime annotation in async worker trait ([6e2a0ea](https://github.com/openjobspec/ojs-rust-sdk/commit/6e2a0eaf5c14d55cb7e6b2b635291401324b1040))


### Performance Improvements

* reduce allocation in job deserialization path ([5da7903](https://github.com/openjobspec/ojs-rust-sdk/commit/5da790360cf69d3c3aa2018ee174912f8c099cbf))

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
