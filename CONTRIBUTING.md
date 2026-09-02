# Contributing to ojs-rust-sdk

Thank you for your interest in contributing to the OJS Rust SDK!

## Getting Started

1. Fork the repository and clone your fork
2. Install Rust 1.75+ via [rustup](https://rustup.rs)
3. Run the test suite to verify your setup:

```bash
cargo test
```

## Development Workflow

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test --all-features
```

### Linting

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
```

### Documentation

```bash
cargo doc --no-deps --all-features
```

## Pull Requests

1. Create a feature branch from `main`
2. Make your changes, ensuring all tests pass
3. Run `cargo fmt` and `cargo clippy` before committing
4. Write clear commit messages describing the change
5. Open a pull request against `main`

### PR Checklist

- [ ] Code compiles without warnings (`cargo clippy -- -D warnings`)
- [ ] All existing tests pass (`cargo test`)
- [ ] New functionality includes tests
- [ ] Code is formatted (`cargo fmt`)
- [ ] Documentation is updated if public API changed

## Release Automation

Release Please reads `release-please-config.json` and
`.release-please-manifest.json`. The manifest records the actual last published
version before a release PR and equals package metadata after that release is
merged. Pre-1.0 `feat` and breaking commits use `bump-minor-pre-major`. The
configuration check accepts both lifecycle states, verifies monotonic SemVer,
and proves future patch, minor, and major releases advance rather than
remaining pinned to the current package version.

The `release-please.yml` workflow requires a `RELEASE_PLEASE_TOKEN` secret
containing a GitHub App installation token or fine-grained personal access
token with repository Contents and Pull Requests write access. Do not replace
it with `GITHUB_TOKEN`: tags and releases created with `GITHUB_TOKEN` do not
trigger the downstream tag-based artifact publication workflow.

Run the configuration regression check locally with:

```bash
python3 .github/scripts/check-release-please.py
```

## Code Style

- Follow standard Rust conventions and `rustfmt` defaults
- Use `thiserror` for error types
- Keep public API surface minimal; use `pub(crate)` for internal items
- Prefer explicit types over inference in public signatures

## Reporting Issues

Open an issue on GitHub with:
- A clear description of the problem
- Steps to reproduce (if applicable)
- Expected vs actual behavior
- Rust version (`rustc --version`)

## License

By contributing, you agree that your contributions will be licensed under the Apache-2.0 License.
