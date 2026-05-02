/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::sync::{
    Arc,
    Mutex,
    MutexGuard,
};

use qubit_atomic::{
    Atomic,
    AtomicCount,
};
use tokio::{
    sync::Notify,
    task::AbortHandle,
};

/// Abort handle tracked with a service-local task marker.
struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    marker: Arc<()>,
    /// Tokio abort handle used by immediate shutdown.
    handle: AbortHandle,
}

/// Shared state for [`crate::TokioExecutorService`].
#[derive(Default)]
pub(crate) struct TokioExecutorServiceState {
    /// Whether shutdown has been requested.
    pub(crate) shutdown: Atomic<bool>,
    /// Number of accepted Tokio tasks that have not finished or been aborted.
    pub(crate) active_tasks: AtomicCount,
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
    pub(crate) fn register_abort_handle(&self, marker: Arc<()>, handle: AbortHandle) {
        let mut handles = self.lock_abort_handles();
        if !handle.is_finished() {
            handles.push(TrackedAbortHandle { marker, handle });
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
                cancellation_count += 1;
            }
        }
        cancellation_count
    }

    /// Wakes termination waiters when shutdown and task completion allow it.
    pub(crate) fn notify_if_terminated(&self) {
        if self.shutdown.load() && self.active_tasks.is_zero() {
            self.terminated_notify.notify_waiters();
        }
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
}
