/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026.
 *    Haixing Hu, Qubit Co. Ltd.
 *
 *    All rights reserved.
 *
 ******************************************************************************/
use std::{
    io,
    sync::mpsc,
    time::Duration,
};

use qubit_executor::service::{
    ExecutorService,
    RejectedExecution,
};
use qubit_tokio_executor::TokioExecutorService;

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
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
async fn test_tokio_executor_service_shutdown_now_waits_for_running_blocking_task() {
    let service = TokioExecutorService::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let handle = service
        .submit_callable(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
            Ok::<usize, io::Error>(42)
        })
        .expect("service should accept task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocking task should start");
    let report = service.shutdown_now();
    let termination_result =
        tokio::time::timeout(Duration::from_millis(50), service.await_termination()).await;

    assert!(termination_result.is_err());
    release_tx
        .send(())
        .expect("blocking task should receive release signal");
    service.await_termination().await;

    assert!(report.cancelled >= 1);
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
    assert_eq!(
        handle.await.expect("running blocking task should finish"),
        42
    );
}

#[tokio::test]
async fn test_tokio_executor_service_shutdown_now_ignores_completed_tasks() {
    let service = TokioExecutorService::new();

    service
        .submit(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept task")
        .await
        .expect("task should complete successfully");

    let report = service.shutdown_now();
    service.await_termination().await;

    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 0);
    assert!(service.is_shutdown());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_completion_keeps_waiting_for_remaining_tasks() {
    let service = TokioExecutorService::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let blocked_handle = service
        .submit(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept blocked task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocking task should start");
    service
        .submit(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept quick task")
        .await
        .expect("quick task should complete successfully");

    service.shutdown();
    let termination_result =
        tokio::time::timeout(Duration::from_millis(50), service.await_termination()).await;

    assert!(termination_result.is_err());
    release_tx
        .send(())
        .expect("blocking task should receive release signal");
    blocked_handle
        .await
        .expect("blocked task should complete successfully");
    service.await_termination().await;
    assert!(service.is_terminated());
}
