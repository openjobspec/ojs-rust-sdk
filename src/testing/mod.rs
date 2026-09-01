//! OJS Testing Module — fake mode, assertions, and test utilities.
//!
//! Implements the OJS Testing Specification (ojs-testing.md).
//!
//! Enable via the `testing` feature:
//!
//! ```toml
//! [dev-dependencies]
//! ojs = { version = "0.1", features = ["testing"] }
//! ```
//!
//! # Usage
//!
//! ```rust
//! # #[cfg(feature = "testing")]
//! # {
//! use ojs::testing::FakeStore;
//!
//! let store = FakeStore::new();
//! store.record_enqueue("email.send", vec![], None, None);
//! store.assert_enqueued("email.send", None);
//! # }
//! ```
//!
//! This module is split into two independent, cohesive actors, each in its
//! own submodule:
//!
//! - `fake_store` — the in-memory fake job store, its recorded-job model,
//!   assertion/match-criteria helpers, and the drain loop
//!   ([`FakeStore`], [`FakeJob`], [`MatchCriteria`]).
//! - `job_builder` — a standalone builder for constructing [`crate::Job`]
//!   values directly in tests ([`JobBuilder`]), independent of any fake
//!   store or drain behavior.

mod fake_store;
mod job_builder;

pub use fake_store::{FakeJob, FakeStore, MatchCriteria};
pub use job_builder::JobBuilder;
