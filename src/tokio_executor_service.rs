/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::sync::Arc;

use qubit_function::Callable;

use qubit_executor::TaskRunner;

use crate::TokioTaskHandle;
use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_service_task_guard::TokioServiceTaskGuard;
use qubit_executor::service::{
    ExecutorService,
    RejectedExecution,
    ShutdownReport,
};

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

/// Tokio-backed blocking executor service routed through `spawn_blocking`.
pub type TokioBlockingExecutorService = TokioExecutorService;

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

        let marker = Arc::new(());
        let guard = TokioServiceTaskGuard::new(Arc::clone(&self.state), Arc::clone(&marker));
        let handle = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            TaskRunner::new(task).call()
        });
        self.state
            .register_abort_handle(marker, handle.abort_handle());
        drop(submission_guard);
        Ok(TokioTaskHandle::new(handle))
    }

    /// Stops accepting new tasks.
    ///
    /// Already accepted tasks are allowed to finish unless they are cancelled
    /// before their blocking closure starts.
    fn shutdown(&self) {
        let _guard = self.state.lock_submission();
        self.state.shutdown.store(true);
        self.state.notify_if_terminated();
    }

    /// Stops accepting new tasks and requests abort for tracked Tokio tasks.
    ///
    /// Tokio cannot abort blocking tasks that have already started. Such tasks
    /// continue running and keep the service active until their closure returns.
    ///
    /// # Returns
    ///
    /// A report with zero queued tasks, the observed active task count, and
    /// the number of Tokio abort handles signalled.
    fn shutdown_now(&self) -> ShutdownReport {
        let _guard = self.state.lock_submission();
        self.state.shutdown.store(true);
        let running = self.state.active_tasks.get();
        let cancellation_count = self.state.abort_tracked_tasks();
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
    /// accepted Tokio blocking tasks have finished or were cancelled before
    /// their closures started.
    fn await_termination(&self) -> Self::Termination<'_> {
        Box::pin(async move {
            let notified = self.state.terminated_notify.notified();
            tokio::pin!(notified);
            loop {
                notified.as_mut().enable();
                if self.is_terminated() {
                    return;
                }
                notified.as_mut().await;
                notified.set(self.state.terminated_notify.notified());
            }
        })
    }
}
