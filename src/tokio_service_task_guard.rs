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
    atomic::{
        AtomicU8,
        Ordering,
    },
};

use crate::tokio_executor_service_state::TokioExecutorServiceState;

/// State value for a task accepted but not yet started.
const TASK_STATE_QUEUED: u8 = 0;
/// State value for a task whose blocking closure has started.
const TASK_STATE_RUNNING: u8 = 1;
/// State value for a task whose service-side accounting has finished.
const TASK_STATE_FINISHED: u8 = 2;

/// Shared service-side accounting tracker for one blocking Tokio task.
struct TokioServiceTaskTracker {
    /// Shared service state updated by this tracker.
    state: Arc<TokioExecutorServiceState>,
    /// Service-local marker for removing the tracked abort handle.
    marker: Arc<()>,
    /// One-way task accounting state.
    task_state: AtomicU8,
}

impl TokioServiceTaskTracker {
    /// Creates a queued task tracker.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared service state whose task counters this tracker updates.
    /// * `marker` - Service-local marker associated with the task.
    ///
    /// # Returns
    ///
    /// A tracker initialized in the queued state.
    pub(crate) fn new(state: Arc<TokioExecutorServiceState>, marker: Arc<()>) -> Self {
        Self {
            state,
            marker,
            task_state: AtomicU8::new(TASK_STATE_QUEUED),
        }
    }

    /// Returns the service-local task marker.
    ///
    /// # Returns
    ///
    /// The marker used to match a tracked Tokio abort handle.
    #[inline]
    pub(crate) fn marker(&self) -> &Arc<()> {
        &self.marker
    }

    /// Moves this task from queued to running if it has not already finished.
    ///
    /// # Returns
    ///
    /// `true` if the task may run, or `false` if it was cancelled while queued.
    pub(crate) fn mark_started(&self) -> bool {
        match self.task_state.compare_exchange(
            TASK_STATE_QUEUED,
            TASK_STATE_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                self.state.mark_task_started();
                true
            }
            Err(TASK_STATE_RUNNING) => true,
            Err(_) => false,
        }
    }

    /// Finishes this task only if it is still queued.
    ///
    /// # Returns
    ///
    /// `true` if queued-task accounting was completed by this call.
    pub(crate) fn finish_queued(&self) -> bool {
        match self.task_state.compare_exchange(
            TASK_STATE_QUEUED,
            TASK_STATE_FINISHED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                self.state.finish_task(false);
                true
            }
            Err(_) => false,
        }
    }

    /// Finishes this task from its current accounting state.
    pub(crate) fn finish(&self) {
        match self.task_state.swap(TASK_STATE_FINISHED, Ordering::AcqRel) {
            TASK_STATE_QUEUED => self.state.finish_task(false),
            TASK_STATE_RUNNING => self.state.finish_task(true),
            _ => {}
        }
    }
}

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
    pub(crate) fn new(state: Arc<TokioExecutorServiceState>, marker: Arc<()>) -> Self {
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
    pub(crate) fn mark_started(&self) -> bool {
        self.tracker.mark_started()
    }

    /// Creates a one-shot callback that finishes queued-task accounting.
    ///
    /// # Returns
    ///
    /// A callback used by service stop handling when Tokio aborts a queued
    /// blocking task before its closure starts.
    pub(crate) fn finish_queued_once_callback(&self) -> impl FnOnce() + Send + 'static {
        let tracker = Arc::clone(&self.tracker);
        move || {
            tracker.finish_queued();
        }
    }

    /// Creates a reusable callback that finishes queued-task accounting.
    ///
    /// # Returns
    ///
    /// A callback used by tracked task handles when user cancellation wins
    /// before the blocking closure starts.
    pub(crate) fn finish_queued_callback(&self) -> impl Fn() + Send + Sync + 'static {
        let tracker = Arc::clone(&self.tracker);
        move || {
            tracker.finish_queued();
        }
    }
}

impl Drop for TokioServiceTaskGuard {
    /// Updates service counters when a task completes or is aborted.
    fn drop(&mut self) {
        self.tracker
            .state
            .remove_abort_handle(self.tracker.marker());
        self.tracker.finish();
    }
}
