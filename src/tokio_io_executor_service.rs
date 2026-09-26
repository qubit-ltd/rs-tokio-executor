// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;

use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_executor::service::StopReport;
use qubit_executor::service::SubmissionError;

use crate::TokioIoExecutorServiceStats;
use crate::TokioTaskHandle;
use crate::tokio_io_executor_service_state::TokioIoExecutorServiceState;
use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;
use crate::tokio_task_registration::TaskRegistration;

/// Tokio-backed executor service for async IO and Future-based tasks.
///
/// Accepted futures are spawned with [`tokio::spawn`], so waiting for external
/// IO does not occupy a dedicated blocking thread.
///
/// # Examples
///
/// ```
/// use qubit_tokio_executor::TokioIoExecutorService;
///
/// let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
/// let service = TokioIoExecutorService::new(runtime.handle().clone());
/// service.shutdown();
/// assert!(service.is_terminated());
/// ```
#[derive(Clone)]
pub struct TokioIoExecutorService {
    /// Shared service state used by all clones of this service.
    state: Arc<TokioIoExecutorServiceState>,
    /// Runtime handle used for all async submissions.
    runtime: tokio::runtime::Handle,
}

/// Maximum number of accepted unfinished async tasks by default.
const DEFAULT_TASK_CAPACITY: usize = 1024;

impl TokioIoExecutorService {
    /// Returns a best-effort snapshot of accepted unfinished futures.
    ///
    /// # Returns
    ///
    /// The configured capacity, accepted unfinished count, and lifecycle.
    /// The count includes futures not yet polled by Tokio.
    #[must_use]
    #[inline]
    pub fn stats(&self) -> TokioIoExecutorServiceStats {
        self.state.stats()
    }

    /// Subscribes to changes that may make another future admissible.
    ///
    /// A notification is only a hint; callers must retry submission because
    /// another producer may consume the available capacity first.
    ///
    /// # Returns
    ///
    /// A receiver that observes capacity and lifecycle changes.
    #[must_use]
    pub fn capacity_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.state.capacity_changes()
    }

    /// Creates a new service instance.
    ///
    /// # Returns
    ///
    /// A Tokio-backed executor service for Future-based tasks.
    #[inline]
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self::with_task_capacity(
            runtime,
            NonZeroUsize::new(DEFAULT_TASK_CAPACITY).expect("default task capacity should be nonzero"),
        )
    }

    /// Creates a service with a maximum number of accepted unfinished futures.
    ///
    /// A slot remains occupied until the future completes or Tokio observes an
    /// abort. Cancelling a future before its first poll also releases the slot
    /// when Tokio drops the task.
    ///
    /// # Parameters
    ///
    /// * `runtime` - Tokio runtime used to execute accepted futures.
    /// * `task_capacity` - Nonzero limit for accepted unfinished futures.
    ///
    /// # Returns
    ///
    /// A Tokio-backed service configured with the supplied capacity.
    pub fn with_task_capacity(runtime: tokio::runtime::Handle, task_capacity: NonZeroUsize) -> Self {
        let state = Arc::new(TokioIoExecutorServiceState::with_task_capacity(task_capacity));
        Self { state, runtime }
    }

    /// Accepts an async task and spawns it on the bound Tokio runtime.
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
    /// requested, or [`SubmissionError::Saturated`] if the unfinished-task
    /// capacity is full.
    pub fn spawn<F, R, E>(&self, future: F) -> Result<TokioTaskHandle<R, E>, SubmissionError>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let admission = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        if !self.state.try_accept_task() {
            return Err(SubmissionError::Saturated);
        }
        let marker = TaskRegistration::new();
        let guard = TokioIoServiceTaskGuard::new(Arc::clone(&self.state), Arc::clone(&marker));

        let handle = self.runtime.spawn(async move {
            let _guard = guard;
            future.await.map_err(TaskExecutionError::Failed)
        });
        self.state.register_abort_handle(marker, handle.abort_handle());
        drop(admission);
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
    #[must_use]
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
    /// `true` only while the lifecycle is
    /// [`ExecutorServiceLifecycle::Running`].
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
    /// `true` only while the lifecycle is
    /// [`ExecutorServiceLifecycle::Stopping`].
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

    /// Awaits service termination without polling.
    ///
    /// The future resolves after shutdown or stop has been requested and every
    /// accepted task has completed or observed Tokio cancellation.
    pub async fn await_termination(&self) {
        self.state.await_termination().await;
    }
}
