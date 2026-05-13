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
    pin::Pin,
    sync::{Arc, Mutex},
};

use qubit_function::{Callable, Runnable};

use qubit_executor::TaskHandle;
use qubit_executor::task::spi::{TaskEndpointPair, TaskRunner};

use crate::TokioBlockingTaskHandle;
use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_service_task_guard::{TokioServiceTaskGuard, TokioServiceTaskTracker};
use qubit_executor::service::{
    ExecutorService, ExecutorServiceLifecycle, StopReport, SubmissionError,
};

/// Tokio-backed service for submitted blocking tasks.
///
/// The service accepts fallible [`Runnable`](qubit_function::Runnable) and
/// [`Callable`] tasks and runs them through Tokio's blocking task pool.
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
    type ResultHandle<R, E>
        = TaskHandle<R, E>
    where
        R: Send + 'static,
        E: Send + 'static;

    type TrackedHandle<R, E>
        = TokioBlockingTaskHandle<R, E>
    where
        R: Send + 'static,
        E: Send + 'static;

    /// Accepts a runnable and runs it through Tokio.
    ///
    /// # Parameters
    ///
    /// * `task` - Runnable to execute on Tokio's blocking task pool.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the task was accepted.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::Shutdown`] if shutdown has already been
    /// requested before the task is accepted.
    fn submit<T, E>(&self, task: T) -> Result<(), SubmissionError>
    where
        T: Runnable<E> + Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        self.state.accept_task();

        let marker = Arc::new(());
        let tracker = Arc::new(TokioServiceTaskTracker::new(
            Arc::clone(&self.state),
            Arc::clone(&marker),
        ));
        let guard = TokioServiceTaskGuard::new(Arc::clone(&tracker));
        let abort_tracker = Arc::clone(&tracker);
        let handle = tokio::task::spawn_blocking(move || {
            let guard = guard;
            if !guard.mark_started() {
                return;
            }
            let mut task = task;
            let runner = TaskRunner::new(move || task.run());
            let _ = runner.call::<(), E>();
        });
        self.state
            .register_abort_handle(marker, handle.abort_handle(), move || {
                abort_tracker.finish_queued();
            });
        drop(submission_guard);
        Ok(())
    }

    /// Accepts a callable and runs it through Tokio.
    ///
    /// # Parameters
    ///
    /// * `task` - Callable to execute on Tokio's blocking task pool.
    ///
    /// # Returns
    ///
    /// A [`TaskHandle`] for the accepted task.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::Shutdown`] if shutdown has already been
    /// requested before the task is accepted.
    fn submit_callable<C, R, E>(&self, task: C) -> Result<Self::ResultHandle<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        self.state.accept_task();

        let (handle, completion) = TaskEndpointPair::new().into_parts();
        completion.accept();
        let completion = Arc::new(Mutex::new(Some(completion)));
        let abort_completion = Arc::clone(&completion);
        let marker = Arc::new(());
        let tracker = Arc::new(TokioServiceTaskTracker::new(
            Arc::clone(&self.state),
            Arc::clone(&marker),
        ));
        let guard = TokioServiceTaskGuard::new(Arc::clone(&tracker));
        let abort_tracker = Arc::clone(&tracker);
        let join_handle = tokio::task::spawn_blocking(move || {
            let guard = guard;
            if !guard.mark_started() {
                return;
            }
            let completion = completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(completion) = completion {
                TaskRunner::new(task).run(completion);
            }
        });
        self.state
            .register_abort_handle(marker, join_handle.abort_handle(), move || {
                let completion = abort_completion
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(completion) = completion {
                    let _cancelled = completion.cancel_unstarted();
                }
                abort_tracker.finish_queued();
            });
        drop(submission_guard);
        Ok(handle)
    }

    /// Accepts a callable and returns an actively tracked handle.
    ///
    /// # Parameters
    ///
    /// * `task` - Callable to execute on Tokio's blocking task pool.
    ///
    /// # Returns
    ///
    /// A [`TokioBlockingTaskHandle`] for the accepted task.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::Shutdown`] if shutdown has already been
    /// requested before the task is accepted.
    fn submit_tracked_callable<C, R, E>(
        &self,
        task: C,
    ) -> Result<Self::TrackedHandle<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let submission_guard = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        self.state.accept_task();

        let (handle, completion) = TaskEndpointPair::new().into_tracked_parts();
        completion.accept();
        let completion = Arc::new(Mutex::new(Some(completion)));
        let abort_completion = Arc::clone(&completion);
        let marker = Arc::new(());
        let tracker = Arc::new(TokioServiceTaskTracker::new(
            Arc::clone(&self.state),
            Arc::clone(&marker),
        ));
        let guard = TokioServiceTaskGuard::new(Arc::clone(&tracker));
        let abort_tracker = Arc::clone(&tracker);
        let join_handle = tokio::task::spawn_blocking(move || {
            let guard = guard;
            if !guard.mark_started() {
                return;
            }
            let completion = completion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(completion) = completion {
                TaskRunner::new(task).run(completion);
            }
        });
        let abort_handle = join_handle.abort_handle();
        self.state
            .register_abort_handle(marker, abort_handle.clone(), move || {
                let completion = abort_completion
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(completion) = completion {
                    let _cancelled = completion.cancel_unstarted();
                }
                abort_tracker.finish_queued();
            });
        drop(submission_guard);
        Ok(TokioBlockingTaskHandle::new(handle, abort_handle, tracker))
    }

    /// Stops accepting new tasks.
    ///
    /// Already accepted tasks are allowed to finish unless they are cancelled
    /// before their blocking closure starts.
    fn shutdown(&self) {
        let _guard = self.state.lock_submission();
        self.state.shutdown();
        self.state.notify_if_terminated();
    }

    /// Stops accepting new tasks and requests abort for tracked Tokio tasks.
    ///
    /// Tokio cannot abort blocking tasks that have already started. Such tasks
    /// continue running and keep the service active until their closure returns.
    ///
    /// # Returns
    ///
    /// A report with queued and running blocking task counts observed when
    /// stop was requested, plus the number of Tokio abort handles signalled.
    fn stop(&self) -> StopReport {
        let _guard = self.state.lock_submission();
        self.state.stop();
        let task_counts = self.state.task_count_snapshot();
        let cancellation_count = self.state.abort_tracked_tasks();
        self.state.notify_if_terminated();
        StopReport::new(task_counts.queued, task_counts.running, cancellation_count)
    }

    /// Returns the current lifecycle state.
    fn lifecycle(&self) -> ExecutorServiceLifecycle {
        self.state.lifecycle()
    }

    /// Returns whether shutdown has been requested.
    fn is_not_running(&self) -> bool {
        self.state.is_not_running()
    }

    /// Returns whether shutdown was requested and all tasks are finished.
    fn is_terminated(&self) -> bool {
        self.lifecycle() == ExecutorServiceLifecycle::Terminated
    }

    /// Blocks until the service has terminated.
    fn wait_termination(&self) {
        self.state.wait_termination();
    }
}

impl TokioExecutorService {
    /// Waits asynchronously until the service has terminated.
    ///
    /// # Returns
    ///
    /// A future that resolves after shutdown or stop has been requested and all
    /// accepted blocking tasks have finished or been aborted before start.
    pub fn await_termination(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
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
