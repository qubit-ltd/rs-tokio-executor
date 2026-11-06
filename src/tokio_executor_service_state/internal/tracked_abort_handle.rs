// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::sync::Arc;

use tokio::task::AbortHandle;

use crate::tokio_task_registration::TokioTaskRegistration;

/// Abort handle tracked with a service-local task marker.
pub(in crate::tokio_executor_service_state) struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    pub(in crate::tokio_executor_service_state) _marker: Arc<TokioTaskRegistration>,
    /// Tokio abort handle used by immediate shutdown.
    pub(in crate::tokio_executor_service_state) handle: AbortHandle,
    /// Completion hook used to publish cancellation for result handles.
    ///
    /// The hook returns `true` only when queued-task accounting was actually
    /// cancelled by the call.
    pub(in crate::tokio_executor_service_state) cancel: Box<dyn FnOnce() -> bool + Send + 'static>,
}
