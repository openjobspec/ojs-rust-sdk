# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability in the OJS Rust SDK, please report it
responsibly. **Do not open a public issue.**

### How to Report

1. **Email:** Send a description to the project maintainers via the email
   listed on the [openjobspec GitHub organization](https://github.com/openjobspec).
2. **GitHub Security Advisory:** Use the
   [private vulnerability reporting](https://github.com/openjobspec/ojs-rust-sdk/security/advisories/new)
   feature on this repository.

### What to Include

- A description of the vulnerability and its potential impact
- Steps to reproduce (proof-of-concept if possible)
- Affected versions
- Any suggested fix or mitigation

### Response Timeline

- **Acknowledgment:** Within 48 hours of receipt.
- **Assessment:** Within 7 days, we will confirm whether the report is accepted
  or declined and provide an initial severity assessment.
- **Fix & Disclosure:** We aim to release a patch within 30 days of confirmation.
  We will coordinate disclosure timing with the reporter.

## Scope

This policy covers the `ojs` Rust crate published on [crates.io](https://crates.io/crates/ojs).
For vulnerabilities in OJS server implementations or other SDKs, please report
to the appropriate repository under the
[openjobspec](https://github.com/openjobspec) organization.

## Security Best Practices

When using the OJS SDK in production:

- **Protect auth tokens:** Use environment variables or secret managers; never
  hard-code tokens in source.
- **Use TLS:** Always connect to OJS servers over HTTPS in production.
- **Pin dependencies:** Use `Cargo.lock` in your applications (not libraries)
  and run `cargo audit` regularly.
- **Keep updated:** Watch for new releases and security advisories.
