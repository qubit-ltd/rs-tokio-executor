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

use qubit_executor::{
    CancelResult,
    TaskExecutionError,
    TaskResult,
};

/// Async handle returned by Tokio-backed executor services.
///
/// Awaiting this handle reports the accepted task's final result, including
/// task failure, panic, or cancellation.
///
/// # Type Parameters
///
/// * `R` - The task success value.
/// * `E` - The task error value.
///
pub struct TokioTaskHandle<R, E> {
    /// Tokio task whose output is the accepted task's final result.
    handle: JoinHandle<TaskResult<R, E>>,
}

impl<R, E> TokioTaskHandle<R, E> {
    /// Creates a handle from a Tokio join handle.
    ///
    /// # Parameters
    ///
    /// * `handle` - The Tokio join handle that resolves to a task result.
    ///
    /// # Returns
    ///
    /// A task handle that can be awaited.
    #[inline]
    pub(crate) fn new(handle: JoinHandle<TaskResult<R, E>>) -> Self {
        Self { handle }
    }

    /// Sends a best-effort abort request to the underlying Tokio task.
    ///
    /// `CancelResult::Cancelled` means this handle requested Tokio abort for a
    /// task that was not observed as finished at the instant of the check.
    /// Completion may still win the race with cancellation, so the final task
    /// outcome is always the value produced by awaiting this handle.
    ///
    /// # Returns
    ///
    /// [`CancelResult::Cancelled`] when an abort request was sent, or
    /// [`CancelResult::AlreadyFinished`] if the Tokio task had already
    /// completed.
    #[must_use]
    #[inline]
    pub fn cancel(&self) -> CancelResult {
        if self.handle.is_finished() {
            return CancelResult::AlreadyFinished;
        }
        self.handle.abort();
        CancelResult::Cancelled
    }

    /// Returns whether the underlying Tokio task has finished.
    ///
    /// # Returns
    ///
    /// `true` if the Tokio task is complete.
    #[inline]
    pub fn is_done(&self) -> bool {
        self.handle.is_finished()
    }
}

impl<R, E> Future for TokioTaskHandle<R, E> {
    type Output = TaskResult<R, E>;

    /// Polls the underlying Tokio task.
    ///
    /// # Parameters
    ///
    /// * `cx` - Async task context used to register the current waker.
    ///
    /// # Returns
    ///
    /// `Poll::Ready` with the task result when the Tokio task completes, or
    /// `Poll::Pending` while it is still running. Tokio cancellation and panic
    /// join errors are converted to [`TaskExecutionError`] values.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.handle).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(error)) => Poll::Ready(Err(join_error_to_task_error(error))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Converts a Tokio join error into a task execution error.
///
/// # Parameters
///
/// * `error` - Join error returned by Tokio for an aborted or panicked task.
///
/// # Returns
///
/// [`TaskExecutionError::Cancelled`] for aborted tasks, otherwise
/// [`TaskExecutionError::Panicked`].
fn join_error_to_task_error<E>(error: JoinError) -> TaskExecutionError<E> {
    if error.is_cancelled() {
        TaskExecutionError::Cancelled
    } else {
        TaskExecutionError::Panicked
    }
}
