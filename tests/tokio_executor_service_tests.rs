/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
//! Tests for [`TokioExecutorService`](qubit_tokio_executor::service::TokioExecutorService).

use std::{
    io,
    sync::atomic::{
        AtomicBool,
        Ordering,
    },
    time::Duration,
};

use qubit_executor::TaskExecutionError;
use qubit_executor::service::{
    ExecutorService,
    RejectedExecution,
};
use qubit_tokio_executor::TokioExecutorService;

static SHARED_CANCELLABLE_TASK_SHOULD_SLEEP: AtomicBool = AtomicBool::new(false);

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
}

fn ok_usize_task() -> Result<usize, io::Error> {
    Ok(42)
}

fn shared_cancellable_task() -> Result<(), io::Error> {
    if SHARED_CANCELLABLE_TASK_SHOULD_SLEEP.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

#[tokio::test]
async fn test_tokio_executor_service_submit_acceptance_is_not_task_success() {
    let service = TokioExecutorService::new();

    service
        .submit(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept the shared runnable")
        .await
        .expect("shared runnable should complete successfully");

    let handle = service
        .submit(|| Err::<(), _>(io::Error::other("task failed")))
        .expect("service should accept the runnable");

    let err = handle
        .await
        .expect_err("accepted runnable should report task failure through handle");
    assert!(matches!(err, TaskExecutionError::Failed(_)));
}

#[tokio::test]
async fn test_tokio_executor_service_submit_callable_returns_value() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>)
        .expect("service should accept the callable");

    assert_eq!(
        handle.await.expect("callable should complete successfully"),
        42,
    );
}

#[tokio::test]
async fn test_tokio_executor_service_shutdown_rejects_new_tasks() {
    let service = TokioExecutorService::new();
    service.shutdown();

    let result = service.submit(ok_unit_task as fn() -> Result<(), io::Error>);

    assert!(matches!(result, Err(RejectedExecution::Shutdown)));
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_await_termination_waits_for_tasks() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit(|| {
            std::thread::sleep(Duration::from_millis(50));
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
async fn test_tokio_executor_service_shutdown_now_aborts_running_task_handle() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit(|| {
            std::thread::sleep(Duration::from_secs(1));
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
async fn test_tokio_task_handle_cancel_requests_abort() {
    let service = TokioExecutorService::new();
    SHARED_CANCELLABLE_TASK_SHOULD_SLEEP.store(false, Ordering::Release);

    service
        .submit(shared_cancellable_task as fn() -> Result<(), io::Error>)
        .expect("service should accept shared task before cancellation")
        .await
        .expect("shared task should complete before cancellation mode");

    SHARED_CANCELLABLE_TASK_SHOULD_SLEEP.store(true, Ordering::Release);

    let handle = service
        .submit(shared_cancellable_task as fn() -> Result<(), io::Error>)
        .expect("service should accept task");

    assert!(handle.cancel());
    tokio::task::yield_now().await;
    assert!(handle.is_done());
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    SHARED_CANCELLABLE_TASK_SHOULD_SLEEP.store(false, Ordering::Release);
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_task_handle_reports_panicked_task() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit(|| -> Result<(), io::Error> { panic!("tokio service panic") })
        .expect("service should accept panicking task");

    assert!(matches!(handle.await, Err(TaskExecutionError::Panicked)));
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_task_handle_panicked_is_not_cancelled() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit(|| -> Result<(), io::Error> { panic!("tokio service panic") })
        .expect("service should accept panicking task");

    let error = handle
        .await
        .expect_err("panicked task should return execution error");
    assert!(error.is_panicked());
    assert!(!error.is_cancelled());
    service.shutdown();
    service.await_termination().await;
}
