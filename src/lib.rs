// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! # Qubit Tokio Executor
//!
//! Tokio-backed executor and executor service implementations.

mod executor_service_lifecycle_bits;
mod tokio_blocking_task_handle;
mod tokio_executor;
mod tokio_executor_service;
mod tokio_executor_service_state;
mod tokio_io_executor_service;
mod tokio_io_executor_service_state;
mod tokio_io_service_task_guard;
mod tokio_runtime;
mod tokio_service_task_guard;
mod tokio_task_handle;
mod tokio_task_slot_cancellation;

#[doc(hidden)]
pub mod testing;

pub use tokio_blocking_task_handle::TokioBlockingTaskHandle;
pub use tokio_executor::TokioExecutor;
pub use tokio_executor_service::{TokioBlockingExecutorService, TokioExecutorService};
pub use tokio_io_executor_service::TokioIoExecutorService;
pub use tokio_task_handle::TokioTaskHandle;
