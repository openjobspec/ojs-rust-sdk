//! Shutdown signal waiting; bounded awaiting of terminal reports that were
//! already in flight when the shutdown grace period expired; and the single
//! forced NACK issued for every job that is still unreported afterwards.

use super::protocol::nack_job;
use super::report::{ActiveJobState, ShutdownClaim};
use crate::transport::DynTransport;
use std::sync::Arc;
use std::time::Duration;

/// The total extra time budget for all forced-shutdown work once the grace
/// period has expired: awaiting in-flight terminal reports, forced NACKs,
/// handler joins, and heartbeat shutdown all share this one absolute
/// deadline.
pub(crate) const FORCED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The portion of [`FORCED_SHUTDOWN_TIMEOUT`] reserved for terminal reports
/// that were already in flight at grace expiry. Handler execution is aborted
/// at grace expiry, but those reports are allowed to finish until this
/// earlier deadline; the remaining budget is what a forced NACK for a report
/// that never settled has left to run in.
pub(crate) const IN_FLIGHT_REPORT_TIMEOUT: Duration = Duration::from_secs(3);

/// Bound forced-release traffic so a large active-job snapshot cannot create
/// an unbounded burst of simultaneous HTTP requests during shutdown.
const FORCED_NACK_CONCURRENCY: usize = 32;

/// Bound how many in-flight terminal reports are awaited/cancelled at once.
/// The reports themselves already run as independent detached tasks, so this
/// only bounds the harvesting work, not report progress.
const IN_FLIGHT_REPORT_CONCURRENCY: usize = 32;

/// Wait for a local shutdown signal: Ctrl-C (SIGINT) on every platform,
/// plus SIGTERM on Unix -- the standard shutdown signal sent by container
/// runtimes and Kubernetes. Returns as soon as either fires.
pub(crate) async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        // `signal()` only fails if the process cannot install a handler at
        // all (e.g. exhausted signal slots); fall back to Ctrl-C only
        // rather than panicking the whole worker over an edge case that
        // does not affect the ability to shut down via Ctrl-C.
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGTERM handler, falling back to Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// How the jobs still active at grace expiry are to be finalized.
#[derive(Debug, Default)]
pub(crate) struct TerminalReportPlan {
    /// Jobs that had not begun reporting. Their terminal report is now
    /// owned exclusively by shutdown, which must force-NACK each exactly
    /// once.
    pub(crate) forced: Vec<(String, Arc<ActiveJobState>)>,
    /// Jobs whose ACK/NACK was already in flight. These are awaited (and,
    /// if they never settle, cancelled) before shutdown may report them.
    pub(crate) in_flight: Vec<(String, Arc<ActiveJobState>)>,
    /// Jobs that had already been reported successfully.
    pub(crate) already_reported: usize,
}

/// Atomically decide, for every still-active job, whether shutdown owns its
/// terminal report, must await an in-flight one, or has nothing to do.
///
/// This must run **before** handler tasks are aborted or awaited: a handler
/// that later resumes from blocking/CPU-bound work then observes the forced
/// claim and skips its own ACK/NACK rather than racing shutdown.
pub(crate) fn plan_terminal_reports(
    jobs: Vec<(String, Arc<ActiveJobState>)>,
) -> TerminalReportPlan {
    let mut plan = TerminalReportPlan::default();
    for (job_id, job_state) in jobs {
        match job_state.claim_for_shutdown() {
            ShutdownClaim::Forced => plan.forced.push((job_id, job_state)),
            ShutdownClaim::ReportInFlight => plan.in_flight.push((job_id, job_state)),
            ShutdownClaim::AlreadyReported => plan.already_reported += 1,
        }
    }
    plan
}

/// Finalize terminal reporting for everything in `plan`.
///
/// Jobs shutdown already owns are force-NACKed immediately -- concurrently
/// with, and never behind, the bounded wait for reports that were still in
/// flight. An in-flight report is given until `report_deadline` to settle;
/// if it does not, it is cancelled and joined, and only then -- with the
/// previous reporter provably stopped -- does shutdown take ownership and
/// send one forced NACK. Every request additionally honors the shared
/// absolute `deadline`.
pub(crate) async fn finish_terminal_reports(
    transport: DynTransport,
    plan: TerminalReportPlan,
    report_deadline: tokio::time::Instant,
    deadline: tokio::time::Instant,
) {
    use futures_util::stream::{self, StreamExt};

    let TerminalReportPlan {
        forced, in_flight, ..
    } = plan;

    let force_owned = force_nack_owned(transport.clone(), forced, deadline);

    let settle_in_flight = async move {
        stream::iter(in_flight)
            .for_each_concurrent(IN_FLIGHT_REPORT_CONCURRENCY, |(job_id, job_state)| {
                let transport = transport.clone();
                async move {
                    if !await_in_flight_report(&job_id, &job_state, report_deadline, deadline).await
                    {
                        // The previous reporter could not be proven stopped,
                        // so reporting again here could duplicate it.
                        return;
                    }
                    if job_state.claim_after_report_settled() {
                        force_nack_one(&transport, &job_id, &job_state, deadline).await;
                    } else {
                        tracing::debug!(
                            job_id = %job_id,
                            "in-flight terminal report completed during shutdown; no forced nack needed"
                        );
                    }
                }
            })
            .await;
    };

    tokio::join!(force_owned, settle_in_flight);
}

