/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::io;

use qubit_executor::TaskExecutionError;
use qubit_tokio_executor::TokioIoExecutorService;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_rejects_new_tasks() {
    let service = TokioIoExecutorService::new();
    service.shutdown();

    let result = service.spawn(async { Ok::<(), io::Error>(()) });

    assert!(matches!(
        result,
        Err(qubit_tokio_executor::service::SubmissionError::Shutdown)
    ));
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_waits_for_task_handles() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    service.shutdown();
    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_aborts_running_task_handle() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    tokio::task::yield_now().await;
    let report = service.stop();

    assert!(report.cancelled >= 1);
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_ignores_completed_tasks() {
    let service = TokioIoExecutorService::new();

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept task")
        .await
        .expect("task should complete successfully");

    let report = service.stop();

    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_sets_stopping_while_task_runs() {
    let service = TokioIoExecutorService::new();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let handle = service
        .spawn(async move {
            release_rx.await.expect("release signal should arrive");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept future");
    tokio::task::yield_now().await;

    let report = service.stop();

    assert!(report.running >= 1);
    assert!(service.is_stopping() || service.is_terminated());
    drop(release_tx);
    assert!(matches!(
        handle.await,
        Err(TaskExecutionError::Cancelled) | Err(TaskExecutionError::Panicked) | Ok(())
    ));
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_completion_keeps_waiting_for_remaining_tasks() {
    let service = TokioIoExecutorService::new();
    let (release_tx, release_rx) = oneshot::channel();

    let pending_handle = service
        .spawn(async move {
            release_rx
                .await
                .expect("pending task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept pending task");

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept quick task")
        .await
        .expect("quick task should complete successfully");

    service.shutdown();

    assert!(!service.is_terminated());
    release_tx
        .send(())
        .expect("pending task should receive release signal");
    pending_handle
        .await
        .expect("pending task should complete successfully");
    assert!(service.is_terminated());
}
