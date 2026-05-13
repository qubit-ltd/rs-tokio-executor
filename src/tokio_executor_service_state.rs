/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::sync::{Arc, Mutex, MutexGuard, atomic::AtomicU8};

use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_lock::Monitor;
use tokio::{sync::Notify, task::AbortHandle};

use crate::executor_service_lifecycle_bits;

/// Abort handle tracked with a service-local task marker.
struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    marker: Arc<()>,
    /// Tokio abort handle used by immediate shutdown.
    handle: AbortHandle,
    /// Completion hook used to publish cancellation for result handles.
    cancel: Box<dyn FnOnce() + Send + 'static>,
}

/// Number of accepted blocking tasks by scheduler state.
#[derive(Default)]
struct TokioExecutorTaskCounts {
    /// Tasks submitted to Tokio but whose blocking closure has not started.
    queued: usize,
    /// Tasks whose blocking closure has started and not yet finished.
    running: usize,
}

impl TokioExecutorTaskCounts {
    /// Records a newly accepted task as queued.
    fn accept_task(&mut self) {
        self.queued += 1;
    }

    /// Moves one accepted task from queued to running.
    fn mark_started(&mut self) {
        debug_assert!(self.queued > 0);
        self.queued = self.queued.saturating_sub(1);
        self.running += 1;
    }

    /// Records task completion from either the queued or running state.
    ///
    /// # Parameters
    ///
    /// * `started` - Whether the task had already moved to running.
    fn finish_task(&mut self, started: bool) {
        if started {
            debug_assert!(self.running > 0);
            self.running = self.running.saturating_sub(1);
        } else {
            debug_assert!(self.queued > 0);
            self.queued = self.queued.saturating_sub(1);
        }
    }

    /// Returns whether no accepted task remains active.
    fn is_empty(&self) -> bool {
        self.queued == 0 && self.running == 0
    }
}

/// Snapshot of blocking task counts used for stop reporting.
pub(crate) struct TokioExecutorTaskCountSnapshot {
    /// Number of queued accepted tasks.
    pub(crate) queued: usize,
    /// Number of running accepted tasks.
    pub(crate) running: usize,
}

/// Shared state for [`crate::TokioExecutorService`].
#[derive(Default)]
pub(crate) struct TokioExecutorServiceState {
    /// Stored lifecycle state before derived termination.
    lifecycle: AtomicU8,
    /// Accepted blocking task counts and synchronous termination monitor.
    task_counts: Monitor<TokioExecutorTaskCounts>,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for tasks accepted by this service.
    abort_handles: Mutex<Vec<TrackedAbortHandle>>,
    /// Notifies waiters once shutdown has completed and no tasks remain active.
    pub(crate) terminated_notify: Notify,
}

impl TokioExecutorServiceState {
    /// Acquires the submission lock while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the submission lock.
    pub(crate) fn lock_submission(&self) -> MutexGuard<'_, ()> {
        self.submission_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records a newly accepted task as queued.
    pub(crate) fn accept_task(&self) {
        self.task_counts.write(TokioExecutorTaskCounts::accept_task);
    }

    /// Moves a task from queued to running.
    pub(crate) fn mark_task_started(&self) {
        self.task_counts
            .write(TokioExecutorTaskCounts::mark_started);
    }

    /// Records task completion or queued-task abortion.
    ///
    /// # Parameters
    ///
    /// * `started` - Whether the task had already started running.
    pub(crate) fn finish_task(&self, started: bool) {
        let terminated = self.task_counts.write(|counts| {
            counts.finish_task(started);
            self.is_not_running() && counts.is_empty()
        });
        if terminated {
            self.notify_termination_waiters();
        }
    }

    /// Returns the current queued and running task counts.
    pub(crate) fn task_count_snapshot(&self) -> TokioExecutorTaskCountSnapshot {
        self.task_counts
            .read(|counts| TokioExecutorTaskCountSnapshot {
                queued: counts.queued,
                running: counts.running,
            })
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
    pub(crate) fn register_abort_handle<F>(&self, marker: Arc<()>, handle: AbortHandle, cancel: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut handles = self.lock_abort_handles();
        if !handle.is_finished() {
            handles.push(TrackedAbortHandle {
                marker,
                handle,
                cancel: Box::new(cancel),
            });
        }
    }

    /// Removes the abort handle associated with the supplied marker.
    ///
    /// # Parameters
    ///
    /// * `marker` - Service-local task marker for the task that finished.
    pub(crate) fn remove_abort_handle(&self, marker: &Arc<()>) {
        self.lock_abort_handles()
            .retain(|tracked| !Arc::ptr_eq(&tracked.marker, marker));
    }

    /// Aborts all currently tracked unfinished tasks.
    ///
    /// # Returns
    ///
    /// Number of unfinished tasks for which an abort request was sent.
    pub(crate) fn abort_tracked_tasks(&self) -> usize {
        let mut cancellation_count = 0usize;
        let mut handles = self.lock_abort_handles();
        for tracked in handles.drain(..) {
            if !tracked.handle.is_finished() {
                tracked.handle.abort();
                (tracked.cancel)();
                cancellation_count += 1;
            }
        }
        cancellation_count
    }

    /// Wakes termination waiters when shutdown and task completion allow it.
    pub(crate) fn notify_if_terminated(&self) {
        let terminated = self
            .task_counts
            .read(|counts| self.is_not_running() && counts.is_empty());
        if terminated {
            self.notify_termination_waiters();
        }
    }

    /// Blocks until the service has reached termination.
    pub(crate) fn wait_termination(&self) {
        self.task_counts.wait_until(
            |counts| self.is_not_running() && counts.is_empty(),
            |_counts| {},
        );
    }

    /// Wakes both synchronous and asynchronous termination waiters.
    fn notify_termination_waiters(&self) {
        self.task_counts.notify_all();
        self.terminated_notify.notify_waiters();
    }

    /// Acquires the abort handle list while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the tracked Tokio abort handles.
    fn lock_abort_handles(&self) -> MutexGuard<'_, Vec<TrackedAbortHandle>> {
        self.abort_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns the observed lifecycle state.
    pub(crate) fn lifecycle(&self) -> ExecutorServiceLifecycle {
        let lifecycle = executor_service_lifecycle_bits::load(&self.lifecycle);
        let has_no_tasks = self.task_counts.read(TokioExecutorTaskCounts::is_empty);
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
    pub(crate) fn shutdown(&self) {
        executor_service_lifecycle_bits::shutdown(&self.lifecycle);
    }

    /// Marks the service as stopping.
    pub(crate) fn stop(&self) {
        executor_service_lifecycle_bits::stop(&self.lifecycle);
    }
}
