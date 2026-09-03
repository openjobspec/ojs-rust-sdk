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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Method, Transport};
    use crate::worker::report::ReportPhase;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Records every forced NACK so tests can assert exactly-once release.
    #[derive(Debug, Default)]
    struct RecordingTransport {
        nacked_job_ids: Mutex<Vec<String>>,
    }

    impl RecordingTransport {
        fn nacked(&self) -> Vec<String> {
            let mut got = self
                .nacked_job_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            got.sort();
            got
        }
    }

    impl Transport for RecordingTransport {
        fn request(
            &self,
            _method: Method,
            path: &str,
            body: Option<serde_json::Value>,
            _raw_path: bool,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = crate::Result<Option<serde_json::Value>>>
                    + Send
                    + '_,
            >,
        > {
            assert_eq!(path, "/workers/nack");
            let job_id = body
                .as_ref()
                .and_then(|b| b.get("job_id"))
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            Box::pin(async move {
                self.nacked_job_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(job_id);
                Ok(None)
            })
        }
    }

    fn deadlines() -> (tokio::time::Instant, tokio::time::Instant) {
        let started = tokio::time::Instant::now();
        (
            started + IN_FLIGHT_REPORT_TIMEOUT,
            started + FORCED_SHUTDOWN_TIMEOUT,
        )
    }

    #[tokio::test]
    async fn test_forced_plan_sends_one_nack_per_job() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        let jobs: Vec<(String, Arc<ActiveJobState>)> = (0..25)
            .map(|i| (format!("job-{i}"), Arc::new(ActiveJobState::new())))
            .collect();
        let plan = plan_terminal_reports(jobs.clone());
        assert_eq!(plan.forced.len(), 25);
        assert!(plan.in_flight.is_empty());

        let (report_deadline, deadline) = deadlines();
        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        let mut want: Vec<String> = jobs.iter().map(|(job_id, _)| job_id.clone()).collect();
        want.sort();
        assert_eq!(concrete.nacked(), want);
        for (_, state) in jobs {
            assert_eq!(state.phase(), ReportPhase::Completed);
        }
    }

    #[tokio::test]
    async fn test_plan_skips_jobs_that_already_reported() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        let reported = Arc::new(ActiveJobState::new());
        assert!(reported.begin_report(|| tokio::spawn(std::future::ready(()))));
        reported.finish_report(Ok(()));

        let plan = plan_terminal_reports(vec![
            ("job-reported".to_string(), reported),
            (
                "job-unreported".to_string(),
                Arc::new(ActiveJobState::new()),
            ),
        ]);
        assert_eq!(plan.already_reported, 1);

        let (report_deadline, deadline) = deadlines();
        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        assert_eq!(concrete.nacked(), ["job-unreported"]);
    }

    #[tokio::test]
    async fn test_plan_blocks_later_handler_report() {
        let state = Arc::new(ActiveJobState::new());
        let plan = plan_terminal_reports(vec![("job-1".to_string(), Arc::clone(&state))]);

        assert_eq!(plan.forced.len(), 1);
        assert!(
            !state.begin_report(|| tokio::spawn(std::future::ready(()))),
            "a later-resuming handler must not claim ACK/NACK reporting"
        );
    }

    #[tokio::test]
    async fn test_in_flight_report_completing_before_deadline_is_not_force_nacked() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        let state = Arc::new(ActiveJobState::new());
        let reporting = Arc::clone(&state);
        assert!(state.begin_report(|| tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            reporting.finish_report(Ok(()));
        })));

        let plan = plan_terminal_reports(vec![("job-in-flight".to_string(), Arc::clone(&state))]);
        assert_eq!(plan.in_flight.len(), 1);
        assert!(plan.forced.is_empty());

        let (report_deadline, deadline) = deadlines();
        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        assert!(
            concrete.nacked().is_empty(),
            "a terminal report that completed within the report deadline must not be duplicated"
        );
        assert_eq!(state.phase(), ReportPhase::Completed);
    }

    #[tokio::test(start_paused = true)]
    async fn test_permanently_pending_report_is_cancelled_then_force_nacked_once() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        let state = Arc::new(ActiveJobState::new());
        assert!(state.begin_report(|| tokio::spawn(std::future::pending::<()>())));

        let plan = plan_terminal_reports(vec![("job-stuck".to_string(), Arc::clone(&state))]);
        assert_eq!(plan.in_flight.len(), 1);

        let (report_deadline, deadline) = deadlines();
        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        assert_eq!(
            concrete.nacked(),
            ["job-stuck"],
            "a report that never settles must be cancelled and then force-nacked exactly once"
        );
        assert_eq!(state.phase(), ReportPhase::Completed);
        assert!(
            tokio::time::Instant::now() <= deadline,
            "cancelling and replacing a stuck report must stay inside the shared deadline"
        );
    }

    #[tokio::test]
    async fn test_failed_in_flight_report_is_released_and_force_nacked() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        let state = Arc::new(ActiveJobState::new());
        let reporting = Arc::clone(&state);
        assert!(state.begin_report(|| tokio::spawn(async move {
            reporting.finish_report(Err("connection reset".to_string()));
        })));

        let plan = plan_terminal_reports(vec![("job-failed".to_string(), Arc::clone(&state))]);
        let (report_deadline, deadline) = deadlines();
        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        assert_eq!(
            concrete.nacked(),
            ["job-failed"],
            "a released (failed) terminal report must still be force-released at shutdown"
        );
    }

    #[tokio::test]
    async fn test_failed_forced_nack_retains_shutdown_ownership() {
        #[derive(Debug)]
        struct FailingTransport;

        impl Transport for FailingTransport {
            fn request(
                &self,
                _method: Method,
                path: &str,
                _body: Option<serde_json::Value>,
                _raw_path: bool,
            ) -> Pin<
                Box<
                    dyn std::future::Future<Output = crate::Result<Option<serde_json::Value>>>
                        + Send
                        + '_,
                >,
            > {
                assert_eq!(path, "/workers/nack");
                Box::pin(async {
                    Err(crate::errors::OjsError::Transport(
                        "server rejected forced nack".to_string(),
                    ))
                })
            }
        }

        let transport: DynTransport = Arc::new(FailingTransport);
        let state = Arc::new(ActiveJobState::new());
        assert_eq!(state.claim_for_shutdown(), ShutdownClaim::Forced);

        force_nack_one(
            &transport,
            "job-failed-forced-nack",
            &state,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;

        assert_eq!(state.phase(), ReportPhase::Reporting);
        assert!(state
            .last_error()
            .is_some_and(|error| error.contains("server rejected forced nack")));
        assert!(
            !state.begin_report(|| tokio::spawn(std::future::ready(()))),
            "a late handler must not report after a forced nack may have reached the server"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_forced_nacks_are_bounded_and_use_one_global_deadline() {
        #[derive(Debug)]
        struct HangingTransport {
            started: AtomicUsize,
        }

        impl Transport for HangingTransport {
            fn request(
                &self,
                _method: Method,
                path: &str,
                _body: Option<serde_json::Value>,
                _raw_path: bool,
            ) -> Pin<
                Box<
                    dyn std::future::Future<Output = crate::Result<Option<serde_json::Value>>>
                        + Send
                        + '_,
                >,
            > {
                assert_eq!(path, "/workers/nack");
                self.started.fetch_add(1, Ordering::SeqCst);
                Box::pin(std::future::pending::<
                    crate::Result<Option<serde_json::Value>>,
                >())
            }
        }

        let concrete = Arc::new(HangingTransport {
            started: AtomicUsize::new(0),
        });
        let transport: DynTransport = concrete.clone();

        let jobs: Vec<(String, Arc<ActiveJobState>)> = (0..1000)
            .map(|i| (format!("job-{i}"), Arc::new(ActiveJobState::new())))
            .collect();

        let (report_deadline, deadline) = deadlines();
        let plan = plan_terminal_reports(jobs.clone());
        let handle = tokio::spawn(async move {
            finish_terminal_reports(transport, plan, report_deadline, deadline).await;
        });

        for _ in 0..128 {
            if concrete.started.load(Ordering::SeqCst) == FORCED_NACK_CONCURRENCY {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            concrete.started.load(Ordering::SeqCst),
            FORCED_NACK_CONCURRENCY,
            "only the bounded first wave should be in flight"
        );

        tokio::time::advance(FORCED_SHUTDOWN_TIMEOUT + Duration::from_millis(1)).await;
        handle.await.unwrap();
        assert_eq!(
            concrete.started.load(Ordering::SeqCst),
            FORCED_NACK_CONCURRENCY,
            "queued jobs must not start after the shared deadline"
        );
        for (job_id, state) in jobs {
            assert_eq!(
                state.phase(),
                ReportPhase::Reporting,
                "{job_id} must remain shutdown-owned after a timed-out or skipped forced nack"
            );
            assert!(
                !state.begin_report(|| tokio::spawn(std::future::ready(()))),
                "{job_id} must reject a late handler report after shutdown's forced claim"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_one_thousand_jobs_are_reported_exactly_once() {
        let concrete = Arc::new(RecordingTransport::default());
        let transport: DynTransport = concrete.clone();

        // A realistic shutdown mix: a third never started reporting, a third
        // has a report in flight that settles, and a third is stuck forever.
        let mut jobs = Vec::new();
        let mut expected_nacks = Vec::new();
        for i in 0..1000 {
            let state = Arc::new(ActiveJobState::new());
            let job_id = format!("job-{i:04}");
            match i % 3 {
                0 => expected_nacks.push(job_id.clone()),
                1 => {
                    let reporting = Arc::clone(&state);
                    assert!(state.begin_report(|| tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        reporting.finish_report(Ok(()));
                    })));
                }
                _ => {
                    assert!(state.begin_report(|| tokio::spawn(std::future::pending::<()>())));
                    expected_nacks.push(job_id.clone());
                }
            }
            jobs.push((job_id, state));
        }
        expected_nacks.sort();

        let started = tokio::time::Instant::now();
        let report_deadline = started + Duration::from_millis(250);
        let deadline = started + FORCED_SHUTDOWN_TIMEOUT;
        let plan = plan_terminal_reports(jobs.clone());
        assert_eq!(plan.forced.len(), 334);
        assert_eq!(plan.in_flight.len(), 666);

        finish_terminal_reports(transport, plan, report_deadline, deadline).await;

        let nacked = concrete.nacked();
        let unique: std::collections::HashSet<&String> = nacked.iter().collect();
        assert_eq!(
            unique.len(),
            nacked.len(),
            "no job may be reported more than once"
        );
        assert_eq!(nacked, expected_nacks);
        for (job_id, state) in jobs {
            assert_eq!(
                state.phase(),
                ReportPhase::Completed,
                "{job_id} must end shutdown with exactly one successful terminal report"
            );
        }
        assert!(tokio::time::Instant::now() <= deadline);
    }
}
