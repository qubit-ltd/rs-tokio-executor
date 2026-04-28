/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use std::sync::Arc;

use crate::tokio_io_executor_service_state::TokioIoExecutorServiceState;

/// Task lifecycle guard for [`crate::TokioIoExecutorService`].
pub(crate) struct TokioIoServiceTaskGuard {
    /// Shared service state updated when the guard is dropped.
    state: Arc<TokioIoExecutorServiceState>,
    /// Service-local marker for removing the tracked abort handle.
    marker: Arc<()>,
}

impl TokioIoServiceTaskGuard {
    /// Creates a guard that decrements the active-task count on drop.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared service state whose active-task count is decremented
    ///   when this guard is dropped.
    /// * `marker` - Service-local marker for the task guarded by this value.
    ///
    /// # Returns
    ///
    /// A lifecycle guard bound to the supplied service state.
    pub(crate) fn new(state: Arc<TokioIoExecutorServiceState>, marker: Arc<()>) -> Self {
        Self { state, marker }
    }
}

impl Drop for TokioIoServiceTaskGuard {
    /// Updates service counters when an async task completes or is aborted.
    fn drop(&mut self) {
        self.state.remove_abort_handle(&self.marker);
        if self.state.active_tasks.dec() == 0 {
            self.state.notify_if_terminated();
        }
    }
}
