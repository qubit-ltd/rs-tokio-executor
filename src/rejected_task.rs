// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fmt;

use qubit_executor::service::SubmissionError;

/// A task returned when an executor service rejects its submission.
///
/// The task is returned unchanged and has not been run or polled. Inspect
/// [`reason`](Self::reason), then use [`into_parts`](Self::into_parts) to
/// recover ownership and decide whether to retry, store, or discard it.
///
/// `Debug` displays only the rejection reason. It intentionally omits the task
/// because captured values may contain private data, and it does not require
/// the task type to implement `Debug`.
///
/// # Type Parameters
///
/// * `T` - The original runnable, callable, or future rejected by the service.
///
/// # Example
///
/// ```
/// use std::io;
///
/// use qubit_executor::service::SubmissionError;
/// use qubit_tokio_executor::TokioIoExecutorService;
///
/// let runtime = tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()
///     .expect("runtime should build");
/// let service = TokioIoExecutorService::new(runtime.handle().clone());
/// service.shutdown();
/// let future = async { Ok::<usize, io::Error>(42) };
/// let rejected = match service.try_spawn(future) {
///     Err(rejected) => rejected,
///     Ok(_) => panic!("shutdown should reject the future"),
/// };
/// let (reason, future) = rejected.into_parts();
/// assert!(matches!(reason, SubmissionError::Shutdown));
/// assert_eq!(runtime.block_on(future).expect("recovered future should run"), 42);
/// ```
pub struct RejectedTask<T> {
    /// The reason the service rejected the task.
    reason: SubmissionError,
    /// The original task returned to the caller.
    task: T,
}

impl<T> RejectedTask<T> {
    /// Returns the reason the service rejected this task.
    #[must_use = "inspect the rejection reason"]
    pub fn reason(&self) -> &SubmissionError {
        &self.reason
    }

    /// Returns the rejection reason and the original task.
    #[must_use = "handle the rejection reason and recover the original task"]
    pub fn into_parts(self) -> (SubmissionError, T) {
        (self.reason, self.task)
    }

    /// Creates a rejected task from its reason and original ownership value.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The original task value.
    ///
    /// # Parameters
    ///
    /// * `reason` - Why the service rejected the task.
    /// * `task` - The original task value.
    pub(crate) fn new(reason: SubmissionError, task: T) -> Self {
        Self { reason, task }
    }
}

impl<T> fmt::Debug for RejectedTask<T> {
    /// Formats the rejection reason without formatting the task.
    ///
    /// # Parameters
    ///
    /// * `formatter` - Formatter receiving the debug representation.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedTask")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl<T> fmt::Display for RejectedTask<T> {
    /// Formats the rejection reason.
    ///
    /// # Parameters
    ///
    /// * `formatter` - Formatter receiving the display representation.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl<T> std::error::Error for RejectedTask<T> {}
