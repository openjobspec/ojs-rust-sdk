//! Durable execution support with checkpoint-based crash recovery.
//!
//! Long-running jobs can save checkpoints to persist intermediate state.
//! If the worker crashes, the job resumes from the last checkpoint.
//!
//! Deterministic helpers ([`DurableContext::now`], [`DurableContext::random`],
//! [`DurableContext::side_effect`]) record their results in a replay log so
//! that re-executions after a crash produce identical values.
//!
//! # Example
//! ```rust,no_run
//! use ojs::{DurableContext, JobContext};
//!
//! async fn data_migration(ctx: DurableContext) -> Result<(), ojs::Error> {
//!     let processed: usize = ctx.resume().await?.unwrap_or(0);
//!     for i in processed..total_rows {
//!         process_row(i);
//!         if i % 1000 == 0 {
//!             ctx.checkpoint(&i).await?;
//!         }
//!     }
//!     ctx.delete().await?;
//!     Ok(())
//! }
//! ```

use std::future::Future;

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::client::Client;
use crate::errors::Error;
use crate::job::Job;

/// A single entry in the deterministic replay log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayEntry {
    /// Sequence number (0-based position in the log).
    pub seq: usize,
    /// Type of operation (`"now"`, `"random"`, `"side_effect"`).
    pub entry_type: String,
    /// Caller-supplied key or default identifier.
    pub key: String,
    /// Serialized result of the operation.
    pub result: serde_json::Value,
}

/// Context for durable job handlers with checkpoint support.
///
/// Wraps a [`Client`] and [`Job`], providing checkpoint persistence and
/// deterministic replay helpers that guarantee identical results across
/// retries after a crash.
pub struct DurableContext {
    client: Client,
    job: Job,
    /// Deterministic replay log for crash-safe re-execution.
    replay_log: Vec<ReplayEntry>,
    /// Current read position in the replay log.
    replay_cursor: usize,
}

impl DurableContext {
    /// Create a new DurableContext wrapping a job and its server connection.
    ///
    /// Starts with an empty replay log.
    pub fn new(client: Client, job: Job) -> Self {
        Self {
            client,
            job,
            replay_log: Vec::new(),
            replay_cursor: 0,
        }
    }

    /// Create a new DurableContext with a pre-populated replay log for resumption.
    ///
    /// When a job is retried after a crash the caller can supply the replay log
    /// that was recorded during the previous execution so that deterministic
    /// helpers return the same values.
    pub fn new_with_replay(client: Client, job: Job, replay_log: Vec<ReplayEntry>) -> Self {
        Self {
            client,
            job,
            replay_log,
            replay_cursor: 0,
        }
    }

    /// Get the underlying job.
    pub fn job(&self) -> &Job {
        &self.job
    }

    /// Get a reference to the current replay log.
    pub fn replay_log(&self) -> &[ReplayEntry] {
        &self.replay_log
    }

    // -----------------------------------------------------------------
    // Checkpoint CRUD
    // -----------------------------------------------------------------

