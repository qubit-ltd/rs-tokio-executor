/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use std::sync::{
    Arc,
    Mutex,
    MutexGuard,
};

use qubit_atomic::{
    Atomic,
    AtomicCount,
};
use qubit_function::Callable;
use tokio::{
    sync::Notify,
    task::AbortHandle,
};

use qubit_executor::TaskRunner;

use crate::TokioTaskHandle;
use qubit_executor::service::{
    ExecutorService,
    RejectedExecution,
    ShutdownReport,
};

/// Shared state for [`TokioExecutorService`].
#[derive(Default)]
struct TokioExecutorServiceState {
    /// Whether shutdown has been requested.
    shutdown: Atomic<bool>,
    /// Number of accepted Tokio tasks that have not finished or been aborted.
    active_tasks: AtomicCount,
    /// Serializes task submission and shutdown transitions.
    submission_lock: Mutex<()>,
    /// Abort handles for tasks accepted by this service.
    abort_handles: Mutex<Vec<AbortHandle>>,
    /// Notifies waiters once shutdown has completed and no tasks remain active.
    terminated_notify: Notify,
}

impl TokioExecutorServiceState {
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

    /// Acquires the abort handle list while tolerating poisoned locks.
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

/// Task lifecycle guard for [`TokioExecutorService`].
struct TokioServiceTaskGuard {
    /// Shared service state updated when the guard is dropped.
    state: Arc<TokioExecutorServiceState>,
}

impl TokioServiceTaskGuard {
    /// Creates a guard that decrements the active task count on drop.
    ///
    /// # Parameters
    ///
    /// * `state` - Shared state whose active-task counter is decremented when
    ///   the guard is dropped.
    ///
    /// # Returns
    ///
    /// A lifecycle guard bound to the supplied service state.
    fn new(state: Arc<TokioExecutorServiceState>) -> Self {
        Self { state }
    }
}

impl Drop for TokioServiceTaskGuard {
    /// Updates service counters when a task completes or is aborted.
    fn drop(&mut self) {
        if self.state.active_tasks.dec() == 0 {
            self.state.notify_if_terminated();
        }
    }
}

/// Tokio-backed service for submitted blocking tasks.
///
/// The service accepts fallible [`Runnable`](qubit_function::Runnable) and
/// [`Callable`] tasks, runs them through Tokio, and
/// returns awaitable handles for their final results.
#[derive(Default, Clone)]
pub struct TokioExecutorService {
    /// Shared service state used by all clones of this service.
    state: Arc<TokioExecutorServiceState>,
}

impl TokioExecutorService {
    /// Creates a new service instance.
    ///
    /// # Returns
    ///
    /// A Tokio-backed executor service.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ExecutorService for TokioExecutorService {
    type Handle<R, E>
        = TokioTaskHandle<R, E>
    where
        R: Send + 'static,
        E: Send + 'static;

    type Termination<'a>
        = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>
    where
        Self: 'a;

    /// Accepts a callable and runs it through Tokio.
    ///
    /// # Parameters
    ///
    /// * `task` - Callable to execute on Tokio's blocking task pool.
    ///
    /// # Returns
    ///
    /// A [`TokioTaskHandle`] for the accepted task.
    ///
    /// # Errors
    ///
    /// Returns [`RejectedExecution::Shutdown`] if shutdown has already been
    /// requested before the task is accepted.
    fn submit_callable<C, R, E>(&self, task: C) -> Result<Self::Handle<R, E>, RejectedExecution>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.shutdown.load() {
            return Err(RejectedExecution::Shutdown);
        }
        self.state.active_tasks.inc();
        drop(submission_guard);

        let guard = TokioServiceTaskGuard::new(Arc::clone(&self.state));
        let handle = tokio::spawn(async move {
            let _guard = guard;
            tokio::task::spawn_blocking(move || TaskRunner::new(task).call())
                .await
                .expect("tokio blocking task should join")
        });
        self.state.lock_abort_handles().push(handle.abort_handle());
        Ok(TokioTaskHandle::new(handle))
    }

    /// Stops accepting new tasks.
    ///
    /// Already accepted tasks are allowed to finish unless cancelled through
    /// their handles or by [`Self::shutdown_now`].
    fn shutdown(&self) {
        let _guard = self.state.lock_submission();
        self.state.shutdown.store(true);
        self.state.notify_if_terminated();
    }

    /// Stops accepting new tasks and aborts tracked Tokio tasks.
    ///
    /// # Returns
    ///
    /// A report with zero queued tasks, the observed active task count, and
    /// the number of Tokio abort handles signalled.
    fn shutdown_now(&self) -> ShutdownReport {
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
    fn is_shutdown(&self) -> bool {
        self.state.shutdown.load()
    }

    /// Returns whether shutdown was requested and all tasks are finished.
    fn is_terminated(&self) -> bool {
        self.is_shutdown() && self.state.active_tasks.is_zero()
    }

    /// Waits until the service has terminated.
    ///
    /// # Returns
    ///
    /// A future that resolves after shutdown has been requested and all
    /// accepted Tokio tasks have finished or been aborted.
    fn await_termination(&self) -> Self::Termination<'_> {
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
