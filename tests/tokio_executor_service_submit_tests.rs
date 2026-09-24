// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::TaskExecutionError;
use qubit_executor::TaskHandle;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
}

fn ok_usize_task() -> Result<usize, io::Error> {
    Ok(42)
}

#[tokio::test]
async fn test_tokio_executor_service_submit_acceptance_is_not_task_success() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());

    service
        .submit(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept the shared runnable");

    let handle = service
        .submit_tracked(|| Err::<(), _>(io::Error::other("task failed")))
        .expect("service should accept the runnable");

    let err = handle
        .await
        .expect_err("accepted runnable should report task failure through handle");
    assert!(matches!(err, TaskExecutionError::Failed(_)));
}

#[tokio::test]
async fn test_tokio_executor_service_submit_callable_returns_value() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());

    let handle = service
        .submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>)
        .expect("service should accept the callable");

    assert_eq!(handle.await.expect("callable should complete successfully"), 42,);
}

#[tokio::test]
async fn test_tokio_executor_service_capacity_bounds_and_reuses_completed_slots() {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first: TaskHandle<(), io::Error> = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should receive start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first task should start");

    assert!(matches!(
        service.submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>),
        Err(SubmissionError::Saturated)
    ));

    release_tx.send(()).expect("first task should be released");
    tokio::task::spawn_blocking(move || first.get())
        .await
        .expect("result waiter should join")
        .expect("first task should finish");
    let replacement: TaskHandle<usize, io::Error> = service
        .submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>)
        .expect("completed task should release its capacity slot");
    assert_eq!(
        tokio::task::spawn_blocking(move || replacement.get())
            .await
            .expect("result waiter should join")
            .expect("replacement task should complete"),
        42
    );
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_executor_service_shutdown_precedes_capacity_error() {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should receive start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first task should start");
    assert!(matches!(
        service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
        Err(SubmissionError::Saturated)
    ));

    service.shutdown();
    assert!(matches!(
        service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
        Err(SubmissionError::Shutdown)
    ));
    release_tx.send(()).expect("first task should be released");
    tokio::task::spawn_blocking(move || first.get())
        .await
        .expect("result waiter should join")
        .expect("first task should finish");
    service.await_termination().await;
}

#[test]
fn test_tokio_executor_service_capacity_reuses_slot_after_queued_cancel() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::with_task_capacity(
            tokio::runtime::Handle::current(),
            NonZeroUsize::new(1).expect("capacity should be nonzero"),
        );
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (blocker_release_tx, blocker_release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker should start");
            blocker_release_rx.recv().expect("blocker should be released");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking worker should be occupied");

        let cancelled = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("service should accept queued task");
        assert!(matches!(
            service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
            Err(SubmissionError::Saturated)
        ));
        assert_eq!(cancelled.cancel(), qubit_executor::CancelResult::Cancelled);
        assert!(matches!(cancelled.await, Err(TaskExecutionError::Cancelled)));

        let replacement = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("queued cancellation should release its capacity slot");
        assert_eq!(replacement.cancel(), qubit_executor::CancelResult::Cancelled);
        assert!(matches!(replacement.await, Err(TaskExecutionError::Cancelled)));

        blocker_release_tx.send(()).expect("blocker should be released");
        blocker.await.expect("blocker should complete");
        service.shutdown();
        service.await_termination().await;
    });
}
