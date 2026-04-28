/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
};

use qubit_executor::TaskExecutionError;

use crate::TokioTaskHandle;
use crate::tokio_io_executor_service_state::TokioIoExecutorServiceState;
use crate::tokio_io_service_task_guard::TokioIoServiceTaskGuard;
use qubit_executor::service::{
    RejectedExecution,
    ShutdownReport,
};

/// Tokio-backed executor service for async IO and Future-based tasks.
///
/// Accepted futures are spawned with [`tokio::spawn`], so waiting for external
/// IO does not occupy a dedicated blocking thread.
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
    /// Returns [`RejectedExecution::Shutdown`] if shutdown has already been
    /// requested before the task is accepted.
    pub fn spawn<F, R, E>(&self, future: F) -> Result<TokioTaskHandle<R, E>, RejectedExecution>
    where
        F: Future<Output = Result<R, E>> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.shutdown.load() {
            return Err(RejectedExecution::Shutdown);
        }
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
    /// their handles or by [`Self::shutdown_now`].
    pub fn shutdown(&self) {
        let _guard = self.state.lock_submission();
        self.state.shutdown.store(true);
        self.state.notify_if_terminated();
    }

    /// Stops accepting new tasks and aborts tracked async tasks.
    ///
    /// # Returns
    ///
    /// A report with zero queued tasks, the observed active-task count, and
    /// the number of Tokio abort handles signalled.
    pub fn shutdown_now(&self) -> ShutdownReport {
        let _guard = self.state.lock_submission();
        self.state.shutdown.store(true);
        let running = self.state.active_tasks.get();
        let cancellation_count = self.state.abort_tracked_tasks();
        self.state.notify_if_terminated();
        ShutdownReport::new(0, running, cancellation_count)
    }

    /// Returns whether shutdown has been requested.
    ///
    /// # Returns
    ///
    /// `true` if this service no longer accepts new async tasks.
    #[inline]
    pub fn is_shutdown(&self) -> bool {
        self.state.shutdown.load()
    }

    /// Returns whether shutdown was requested and all async tasks are finished.
    ///
    /// # Returns
    ///
    /// `true` only after shutdown has been requested and no accepted async
    /// tasks remain active.
    #[inline]
    pub fn is_terminated(&self) -> bool {
        self.is_shutdown() && self.state.active_tasks.is_zero()
    }

    /// Waits until the service has terminated.
    ///
    /// # Returns
    ///
    /// A future that resolves after shutdown has been requested and all
    /// accepted async tasks have finished or been aborted.
    pub fn await_termination(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            loop {
                if self.is_terminated() {
                    return;
                }
                self.state.terminated_notify.notified().await;
            }
        })
    }
}
