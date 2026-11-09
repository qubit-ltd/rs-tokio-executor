// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;
use std::time::Instant;

use parking_lot::Mutex;
use parking_lot::MutexGuard;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_lock::ParkingLotMonitor;
use tokio::sync::Notify;
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::TokioExecutorServiceStats;
use crate::executor_service_lifecycle_bits;
use crate::tokio_task_registration::TokioTaskRegistration;
use crate::tokio_task_registration::key;

mod internal;

use internal::TokioExecutorTaskCounts;
use internal::TrackedAbortHandle;

/// Maximum number of accepted unfinished blocking tasks by default.
const DEFAULT_TASK_CAPACITY: usize = 1024;

/// Shared state for [`crate::TokioExecutorService`].
pub(crate) struct TokioExecutorServiceState {
    /// Stored lifecycle state before derived termination.
    lifecycle: AtomicU8,
    /// Accepted blocking task counts and synchronous termination monitor.
    task_counts: ParkingLotMonitor<TokioExecutorTaskCounts>,
    /// Maximum number of accepted blocking tasks that have not finished.
    task_capacity: NonZeroUsize,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for tasks accepted by this service.
    abort_handles: Mutex<HashMap<usize, TrackedAbortHandle>>,
    /// Notifies waiters once shutdown has completed and no tasks remain
    /// active.
    pub(crate) terminated_notify: Notify,
    /// Generation counter notifying asynchronous capacity waiters.
    capacity_tx: watch::Sender<u64>,
}

impl Default for TokioExecutorServiceState {
    /// Creates service state with the default task capacity.
    fn default() -> Self {
        Self::with_task_capacity(
            NonZeroUsize::new(DEFAULT_TASK_CAPACITY).expect("default task capacity should be nonzero"),
        )
    }
}

impl TokioExecutorServiceState {
    /// Creates state with an explicit accepted-task capacity.
    ///
    /// # Parameters
    ///
    /// * `task_capacity` - Maximum queued and running tasks accepted at once.
    ///
    /// # Returns
    ///
    /// Shared service state configured with the supplied nonzero capacity.
    pub(crate) fn with_task_capacity(task_capacity: NonZeroUsize) -> Self {
        Self {
            lifecycle: AtomicU8::default(),
            task_counts: ParkingLotMonitor::new(TokioExecutorTaskCounts::default()),
            task_capacity,
            submission_lock: Mutex::new(()),
            abort_handles: Mutex::new(HashMap::new()),
            terminated_notify: Notify::new(),
            capacity_tx: watch::channel(0).0,
        }
    }

