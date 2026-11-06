// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::MutexGuard;
use qubit_executor::TaskHandle;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_executor::service::StopReport;
use qubit_executor::service::SubmissionError;
use qubit_executor::task::spi::TaskEndpointPair;
use qubit_executor::task::spi::TaskRunner;
use qubit_function::Callable;
use qubit_function::Runnable;
use tokio::pin;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use crate::RejectedTask;
use crate::TokioBlockingTaskHandle;
use crate::TokioExecutorServiceStats;
use crate::tokio_executor_service_state::TokioExecutorServiceState;
use crate::tokio_service_task_guard::TokioServiceTaskGuard;
use crate::tokio_task_registration::TokioTaskRegistration;
use crate::tokio_task_slot_cancellation::cancel_unstarted_task_slot_if_queued;
use crate::tokio_task_slot_cancellation::share_task_slot;
use crate::tokio_task_slot_cancellation::take_task_slot;

/// Tokio-backed service for submitted blocking tasks.
///
/// The service accepts fallible [`Runnable`] and [`Callable`] tasks and runs
/// them through Tokio's blocking task pool.
///
/// # Examples
///
/// ```
/// use qubit_executor::service::ExecutorService;
/// use qubit_tokio_executor::TokioExecutorService;
///
/// let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
/// let service = TokioExecutorService::new(runtime.handle().clone());
/// service.shutdown();
/// service.wait_termination();
/// ```
#[derive(Clone)]
pub struct TokioExecutorService {
    /// Shared service state used by all clones of this service.
    state: Arc<TokioExecutorServiceState>,
    /// Runtime handle used for all submissions.
    runtime: Handle,
}

/// Maximum number of accepted unfinished blocking tasks by default.
const DEFAULT_TASK_CAPACITY: usize = 1024;

/// Tokio-backed blocking executor service routed through `spawn_blocking`.
pub type TokioBlockingExecutorService = TokioExecutorService;

impl TokioExecutorService {
    /// Returns a best-effort snapshot of current task counts and lifecycle.
    ///
    /// # Returns
    ///
    /// The configured task capacity, queued and running counts, and lifecycle.
    #[must_use]
    #[inline]
    pub fn stats(&self) -> TokioExecutorServiceStats {
        self.state.stats()
    }

