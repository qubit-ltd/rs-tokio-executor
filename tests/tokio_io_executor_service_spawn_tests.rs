// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::pending;
use std::io;
use std::num::NonZeroUsize;

use qubit_executor::TaskExecutionError;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_executor_service_spawn_acceptance_is_not_task_success() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept async runnable")
        .await
        .expect("async runnable should complete successfully");

    let handle = service
        .spawn(async { Err::<(), _>(io::Error::other("task failed")) })
        .expect("service should accept async task");

    let err = handle
        .await
        .expect_err("accepted async task should report failure through handle");
    assert!(matches!(err, TaskExecutionError::Failed(_)));
}

#[tokio::test]
async fn test_tokio_io_executor_service_spawn_returns_value() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    let handle = service
        .spawn(async { Ok::<usize, io::Error>(42) })
        .expect("service should accept async callable");

    assert_eq!(handle.await.expect("async callable should complete successfully"), 42,);
}

#[tokio::test]
async fn test_tokio_io_executor_service_capacity_bounds_and_reuses_completed_slots() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let first = service
        .spawn(async move {
            started_tx.send(()).expect("test should receive start signal");
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx.await.expect("first task should start");

    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));

    assert_eq!(first.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(first.await, Err(TaskExecutionError::Cancelled)));
    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("completed future should release its capacity slot")
        .await
        .expect("replacement future should complete");
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_precedes_capacity_error() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let first = service
        .spawn(async {
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first future");
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));

    service.shutdown();
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Shutdown)
    ));
    assert_eq!(first.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(first.await, Err(TaskExecutionError::Cancelled)));
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_io_executor_service_capacity_reuses_slot_after_pre_poll_abort() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let cancelled = service
        .spawn(async {
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the future");
    assert_eq!(cancelled.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(cancelled.await, Err(TaskExecutionError::Cancelled)));

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("pre-poll cancellation should release its capacity slot")
        .await
        .expect("replacement future should complete");
    service.shutdown();
    service.await_termination().await;
}
