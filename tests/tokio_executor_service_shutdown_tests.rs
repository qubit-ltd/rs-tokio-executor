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
    io,
    sync::mpsc,
    time::Duration,
};

use qubit_executor::{
    TaskExecutionError,
    service::{
        ExecutorService,
        SubmissionError,
    },
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

    assert!(matches!(result, Err(SubmissionError::Shutdown)));
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_wait_termination_waits_for_tasks() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit_tracked(|| {
            std::thread::sleep(Duration::from_millis(50));
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    service.shutdown();
    service.wait_termination();

    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_await_termination_waits_for_tasks() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit_tracked(|| {
            std::thread::sleep(Duration::from_millis(50));
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    service.shutdown();
    service.await_termination().await;

    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_stop_reports_no_cancelled_running_blocking_task() {
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
    let report = service.stop();
    let waiter_service = service.clone();
    let termination_result = tokio::time::timeout(
        Duration::from_millis(50),
        tokio::task::spawn_blocking(move || waiter_service.wait_termination()),
    )
    .await;

    assert!(termination_result.is_err());
    release_tx
        .send(())
        .expect("blocking task should receive release signal");
    service.wait_termination();

    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
    assert_eq!(
        handle.await.expect("running blocking task should finish"),
        42
    );
}

#[test]
fn test_tokio_executor_service_stop_cancels_queued_detached_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        service
            .submit(ok_unit_task as fn() -> Result<(), io::Error>)
            .expect("service should accept queued detached task");
        let report = service.stop();
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");

        assert_eq!(report.queued, 1);
        assert_eq!(report.running, 0);
        assert_eq!(report.cancelled, 1);
        assert!(service.is_not_running());
    });
}

#[test]
fn test_tokio_executor_service_stop_cancels_queued_tracked_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        let handle = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("service should accept queued task");
        let report = service.stop();
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");
        service.wait_termination();

        assert_eq!(report.queued, 1);
        assert_eq!(report.running, 0);
        assert_eq!(report.cancelled, 1);
        assert!(service.is_not_running());
        assert!(service.is_terminated());
        assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    });
}

#[tokio::test]
async fn test_tokio_executor_service_stop_ignores_completed_tasks() {
    let service = TokioExecutorService::new();

    service
        .submit_tracked(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept task")
        .await
        .expect("task should complete successfully");
    service.shutdown();
    service.wait_termination();

    let report = service.stop();

    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_completion_keeps_waiting_for_remaining_tasks() {
    let service = TokioExecutorService::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let blocked_handle = service
        .submit_tracked(move || {
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
        .submit_tracked(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept quick task")
        .await
        .expect("quick task should complete successfully");

    service.shutdown();
    let waiter_service = service.clone();
    let termination_result = tokio::time::timeout(
        Duration::from_millis(50),
        tokio::task::spawn_blocking(move || waiter_service.wait_termination()),
    )
    .await;

    assert!(termination_result.is_err());
    release_tx
        .send(())
        .expect("blocking task should receive release signal");
    blocked_handle
        .await
        .expect("blocked task should complete successfully");
    service.wait_termination();
    assert!(service.is_terminated());
}