    /// Subscribes to changes that may make another blocking task admissible.
    ///
    /// Subscribe before the first submission attempt. After a saturated
    /// attempt, wait for [`tokio::sync::watch::Receiver::changed`] and then
    /// retry; the notification reserves no slot and gives producers no fairness
    /// guarantee. Its generation value is not a capacity count.
    ///
    /// # Returns
    ///
    /// A receiver that observes capacity and lifecycle changes.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use std::num::NonZeroUsize;
    /// use std::sync::{Arc, Mutex};
    /// use std::sync::mpsc;
    ///
    /// use qubit_executor::service::{ExecutorService, SubmissionError};
    /// use qubit_tokio_executor::TokioExecutorService;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let service = TokioExecutorService::with_task_capacity(
    ///         tokio::runtime::Handle::current(), NonZeroUsize::new(1).unwrap());
    ///     let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    ///     let (release_tx, release_rx) = mpsc::channel();
    ///     let started_tx = Mutex::new(Some(started_tx));
    ///     let blocker = service.submit_callable(move || {
    ///         started_tx.lock().unwrap().take().unwrap().send(()).unwrap();
    ///         release_rx.recv().unwrap();
    ///         Ok::<(), io::Error>(())
    ///     }).unwrap();
    ///     started_rx.await.unwrap();
    ///
    ///     // Keep ownership until an attempt actually runs. Rejected submission
    ///     // consumes only its wrapper, while the original callable stays here.
    ///     let task = Arc::new(Mutex::new(Some(|| Ok::<usize, io::Error>(42))));
    ///     let mut changes = service.capacity_changes();
    ///     let mut released = false;
    ///     let result = loop {
    ///         let task = Arc::clone(&task);
    ///         let attempt = service.submit_callable(move || {
    ///             let callable = task.lock().unwrap().take().unwrap();
    ///             callable()
    ///         });
    ///         match attempt {
    ///             Ok(handle) => break handle.await.unwrap(),
    ///             Err(SubmissionError::Saturated) => {
    ///                 if !released {
    ///                     release_tx.send(()).unwrap();
    ///                     released = true;
    ///                 }
    ///                 changes.changed().await.unwrap();
    ///             },
    ///             Err(error) => panic!("submission rejected: {error}"),
    ///         }
    ///     };
    ///     assert_eq!(result, 42);
    ///     blocker.await.unwrap();
    ///     service.shutdown();
    ///     service.await_termination().await;
    /// }
    #[must_use]
    pub fn capacity_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.state.capacity_changes()
    }

    /// Creates a new service instance with the default capacity of 1024
    /// unfinished tasks.
    ///
    /// Use [`Self::with_task_capacity`] to choose another nonzero limit.
    ///
    /// # Returns
    ///
    /// A Tokio-backed executor service.
    ///
    /// # Parameters
    ///
    /// * `runtime` - Tokio runtime used to execute accepted blocking tasks.
    #[must_use]
    #[inline]
    pub fn new(runtime: Handle) -> Self {
        Self::with_task_capacity(
            runtime,
            NonZeroUsize::new(DEFAULT_TASK_CAPACITY).expect("default task capacity should be nonzero"),
        )
    }

    /// Creates a service with a maximum number of accepted unfinished tasks.
    ///
    /// The capacity counts both queued and running `spawn_blocking` tasks.
    /// Cancelling a queued task or finishing a running task releases one slot;
    /// Tokio cannot cancel a blocking closure after it has started.
    ///
    /// # Parameters
    ///
    /// * `runtime` - Tokio runtime used to execute accepted blocking tasks.
    /// * `task_capacity` - Nonzero limit for accepted unfinished tasks.
    ///
    /// # Returns
    ///
    /// A Tokio-backed service configured with the supplied capacity.
    #[must_use]
    pub fn with_task_capacity(runtime: Handle, task_capacity: NonZeroUsize) -> Self {
        let state = Arc::new(TokioExecutorServiceState::with_task_capacity(task_capacity));
        Self { state, runtime }
    }

    /// Attempts to accept a runnable while retaining it if submission is
    /// rejected.
    ///
    /// A rejected runnable is returned unchanged and is neither run nor
    /// dropped. Shutdown is checked before capacity saturation.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The runnable to submit.
    /// * `E` - The runnable's error type.
    ///
    /// # Parameters
    ///
    /// * `task` - The runnable to submit.
    pub fn try_submit<T, E>(&self, task: T) -> Result<(), RejectedTask<T>>
    where
        T: Runnable<E> + Send + 'static,
        E: Send + 'static,
    {
        let (marker, guard, admission) = match self.prepare_blocking_submission() {
            Ok(submission) => submission,
            Err(reason) => return Err(RejectedTask::new(reason, task)),
        };
        let abort_queued_task = guard.finish_queued_once_callback();
        let (_, sender) = self.spawn_accepted_blocking_task(marker, admission, guard, abort_queued_task);
        if let Err(task) = sender.send(move || {
            let mut task = task;
            let runner = TaskRunner::new(move || task.run());
            let _ = runner.call::<(), E>();
        }) {
            drop(task);
        }
        Ok(())
    }

    /// Attempts to accept a callable while retaining it if submission is
    /// rejected.
    ///
    /// A rejected callable is returned unchanged and has not been called.
    /// Shutdown is checked before capacity saturation.
    ///
    /// # Type Parameters
    ///
    /// * `C` - The callable to submit.
    /// * `R` - The callable's result type.
    /// * `E` - The callable's error type.
    ///
    /// # Parameters
    ///
    /// * `task` - The callable to submit.
    pub fn try_submit_callable<C, R, E>(&self, task: C) -> Result<TaskHandle<R, E>, RejectedTask<C>>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let (marker, guard, admission) = match self.prepare_blocking_submission() {
            Ok(submission) => submission,
            Err(reason) => return Err(RejectedTask::new(reason, task)),
        };
        let (handle, completion) = TaskEndpointPair::new().into_parts();
        completion.accept();
        let completion = share_task_slot(completion);
        let abort_completion = Arc::clone(&completion);
        let abort_queued_task = guard.finish_queued_once_callback();
        let (_, sender) = self.spawn_accepted_blocking_task(marker, admission, guard, move || {
            cancel_unstarted_task_slot_if_queued(&abort_completion, abort_queued_task)
        });
        if let Err(task) = sender.send(move || {
            if let Some(completion) = take_task_slot(&completion) {
                TaskRunner::new(task).run(completion);
            }
        }) {
            drop(task);
        }
        Ok(handle)
    }

    /// Attempts to accept a tracked callable while retaining it if rejected.
    ///
    /// A rejected callable is returned unchanged and has not been called.
    /// Shutdown is checked before capacity saturation.
    ///
    /// # Type Parameters
    ///
    /// * `C` - The callable to submit.
    /// * `R` - The callable's result type.
    /// * `E` - The callable's error type.
    ///
    /// # Parameters
    ///
    /// * `task` - The callable to submit.
    pub fn try_submit_tracked_callable<C, R, E>(
        &self,
        task: C,
    ) -> Result<TokioBlockingTaskHandle<R, E>, RejectedTask<C>>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        let (marker, guard, admission) = match self.prepare_blocking_submission() {
            Ok(submission) => submission,
            Err(reason) => return Err(RejectedTask::new(reason, task)),
        };
        let (handle, completion) = TaskEndpointPair::new().into_tracked_parts();
        completion.accept();
        let completion = share_task_slot(completion);
        let abort_completion = Arc::clone(&completion);
        let abort_queued_task = guard.finish_queued_once_callback();
        let cancel_queued_task = guard.cancel_queued_callback();
        let (abort_handle, sender) = self.spawn_accepted_blocking_task(marker, admission, guard, move || {
            cancel_unstarted_task_slot_if_queued(&abort_completion, abort_queued_task)
        });
        if let Err(task) = sender.send(move || {
            if let Some(completion) = take_task_slot(&completion) {
                TaskRunner::new(task).run(completion);
            }
        }) {
            drop(task);
        }
        Ok(TokioBlockingTaskHandle::new(handle, abort_handle, cancel_queued_task))
    }

    /// Prepares a blocking-task submission context while holding admission.
    ///
    /// # Returns
    ///
    /// The accepted marker and lifecycle guard for the queued blocking task.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionError::Shutdown`] if the service is not running, or
    /// [`SubmissionError::Saturated`] if its unfinished-task capacity is full.
    /// Tasks are submitted to the runtime handle captured by [`Self::new`].
    fn prepare_blocking_submission(
        &self,
    ) -> Result<(Arc<TokioTaskRegistration>, TokioServiceTaskGuard, MutexGuard<'_, ()>), SubmissionError> {
        let admission = self.state.lock_submission();
        if self.state.is_not_running() {
            return Err(SubmissionError::Shutdown);
        }
        if !self.state.try_accept_task() {
            return Err(SubmissionError::Saturated);
        }
        let marker = TokioTaskRegistration::new();
        let guard = TokioServiceTaskGuard::new(Arc::clone(&self.state), Arc::clone(&marker));
        Ok((marker, guard, admission))
    }

    /// Spawns a queued blocking task and registers its abort hook.
    ///
    /// # Type Parameters
    ///
    /// * `F` - User task closure delivered after admission is released.
    /// * `C` - Hook used to cancel queued service accounting.
    ///
    /// # Parameters
    ///
    /// * `marker` - Registration marker for the accepted task.
    /// * `_admission` - Guard that holds admission through spawn and
    ///   registration.
    /// * `guard` - Lifecycle guard for the accepted task.
    /// * `cancel` - Queued-cancellation hook registered with the service.
    ///
    /// # Returns
    ///
    /// The registered abort handle and sender used to deliver the user task.
    fn spawn_accepted_blocking_task<F, C>(
        &self,
        marker: Arc<TokioTaskRegistration>,
        _admission: MutexGuard<'_, ()>,
        guard: TokioServiceTaskGuard,
        cancel: C,
    ) -> (AbortHandle, tokio::sync::oneshot::Sender<F>)
    where
        F: FnOnce() + Send + 'static,
        C: FnOnce() -> bool + Send + 'static,
    {
        let (sender, receiver) = tokio::sync::oneshot::channel::<F>();
        let join_handle = self.runtime.spawn_blocking(move || {
            if let Ok(task) = receiver.blocking_recv()
                && guard.mark_started()
            {
                task();
            }
        });
        let abort_handle = join_handle.abort_handle();
        self.state.register_abort_handle(marker, abort_handle.clone(), cancel);
        (abort_handle, sender)
    }
}

