// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
/// Number of accepted blocking tasks by scheduler state.
#[derive(Default)]
pub(in crate::tokio_executor_service_state) struct TokioExecutorTaskCounts {
    /// Tasks submitted to Tokio but whose blocking closure has not started.
    pub(in crate::tokio_executor_service_state) queued: usize,
    /// Tasks whose blocking closure has started and not yet finished.
    pub(in crate::tokio_executor_service_state) running: usize,
}

impl TokioExecutorTaskCounts {
    /// Records a newly accepted task as queued.
    pub(in crate::tokio_executor_service_state) fn accept_task(&mut self) {
        self.queued += 1;
    }

    /// Moves one accepted task from queued to running.
    pub(in crate::tokio_executor_service_state) fn mark_started(&mut self) {
        debug_assert!(self.queued > 0);
        self.queued = self.queued.saturating_sub(1);
        self.running += 1;
    }

    /// Records task completion from either the queued or running state.
    ///
    /// # Parameters
    ///
    /// * `started` - Whether the task had already moved to running.
    pub(in crate::tokio_executor_service_state) fn finish_task(&mut self, started: bool) {
        if started {
            debug_assert!(self.running > 0);
            self.running = self.running.saturating_sub(1);
        } else {
            debug_assert!(self.queued > 0);
            self.queued = self.queued.saturating_sub(1);
        }
    }

    /// Returns whether no accepted task remains active.
    pub(in crate::tokio_executor_service_state) fn is_empty(&self) -> bool {
        self.queued == 0 && self.running == 0
    }
}
