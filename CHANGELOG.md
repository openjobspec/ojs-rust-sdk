# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0](https://github.com/openjobspec/ojs-rust-sdk/compare/v0.3.0...v0.4.0) (2026-04-20)


### Features

* add batch enqueue support ([6c732f8](https://github.com/openjobspec/ojs-rust-sdk/commit/6c732f8b5d015bf533b6515eb07ef5af8e987902))
* add graceful shutdown signal handler ([810c444](https://github.com/openjobspec/ojs-rust-sdk/commit/810c4446188997ddab3f56702c4b14b746183c29))
* add initial project structure ([7177ff0](https://github.com/openjobspec/ojs-rust-sdk/commit/7177ff0ff5d35adce9961b843eaaa472bbfa77ac))
* add initial project structure ([d31fd28](https://github.com/openjobspec/ojs-rust-sdk/commit/d31fd286607287b801eb1f6e6303f479a58b95b9))
* add retry backoff configuration ([9506758](https://github.com/openjobspec/ojs-rust-sdk/commit/950675839f26d26bd27ed0db983fde734d75b184))
* add workflow chain primitive ([4a39209](https://github.com/openjobspec/ojs-rust-sdk/commit/4a39209a545932af63eb16843acefa57cff77d7f))
* add workflow group API with parallel job dispatch ([9e6bc9a](https://github.com/openjobspec/ojs-rust-sdk/commit/9e6bc9ac63a7a1e84dd26ada0fb244608648e324))
* expose batch enqueue endpoint ([bdca7a8](https://github.com/openjobspec/ojs-rust-sdk/commit/bdca7a8f4ba82b7eade70be11be55478c7c4cf7a))
* implement core handler interfaces ([3af9c73](https://github.com/openjobspec/ojs-rust-sdk/commit/3af9c738bd28b89f679d95750c7202e913be54f4))
* implement core handler interfaces ([a564012](https://github.com/openjobspec/ojs-rust-sdk/commit/a56401251c9433c2bfcda9ec0afb3c0bf0543933))


### Bug Fixes

* correct job state transition guard ([a8c00f3](https://github.com/openjobspec/ojs-rust-sdk/commit/a8c00f3d662321756147766fb78d0d561e02eb76))
* correct timestamp serialization ([b5e5fce](https://github.com/openjobspec/ojs-rust-sdk/commit/b5e5fce772d4e787d40a02a65d9f60987628e29f))
* handle connection timeout in worker poll loop ([c2d2ae5](https://github.com/openjobspec/ojs-rust-sdk/commit/c2d2ae5c3b29f270c66497861be504ec181eabe1))
* handle nil pointer in middleware chain ([69c285e](https://github.com/openjobspec/ojs-rust-sdk/commit/69c285ee375447c3a8c1abbe707f4ef14fe6ddd9))
* prevent double-close on worker pool ([4e0c044](https://github.com/openjobspec/ojs-rust-sdk/commit/4e0c044a1ca230ffc7c7df384068efd558f2b4ad))
* resolve edge case in input validation ([4c80811](https://github.com/openjobspec/ojs-rust-sdk/commit/4c80811e8aa1dacb51c53993b565596680f48c56))
* resolve edge case in input validation ([40d6456](https://github.com/openjobspec/ojs-rust-sdk/commit/40d6456dfcc87156853799852923f5ca589819ba))
* resolve edge case in job cancellation polling ([1bc01f7](https://github.com/openjobspec/ojs-rust-sdk/commit/1bc01f778657101135b7926e8e62bd79d739ba34))


### Performance Improvements

* cache compiled regex patterns ([3b28c41](https://github.com/openjobspec/ojs-rust-sdk/commit/3b28c41cda0f102c0b85d261e376c3dbfd5c95d5))
* optimize data processing loop ([d29ee69](https://github.com/openjobspec/ojs-rust-sdk/commit/d29ee6998362516097785f4d934ad46682ddea51))
* optimize data processing loop ([6330871](https://github.com/openjobspec/ojs-rust-sdk/commit/6330871599e3be3e049acf370b6b8eae4358cb35))
* reduce allocations in hot path ([09abd39](https://github.com/openjobspec/ojs-rust-sdk/commit/09abd39a696a9c595534b6e9f3398f871d930ec0))

## [Unreleased]

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
