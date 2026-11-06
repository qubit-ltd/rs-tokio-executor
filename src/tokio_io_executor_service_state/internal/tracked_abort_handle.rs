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
pub(in crate::tokio_io_executor_service_state) struct TrackedAbortHandle {
    /// Marker shared with the lifecycle guard for the same task.
    pub(in crate::tokio_io_executor_service_state) _marker: Arc<TokioTaskRegistration>,
    /// Tokio abort handle used by immediate shutdown.
    pub(in crate::tokio_io_executor_service_state) handle: AbortHandle,
}
