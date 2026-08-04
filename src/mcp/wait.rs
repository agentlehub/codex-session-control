use std::time::Duration;

use crate::{
    app_server::AppServerConnection,
    error::{ToolErrorCategory, ToolErrorData},
    model::{ThreadSnapshot, ThreadStatus},
};

use super::contract::*;

#[derive(Debug)]
pub(super) struct WaitBaseline {
    input_index: usize,
    previous: ThreadSnapshot,
}

#[derive(Debug)]
pub(super) enum ObservationOutcome {
    Ready(Vec<String>),
    Continue,
    Error(Vec<ToolErrorData>),
}

pub(super) async fn threads_wait(
    connection: &mut AppServerConnection,
    thread_ids: &[String],
    timeout: Duration,
) -> Result<ThreadsWaitResult, ToolErrorData> {
    let (initial, errors) = snapshot_pass(connection, thread_ids).await?;
    if !errors.is_empty() {
        return Ok(ThreadsWaitResult {
            reason: ThreadsWaitReason::Error,
            trigger_thread_ids: Vec::new(),
            threads: initial,
            errors,
        });
    }

    let mut baselines: Vec<WaitBaseline> = initial
        .iter()
        .cloned()
        .enumerate()
        .map(|(input_index, previous)| WaitBaseline {
            input_index,
            previous,
        })
        .collect();
    match observe(&baselines, &initial, true) {
        ObservationOutcome::Ready(trigger_thread_ids) => {
            return Ok(ThreadsWaitResult {
                reason: ThreadsWaitReason::Ready,
                trigger_thread_ids,
                threads: initial,
                errors: Vec::new(),
            });
        }
        ObservationOutcome::Continue => {}
        ObservationOutcome::Error(_) => unreachable!("initial errors are handled above"),
    }
    if timeout.is_zero() {
        return Ok(wait_timeout_result(initial));
    }

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                let threads = baselines
                    .into_iter()
                    .map(|baseline| baseline.previous)
                    .collect();
                return Ok(wait_timeout_result(threads));
            }
            observation = connection.wait_for_notification_or_quiet() => {
                observation?;
            }
        }

        let (current, errors) = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                let threads = baselines
                    .into_iter()
                    .map(|baseline| baseline.previous)
                    .collect();
                return Ok(wait_timeout_result(threads));
            }
            pass = snapshot_pass(connection, thread_ids) => pass?,
        };
        let outcome = if errors.is_empty() {
            observe(&baselines, &current, false)
        } else {
            ObservationOutcome::Error(errors)
        };
        match outcome {
            ObservationOutcome::Ready(trigger_thread_ids) => {
                return Ok(ThreadsWaitResult {
                    reason: ThreadsWaitReason::Ready,
                    trigger_thread_ids,
                    threads: current,
                    errors: Vec::new(),
                });
            }
            ObservationOutcome::Error(errors) => {
                return Ok(ThreadsWaitResult {
                    reason: ThreadsWaitReason::Error,
                    trigger_thread_ids: Vec::new(),
                    threads: current,
                    errors,
                });
            }
            ObservationOutcome::Continue => {
                for (baseline, snapshot) in baselines.iter_mut().zip(current) {
                    baseline.previous = snapshot;
                }
            }
        }
    }
}

pub(super) async fn snapshot_pass(
    connection: &mut AppServerConnection,
    thread_ids: &[String],
) -> Result<(Vec<ThreadSnapshot>, Vec<ToolErrorData>), ToolErrorData> {
    let mut snapshots = Vec::with_capacity(thread_ids.len());
    let mut errors = Vec::new();
    for thread_id in thread_ids {
        match connection.compact_snapshot(thread_id).await {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(error) if error.category == ToolErrorCategory::AuthorityTransportFailure => {
                return Err(error);
            }
            Err(mut error) => {
                error.thread_id = Some(thread_id.clone());
                errors.push(error);
            }
        }
    }
    Ok((snapshots, errors))
}

pub(super) fn observe(
    baselines: &[WaitBaseline],
    current: &[ThreadSnapshot],
    initial: bool,
) -> ObservationOutcome {
    let triggers = baselines
        .iter()
        .filter_map(|baseline| {
            let snapshot = current.get(baseline.input_index)?;
            let ready = immediately_ready(snapshot)
                || (!initial
                    && matches!(baseline.previous.status, ThreadStatus::Active { .. })
                    && !matches!(snapshot.status, ThreadStatus::Active { .. }))
                || (!initial
                    && baseline.previous.active_turn_id.is_some()
                    && snapshot.active_turn_id.is_none());
            ready.then(|| snapshot.thread_id.clone())
        })
        .collect::<Vec<_>>();
    if triggers.is_empty() {
        ObservationOutcome::Continue
    } else {
        ObservationOutcome::Ready(triggers)
    }
}

pub(super) fn immediately_ready(snapshot: &ThreadSnapshot) -> bool {
    match &snapshot.status {
        ThreadStatus::Active { active_flags } => !active_flags.is_empty(),
        ThreadStatus::NotLoaded | ThreadStatus::Idle | ThreadStatus::SystemError => true,
    }
}

pub(super) fn wait_timeout_result(threads: Vec<ThreadSnapshot>) -> ThreadsWaitResult {
    ThreadsWaitResult {
        reason: ThreadsWaitReason::Timeout,
        trigger_thread_ids: Vec::new(),
        threads,
        errors: Vec::new(),
    }
}
