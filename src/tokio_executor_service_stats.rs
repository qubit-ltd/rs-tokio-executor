// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Best-effort snapshot of a Tokio blocking executor service.

use qubit_executor::service::ExecutorServiceLifecycle;

/// Queue and running counts for a Tokio blocking executor service.
///
/// Counts are sampled under the task-count monitor. Lifecycle is sampled
/// independently and may describe an adjacent instant.
///
/// # Examples
///
/// ```
/// use qubit_tokio_executor::TokioExecutorService;
/// use qubit_executor::service::ExecutorService;
///
/// let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime should build");
/// let service = TokioExecutorService::new(runtime.handle().clone());
/// let stats = service.stats();
/// assert!(stats.task_capacity > 0);
/// service.shutdown();
/// runtime.block_on(service.await_termination());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokioExecutorServiceStats {
    /// Observed service lifecycle.
    pub lifecycle: ExecutorServiceLifecycle,
    /// Maximum number of accepted unfinished tasks.
    pub task_capacity: usize,
    /// Accepted tasks waiting for a Tokio blocking worker.
    pub queued: usize,
    /// Tasks currently running on Tokio blocking workers.
    pub running: usize,
}
