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

use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutorService;

#[test]
fn test_tokio_task_handle_cancel_requests_abort_for_queued_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
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

        let service = TokioExecutorService::new();
        let handle = service
            .submit(|| Ok::<(), io::Error>(()))
            .expect("service should accept queued task");

        assert!(handle.cancel());
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle.is_done() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled task should finish");
        assert!(handle.is_done());
        assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
        service.shutdown();
        service.await_termination().await;
    });
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
