// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use qubit_executor::TrackedTask;
use qubit_executor::executor::Executor;
use qubit_executor::service::SubmissionError;
use qubit_executor::task::spi::TaskEndpointPair;
use qubit_executor::task::spi::TaskRunner;
use qubit_function::Callable;

/// Executes callable tasks on Tokio's blocking task pool.
///
/// `TokioExecutor` implements [`Executor`] by submitting work to Tokio's
/// blocking task pool and returning the standard tracked task handle.
///
/// # Semantics
///
/// * **`call` schedules work immediately** — [`Executor::call`] runs
///   [`tokio::task::spawn_blocking`] through the runtime handle captured by
///   [`Self::new`] before it returns.
/// * **The caller need not enter the bound runtime** — `call` may be invoked
///   from another runtime or a plain thread while the bound runtime remains
///   alive; the task is still submitted to the bound runtime.
/// * **Await the returned tracked task on Tokio** — the returned
///   [`TrackedTask`] implements [`IntoFuture`](std::future::IntoFuture), so it
///   can be awaited inside a Tokio-driven async context after submission
///   succeeds.
/// * **Blocking pool** — the closure runs on Tokio's *blocking* thread pool,
///   not on the core async worker threads, so heavy synchronous work does not
///   starve other async tasks on the runtime.
/// * **Standard tracked-task cancellation** — the returned [`TrackedTask`] can
///   cancel the user callable before it starts, but it does not own Tokio's
///   [`AbortHandle`](tokio::task::AbortHandle). If the Tokio blocking queue has
///   already accepted the wrapper closure, that wrapper may still wait for a
///   blocking thread and then observe the cancelled tracked state without
///   running the user callable. Use
///   [`TokioExecutorService`](crate::TokioExecutorService) and
///   [`TokioBlockingTaskHandle`](crate::TokioBlockingTaskHandle) when queued
///   Tokio blocking work must be aborted directly.
/// * **Compared to thread-per-task execution** — unlike
///   [`ThreadPerTaskExecutor`](qubit_executor::executor::ThreadPerTaskExecutor),
///   this type **reuses** Tokio-managed blocking threads (bounded pool) instead
///   of one new [`std::thread`] per task, and can return a handle that is
///   either awaited or read with blocking `get`.
///
/// # Examples
///
/// The following uses a single-thread [`Runtime`](tokio::runtime::Runtime) only
/// to keep the snippet self-contained; [`#[tokio::main]`](https://docs.rs/tokio/latest/tokio/attr.main.html)
/// or a multi-thread runtime are equally valid.
///
/// ```rust
/// use std::io;
///
/// use qubit_executor::executor::Executor;
/// use qubit_tokio_executor::TokioExecutor;
///
/// # fn main() -> io::Result<()> {
/// tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()?
///     .block_on(async {
///         let executor = TokioExecutor::new(tokio::runtime::Handle::current());
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
#[derive(Debug, Clone)]
pub struct TokioExecutor {
    handle: tokio::runtime::Handle,
}

impl TokioExecutor {
    /// Creates an executor bound to the supplied Tokio runtime.
    ///
    /// The executor may be called from another runtime or from a thread that
    /// is not currently entered into Tokio; tasks are always submitted to this
    /// handle's runtime.
    ///
    /// # Parameters
    ///
    /// * `handle` - Runtime handle that receives blocking task submissions.
    ///
    /// # Returns
    ///
    /// An executor permanently bound to `handle`'s runtime.
    #[inline]
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self { handle }
    }
}

impl Executor for TokioExecutor {
    /// Spawns the callable on Tokio's blocking task pool.
    ///
    /// This method invokes [`tokio::task::spawn_blocking`] on the runtime
    /// handle captured by [`Self::new`].
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
    /// The bound runtime handle determines where the task is submitted.
    fn call<C, R, E>(&self, task: C) -> Result<TrackedTask<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let (handle, slot) = TaskEndpointPair::new().into_tracked_parts();
        self.handle.spawn_blocking(move || {
            TaskRunner::new(task).run(slot);
        });
        Ok(handle)
    }
}