/// Best-effort concurrent forced release of jobs whose terminal reporting is
/// already owned by shutdown.
///
/// Requests are concurrency-bounded and all share the same absolute deadline
/// as handler/heartbeat termination. Jobs waiting behind the concurrency
/// bound stay owned by shutdown even if the deadline arrives before their
/// request can start, so a later-resuming handler still cannot emit a second
/// terminal report.
async fn force_nack_owned(
    transport: DynTransport,
    jobs: Vec<(String, Arc<ActiveJobState>)>,
    deadline: tokio::time::Instant,
) {
    use futures_util::stream::{self, StreamExt};

    if jobs.is_empty() {
        return;
    }

    stream::iter(jobs)
        .for_each_concurrent(FORCED_NACK_CONCURRENCY, |(job_id, job_state)| {
            let transport = transport.clone();
            async move {
                force_nack_one(&transport, &job_id, &job_state, deadline).await;
            }
        })
        .await;
}

/// Await one already-started terminal report, cancelling it if it does not
/// settle by `report_deadline`.
///
/// Returns `true` when the previous reporter is provably no longer running
/// (so ownership may safely be taken over), and `false` when even the
/// cancelled task could not be joined within the shared absolute deadline.
async fn await_in_flight_report(
    job_id: &str,
    job_state: &Arc<ActiveJobState>,
    report_deadline: tokio::time::Instant,
    deadline: tokio::time::Instant,
) -> bool {
    let Some(mut task) = job_state.take_report_task() else {
        // The report settled (and cleared its handle) between planning and
        // now; its recorded phase is authoritative.
        tracing::debug!(
            job_id = %job_id,
            phase = ?job_state.phase(),
            "terminal report already settled before shutdown could await it"
        );
        return true;
    };

    if tokio::time::timeout_at(report_deadline, &mut task)
        .await
        .is_ok()
    {
        return true;
    }

    tracing::warn!(
        job_id = %job_id,
        "terminal report did not complete within the shutdown report deadline, cancelling it"
    );
    task.abort();

    if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
        tracing::warn!(
            job_id = %job_id,
            "cancelled terminal report could not be joined before the shutdown deadline; \
             skipping the forced nack to preserve exactly-once reporting"
        );
        return false;
    }

    true
}

/// Send the single forced NACK for a job whose terminal report shutdown
/// owns, recording the outcome on the job's state.
async fn force_nack_one(
    transport: &DynTransport,
    job_id: &str,
    job_state: &Arc<ActiveJobState>,
    deadline: tokio::time::Instant,
) {
    if let Some(error) = job_state.last_error() {
        tracing::debug!(
            job_id = %job_id,
            previous_error = %error,
            "force-releasing a job whose own terminal report failed"
        );
    }

    if tokio::time::Instant::now() >= deadline {
        tracing::warn!(job_id = %job_id, "forced nack at shutdown skipped after the global shutdown deadline elapsed");
        job_state.finish_shutdown_report(Err(
            "shutdown deadline elapsed before the forced nack could start".to_string(),
        ));
        return;
    }

    let outcome = tokio::time::timeout_at(
        deadline,
        nack_job(
            transport,
            job_id,
            "shutdown",
            "worker shut down before the job finished within the grace period",
            true,
        ),
    )
    .await;

    match outcome {
        Ok(Ok(())) => job_state.finish_shutdown_report(Ok(())),
        Ok(Err(e)) => {
            tracing::warn!(job_id = %job_id, error = %e, "forced nack at shutdown failed");
            job_state.finish_shutdown_report(Err(e.to_string()));
        }
        Err(_) => {
            tracing::warn!(job_id = %job_id, "forced nack at shutdown hit the global shutdown deadline");
            job_state.finish_shutdown_report(Err(
                "forced nack hit the global shutdown deadline".to_string()
            ));
        }
    }
}
