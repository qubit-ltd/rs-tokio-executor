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

use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_executor::service::StopReport;
use qubit_executor::service::SubmissionError;

use crate::RejectedTask;
use crate::TokioIoExecutorServiceStats;
use crate::TokioTaskHandle;
use crate::internal::tokio_guarded_io_future::TokioGuardedIoFuture;
use crate::internal::tokio_io_spawn_handoff::TokioIoSpawnHandoff;
use crate::tokio_io_executor_service_state::TokioIoExecutorServiceState;
use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;
use crate::tokio_task_registration::TokioTaskRegistration;

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
    /// Creates a new service instance with the default capacity of 1024
    /// unfinished tasks.
    ///
    /// Use [`Self::with_task_capacity`] to choose another nonzero limit.
    ///
    /// # Returns
    ///
    /// A Tokio-backed executor service for Future-based tasks.
    ///
    /// # Parameters
    ///
    /// * `runtime` - Tokio runtime used to execute accepted futures.
    ///
    /// The supplied handle does not drive the runtime. In particular, a
    /// current-thread runtime advances only while its owner calls
    /// `Runtime::block_on`. See the [runtime guide](https://github.com/qubit-ltd/rs-tokio-executor/blob/main/doc/user_guide.md#runtime-lifetime-and-driver-progress).
    #[must_use]
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
    /// Keep the bound runtime alive and driven until accepted futures finish.
    /// A `Handle` alone does not drive a current-thread runtime; its owner must
    /// call `Runtime::block_on`. See the [runtime guide](https://github.com/qubit-ltd/rs-tokio-executor/blob/main/doc/user_guide.md#runtime-lifetime-and-driver-progress).
    ///
    /// # Returns
    ///
    /// A Tokio-backed service configured with the supplied capacity.
    #[must_use]
    pub fn with_task_capacity(runtime: tokio::runtime::Handle, task_capacity: NonZeroUsize) -> Self {
        let state = Arc::new(TokioIoExecutorServiceState::with_task_capacity(task_capacity));
        Self { state, runtime }
    }

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
    /// Subscribe before the first submission attempt. After a saturated
    /// attempt, wait for [`tokio::sync::watch::Receiver::changed`] and then
    /// retry; the notification reserves no slot and gives producers no fairness
    /// guarantee. Its generation value is not a capacity count.
    ///
    /// # Returns
    ///
    /// A receiver that observes capacity and lifecycle changes.
    #[must_use]
    pub fn capacity_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.state.capacity_changes()
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
    ///
    /// # Type Parameters
    ///
    /// * `F` - Future submitted for execution.
    /// * `R` - Value returned by the future.
    /// * `E` - Error returned by the future.
    pub fn spawn<F, R, E>(&self, future: F) -> Result<TokioTaskHandle<R, E>, SubmissionError>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        self.try_spawn(future).map_err(|rejected| rejected.into_parts().0)
    }

    /// Attempts to accept an async task while retaining its future if rejected.
    ///
    /// A rejected future is returned unchanged; it has not been polled or
    /// dropped. Shutdown is checked before capacity saturation.
    ///
    /// # Type Parameters
    ///
    /// * `F` - The future to submit.
    /// * `R` - The future's result type.
    /// * `E` - The future's error type.
    ///
    /// # Parameters
    ///
    /// * `future` - The future to submit.
    pub fn try_spawn<F, R, E>(&self, future: F) -> Result<TokioTaskHandle<R, E>, RejectedTask<F>>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        self.prepare_spawn(future)
            .map(TokioIoSpawnHandoff::deliver)
            .map_err(|(reason, future)| RejectedTask::new(reason, future))
    }

    /// Registers a receiver-only Tokio task while keeping the user future
    /// outside the admission critical section.
    ///
    /// # Type Parameters
    ///
    /// * `F` - The future to retain for delivery after admission.
    /// * `R` - The future's result type.
    /// * `E` - The future's error type.
    ///
    /// # Parameters
    ///
    /// * `future` - The user future retained by the returned handoff.
    ///
    /// # Returns
    ///
    /// A registered handoff that can deliver the future after this method
    /// releases admission.
    ///
    /// # Errors
    ///
    /// Returns the submission error and original future if admission is
    /// rejected.
    pub(crate) fn prepare_spawn<F, R, E>(&self, future: F) -> Result<TokioIoSpawnHandoff<F, R, E>, (SubmissionError, F)>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let _admission = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err((SubmissionError::Shutdown, future));
        }
        if !self.state.try_accept_task() {
            return Err((SubmissionError::Saturated, future));
        }
        let marker = TokioTaskRegistration::new();
        let guard = TokioIoServiceTaskGuard::new(Arc::clone(&self.state), Arc::clone(&marker));
        let (sender, receiver) = tokio::sync::oneshot::channel::<TokioGuardedIoFuture<F>>();
        let handle = self.runtime.spawn(async move {
            match receiver.await {
                Ok(future) => future.await,
                Err(_) => Err(qubit_executor::TaskExecutionError::Cancelled),
            }
        });
        self.state.register_abort_handle(marker, handle.abort_handle());
        Ok(TokioIoSpawnHandoff::new(future, handle, sender, guard))
    }

    /// Stops accepting new async tasks.
    ///
    /// Already accepted tasks are allowed to finish unless aborted through
    /// their handles or by [`Self::stop`].
    pub fn shutdown(&self) {
        {
            let _guard = self.state.lock_submission();
            self.state.shutdown();
        }
        self.state.notify_capacity_changed();
        self.state.notify_termination_waiters();
    }

    /// Stops accepting new tasks and aborts tracked async tasks.
    ///
    /// # Returns
    ///
    /// A report with zero queued tasks, the observed active-task count, and
    /// the number of Tokio abort handles signalled.
    #[must_use]
    pub fn stop(&self) -> StopReport {
        let running = {
            let _guard = self.state.lock_submission();
            self.state.stop();
            self.state.active_tasks.get()
        };
        let cancellation_count = self.state.abort_tracked_tasks();
        self.state.notify_capacity_changed();
        self.state.notify_termination_waiters();
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
