// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Best-effort snapshot of a Tokio async executor service.

use qubit_executor::service::ExecutorServiceLifecycle;

/// Accepted-task count for a Tokio async executor service.
///
/// `accepted_unfinished` includes tasks not yet polled by Tokio; it does not
/// indicate how many futures are currently being polled.
///
/// # Examples
///
/// ```
/// use qubit_tokio_executor::TokioIoExecutorService;
/// use qubit_executor::service::ExecutorService;
///
/// let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime should build");
/// let service = TokioIoExecutorService::new(runtime.handle().clone());
/// let stats = service.stats();
/// assert_eq!(stats.accepted_unfinished, 0);
/// service.shutdown();
/// runtime.block_on(service.await_termination());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokioIoExecutorServiceStats {
    /// Observed service lifecycle.
    pub lifecycle: ExecutorServiceLifecycle,
    /// Maximum number of accepted futures that have not completed.
    pub task_capacity: usize,
    /// Accepted futures that have not completed or been dropped after abort.
    pub accepted_unfinished: usize,
}
