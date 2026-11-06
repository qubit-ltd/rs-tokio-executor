// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use crate::TokioTaskHandle;
use crate::internal::tokio_guarded_io_future::TokioGuardedIoFuture;
use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;

/// Registered IO submission whose user future has not yet been delivered.
///
/// # Type Parameters
///
/// * `F` - User future awaiting delivery to its registered Tokio task.
/// * `R` - Result returned by the user future.
/// * `E` - Error returned by the user future.
pub(crate) struct TokioIoSpawnHandoff<F, R, E> {
    /// User future retained by the submitting caller until delivery.
    ///
    /// This field precedes `guard` so dropping a pending handoff destroys user
    /// state before releasing its accepted-task accounting.
    future: F,
    /// Tokio task handle registered under admission.
    handle: tokio::task::JoinHandle<qubit_executor::TaskResult<R, E>>,
    /// One-shot channel registered under admission.
    sender: tokio::sync::oneshot::Sender<TokioGuardedIoFuture<F>>,
    /// Capacity guard retained until the guarded future is dropped.
    guard: TokioIoServiceTaskGuard,
}

impl<F, R, E> TokioIoSpawnHandoff<F, R, E> {
    /// Creates a handoff that retains the user future until it can be
    /// delivered.
    ///
    /// # Type Parameters
    ///
    /// * `F` - User future awaiting delivery.
    /// * `R` - Result returned by the user future.
    /// * `E` - Error returned by the user future.
    ///
    /// # Parameters
    ///
    /// * `future` - User future retained until delivery.
    /// * `handle` - Tokio task registered under admission.
    /// * `sender` - Channel endpoint used to deliver the guarded future.
    /// * `guard` - Accepted-task accounting guard.
    pub(crate) fn new(
        future: F,
        handle: tokio::task::JoinHandle<qubit_executor::TaskResult<R, E>>,
        sender: tokio::sync::oneshot::Sender<TokioGuardedIoFuture<F>>,
        guard: TokioIoServiceTaskGuard,
    ) -> Self {
        Self {
            future,
            handle,
            sender,
            guard,
        }
    }

    /// Delivers the user future after the admission lock has been released.
    ///
    /// # Returns
    ///
    /// A handle for the registered Tokio task.
    pub(crate) fn deliver(self) -> TokioTaskHandle<R, E> {
        let Self {
            handle,
            sender,
            guard,
            future,
        } = self;
        let payload = TokioGuardedIoFuture::new(future, guard);
        if let Err(payload) = sender.send(payload) {
            drop(payload);
        }
        TokioTaskHandle::new(handle)
    }
}
