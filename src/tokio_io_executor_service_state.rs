// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
// qubit-style: allow multiple-public-types
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use parking_lot::Mutex;
use parking_lot::MutexGuard;
use qubit_atomic::AtomicCount;
use qubit_executor::service::ExecutorServiceLifecycle;
use tokio::pin;
use tokio::sync::Notify;
use tokio::sync::watch;
use tokio::task::AbortHandle;

/// Maximum number of accepted unfinished async tasks by default.
const DEFAULT_TASK_CAPACITY: usize = 1024;

use crate::TokioIoExecutorServiceStats;
use crate::executor_service_lifecycle_bits;
use crate::tokio_task_registration::TaskRegistration;
use crate::tokio_task_registration::key;

/// Abort handle tracked with a service-local task marker.
struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    _marker: Arc<TaskRegistration>,
    /// Tokio abort handle used by immediate shutdown.
    handle: AbortHandle,
}

/// Shared state for [`crate::TokioIoExecutorService`].
pub(crate) struct TokioIoExecutorServiceState {
    /// Stored lifecycle state before derived termination.
    lifecycle: AtomicU8,
    /// Number of accepted async tasks that have not finished or been aborted.
    pub(crate) active_tasks: AtomicCount,
    /// Maximum number of accepted async tasks that have not finished.
    task_capacity: NonZeroUsize,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for async tasks accepted by this service.
    abort_handles: Mutex<HashMap<usize, TrackedAbortHandle>>,
    /// Wakes async termination waiters after lifecycle-affecting changes.
    termination_notify: Notify,
    /// Generation counter notifying asynchronous capacity waiters.
    capacity_tx: watch::Sender<u64>,
}

impl Default for TokioIoExecutorServiceState {
    /// Creates service state with the default task capacity.
    fn default() -> Self {
        Self::with_task_capacity(
            NonZeroUsize::new(DEFAULT_TASK_CAPACITY).expect("default task capacity should be nonzero"),
        )
    }
}

impl TokioIoExecutorServiceState {
    /// Creates state with an explicit accepted-task capacity.
    ///
    /// # Parameters
    ///
    /// * `task_capacity` - Maximum accepted futures that have not completed.
    ///
    /// # Returns
    ///
    /// Shared service state configured with the supplied nonzero capacity.
    pub(crate) fn with_task_capacity(task_capacity: NonZeroUsize) -> Self {
        Self {
            lifecycle: AtomicU8::default(),
            active_tasks: AtomicCount::default(),
            task_capacity,
            submission_lock: Mutex::new(()),
            abort_handles: Mutex::new(HashMap::new()),
            termination_notify: Notify::new(),
            capacity_tx: watch::channel(0).0,
        }
    }

    /// Acquires the submission lock while tolerating poisoned locks.
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

    /// Returns a snapshot of accepted unfinished futures.
    ///
    /// # Returns
    ///
    /// The configured capacity, accepted unfinished count, and lifecycle. The
    /// counts are independently sampled.
    pub(crate) fn stats(&self) -> TokioIoExecutorServiceStats {
        TokioIoExecutorServiceStats {
            lifecycle: self.lifecycle(),
            task_capacity: self.task_capacity.get(),
            accepted_unfinished: self.active_tasks.get(),
        }
    }

    /// Publishes a task-capacity or lifecycle change.
    pub(crate) fn notify_capacity_changed(&self) {
        self.capacity_tx
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// Reserves a capacity slot for one accepted future.
    ///
    /// The caller must hold the submission lock so two new submissions cannot
    /// reserve the same final slot.
    ///
    /// # Returns
    ///
    /// `true` if capacity was reserved; `false` if the active-task limit has
    /// been reached.
    pub(crate) fn try_accept_task(&self) -> bool {
        if self.active_tasks.get() >= self.task_capacity.get() {
            return false;
        }
        self.active_tasks.inc();
        true
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
    pub(crate) fn register_abort_handle(&self, marker: Arc<TaskRegistration>, handle: AbortHandle) {
        let mut handles = self.lock_abort_handles();
        if !marker.is_finished() && !handle.is_finished() {
            let previous = handles.insert(
                key(&marker),
                TrackedAbortHandle {
                    _marker: marker,
                    handle,
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
    pub(crate) fn remove_abort_handle(&self, marker: &Arc<TaskRegistration>) {
        marker.finish();
        self.lock_abort_handles().remove(&key(marker));
    }

    /// Aborts all currently tracked unfinished tasks.
    ///
    /// # Returns
    ///
    /// Number of unfinished tasks for which an abort request was sent.
    pub(crate) fn abort_tracked_tasks(&self) -> usize {
        let mut cancellation_count = 0usize;
        let handles = std::mem::take(&mut *self.lock_abort_handles());
        for (_, tracked) in handles {
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
    fn lock_abort_handles(&self) -> MutexGuard<'_, HashMap<usize, TrackedAbortHandle>> {
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
        self.notify_capacity_changed();
        self.termination_notify.notify_waiters();
    }

    /// Marks the service as stopping.
    pub(crate) fn stop(&self) {
        executor_service_lifecycle_bits::stop(&self.lifecycle);
        self.notify_capacity_changed();
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
        pin!(notified);
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
