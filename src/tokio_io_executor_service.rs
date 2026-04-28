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
    sync::{
        Arc,
        Mutex,
        MutexGuard,
    },
};

use qubit_atomic::{
    Atomic,
    AtomicCount,
};
use tokio::{
    sync::Notify,
    task::AbortHandle,
};

use qubit_executor::TaskExecutionError;

use crate::TokioTaskHandle;
use qubit_executor::service::{
    RejectedExecution,
    ShutdownReport,
};

/// Shared state for [`TokioIoExecutorService`].
#[derive(Default)]
struct TokioIoExecutorServiceState {
    /// Whether shutdown has been requested.
    shutdown: Atomic<bool>,
    /// Number of accepted async tasks that have not finished or been aborted.
    active_tasks: AtomicCount,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for async tasks accepted by this service.
    abort_handles: Mutex<Vec<AbortHandle>>,
    /// Notifies waiters once shutdown has completed and no tasks remain active.
    terminated_notify: Notify,
}

impl TokioIoExecutorServiceState {
    /// Acquires the submission lock while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the submission lock.
    fn lock_submission(&self) -> MutexGuard<'_, ()> {
        self.submission_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Acquires the abort-handle list while tolerating poisoned locks.
    ///
    /// # Returns
    ///
    /// A guard for the tracked Tokio abort handles.
    fn lock_abort_handles(&self) -> MutexGuard<'_, Vec<AbortHandle>> {
        self.abort_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wakes termination waiters when shutdown and task completion allow it.
    fn notify_if_terminated(&self) {
        if self.shutdown.load() && self.active_tasks.is_zero() {
            self.terminated_notify.notify_waiters();
        }
    }
}

/// Task lifecycle guard for [`TokioIoExecutorService`].
struct TokioIoServiceTaskGuard {
    /// Shared service state updated when the guard is dropped.
    state: Arc<TokioIoExecutorServiceState>,
}

impl TokioIoServiceTaskGuard {
    /// Creates a guard that decrements the active-task count on drop.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared service state whose active-task count is decremented
    ///   when this guard is dropped.
    ///
    /// # Returns
    ///
    /// A lifecycle guard bound to the supplied service state.
    fn new(state: Arc<TokioIoExecutorServiceState>) -> Self {
        Self { state }
    }
}

impl Drop for TokioIoServiceTaskGuard {
    /// Updates service counters when an async task completes or is aborted.
    fn drop(&mut self) {
        if self.state.active_tasks.dec() == 0 {
            self.state.notify_if_terminated();
        }
    }
}

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
        drop(submission_guard);

        let guard = TokioIoServiceTaskGuard::new(Arc::clone(&self.state));
        let handle = tokio::spawn(async move {
            let _guard = guard;
            future.await.map_err(TaskExecutionError::Failed)
        });
        self.state.lock_abort_handles().push(handle.abort_handle());
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
        let mut handles = self.state.lock_abort_handles();
        let cancellation_count = handles.len();
        for handle in handles.drain(..) {
            handle.abort();
        }
        drop(handles);
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
