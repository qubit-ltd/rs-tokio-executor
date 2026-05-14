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
    sync::Arc,
};

use qubit_executor::TaskExecutionError;

use crate::TokioTaskHandle;
use crate::tokio_io_executor_service_state::TokioIoExecutorServiceState;
use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;
use crate::tokio_runtime::ensure_tokio_runtime_entered;
use qubit_executor::service::{
    ExecutorServiceLifecycle,
    StopReport,
    SubmissionError,
};

/// Tokio-backed executor service for async IO and Future-based tasks.
///
/// Accepted futures are spawned with [`tokio::spawn`], so waiting for external
/// IO does not occupy a dedicated blocking thread.
///
/// `TokioIoExecutorService` intentionally has no service-level
/// `await_termination` method. Await the task handles returned by
/// [`Self::spawn`] when the caller needs to observe async task completion.
///
/// ```compile_fail
/// # async fn check() {
/// use qubit_tokio_executor::TokioIoExecutorService;
///
/// let service = TokioIoExecutorService::new();
/// service.await_termination().await;
/// # }
/// ```
#[derive(Default, Clone)]
pub struct TokioIoExecutorService {
    /// Shared service state used by all clones of this service.
    state: Arc<TokioIoExecutorServiceState>,
}

impl TokioIoExecutorService {
    /// Creates a new service instance.
    ///
    /// # Returns
    ///
    /// A Tokio-backed executor service for Future-based tasks.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts an async task and spawns it on the current Tokio runtime.
    ///
    /// # Parameters
    ///
    /// * `future` - Future to execute on Tokio's async scheduler.
    ///
    /// # Returns
    ///
    /// A [`TokioTaskHandle`] for the accepted task.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::Shutdown`] if shutdown has already been
    /// requested before the task is accepted. Returns
    /// [`SubmissionError::WorkerSpawnFailed`] if the current thread is not
    /// entered into a Tokio runtime.
    pub fn spawn<F, R, E>(&self, future: F) -> Result<TokioTaskHandle<R, E>, SubmissionError>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        ensure_tokio_runtime_entered()?;
        self.state.active_tasks.inc();

        let marker = Arc::new(());
        let guard = TokioIoServiceTaskGuard::new(Arc::clone(&self.state), Arc::clone(&marker));
        let handle = tokio::spawn(async move {
            let _guard = guard;
            future.await.map_err(TaskExecutionError::Failed)
        });
        self.state
            .register_abort_handle(marker, handle.abort_handle());
        drop(submission_guard);
        Ok(TokioTaskHandle::new(handle))
    }

    /// Stops accepting new async tasks.
    ///
    /// Already accepted tasks are allowed to finish unless aborted through
    /// their handles or by [`Self::stop`].
    pub fn shutdown(&self) {
        let _guard = self.state.lock_submission();
        self.state.shutdown();
    }

    /// Stops accepting new tasks and aborts tracked async tasks.
    ///
    /// # Returns
    ///
    /// A report with zero queued tasks, the observed active-task count, and
    /// the number of Tokio abort handles signalled.
    pub fn stop(&self) -> StopReport {
        let _guard = self.state.lock_submission();
        self.state.stop();
        let running = self.state.active_tasks.get();
        let cancellation_count = self.state.abort_tracked_tasks();
        StopReport::new(0, running, cancellation_count)
    }

    /// Returns the current lifecycle state.
    ///
    /// # Returns
    ///
    /// [`ExecutorServiceLifecycle::Terminated`] after shutdown or stop and
    /// once no accepted async task remains active.
    #[inline]
    pub fn lifecycle(&self) -> ExecutorServiceLifecycle {
        self.state.lifecycle()
    }

    /// Returns whether this service still accepts async tasks.
    ///
    /// # Returns
    ///
    /// `true` only while the lifecycle is [`ExecutorServiceLifecycle::Running`].
    #[inline]
    pub fn is_running(&self) -> bool {
        self.lifecycle() == ExecutorServiceLifecycle::Running
    }

    /// Returns whether graceful shutdown is in progress.
    ///
    /// # Returns
    ///
    /// `true` only while the lifecycle is
    /// [`ExecutorServiceLifecycle::ShuttingDown`].
    #[inline]
    pub fn is_shutting_down(&self) -> bool {
        self.lifecycle() == ExecutorServiceLifecycle::ShuttingDown
    }

    /// Returns whether abrupt stop is in progress.
    ///
    /// # Returns
    ///
    /// `true` only while the lifecycle is [`ExecutorServiceLifecycle::Stopping`].
    #[inline]
    pub fn is_stopping(&self) -> bool {
        self.lifecycle() == ExecutorServiceLifecycle::Stopping
    }

    /// Returns whether shutdown has been requested.
    ///
    /// # Returns
    ///
    /// `true` if this service no longer accepts new async tasks.
    #[inline]
    pub fn is_not_running(&self) -> bool {
        self.state.is_not_running()
    }

    /// Returns whether shutdown was requested and all async tasks are finished.
    ///
    /// # Returns
    ///
    /// `true` only after shutdown has been requested and no accepted async
    /// tasks remain active.
    #[inline]
    pub fn is_terminated(&self) -> bool {
        self.lifecycle() == ExecutorServiceLifecycle::Terminated
    }
}
