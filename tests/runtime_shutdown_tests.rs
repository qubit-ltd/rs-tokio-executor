// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::TaskExecutionError;
use qubit_executor::executor::Executor;
use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutor;
use qubit_tokio_executor::TokioExecutorService;
use qubit_tokio_executor::TokioIoExecutorService;

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Releases a test blocking worker during normal return or panic unwinding.
struct ReleaseOnDrop(Option<mpsc::Sender<()>>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

#[test]
fn tokio_executor_finishes_started_blocking_task_after_background_shutdown() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let executor = TokioExecutor::new(runtime.handle().clone());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = executor
        .call(move || {
            started_tx.send(()).expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            Ok::<usize, io::Error>(42)
        })
        .expect("executor should accept task while runtime is alive");
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking closure should start");

    // Runtime destruction occurs on this ordinary test thread, never inside
    // a Tokio async context. Background shutdown avoids waiting on the gate.
    runtime.shutdown_background();
    release_tx
        .send(())
        .expect("running closure should be released");

    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    let result = observer.block_on(async {
        tokio::time::timeout(OBSERVE_TIMEOUT, async move { handle.await }).await
    });
    assert_eq!(
        result
            .expect("task should finish after release")
            .expect("call should succeed"),
        42
    );
}

#[test]
fn blocking_service_waits_for_started_task_after_runtime_background_shutdown() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            Ok::<usize, io::Error>(42)
        })
        .expect("service should accept task while runtime is alive");
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking closure should start");

    runtime.shutdown_background();
    release_tx
        .send(())
        .expect("running closure should be released");

    assert_eq!(handle.get().expect("blocking task should finish"), 42);
    let stats = service.stats();
    assert_eq!(stats.queued, 0);
    assert_eq!(stats.running, 0);
    service.shutdown();
    assert!(service.wait_termination_timeout(OBSERVE_TIMEOUT));
    assert!(service.is_terminated());
}

#[test]
fn io_service_drops_pending_future_and_terminates_after_runtime_shutdown() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioIoExecutorService::new(runtime.handle().clone());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let handle = runtime.block_on(async {
        let handle = service
            .spawn(async move {
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
                Ok::<(), io::Error>(())
            })
            .expect("service should accept task while runtime is alive");
        started_rx.await.expect("future should be polled");
        handle
    });

    runtime.shutdown_background();
    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    let result = observer.block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, handle).await });
    assert!(matches!(
        result.expect("aborted task should resolve"),
        Err(TaskExecutionError::Cancelled)
    ));
    service.shutdown();
    assert!(service.is_terminated());
    assert_eq!(service.stats().accepted_unfinished, 0);
}

#[test]
fn blocking_service_runtime_closes_with_ordinary_and_tracked_tasks_queued() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let _release_guard = ReleaseOnDrop(Some(release_tx));
    let blocker = runtime.handle().spawn_blocking(move || {
        started_tx
            .send(())
            .expect("test should observe blocker start");
        release_rx.recv().expect("test should release blocker");
    });
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking pool slot should be occupied");

    let (ordinary_started_tx, ordinary_started_rx) = mpsc::channel();
    let (tracked_started_tx, tracked_started_rx) = mpsc::channel();
    let ordinary = service
        .submit_callable(move || {
            ordinary_started_tx
                .send(())
                .expect("observe ordinary task start");
            Ok::<usize, io::Error>(1)
        })
        .expect("ordinary task should be accepted into the queue");
    let tracked = service
        .submit_tracked_callable(move || {
            tracked_started_tx
                .send(())
                .expect("observe tracked task start");
            Ok::<usize, io::Error>(2)
        })
        .expect("tracked task should be accepted into the queue");
    assert_eq!(service.stats().queued, 2);
    assert!(matches!(
        ordinary_started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        tracked_started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    runtime.shutdown_background();
    // Releasing the worker lets Tokio start accepted queued work. The guard
    // also releases it if an assertion above unwinds.
    drop(_release_guard);

    let blocker_runtime = tokio::runtime::Runtime::new().expect("observer runtime should build");
    blocker_runtime
        .block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, blocker).await })
        .expect("blocker should finish after release")
        .expect("blocking worker should not panic");
    let ordinary_observation = tokio::runtime::Runtime::new()
        .expect("ordinary observer runtime should build")
        .block_on(async {
            tokio::time::timeout(OBSERVE_TIMEOUT, async move { ordinary.await }).await
        });
    let tracked_observation = tokio::runtime::Runtime::new()
        .expect("tracked observer runtime should build")
        .block_on(async {
            tokio::time::timeout(OBSERVE_TIMEOUT, async move { tracked.await }).await
        });
    assert_eq!(
        ordinary_observation
            .expect("ordinary task should resolve")
            .expect("ordinary task should succeed"),
        1
    );
    assert_eq!(
        tracked_observation
            .expect("tracked task should resolve")
            .expect("tracked task should succeed"),
        2
    );
    ordinary_started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("ordinary task should start after the blocker is released");
    tracked_started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("tracked task should start after the blocker is released");
    let stats = service.stats();
    assert_eq!(stats.queued, 0);
    assert_eq!(stats.running, 0);
    service.shutdown();
    assert!(service.wait_termination_timeout(OBSERVE_TIMEOUT));
    assert!(service.is_terminated());
}

