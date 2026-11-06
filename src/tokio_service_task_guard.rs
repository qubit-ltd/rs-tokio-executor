// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::sync::Arc;

use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_task_registration::TokioTaskRegistration;

mod internal;

use internal::TokioServiceTaskTracker;

/// Task lifecycle guard for [`crate::TokioExecutorService`].
pub(crate) struct TokioServiceTaskGuard {
    /// Shared task tracker updated when the guard is dropped.
    tracker: Arc<TokioServiceTaskTracker>,
}

impl TokioServiceTaskGuard {
    /// Creates a guard that finishes service-side task accounting on drop.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared service state whose task counters this guard updates.
    /// * `marker` - Service-local marker associated with the task.
    ///
    /// # Returns
    ///
    /// A lifecycle guard bound to the supplied tracker.
    pub(crate) fn new(state: Arc<TokioExecutorServiceState>, marker: Arc<TokioTaskRegistration>) -> Self {
        Self {
            tracker: Arc::new(TokioServiceTaskTracker::new(state, marker)),
        }
    }

    /// Marks the guarded task as started.
    ///
    /// # Returns
    ///
    /// `true` if the task should run, or `false` if it was already cancelled
    /// while queued.
    #[inline]
    pub(crate) fn mark_started(&self) -> bool {
        self.tracker.mark_started()
    }

    /// Creates a one-shot callback that finishes queued-task accounting.
    ///
    /// # Returns
    ///
    /// A callback used by service stop handling when Tokio aborts a queued
    /// blocking task before its closure starts. The callback returns `true`
    /// only when this call completed queued-task accounting.
    pub(crate) fn finish_queued_once_callback(&self) -> impl FnOnce() -> bool + Send + 'static {
        let tracker = Arc::clone(&self.tracker);
        move || tracker.finish_queued()
    }

    /// Creates a reusable callback that cancels queued-task accounting.
    ///
    /// # Returns
    ///
    /// A callback used by tracked task handles when user cancellation wins.
    /// It removes the service abort handle before finishing queued accounting
    /// so later service stops do not count the same task again.
    pub(crate) fn cancel_queued_callback(&self) -> impl Fn() + Send + Sync + 'static {
        let tracker = Arc::clone(&self.tracker);
        move || {
            tracker.cancel_queued();
        }
    }
}

impl Drop for TokioServiceTaskGuard {
    /// Updates service counters when a task completes or is aborted.
    fn drop(&mut self) {
        self.tracker.state.remove_abort_handle(self.tracker.marker());
        self.tracker.finish();
    }
}
