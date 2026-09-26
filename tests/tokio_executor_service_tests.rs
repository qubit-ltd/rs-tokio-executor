// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use qubit_executor::service::ExecutorService;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;

#[tokio::test]
async fn test_tokio_executor_service_runs_blocking_tasks_and_rejects_after_shutdown() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .submit_callable(|| Ok::<_, io::Error>("done".to_owned()))
        .expect("service should accept callable");

    assert_eq!("done", handle.await.expect("callable should finish"));

    service.shutdown();
    assert!(matches!(
        service.submit(|| Ok::<_, io::Error>(())),
        Err(SubmissionError::Shutdown),
    ));
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_executor_service_runs_tracked_callable() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .submit_tracked_callable(|| Ok::<_, io::Error>(17))
        .expect("service should accept tracked callable");

    assert_eq!(handle.await.expect("tracked callable should finish"), 17);

    service.shutdown();
    service.wait_termination();
}

#[test]
fn test_tokio_executor_service_rejects_callable_submissions_after_shutdown() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());

    service.shutdown();
    service.wait_termination();

    let callable = service.submit_callable(|| Ok::<_, io::Error>(1));
    assert!(matches!(callable, Err(SubmissionError::Shutdown)));

    let tracked = service.submit_tracked_callable(|| Ok::<_, io::Error>(2));
    assert!(matches!(tracked, Err(SubmissionError::Shutdown)));
}

#[test]
fn test_tokio_executor_service_submit_without_runtime_returns_submission_error() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());
    let handle = service
        .submit_callable(|| Ok::<usize, io::Error>(42))
        .expect("explicit runtime handle should allow submission");
    assert_eq!(handle.get().expect("task should succeed"), 42);
    service.shutdown();
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_executor_service_submit_runs_detached_task() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let completed = Arc::new(AtomicBool::new(false));
    let completed_for_task = Arc::clone(&completed);

    service
        .submit(move || {
            completed_for_task.store(true, Ordering::Release);
            Ok::<(), std::io::Error>(())
        })
        .expect("service should accept runnable");

    service.shutdown();
    service.wait_termination();

    assert!(completed.load(Ordering::Acquire));
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Terminated);
    assert!(service.is_not_running());
    assert!(service.is_terminated());

    let rejected = service.submit(|| Ok::<(), std::io::Error>(()));
    assert!(matches!(rejected, Err(SubmissionError::Shutdown)));
}

#[tokio::test]
async fn test_tokio_executor_service_stats_and_capacity_changes() {
    let capacity = std::num::NonZeroUsize::new(2).expect("capacity is nonzero");
    let service = TokioExecutorService::with_task_capacity(tokio::runtime::Handle::current(), capacity);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let running = service
        .submit_tracked_callable(move || {
            started_tx.send(()).expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            Ok::<(), io::Error>(())
        })
        .expect("first task should be accepted");
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("blocking task should start");
    let queued = service
        .submit_tracked_callable(|| Ok::<(), io::Error>(()))
        .expect("second task should be accepted");
    let mut changes = service.capacity_changes();

    let stats = service.stats();
    assert_eq!(stats.task_capacity, 2);
    assert_eq!(stats.queued, 1);
    assert_eq!(stats.running, 1);

    release_tx.send(()).expect("running task should be released");
    tokio::time::timeout(std::time::Duration::from_secs(1), changes.changed())
        .await
        .expect("task completion should publish capacity change")
        .expect("capacity notification sender should remain open");
    running.await.expect("running task should finish");
    queued.await.expect("queued task should finish");
    service.shutdown();
    service.await_termination().await;
}