#[test]
fn io_service_runtime_closes_before_accepted_future_is_first_polled() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::new(runtime.handle().clone());
    // A current-thread runtime does not poll a newly spawned task again until
    // block_on is entered, so this returns an accepted but unpolled future.
    let handle = runtime.block_on(async {
        service
            .spawn(async { Ok::<usize, io::Error>(3) })
            .expect("future should be accepted")
    });
    assert_eq!(service.stats().accepted_unfinished, 1);
    runtime.shutdown_background();

    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    let result = observer.block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, handle).await });
    assert!(matches!(
        result.expect("dropped task should resolve"),
        Err(TaskExecutionError::Cancelled)
    ));
    service.shutdown();
    assert!(service.is_terminated());
    assert_eq!(service.stats().accepted_unfinished, 0);
}

#[test]
fn services_accept_submission_after_bound_runtime_shutdown_without_panicking() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let handle = runtime.handle().clone();
    runtime.shutdown_background();

    // These APIs currently admit based on service lifecycle, not a liveness
    // probe of Tokio's runtime. Record the returned status without presuming
    // that a closed runtime must map to SubmissionError::Shutdown.
    let blocking = TokioExecutorService::new(handle.clone());
    let ordinary = blocking.submit_callable(|| Ok::<usize, io::Error>(1));
    assert!(
        ordinary.is_ok(),
        "closed-runtime submission should not panic"
    );
    let tracked = blocking.submit_tracked_callable(|| Ok::<usize, io::Error>(2));
    assert!(
        tracked.is_ok(),
        "closed-runtime tracked submission should not panic"
    );
    blocking.shutdown();
    let blocking_terminated = blocking.wait_termination_timeout(OBSERVE_TIMEOUT);
    let blocking_stats = blocking.stats();
    assert!(
        blocking_terminated,
        "accepted tasks should leave service accounting"
    );
    assert_eq!(blocking_stats.queued, 0);
    assert_eq!(blocking_stats.running, 0);
    if let Ok(task) = ordinary {
        let observer = tokio::runtime::Runtime::new().expect("ordinary observer should build");
        let result = observer.block_on(async {
            tokio::time::timeout(OBSERVE_TIMEOUT, async move { task.await }).await
        });
        assert!(matches!(result, Ok(Err(TaskExecutionError::Dropped))));
    }
    if let Ok(task) = tracked {
        let observer = tokio::runtime::Runtime::new().expect("tracked observer should build");
        let result = observer.block_on(async {
            tokio::time::timeout(OBSERVE_TIMEOUT, async move { task.await }).await
        });
        assert!(matches!(result, Ok(Err(TaskExecutionError::Dropped))));
    }

    let io_service = TokioIoExecutorService::new(handle);
    let io_task = io_service.spawn(async { Ok::<usize, io::Error>(2) });
    assert!(
        io_task.is_ok(),
        "closed-runtime submission should not panic"
    );
    io_service.shutdown();
    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    if let Ok(task) = io_task {
        assert!(matches!(
            observer.block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, task).await }),
            Ok(Err(TaskExecutionError::Cancelled))
        ));
    }
    assert!(io_service.is_terminated());
    assert_eq!(io_service.stats().accepted_unfinished, 0);
}

#[test]
fn tokio_executor_runtime_closes_while_task_is_queued_before_start() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");
    let executor = TokioExecutor::new(runtime.handle().clone());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_guard = ReleaseOnDrop(Some(release_tx));
    let blocker = runtime.handle().spawn_blocking(move || {
        started_tx
            .send(())
            .expect("test should observe blocker start");
        release_rx.recv().expect("test should release blocker");
    });
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking pool slot should be occupied");

    let (task_started_tx, task_started_rx) = mpsc::channel();
    let task = executor
        .call(move || {
            task_started_tx.send(()).expect("observe queued task start");
            Ok::<usize, io::Error>(7)
        })
        .expect("task should be accepted into the blocking queue");
    assert!(matches!(
        task_started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    runtime.shutdown_background();
    drop(release_guard);

    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    observer
        .block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, blocker).await })
        .expect("blocker should finish")
        .expect("blocker should not panic");
    let result = observer
        .block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, async move { task.await }).await });
    assert_eq!(
        result
            .expect("queued task handle should resolve")
            .expect("queued task should succeed"),
        7
    );
    task_started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("queued task should start after the blocker is released");
}

#[test]
fn tokio_executor_submission_after_runtime_shutdown_returns_dropped_result() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let executor = TokioExecutor::new(runtime.handle().clone());
    runtime.shutdown_background();
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_by_task = std::sync::Arc::clone(&ran);

    let task = executor
        .call(move || {
            ran_by_task.store(true, std::sync::atomic::Ordering::Release);
            Ok::<usize, io::Error>(9)
        })
        .expect("TokioExecutor does not report runtime shutdown as submission error");
    let observer = tokio::runtime::Runtime::new().expect("observer runtime should build");
    let result = observer
        .block_on(async { tokio::time::timeout(OBSERVE_TIMEOUT, async move { task.await }).await });
    assert!(matches!(
        result.expect("accepted task handle should resolve"),
        Err(TaskExecutionError::Dropped)
    ));
    assert!(!ran.load(std::sync::atomic::Ordering::Acquire));
}
