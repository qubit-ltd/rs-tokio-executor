/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
//! Tests for [`TokioIoExecutorService`](qubit_tokio_executor::service::TokioIoExecutorService).

use std::io;

use qubit_executor::TaskExecutionError;
use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_executor_service_spawn_acceptance_is_not_task_success() {
    let service = TokioIoExecutorService::new();

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
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async { Ok::<usize, io::Error>(42) })
        .expect("service should accept async callable");

    assert_eq!(
        handle
            .await
            .expect("async callable should complete successfully"),
        42,
    );
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_rejects_new_tasks() {
    let service = TokioIoExecutorService::new();
    service.shutdown();

    let result = service.spawn(async { Ok::<(), io::Error>(()) });

    assert!(matches!(
        result,
        Err(qubit_tokio_executor::service::RejectedExecution::Shutdown)
    ));
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_await_termination_waits_for_tasks() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    service.shutdown();
    service.await_termination().await;

    handle.await.expect("task should complete successfully");
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_now_aborts_running_task_handle() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    tokio::task::yield_now().await;
    let report = service.shutdown_now();
    service.await_termination().await;

    assert!(report.cancelled >= 1);
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
}

#[tokio::test]
async fn test_tokio_io_task_handle_cancel_requests_abort() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    assert!(handle.cancel());
    tokio::task::yield_now().await;
    assert!(handle.is_done());
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_io_task_handle_reports_panicked_task() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async move {
            panic!("tokio io service panic");
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        })
        .expect("service should accept panicking task");

    assert!(matches!(handle.await, Err(TaskExecutionError::Panicked)));
    service.shutdown();
    service.await_termination().await;
}
