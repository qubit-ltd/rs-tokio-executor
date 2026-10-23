// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_task_registration::TaskRegistration;

/// State value for a task accepted but not yet started.
const TASK_STATE_QUEUED: u8 = 0;
/// State value for a task whose blocking closure has started.
const TASK_STATE_RUNNING: u8 = 1;
/// State value for a task whose service-side accounting has finished.
const TASK_STATE_FINISHED: u8 = 2;

/// Shared service-side accounting tracker for one blocking Tokio task.
pub(super) struct TokioServiceTaskTracker {
    /// Shared service state updated by this tracker.
    state: Arc<TokioExecutorServiceState>,
    /// Service-local marker for removing the tracked abort handle.
    marker: Arc<TaskRegistration>,
    /// One-way task accounting state.
    task_state: AtomicU8,
}

impl TokioServiceTaskTracker {
    /// Creates a queued task tracker.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared service state whose task counters this tracker
    ///   updates.
    /// * `marker` - Service-local marker associated with the task.
    ///
    /// # Returns
    ///
    /// A tracker initialized in the queued state.
    pub(super) fn new(state: Arc<TokioExecutorServiceState>, marker: Arc<TaskRegistration>) -> Self {
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
    #[must_use]
    #[inline]
    pub(super) fn marker(&self) -> &Arc<TaskRegistration> {
        &self.marker
    }

    /// Moves this task from queued to running if it has not already finished.
    ///
    /// # Returns
    ///
    /// `true` if the task may run, or `false` if it was cancelled while queued.
    pub(super) fn mark_started(&self) -> bool {
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
    pub(super) fn finish_queued(&self) -> bool {
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

    /// Removes this task from abort tracking and finishes queued accounting.
    ///
    /// # Returns
    ///
    /// `true` if queued-task accounting was completed by this call.
    pub(super) fn cancel_queued(&self) -> bool {
        self.state.remove_abort_handle(&self.marker);
        self.finish_queued()
    }

    /// Finishes this task from its current accounting state.
    pub(super) fn finish(&self) {
        match self.task_state.swap(TASK_STATE_FINISHED, Ordering::AcqRel) {
            TASK_STATE_QUEUED => self.state.finish_task(false),
            TASK_STATE_RUNNING => self.state.finish_task(true),
            _ => {}
        }
    }
}
