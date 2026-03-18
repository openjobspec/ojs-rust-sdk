//! Agent API client for fork/merge branching, pause/resume human-in-the-loop
//! control, and deterministic replay of agent job executions.

use crate::errors::OjsError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Controls how conflicting turns are reconciled during a merge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// Keep changes from branch A on conflict.
    Ours,
    /// Keep changes from branch B on conflict.
    Theirs,
    /// Combine non-overlapping changes from both branches.
    Union,
}

/// Configures where a new branch diverges from the main execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkOptions {
    /// Zero-based turn index at which the fork begins.
    pub at_turn: u64,
    /// Human-readable label for the new branch.
    pub branch_name: String,
}

/// Identifiers produced by a successful fork.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkResult {
    /// Unique identifier of the newly created branch.
    pub branch_id: String,
    /// Content-addressable hash of the branch snapshot.
    pub content_id: String,
}

/// Specifies the two branches to merge and the strategy to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOptions {
    /// Identifier of the first branch.
    pub branch_a: String,
    /// Identifier of the second branch.
    pub branch_b: String,
    /// Conflict resolution approach.
    pub strategy: MergeStrategy,
}

/// Outcome of a merge operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    /// Identifier of the resulting merged branch.
    pub merged_id: String,
    /// Turn or field paths that could not be auto-resolved.
    pub conflicts: Vec<String>,
}

/// Human reviewer's verdict for a paused agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeDecision {
    /// Whether the agent should continue execution.
    pub approved: bool,
    /// Optional note from the reviewer.
    #[serde(default)]
    pub comment: String,
    /// Arbitrary key-value pairs attached to the decision.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Configures a deterministic replay of a previous execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayOptions {
    /// Zero-based turn index from which replay begins.
    pub from_turn: u64,
    /// Maps provider names to canned response identifiers.
    #[serde(default)]
    pub mock_providers: HashMap<String, String>,
}

/// Summarises a completed replay run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    /// Total number of turns that were replayed.
    pub steps: u64,
    /// Turns where replay output differed from the original.
    pub divergences: Vec<Divergence>,
}

/// A single point where a replay's output differed from original execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Divergence {
    /// Zero-based index of the diverging turn.
    pub turn: u64,
    /// Output from the original execution.
    pub expected: String,
    /// Output produced during the replay.
    pub actual: String,
}

/// Thin HTTP client for the OJS Agent API.
#[derive(Debug, Clone)]
pub struct AgentClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl AgentClient {
    /// Creates a new `AgentClient` pointed at the given base URL.
    pub fn new(base_url: &str) -> Result<Self, OjsError> {
        if base_url.is_empty() {
            return Err(OjsError::Builder("base URL must not be empty".into()));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http_client: reqwest::Client::new(),
        })
    }

    /// Creates a new `AgentClient` with a custom `reqwest::Client`.
    pub fn with_client(base_url: &str, client: reqwest::Client) -> Result<Self, OjsError> {
        if base_url.is_empty() {
            return Err(OjsError::Builder("base URL must not be empty".into()));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http_client: client,
        })
    }

    /// Creates a new execution branch diverging at the turn specified in opts.
    pub async fn fork(&self, job_id: &str, opts: &ForkOptions) -> Result<ForkResult, OjsError> {
        let url = format!("{}/v1/agent/jobs/{}/fork", self.base_url, job_id);
        let resp = self
            .http_client
            .post(&url)
            .json(opts)
            .send()
            .await
            .map_err(|e| OjsError::Transport(e.to_string()))?;
        Self::handle_response(resp).await
    }

    /// Combines two branches using the strategy specified in opts.
    pub async fn merge(&self, job_id: &str, opts: &MergeOptions) -> Result<MergeResult, OjsError> {
        let url = format!("{}/v1/agent/jobs/{}/merge", self.base_url, job_id);
        let resp = self
            .http_client
            .post(&url)
            .json(opts)
            .send()
            .await
            .map_err(|e| OjsError::Transport(e.to_string()))?;
        Self::handle_response(resp).await
    }

    /// Requests that the agent stop execution after the current turn.
    pub async fn pause(&self, job_id: &str, reason: &str) -> Result<(), OjsError> {
        let url = format!("{}/v1/agent/jobs/{}/pause", self.base_url, job_id);
        let body = serde_json::json!({ "reason": reason });
        let resp = self
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OjsError::Transport(e.to_string()))?;
        Self::handle_empty_response(resp).await
    }

    /// Instructs a paused agent to continue or abort based on the decision.
    pub async fn resume(&self, job_id: &str, decision: &ResumeDecision) -> Result<(), OjsError> {
        let url = format!("{}/v1/agent/jobs/{}/resume", self.base_url, job_id);
        let resp = self
            .http_client
            .post(&url)
            .json(decision)
            .send()
            .await
            .map_err(|e| OjsError::Transport(e.to_string()))?;
        Self::handle_empty_response(resp).await
    }

    /// Re-executes the given job deterministically from the specified turn.
    pub async fn replay(
        &self,
        job_id: &str,
        opts: &ReplayOptions,
    ) -> Result<ReplayResult, OjsError> {
        let url = format!("{}/v1/agent/jobs/{}/replay", self.base_url, job_id);
        let resp = self
            .http_client
            .post(&url)
            .json(opts)
            .send()
            .await
            .map_err(|e| OjsError::Transport(e.to_string()))?;
        Self::handle_response(resp).await
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, OjsError> {
        let status = resp.status().as_u16();
        match status {
            200..=299 => resp
                .json::<T>()
                .await
                .map_err(|e| OjsError::Transport(e.to_string())),
            404 => Err(OjsError::Transport("agent not found".into())),
            409 => Err(OjsError::Transport("branch conflict".into())),
            422 => Err(OjsError::Transport("agent is not paused".into())),
            _ => Err(OjsError::Transport(format!("unexpected status {status}"))),
        }
    }

    async fn handle_empty_response(resp: reqwest::Response) -> Result<(), OjsError> {
        let status = resp.status().as_u16();
        match status {
            200..=299 => Ok(()),
            404 => Err(OjsError::Transport("agent not found".into())),
            409 => Err(OjsError::Transport("branch conflict".into())),
            422 => Err(OjsError::Transport("agent is not paused".into())),
            _ => Err(OjsError::Transport(format!("unexpected status {status}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_options_serialize() {
        let opts = ForkOptions {
            at_turn: 3,
            branch_name: "experiment-a".to_string(),
        };
        let json = serde_json::to_value(&opts).unwrap();
        assert_eq!(json["at_turn"], 3);
        assert_eq!(json["branch_name"], "experiment-a");
    }

    #[test]
    fn test_merge_strategy_serialize() {
        let json = serde_json::to_string(&MergeStrategy::Ours).unwrap();
        assert!(json.contains("ours"));
    }

    #[test]
    fn test_client_empty_url() {
        let result = AgentClient::new("");
        assert!(result.is_err());
    }
}
