// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! # Qubit Tokio Executor
//!
//! Tokio-backed executors and executor services for Qubit tasks (version
//! 0.12.0).
//!
//! Choose a type by the work you need to schedule and the lifecycle controls
//! you need:
//!
//! | Type | Input | Execution domain | Lifecycle and capacity |
//! | --- | --- | --- | --- |
//! | [`TokioExecutor`] | Callable | Tokio blocking pool | Lightweight executor; no service capacity or shutdown tracking |
//! | [`TokioExecutorService`] (alias [`TokioBlockingExecutorService`]) | Runnable or callable | Tokio blocking pool | Tracks accepted unfinished work, supports capacity limits, shutdown, and termination waiting |
//! | [`TokioIoExecutorService`] | Future | Tokio async scheduler | Tracks accepted unfinished futures, supports capacity limits, shutdown, and termination waiting |
//!
//! Use [`TokioExecutor`] for individual blocking callables when service-level
//! admission control is unnecessary. Use [`TokioExecutorService`] when you
//! need bounded admission and lifecycle coordination for blocking tasks. Use
//! [`TokioIoExecutorService`] for asynchronous futures; waiting for I/O then
//! does not occupy a blocking thread.
//!
//! # Example
//!
//! This example submits a fallible callable through the public
//! [`qubit_executor::service::ExecutorService`] trait, awaits its result, and
//! waits for service termination:
//!
//! ```
//! use std::io;
//!
//! use qubit_executor::service::ExecutorService;
//! use qubit_tokio_executor::TokioExecutorService;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = tokio::runtime::Runtime::new()?;
//!     runtime.block_on(async {
//!         let service = TokioExecutorService::new(tokio::runtime::Handle::current());
//!         let task = service.submit_callable(|| Ok::<usize, io::Error>(40 + 2))?;
//!         assert_eq!(task.await?, 42);
//!         service.shutdown();
//!         service.await_termination().await;
//!         Ok::<(), Box<dyn std::error::Error>>(())
//!     })
//! }
//! ```
//!
//! # Capacity, rejection, and runtime limits
//!
//! Both services default to at most 1024 accepted unfinished tasks; their
//! `with_task_capacity` constructors configure another nonzero limit. A
//! `Saturated` [`qubit_executor::service::SubmissionError`] means the service
//! is still open but at capacity. `Shutdown` means admission has closed. The
//! `try_submit` and `try_spawn` methods return a [`RejectedTask`] with the
//! original task when admission fails; the convenience submission methods
//! consume the rejected task.
//!
//! Dropping a task handle does not cancel its task. A queued blocking task can
//! be aborted by the blocking service, but a blocking closure cannot be
//! interrupted after it starts. Service capacity and shutdown describe the
//! service's admission and accounting state, not whether Tokio's runtime is
//! alive. A runtime [`Handle`](tokio::runtime::Handle) neither owns nor drives
//! its runtime. Keep the runtime alive while work is outstanding. Async work
//! on a current-thread runtime needs its owner to drive it (for example with
//! [`Runtime::block_on`](tokio::runtime::Runtime::block_on)); synchronous
//! callables on Tokio's blocking pool do not need `block_on` merely to execute.
//! If a blocking callable waits on async work bound to that runtime, the async
//! work still needs runtime progress.
//!
//! # Further reading
//!
//! The [0.12.0 user guide](https://github.com/qubit-ltd/rs-tokio-executor/blob/v0.12.0/doc/user_guide.md)
//! covers application usage. The [0.12.0 concurrency design](https://github.com/qubit-ltd/rs-tokio-executor/blob/v0.12.0/doc/concurrency-design.md)
//! documents implementation protocols for maintainers. These links are pinned
//! to the crate release and describe version 0.12.0.

mod executor_service_lifecycle_bits;
mod internal;
mod rejected_task;
mod tokio_blocking_task_handle;
mod tokio_executor;
mod tokio_executor_service;
mod tokio_executor_service_state;
mod tokio_executor_service_stats;
mod tokio_io_executor_service;
mod tokio_io_executor_service_state;
mod tokio_io_executor_service_stats;
mod tokio_io_service_task_guard;
mod tokio_service_task_guard;
mod tokio_task_handle;
mod tokio_task_registration;
mod tokio_task_slot_cancellation;

#[cfg(test)]
mod tests;

pub use rejected_task::RejectedTask;
pub use tokio_blocking_task_handle::TokioBlockingTaskHandle;
pub use tokio_executor::TokioExecutor;
pub use tokio_executor_service::TokioBlockingExecutorService;
pub use tokio_executor_service::TokioExecutorService;
pub use tokio_executor_service_stats::TokioExecutorServiceStats;
pub use tokio_io_executor_service::TokioIoExecutorService;
pub use tokio_io_executor_service_stats::TokioIoExecutorServiceStats;
pub use tokio_task_handle::TokioTaskHandle;
