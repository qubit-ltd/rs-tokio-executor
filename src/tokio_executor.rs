/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use qubit_function::Callable;

use qubit_executor::{
    TrackedTask,
    executor::Executor,
    service::SubmissionError,
    task::spi::{
        TaskEndpointPair,
        TaskRunner,
    },
};

use crate::tokio_runtime::ensure_tokio_runtime_entered;

/// Executes callable tasks on Tokio's blocking task pool.
///
/// `TokioExecutor` implements [`Executor`] by submitting work to Tokio's
/// blocking task pool and returning the standard tracked task handle.
///
/// # Semantics
///
/// * **`call` schedules work immediately** — [`Executor::call`] runs
///   [`tokio::task::spawn_blocking`] **synchronously** before it returns. A
///   Tokio runtime must **already be active** on the current thread when `call`
///   runs (for example inside an `async` block executed under
///   [`Runtime::block_on`](tokio::runtime::Runtime::block_on) or
///   [`#[tokio::main]`](https://docs.rs/tokio/latest/tokio/attr.main.html)).
///   Calling `call` first and only then entering a runtime is rejected with
///   [`SubmissionError::WorkerSpawnFailed`].
/// * **Any normal Tokio entry point works** — you are **not** restricted to
///   [`Builder::new_current_thread`](tokio::runtime::Builder::new_current_thread);
///   a multi-thread [`Runtime`](tokio::runtime::Runtime) or an async handler in
///   a server is fine, as long as `call` happens while that runtime is running.
/// * **Await the returned tracked task on Tokio** — the returned
///   [`TrackedTask`] implements [`IntoFuture`](std::future::IntoFuture), so it
///   can be awaited inside a Tokio-driven async context after submission
///   succeeds.
/// * **Blocking pool** — the closure runs on Tokio's *blocking* thread pool, not
///   on the core async worker threads, so heavy synchronous work does not
///   starve other async tasks on the runtime.
/// * **Standard tracked-task cancellation** — the returned [`TrackedTask`]
///   can cancel the user callable before it starts, but it does not own Tokio's
///   [`AbortHandle`](tokio::task::AbortHandle). If the Tokio blocking queue has
///   already accepted the wrapper closure, that wrapper may still wait for a
///   blocking thread and then observe the cancelled tracked state without
///   running the user callable. Use [`TokioExecutorService`](crate::TokioExecutorService)
///   and [`TokioBlockingTaskHandle`](crate::TokioBlockingTaskHandle) when
///   queued Tokio blocking work must be aborted directly.
/// * **Compared to
///   [`ThreadPerTaskExecutor`](qubit_executor::executor::ThreadPerTaskExecutor)** —
///   this type **reuses** Tokio-managed blocking threads (bounded pool) instead
///   of one new [`std::thread`] per task, and can return a handle that is either
///   awaited or read with blocking `get`.
///
/// # Examples
///
/// The following uses a single-thread [`Runtime`](tokio::runtime::Runtime) only to keep the snippet
/// self-contained; [`#[tokio::main]`](https://docs.rs/tokio/latest/tokio/attr.main.html)
/// or a multi-thread runtime are equally valid.
///
/// ```rust
/// use std::io;
///
/// use qubit_tokio_executor::{
///     Executor,
///     TokioExecutor,
/// };
///
/// # fn main() -> io::Result<()> {
/// tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()?
///     .block_on(async {
///         let executor = TokioExecutor;
///         let value = executor
///             .call(|| Ok::<i32, io::Error>(40 + 2))
///             .expect("executor should accept callable")
///             .await
///             .expect("callable should complete successfully");
///         assert_eq!(value, 42);
///         Ok::<(), io::Error>(())
///     })?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioExecutor;

impl Executor for TokioExecutor {
    /// Spawns the callable on Tokio's blocking task pool.
    ///
    /// This method invokes [`tokio::task::spawn_blocking`] **before** returning.
    /// A Tokio runtime must be active when this method runs; see [`TokioExecutor`].
    ///
    /// # Parameters
    ///
    /// * `task` - Callable to run on Tokio's blocking task pool.
    ///
    /// # Returns
    ///
    /// A tracked task handle for the accepted callable.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::WorkerSpawnFailed`] when the current thread
    /// is not entered into a Tokio runtime.
    fn call<C, R, E>(&self, task: C) -> Result<TrackedTask<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        ensure_tokio_runtime_entered().map(|()| {
            let (handle, slot) = TaskEndpointPair::new().into_tracked_parts();
            tokio::task::spawn_blocking(move || {
                TaskRunner::new(task).run(slot);
            });
            handle
        })
    }
}
