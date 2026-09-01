//! Worker protocol request/report helpers: per-job dispatch
//! ([`process_job`]) and the `ack`/`nack` report calls it makes against the
//! OJS worker protocol.

use super::context::JobContext;
use crate::errors::OjsError;
use crate::job::{AckRequest, Job, NackError, NackRequest};
use crate::middleware::{HandlerFn, MiddlewareChain};
use crate::transport::{self, DynTransport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Look up the registered handler for `job`'s type, wrap it with the
/// current middleware chain, build its [`JobContext`], execute it, and
/// report the outcome to the server via `ack`/`nack`.
pub(crate) async fn process_job(
    transport: &DynTransport,
    worker_id: &str,
    handlers: &Arc<RwLock<HashMap<String, HandlerFn>>>,
    middleware: &Arc<RwLock<MiddlewareChain>>,
    job: Job,
) -> crate::Result<()> {
    let job_type = job.job_type.clone();
    let job_id = job.id.clone();

    tracing::debug!(
        job_id = %job_id,
        job_type = %job_type,
        attempt = job.attempt,
        "processing job"
    );

    // Look up handler
    let handler: Option<HandlerFn> = {
        let handlers = handlers.read().await;
        handlers.get(&job_type).cloned()
    };

    let handler = match handler {
        Some(h) => h,
        None => {
            tracing::error!(job_type = %job_type, "no handler registered");
            nack_job(
                transport,
                &job_id,
                "handler_error",
                &format!("no handler registered for job type: {}", job_type),
                false,
            )
            .await?;
            return Ok(());
        }
    };

    // Wrap handler with middleware
    let wrapped = {
        let mw = middleware.read().await;
        mw.wrap(handler)
    };

    // Build job context
    let ctx = JobContext {
        attempt: job.attempt.max(1),
        queue: job.queue.clone(),
        workflow_id: job
            .meta
            .as_ref()
            .and_then(|m| m.get("workflow_id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        parent_results: None,
        transport: transport.clone(),
        worker_id: worker_id.to_string(),
        job,
    };

    // Execute
    match wrapped(ctx).await {
        Ok(result) => {
            tracing::debug!(job_id = %job_id, "job completed successfully");
            ack_job(transport, &job_id, result).await?;
        }
        Err(e) => {
            tracing::warn!(job_id = %job_id, error = %e, "job failed");
            let (code, message, retryable) = match &e {
                OjsError::NonRetryable(msg) => ("handler_error".to_string(), msg.clone(), false),
                OjsError::Handler(msg) => ("handler_error".to_string(), msg.clone(), true),
                other => ("handler_error".to_string(), other.to_string(), true),
            };
            nack_job(transport, &job_id, &code, &message, retryable).await?;
        }
    }

    Ok(())
}

pub(crate) async fn ack_job(
    transport: &DynTransport,
    job_id: &str,
    result: serde_json::Value,
) -> crate::Result<()> {
    let req = AckRequest {
        job_id: job_id.to_string(),
        result: if result.is_null() { None } else { Some(result) },
    };
    transport::transport_post_no_response(transport, "/workers/ack", &req).await
}

pub(crate) async fn nack_job(
    transport: &DynTransport,
    job_id: &str,
    code: &str,
    message: &str,
    retryable: bool,
) -> crate::Result<()> {
    let req = NackRequest {
        job_id: job_id.to_string(),
        error: NackError {
            code: code.to_string(),
            message: message.to_string(),
            retryable: Some(retryable),
            details: None,
        },
    };
    transport::transport_post_no_response(transport, "/workers/nack", &req).await
}
