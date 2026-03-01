//! Durable execution support with checkpoint-based crash recovery.
//!
//! Long-running jobs can save checkpoints to persist intermediate state.
//! If the worker crashes, the job resumes from the last checkpoint.
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
//!     Ok(())
//! }
//! ```

use serde::{de::DeserializeOwned, Serialize};

use crate::client::Client;
use crate::errors::Error;
use crate::job::Job;

/// Context for durable job handlers with checkpoint support.
pub struct DurableContext {
    client: Client,
    job: Job,
}

impl DurableContext {
    /// Create a new DurableContext wrapping a job and its server connection.
    pub fn new(client: Client, job: Job) -> Self {
        Self { client, job }
    }

    /// Get the underlying job.
    pub fn job(&self) -> &Job {
        &self.job
    }

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
}
