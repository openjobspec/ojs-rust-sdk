//! Worker protocol request/report helpers: per-job dispatch
//! ([`process_job`]) and the `ack`/`nack` report calls it makes against the
//! OJS worker protocol.

use super::context::JobContext;
use super::ActiveJobState;
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
    job_state: Arc<ActiveJobState>,
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
            report_nack(
                transport,
                &job_state,
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
            report_ack(transport, &job_state, &job_id, result).await?;
        }
        Err(e) => {
            tracing::warn!(job_id = %job_id, error = %e, "job failed");
            let (code, message, retryable) = match &e {
                OjsError::NonRetryable(msg) => (
                    crate::errors::ERR_HANDLER_ERROR.to_string(),
                    msg.clone(),
                    false,
                ),
                OjsError::Timeout(msg) => {
                    (crate::errors::ERR_TIMEOUT.to_string(), msg.clone(), true)
                }
                OjsError::Handler(msg) => (
                    crate::errors::ERR_HANDLER_ERROR.to_string(),
                    msg.clone(),
                    true,
                ),
                other => (
                    crate::errors::ERR_HANDLER_ERROR.to_string(),
                    other.to_string(),
                    true,
                ),
            };
            report_nack(transport, &job_state, &job_id, &code, &message, retryable).await?;
        }
    }

    Ok(())
}

/// The terminal report a completed handler owes the server.
#[derive(Debug)]
enum TerminalReport {
    Ack(serde_json::Value),
    Nack {
        code: String,
        message: String,
        retryable: bool,
    },
}

impl TerminalReport {
    async fn send(self, transport: &DynTransport, job_id: &str) -> crate::Result<()> {
        match self {
            TerminalReport::Ack(result) => ack_job(transport, job_id, result).await,
            TerminalReport::Nack {
                code,
                message,
                retryable,
            } => nack_job(transport, job_id, &code, &message, retryable).await,
        }
    }
}

async fn report_ack(
    transport: &DynTransport,
    job_state: &Arc<ActiveJobState>,
    job_id: &str,
    result: serde_json::Value,
) -> crate::Result<()> {
    report_terminal(transport, job_state, job_id, TerminalReport::Ack(result)).await
}

async fn report_nack(
    transport: &DynTransport,
    job_state: &Arc<ActiveJobState>,
    job_id: &str,
    code: &str,
    message: &str,
    retryable: bool,
) -> crate::Result<()> {
    report_terminal(
        transport,
        job_state,
        job_id,
        TerminalReport::Nack {
            code: code.to_string(),
            message: message.to_string(),
            retryable,
        },
    )
    .await
}

/// Claim the job's terminal report and perform it exactly once.
///
/// The request itself runs in a **detached task** registered on the job's
/// [`ActiveJobState`], not inline in the handler task. That is what lets a
/// graceful shutdown abort handler *execution* at grace expiry while an
/// already-started ACK/NACK keeps running to completion; shutdown awaits
/// (and, if it never settles, cancels) that task through the same state.
///
/// The calling job task still awaits the outcome, so the worker's
/// active-job accounting continues to cover terminal reporting and the
/// error is still returned to the caller. If this task is cancelled while
/// waiting, the detached report -- and the shutdown path -- own the outcome
/// from that point on.
async fn report_terminal(
    transport: &DynTransport,
    job_state: &Arc<ActiveJobState>,
    job_id: &str,
    report: TerminalReport,
) -> crate::Result<()> {
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();

    let started = {
        let transport = transport.clone();
        let state = Arc::clone(job_state);
        let job_id = job_id.to_string();
        job_state.begin_report(move || {
            tokio::spawn(async move {
                let outcome = report.send(&transport, &job_id).await;
                state.finish_report(match &outcome {
                    Ok(()) => Ok(()),
                    Err(e) => Err(e.to_string()),
                });
                if let Err(e) = &outcome {
                    tracing::warn!(job_id = %job_id, error = %e, "terminal report failed");
                }
                // The job task may already be gone (aborted at grace
                // expiry); its outcome is then owned by the shutdown path.
                let _ = result_tx.send(outcome);
            })
        })
    };

    if !started {
        tracing::debug!(
            job_id = %job_id,
            "skipping duplicate terminal report: another owner already reported this job"
        );
        return Ok(());
    }

    match result_rx.await {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::debug!(
                job_id = %job_id,
                "terminal report task ended without reporting its outcome (cancelled during forced shutdown)"
            );
            Ok(())
        }
    }
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
