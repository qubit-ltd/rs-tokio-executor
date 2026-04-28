/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use std::sync::Arc;

use crate::tokio_executor_service_state::TokioExecutorServiceState;

/// Task lifecycle guard for [`crate::TokioExecutorService`].
pub(crate) struct TokioServiceTaskGuard {
    /// Shared service state updated when the guard is dropped.
    state: Arc<TokioExecutorServiceState>,
    /// Service-local marker for removing the tracked abort handle.
    marker: Arc<()>,
}

impl TokioServiceTaskGuard {
    /// Creates a guard that decrements the active task count on drop.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared state whose active-task counter is decremented when
    ///   the guard is dropped.
    /// * `marker` - Service-local marker for the task guarded by this value.
    ///
    /// # Returns
    ///
    /// A lifecycle guard bound to the supplied service state.
    pub(crate) fn new(state: Arc<TokioExecutorServiceState>, marker: Arc<()>) -> Self {
        Self { state, marker }
    }
}

impl Drop for TokioServiceTaskGuard {
    /// Updates service counters when a task completes or is aborted.
    fn drop(&mut self) {
        self.state.remove_abort_handle(&self.marker);
        if self.state.active_tasks.dec() == 0 {
            self.state.notify_if_terminated();
        }
    }
}