impl ExecutorService for TokioExecutorService {
    /// Result handle type returned by callable submissions.
    type ResultHandle<R, E>
        = TaskHandle<R, E>
    where
        R: Send + 'static,
        E: Send + 'static;

    /// Tracked handle type returned by tracked callable submissions.
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
    /// requested before the task is accepted, or [`SubmissionError::Saturated`]
    /// if the unfinished-task capacity is full. Shutdown is checked first.
    ///
    /// # Type Parameters
    ///
    /// * `T` - Runnable submitted for execution.
    /// * `E` - Error type reported by the runnable.
    fn submit<T, E>(&self, task: T) -> Result<(), SubmissionError>
    where
        T: Runnable<E> + Send + 'static,
        E: Send + 'static,
    {
        self.try_submit(task).map_err(|rejected| rejected.into_parts().0)
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
    /// requested before the task is accepted, or [`SubmissionError::Saturated`]
    /// if the unfinished-task capacity is full. Shutdown is checked first.
    ///
    /// # Type Parameters
    ///
    /// * `C` - Callable submitted for execution.
    /// * `R` - Value returned by the callable.
    /// * `E` - Error returned by the callable.
    fn submit_callable<C, R, E>(&self, task: C) -> Result<Self::ResultHandle<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        self.try_submit_callable(task)
            .map_err(|rejected| rejected.into_parts().0)
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
    /// requested before the task is accepted, or [`SubmissionError::Saturated`]
    /// if the unfinished-task capacity is full. Shutdown is checked first.
    ///
    /// # Type Parameters
    ///
    /// * `C` - Callable submitted for execution.
    /// * `R` - Value returned by the callable.
    /// * `E` - Error returned by the callable.
    fn submit_tracked_callable<C, R, E>(&self, task: C) -> Result<Self::TrackedHandle<R, E>, SubmissionError>
    where
        C: Callable<R, E> + Send + 'static,
        R: Send + 'static,
        E: Send + 'static,
    {
        self.try_submit_tracked_callable(task)
            .map_err(|rejected| rejected.into_parts().0)
    }

    /// Stops accepting new tasks.
    ///
    /// Already accepted tasks are allowed to finish unless they are cancelled
    /// before their blocking closure starts.
    fn shutdown(&self) {
        {
            let _guard = self.state.lock_submission();
            self.state.shutdown();
        }
        self.state.notify_capacity_changed();
        self.state.notify_if_terminated();
    }

    /// Stops accepting new tasks and requests abort for tracked Tokio tasks.
    ///
    /// Tokio cannot abort blocking tasks that have already started. Such tasks
    /// continue running and keep the service active until their closure
    /// returns.
    ///
    /// # Returns
    ///
    /// A report with queued and running blocking task counts observed when
    /// stop was requested, plus the number of queued blocking tasks that were
    /// actually cancelled before their blocking closures started.
    fn stop(&self) -> StopReport {
        let (queued_count, running_count) = {
            let _guard = self.state.lock_submission();
            self.state.stop();
            self.state.task_count_snapshot()
        };
        let cancellation_count = self.state.abort_tracked_tasks();
        self.state.notify_capacity_changed();
        self.state.notify_if_terminated();
        StopReport::new(queued_count, running_count, cancellation_count)
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

    /// Waits at most `timeout` for the blocking Tokio service to terminate.
    ///
    /// # Parameters
    ///
    /// * `timeout` - Maximum duration to wait for termination.
    fn wait_termination_timeout(&self, timeout: Duration) -> bool {
        self.state.wait_termination_timeout(timeout)
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
            pin!(notified);
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
