// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::IntoFuture;

use qubit_executor::CancelResult;
use qubit_executor::TaskResult;
use qubit_executor::TaskStatus;
use qubit_executor::TrackedTask;
use qubit_executor::TryGet;
use qubit_executor::task::TaskHandleFuture;
use qubit_executor::task::spi::TaskResultHandle;
use qubit_executor::task::spi::TrackedTaskHandle;
use tokio::task::AbortHandle;

/// Callback used to finish service-side queued-task accounting.
type CancelQueuedTask = Box<dyn Fn() + Send + Sync + 'static>;

/// Tracked handle for tasks submitted to Tokio's blocking task pool.
///
/// This handle wraps the standard [`TrackedTask`] result/status endpoint and
/// additionally keeps Tokio's [`AbortHandle`] to request pre-start
/// cancellation of queued `spawn_blocking` work.
///
/// Tokio processes the request asynchronously. A cancelled result or service
/// termination does not guarantee that Tokio has already dropped the queued
/// closure or its captures.
///
/// Tokio cannot abort blocking work after the closure has started. In that
/// case [`Self::cancel`] reports [`CancelResult::AlreadyRunning`] through the
/// underlying tracked task state.
pub struct TokioBlockingTaskHandle<R, E> {
    /// Standard tracked task endpoint used for result and status observation.
    handle: TrackedTask<R, E>,
    /// Tokio abort handle used to request cancellation of queued blocking
    /// work.
    abort_handle: AbortHandle,
    /// Callback that completes queued-task accounting after cancellation wins.
    cancel_queued_task: CancelQueuedTask,
}

impl<R, E> TokioBlockingTaskHandle<R, E> {
    /// Creates a blocking task handle.
    ///
    /// # Parameters
    ///
    /// * `handle` - Standard tracked task endpoint.
    /// * `abort_handle` - Tokio abort handle for the submitted blocking task.
    /// * `cancel_queued_task` - Callback that finishes service-side queued task
    ///   accounting when cancellation wins before the task starts.
    ///
    /// # Returns
    ///
    /// A tracked Tokio blocking task handle.
    #[inline]
    pub(crate) fn new<F>(handle: TrackedTask<R, E>, abort_handle: AbortHandle, cancel_queued_task: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            handle,
            abort_handle,
            cancel_queued_task: Box::new(cancel_queued_task),
        }
    }

    /// Waits for the task to finish and returns its final result.
    ///
    /// This method blocks the current thread until a result is available.
    ///
    /// # Returns
    ///
    /// The final task result.
    #[inline]
    pub fn get(self) -> TaskResult<R, E>
    where
        R: Send,
        E: Send,
    {
        <Self as TaskResultHandle<R, E>>::get(self)
    }

    /// Attempts to retrieve the final result without blocking.
    ///
    /// # Returns
    ///
    /// A ready result or the pending handle.
    #[inline]
    pub fn try_get(self) -> TryGet<Self, R, E>
    where
        R: Send,
        E: Send,
    {
        <Self as TaskResultHandle<R, E>>::try_get(self)
    }

    /// Returns whether the task has installed a terminal state.
    ///
    /// # Returns
    ///
    /// `true` after the task succeeds, fails, panics, is cancelled, or loses
    /// its runner endpoint.
    #[inline]
    pub fn is_done(&self) -> bool
    where
        R: Send,
        E: Send,
    {
        <Self as TaskResultHandle<R, E>>::is_done(self)
    }

    /// Returns the currently observed task status.
    ///
    /// # Returns
    ///
    /// The current task status.
    #[inline]
    pub fn status(&self) -> TaskStatus {
        self.handle.status()
    }

    /// Attempts to cancel this task before its blocking closure starts.
    ///
    /// When cancellation wins the pending-state race, this method publishes the
    /// cancellation result, completes service-side queued accounting, and asks
    /// Tokio to abort the queued `spawn_blocking` task. Tokio performs that
    /// cleanup asynchronously, so this method and awaiting this handle do not
    /// wait for the queued closure or its captures to be dropped.
    ///
    /// # Returns
    ///
    /// The observed cancellation outcome.
    #[must_use]
    #[inline]
    pub fn cancel(&self) -> CancelResult {
        let result = self.handle.cancel();
        if result == CancelResult::Cancelled {
            (self.cancel_queued_task)();
            self.abort_handle.abort();
        }
        result
    }
}

impl<R, E> TaskResultHandle<R, E> for TokioBlockingTaskHandle<R, E>
where
    R: Send,
    E: Send,
{
    /// Returns whether the tracked state is terminal.
    #[inline]
    fn is_done(&self) -> bool {
        self.handle.is_done()
    }

    /// Blocks until the underlying result handle yields a result.
    #[inline]
    fn get(self) -> TaskResult<R, E> {
        self.handle.get()
    }

    /// Attempts to retrieve the underlying result without blocking.
    #[inline]
    fn try_get(self) -> TryGet<Self, R, E> {
        let Self {
            handle,
            abort_handle,
            cancel_queued_task,
        } = self;
        match handle.try_get() {
            TryGet::Ready(result) => TryGet::Ready(result),
            TryGet::Pending(handle) => TryGet::Pending(Self {
                handle,
                abort_handle,
                cancel_queued_task,
            }),
        }
    }
}

impl<R, E> TrackedTaskHandle<R, E> for TokioBlockingTaskHandle<R, E>
where
    R: Send,
    E: Send,
{
    /// Returns the currently observed task status.
    #[inline]
    fn status(&self) -> TaskStatus {
        self.handle.status()
    }

    /// Attempts to cancel the task before it starts.
    #[inline]
    fn cancel(&self) -> CancelResult {
        Self::cancel(self)
    }
}

impl<R, E> IntoFuture for TokioBlockingTaskHandle<R, E> {
    type Output = TaskResult<R, E>;
    type IntoFuture = TaskHandleFuture<R, E>;

    /// Converts this tracked handle into a future resolving to the task result.
    #[inline]
    fn into_future(self) -> Self::IntoFuture {
        self.handle.into_future()
    }
}