    /// Acquires the submission lock.
    ///
    /// # Returns
    ///
    /// A guard for the submission lock.
    pub(crate) fn lock_submission(&self) -> MutexGuard<'_, ()> {
        self.submission_lock.lock()
    }

    /// Subscribes to task-capacity or lifecycle changes.
    ///
    /// # Returns
    ///
    /// A receiver that provides hints; callers must retry admission after each
    /// notification.
    pub(crate) fn capacity_changes(&self) -> watch::Receiver<u64> {
        self.capacity_tx.subscribe()
    }

    /// Returns a best-effort snapshot of queue, worker, and lifecycle state.
    ///
    /// # Returns
    ///
    /// The configured capacity and task counts. Lifecycle may be sampled at an
    /// adjacent instant to the counts.
    pub(crate) fn stats(&self) -> TokioExecutorServiceStats {
        let (queued, running) = self.task_count_snapshot();
        TokioExecutorServiceStats {
            lifecycle: self.lifecycle(),
            task_capacity: self.task_capacity.get(),
            queued,
            running,
        }
    }

    /// Publishes a task-capacity or lifecycle change.
    pub(crate) fn notify_capacity_changed(&self) {
        self.capacity_tx
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// Reserves a capacity slot and records a newly accepted task as queued.
    ///
    /// The caller must hold the submission lock so capacity cannot be reserved
    /// concurrently with shutdown admission.
    ///
    /// # Returns
    ///
    /// `true` if capacity was reserved; `false` if the unfinished-task limit
    /// has been reached.
    pub(crate) fn try_accept_task(&self) -> bool {
        self.task_counts.with_write(|counts| {
            if counts.queued.saturating_add(counts.running) >= self.task_capacity.get() {
                return false;
            }
            counts.accept_task();
            true
        })
    }

    /// Moves a task from queued to running.
    pub(crate) fn mark_task_started(&self) {
        self.task_counts.with_write(TokioExecutorTaskCounts::mark_started);
    }

    /// Records task completion or queued-task abortion.
    ///
    /// # Parameters
    ///
    /// * `started` - Whether the task had already started running.
    pub(crate) fn finish_task(&self, started: bool) {
        let terminated = self.task_counts.with_write(|counts| {
            counts.finish_task(started);
            self.is_not_running() && counts.is_empty()
        });
        self.notify_capacity_changed();
        if terminated {
            self.notify_termination_waiters();
        }
    }

    /// Returns the current queued and running task counts.
    ///
    /// # Returns
    ///
    /// A tuple whose first element is the queued count and second element is
    /// the running count.
    pub(crate) fn task_count_snapshot(&self) -> (usize, usize) {
        self.task_counts.with_read(|counts| (counts.queued, counts.running))
    }

    /// Registers an abort handle if the task has not already finished.
    ///
    /// The abort-handle lock is held while checking completion and pushing the
    /// handle so a concurrently finishing task either removes the pushed handle
    /// or observes completion and leaves no stale entry behind.
    ///
    /// # Parameters
    ///
    /// * `marker` - Service-local task marker shared with the lifecycle guard.
    /// * `handle` - Tokio abort handle for the accepted task.
    /// * `cancel` - Hook that publishes queued-task cancellation and reports
    ///   whether queued service accounting was actually cancelled.
    ///
    /// # Type Parameters
    ///
    /// * `F` - Queued-cancellation hook type.
    pub(crate) fn register_abort_handle<F>(&self, marker: Arc<TokioTaskRegistration>, handle: AbortHandle, cancel: F)
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        let mut handles = self.lock_abort_handles();
        if !marker.is_finished() && !handle.is_finished() {
            let previous = handles.insert(
                key(&marker),
                TrackedAbortHandle {
                    _marker: marker,
                    handle,
                    cancel: Box::new(cancel),
                },
            );
            debug_assert!(previous.is_none());
        }
    }

    /// Removes the abort handle associated with the supplied marker.
    ///
    /// # Parameters
    ///
    /// * `marker` - Service-local task marker for the task that finished.
    pub(crate) fn remove_abort_handle(&self, marker: &Arc<TokioTaskRegistration>) {
        marker.finish();
        let removed = {
            let mut handles = self.lock_abort_handles();
            handles.remove(&key(marker))
        };
        drop(removed);
    }

    /// Aborts all currently tracked unfinished tasks.
    ///
    /// # Returns
    ///
    /// Number of queued tasks whose service-side accounting was cancelled.
    pub(crate) fn abort_tracked_tasks(&self) -> usize {
        let mut cancellation_count = 0usize;
        let handles = std::mem::take(&mut *self.lock_abort_handles());
        for (_, tracked) in handles {
            if !tracked.handle.is_finished() {
                tracked.handle.abort();
                if (tracked.cancel)() {
                    cancellation_count += 1;
                }
            }
        }
        cancellation_count
    }

    /// Wakes termination waiters when shutdown and task completion allow it.
    pub(crate) fn notify_if_terminated(&self) {
        let terminated = self
            .task_counts
            .with_read(|counts| self.is_not_running() && counts.is_empty());
        if terminated {
            self.notify_termination_waiters();
        }
    }

    /// Blocks until the service has reached termination.
    pub(crate) fn wait_termination(&self) {
        self.wait_termination_inner(None);
    }

    /// Waits for termination while signaling when the monitor observes a
    /// predicate value that still has outstanding work.
    ///
    /// # Parameters
    ///
    /// * `checked_tx` - Non-blocking signal sent after the monitor observes
    ///   outstanding work.
    #[cfg(test)]
    pub(crate) fn wait_termination_with_observer(&self, checked_tx: std::sync::mpsc::SyncSender<()>) {
        self.wait_termination_inner(Some(&checked_tx));
    }

    /// Waits for termination and optionally reports a non-terminated
    /// predicate observation to a test observer.
    ///
    /// # Parameters
    ///
    /// * `checked_tx` - Optional non-blocking channel for the test observer.
    fn wait_termination_inner(&self, checked_tx: Option<&std::sync::mpsc::SyncSender<()>>) {
        self.task_counts.wait_until_ready(|counts| {
            let terminated = self.is_not_running() && counts.is_empty();
            if let Some(checked_tx) = checked_tx
                && !terminated
            {
                let _ = checked_tx.try_send(());
            }
            terminated
        });
    }

    /// Waits until termination or the total timeout expires.
    ///
    /// # Parameters
    ///
    /// * `timeout` - Maximum total duration to wait.
    pub(crate) fn wait_termination_timeout(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.is_not_running() && self.task_counts.with_read(TokioExecutorTaskCounts::is_empty) {
                return true;
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return false;
            }
            match self
                .task_counts
                .wait_until_ready_with_total_timeout(remaining.min(Duration::from_secs(3600)), |counts| {
                    self.is_not_running() && counts.is_empty()
                }) {
                Ok(result) if result.is_ready() => return true,
                Ok(_) => {}
                Err(_) => {
                    return self.is_not_running() && self.task_counts.with_read(TokioExecutorTaskCounts::is_empty);
                }
            }
        }
    }

    /// Wakes both synchronous and asynchronous termination waiters.
    fn notify_termination_waiters(&self) {
        self.task_counts.notify_all();
        self.terminated_notify.notify_waiters();
    }

    /// Acquires the abort-handle list lock.
    ///
    /// # Returns
    ///
    /// A guard for the tracked Tokio abort handles.
    fn lock_abort_handles(&self) -> MutexGuard<'_, HashMap<usize, TrackedAbortHandle>> {
        self.abort_handles.lock()
    }

    /// Returns the observed lifecycle state.
    pub(crate) fn lifecycle(&self) -> ExecutorServiceLifecycle {
        let lifecycle = executor_service_lifecycle_bits::load(&self.lifecycle);
        let has_no_tasks = self.task_counts.with_read(TokioExecutorTaskCounts::is_empty);
        if lifecycle != ExecutorServiceLifecycle::Running && has_no_tasks {
            ExecutorServiceLifecycle::Terminated
        } else {
            lifecycle
        }
    }

    /// Returns whether shutdown or stop has been requested.
    pub(crate) fn is_not_running(&self) -> bool {
        executor_service_lifecycle_bits::load(&self.lifecycle) != ExecutorServiceLifecycle::Running
    }

    /// Marks the service as shutting down.
    ///
    /// The caller publishes the lifecycle change after releasing the
    /// submission lock so registered capacity Wakers may safely reenter.
    pub(crate) fn shutdown(&self) {
        executor_service_lifecycle_bits::shutdown(&self.lifecycle);
    }

    /// Marks the service as stopping.
    ///
    /// The caller publishes the lifecycle change after releasing the
    /// submission lock so registered capacity Wakers may safely reenter.
    pub(crate) fn stop(&self) {
        executor_service_lifecycle_bits::stop(&self.lifecycle);
    }
}
