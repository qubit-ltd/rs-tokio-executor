/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
//! # Qubit Tokio Executor
//!
//! Tokio-backed executor and executor service implementations.
//!

mod tokio_execution;
mod tokio_executor;
mod tokio_executor_service;
mod tokio_executor_service_state;
mod tokio_io_executor_service;
mod tokio_io_executor_service_state;
mod tokio_io_service_task_guard;
mod tokio_service_task_guard;
mod tokio_task_handle;

pub use qubit_executor::executor::{
    Executor,
    FutureExecutor,
};
pub use qubit_executor::service::{
    ExecutorService,
    RejectedExecution,
    ShutdownReport,
};
pub use tokio_execution::TokioExecution;
pub use tokio_executor::TokioExecutor;
pub use tokio_executor_service::TokioExecutorService;
pub use tokio_io_executor_service::TokioIoExecutorService;
pub use tokio_task_handle::TokioTaskHandle;

/// Tokio-backed blocking executor service routed through `spawn_blocking`.
pub type TokioBlockingExecutorService = TokioExecutorService;

/// Executor service compatibility exports for Tokio-backed users.
pub mod service {
    pub use crate::{
        ExecutorService,
        RejectedExecution,
        ShutdownReport,
        TokioBlockingExecutorService,
        TokioExecutorService,
        TokioIoExecutorService,
        TokioTaskHandle,
    };
}
