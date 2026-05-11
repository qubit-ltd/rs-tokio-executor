/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::{
    future::Future,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use tokio::task::{
    JoinError,
    JoinHandle,
};

/// Future-backed execution returned by [`TokioExecutor`](super::TokioExecutor).
///
/// This struct **implements [`Future`]**:
/// [`Output`](std::future::Future::Output) is [`Result<R, E>`](Result) — the
/// success value `R` or the callable's error `E`. Await this type (on a
/// Tokio-driven async context) to receive that result; until then the underlying
/// blocking task may still be running.
///
/// Tokio join errors are not part of that `Result`: task panics are resumed and
/// cancellations panic when the future is awaited.
///
/// # Type Parameters
///
/// * `R` - The task success value.
/// * `E` - The task error value.
///
pub struct TokioExecution<R, E> {
    /// Tokio join handle for the blocking task.
    handle: JoinHandle<Result<R, E>>,
}

impl<R, E> TokioExecution<R, E> {
    /// Creates a Tokio execution wrapper.
    ///
    /// # Parameters
    ///
    /// * `handle` - The Tokio join handle that produces the task result.
    ///
    /// # Returns
    ///
    /// A future-backed execution wrapper.
    #[inline]
    pub fn new(handle: JoinHandle<Result<R, E>>) -> Self {
        Self { handle }
    }

    /// Returns whether the Tokio task has finished.
    ///
    /// # Returns
    ///
    /// `true` if the underlying Tokio task has completed.
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Requests cancellation of the underlying Tokio task.
    ///
    /// Tokio can cancel a blocking task only before it starts. If the blocking
    /// closure is already running, this request is best-effort and awaiting the
    /// execution will still wait for the closure to finish.
    ///
    /// # Returns
    ///
    /// `true` after the cancellation request has been sent to Tokio.
    #[inline]
    pub fn cancel(&self) -> bool {
        self.handle.abort();
        true
    }
}

impl<R, E> Future for TokioExecution<R, E> {
    type Output = Result<R, E>;

    /// Polls the underlying Tokio task.
    ///
    /// # Parameters
    ///
    /// * `cx` - Async task context used to register the current waker.
    ///
    /// # Returns
    ///
    /// `Poll::Ready` with the callable result when the Tokio task completes,
    /// or `Poll::Pending` while the task is still running.
    ///
    /// # Panics
    ///
    /// Panics if Tokio reports the blocking task was cancelled. If the task
    /// panicked, this method resumes the original panic payload.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.handle).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(error)) => handle_join_error(error),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Converts a Tokio join error into this execution's panic behavior.
///
/// # Parameters
///
/// * `error` - Tokio join error returned while awaiting the blocking task.
///
/// # Returns
///
/// This function never returns normally for a join error; its return type
/// matches the call site.
///
/// # Panics
///
/// Resumes the task panic when Tokio reports a panic, or panics with a
/// cancellation message when the task was cancelled.
fn handle_join_error<R, E>(error: JoinError) -> Poll<Result<R, E>> {
    let payload = error
        .try_into_panic()
        .unwrap_or_else(|_| Box::new("tokio execution was cancelled before completion"));
    std::panic::resume_unwind(payload);
}