    /// Save a checkpoint with the given state. Overwrites any previous checkpoint.
    pub async fn checkpoint<T: Serialize>(&self, state: &T) -> Result<(), Error> {
        let body = serde_json::json!({ "state": state });
        let url = format!(
            "{}/ojs/v1/jobs/{}/checkpoint",
            self.client.base_url(),
            self.job.id
        );

        let resp = self
            .client
            .http_client()
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(Error::Server(format!(
                "checkpoint save failed: {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Resume from the last checkpoint. Returns `None` if no checkpoint exists.
    pub async fn resume<T: DeserializeOwned>(&self) -> Result<Option<T>, Error> {
        let url = format!(
            "{}/ojs/v1/jobs/{}/checkpoint",
            self.client.base_url(),
            self.job.id
        );

        let resp = self
            .client
            .http_client()
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        if resp.status().as_u16() == 404 {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(Error::Server(format!(
                "checkpoint get failed: {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Deserialization(e.to_string()))?;

        if let Some(state) = body.get("state") {
            let value: T = serde_json::from_value(state.clone())
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Delete the checkpoint for this job.
    ///
    /// Sends an HTTP DELETE to `/ojs/v1/jobs/{id}/checkpoint`.
    /// Returns `Ok(())` on 200 or 404 (idempotent).
    pub async fn delete(&self) -> Result<(), Error> {
        let url = format!(
            "{}/ojs/v1/jobs/{}/checkpoint",
            self.client.base_url(),
            self.job.id
        );

        let resp = self
            .client
            .http_client()
            .delete(&url)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let status = resp.status().as_u16();
        if resp.status().is_success() || status == 404 {
            return Ok(());
        }

        Err(Error::Server(format!(
            "checkpoint delete failed: {}",
            resp.status()
        )))
    }

    // -----------------------------------------------------------------
    // Deterministic helpers
    // -----------------------------------------------------------------

    /// Return the current UTC time deterministically.
    ///
    /// On the first execution, records `Utc::now()` in the replay log.
    /// On retry (when a replay entry exists at the current cursor), returns
    /// the previously recorded timestamp without calling the system clock.
    pub fn now(&mut self) -> DateTime<Utc> {
        if let Some(entry) = self.replay_log.get(self.replay_cursor) {
            if entry.entry_type == "now" {
                self.replay_cursor += 1;
                return serde_json::from_value(entry.result.clone())
                    .expect("corrupted replay log: invalid timestamp in now() entry");
            }
        }

        let now = Utc::now();
        self.replay_log.push(ReplayEntry {
            seq: self.replay_cursor,
            entry_type: "now".to_string(),
            key: "now".to_string(),
            result: serde_json::to_value(&now).expect("DateTime<Utc> serialization cannot fail"),
        });
        self.replay_cursor += 1;
        now
    }

    /// Generate `num_bytes` of random data, returned as a hex-encoded string.
    ///
    /// On the first execution, generates cryptographically random bytes and
    /// records the hex string in the replay log. On retry, returns the
    /// previously recorded value.
    pub fn random(&mut self, num_bytes: usize) -> String {
        if let Some(entry) = self.replay_log.get(self.replay_cursor) {
            if entry.entry_type == "random" {
                self.replay_cursor += 1;
                return serde_json::from_value(entry.result.clone())
                    .expect("corrupted replay log: invalid hex in random() entry");
            }
        }

        let mut buf = vec![0u8; num_bytes];
        rand::thread_rng().fill_bytes(&mut buf);
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();

        self.replay_log.push(ReplayEntry {
            seq: self.replay_cursor,
            entry_type: "random".to_string(),
            key: "random".to_string(),
            result: serde_json::to_value(&hex).expect("String serialization cannot fail"),
        });
        self.replay_cursor += 1;
        hex
    }

    /// Execute a side effect exactly once, recording its result for deterministic replay.
    ///
    /// On the first execution with a given `key`, invokes the closure `f` and
    /// stores its serialized result in the replay log. On retry, deserializes
    /// and returns the previously stored value without calling `f`.
    pub async fn side_effect<T, F, Fut>(&mut self, key: &str, f: F) -> Result<T, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
        T: Serialize + DeserializeOwned,
    {
        if let Some(entry) = self.replay_log.get(self.replay_cursor) {
            if entry.entry_type == "side_effect" && entry.key == key {
                self.replay_cursor += 1;
                return serde_json::from_value(entry.result.clone())
                    .map_err(|e| Error::Deserialization(e.to_string()));
            }
        }

        let result = f().await?;
        let value = serde_json::to_value(&result)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        self.replay_log.push(ReplayEntry {
            seq: self.replay_cursor,
            entry_type: "side_effect".to_string(),
            key: key.to_string(),
            result: value,
        });
        self.replay_cursor += 1;
        Ok(result)
    }
}
