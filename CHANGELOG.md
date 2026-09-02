# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-09-02

### Added
- Additive Lambda push delivery context and pluggable replay-protection storage
- Cross-task `Worker::shutdown()` support with latched pre-start shutdown requests

### Fixed
- Deadline-bounded graceful shutdown with exactly-once terminal reporting across handler/report races
- SSE status/content-type handling, prompt receiver-drop cancellation, empty event-ID reset, and strict cross-chunk UTF-8 decoding
- Signing-secret validation, secret-safe `Debug` output, workflow-default validation, and UTF-8 byte-based queue/job-type limits
- Type-independent `FakeStore` criteria filtering
- Preserved Rust 1.75 CI reproducibility in `Cargo.lock` without constraining downstream dependency resolution
- Updated the locked `h2` dependency to the patched 0.4.16 release

### Changed
- Release validation now includes package/publish dry-runs, a clean consumer smoke test, SBOM generation, and build provenance attestation
- Security, license, and source deny checks run on pinned current stable Rust while the explicit Rust 1.75 build/test matrix remains unchanged

## [0.4.1] - 2026-04-21

### Fixed
- Recorder traces now use real UTC RFC 3339 timestamps instead of the Unix epoch placeholder

## [0.4.0] - 2026-04-20

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
