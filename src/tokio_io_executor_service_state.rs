// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use parking_lot::Mutex;
use parking_lot::MutexGuard;
use std::sync::{Arc, atomic::AtomicU8};

use qubit_atomic::AtomicCount;
use qubit_executor::service::ExecutorServiceLifecycle;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use crate::executor_service_lifecycle_bits;

/// Abort handle tracked with a service-local task marker.
struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    marker: Arc<()>,
    /// Tokio abort handle used by immediate shutdown.
    handle: AbortHandle,
}

/// Shared state for [`crate::TokioIoExecutorService`].
#[derive(Default)]
pub(crate) struct TokioIoExecutorServiceState {
    /// Stored lifecycle state before derived termination.
    lifecycle: AtomicU8,
    /// Number of accepted async tasks that have not finished or been aborted.
    pub(crate) active_tasks: AtomicCount,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for async tasks accepted by this service.
    abort_handles: Mutex<Vec<TrackedAbortHandle>>,
    /// Wakes async termination waiters after lifecycle-affecting changes.
    termination_notify: Notify,
}

impl TokioIoExecutorServiceState {
    /// Acquires the submission lock while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the submission lock.
    pub(crate) fn lock_submission(&self) -> MutexGuard<'_, ()> {
        self.submission_lock.lock()
    }

    /// Returns the submission lock used for admission control.
    #[inline]
    pub(crate) fn submission_lock(&self) -> &Mutex<()> {
        &self.submission_lock
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

    /// Acquires the abort-handle list while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the tracked Tokio abort handles.
    fn lock_abort_handles(&self) -> MutexGuard<'_, Vec<TrackedAbortHandle>> {
        self.abort_handles.lock()
    }

    /// Returns the observed lifecycle state.
    pub(crate) fn lifecycle(&self) -> ExecutorServiceLifecycle {
        let lifecycle = executor_service_lifecycle_bits::load(&self.lifecycle);
        if lifecycle != ExecutorServiceLifecycle::Running && self.active_tasks.is_zero() {
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
        self.termination_notify.notify_waiters();
    }

    /// Marks the service as stopping.
    pub(crate) fn stop(&self) {
        executor_service_lifecycle_bits::stop(&self.lifecycle);
        self.termination_notify.notify_waiters();
    }

    /// Wakes async waiters after a task or lifecycle transition changes.
    pub(crate) fn notify_termination_waiters(&self) {
        self.termination_notify.notify_waiters();
    }

    /// Waits until shutdown has completed and all accepted tasks are gone.
    ///
    /// Registers the next notification before observing lifecycle state so an
    /// adjacent termination transition cannot be missed.
    pub(crate) async fn await_termination(&self) {
        let notified = self.termination_notify.notified();
        tokio::pin!(notified);
        loop {
            notified.as_mut().enable();
            if self.lifecycle() == ExecutorServiceLifecycle::Terminated {
                return;
            }
            notified.as_mut().await;
            notified.set(self.termination_notify.notified());
        }
    }
}
