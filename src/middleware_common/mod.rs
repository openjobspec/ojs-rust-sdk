//! Common middleware implementations for OJS job processing.
//!
//! This module provides ready-to-use middleware for logging, timeouts, and metrics.
//!
//! Enable via the `common-middleware` feature:
//!
//! ```toml
//! [dependencies]
//! ojs = { version = "0.1", features = ["common-middleware"] }
//! ```

pub mod logging;
pub mod metrics;
pub mod timeout;
