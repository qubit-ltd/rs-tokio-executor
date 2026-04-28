/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use qubit_function::Callable;

use crate::TokioExecution;
use qubit_executor::executor::{
    Executor,
    FutureExecutor,
};

/// Executes callable tasks on Tokio's blocking task pool.
///
/// `TokioExecutor` is a [`FutureExecutor`]: its [`Executor::call`] and
/// [`Executor::execute`] methods return a [`TokioExecution`] value that
/// **implements [`Future`]** with
/// [`Output`](std::future::Future::Output) `= Result<R, E>`. You
/// obtain the callable's result by **`.await`ing** (or polling) that future; it
/// is **not** a resolved [`Result`] at return time.
///
/// # Semantics
///
/// * **`call` schedules work immediately** — [`Executor::call`] runs
///   [`tokio::task::spawn_blocking`] **synchronously** before it returns. A
///   Tokio runtime must **already be active** on the current thread when `call`
///   runs (for example inside an `async` block executed under
///   [`Runtime::block_on`](tokio::runtime::Runtime::block_on) or
///   [`#[tokio::main]`](https://docs.rs/tokio/latest/tokio/attr.main.html)).
///   Calling `call` first and only then entering a runtime is wrong: the
///   blocking task was submitted with **no** runtime at `call` time.
/// * **Any normal Tokio entry point works** — you are **not** restricted to
///   [`Builder::new_current_thread`](tokio::runtime::Builder::new_current_thread);
///   a multi-thread [`Runtime`](tokio::runtime::Runtime) or an async handler in
///   a server is fine, as long as `call` happens while that runtime is running.
/// * **Await the returned future on Tokio** — the [`TokioExecution`] polls a
///   [`JoinHandle`](tokio::task::JoinHandle); complete it with `.await` inside
///   the same kind of Tokio-driven async context.
/// * **Blocking pool** — the closure runs on Tokio's *blocking* thread pool, not
///   on the core async worker threads, so heavy synchronous work does not
///   starve other async tasks on the runtime.
/// * **Compared to
///   [`ThreadPerTaskExecutor`](qubit_executor::executor::ThreadPerTaskExecutor)** —
///   this type **reuses** Tokio-managed blocking threads (bounded pool) instead
///   of one new [`std::thread`] per task, and you **await** the result instead
///   of calling a blocking [`TaskHandle::get`](qubit_executor::TaskHandle::get).
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
///         let value = executor.call(|| Ok::<i32, io::Error>(40 + 2)).await?;
///         assert_eq!(value, 42);
///         Ok::<(), io::Error>(())
///     })?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioExecutor;

impl Executor for TokioExecutor {
    type Execution<R, E>
        = TokioExecution<R, E>
    where
        R: Send + 'static,
        E: std::fmt::Display + Send + 'static;

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
    /// A [`TokioExecution`] that implements [`Future`] with
    /// [`Output`](std::future::Future::Output) `= Result<R, E>`. Await it to obtain the
    /// callable's result.
    fn call<C, R, E>(&self, mut task: C) -> Self::Execution<R, E>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: std::fmt::Display + Send + 'static,
    {
        // `spawn_blocking` runs now and requires `Handle::current()` — caller must
        // already be inside a Tokio runtime (see struct-level documentation).
        TokioExecution::new(tokio::task::spawn_blocking(move || task.call()))
    }
}

impl FutureExecutor for TokioExecutor {}
