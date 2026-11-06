// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use qubit_executor::TaskExecutionError;

use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;

/// Future wrapper that drops the submitted IO future before its task guard.
///
/// Field order is part of the lifecycle protocol: Rust drops `future` before
/// `_guard`, so cancellation cannot release task accounting before user-owned
/// future state has been destroyed.
///
/// # Type Parameters
///
/// * `F` - User future submitted to the IO executor service.
pub(crate) struct TokioGuardedIoFuture<F> {
    /// Pinned user future, dropped before `_guard` on every exit path.
    future: Pin<Box<F>>,
    /// Updates service lifecycle accounting after the user future is dropped.
    _guard: TokioIoServiceTaskGuard,
}

impl<F> TokioGuardedIoFuture<F> {
    /// Pins the user future and associates its lifecycle guard.
    ///
    /// # Parameters
    ///
    /// * `future` - User future whose result is tracked by the service.
    /// * `guard` - Lifecycle guard that releases service accounting on drop.
    ///
    /// # Returns
    ///
    /// A future wrapper that drops the user future before its lifecycle guard.
    pub(crate) fn new(future: F, guard: TokioIoServiceTaskGuard) -> Self {
        Self {
            future: Box::pin(future),
            _guard: guard,
        }
    }
}

impl<F, R, E> Future for TokioGuardedIoFuture<F>
where
    F: Future<Output = Result<R, E>>,
{
    /// Result returned by the submitted future or its execution wrapper.
    type Output = Result<R, TaskExecutionError<E>>;

    /// Polls the user future and maps its error into a task execution error.
    ///
    /// # Parameters
    ///
    /// * `cx` - Async task context used to register the current waker.
    ///
    /// # Returns
    ///
    /// The user future's mapped result when ready, or `Poll::Pending` while it
    /// remains incomplete.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.future
            .as_mut()
            .poll(cx)
            .map(|result| result.map_err(TaskExecutionError::Failed))
    }
}
