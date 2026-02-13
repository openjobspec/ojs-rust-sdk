# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
