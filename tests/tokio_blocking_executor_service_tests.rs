/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
//! Smoke tests for [`TokioBlockingExecutorService`](qubit_tokio_executor::service::TokioBlockingExecutorService).

use std::{
    io,
    sync::mpsc,
    time::Duration,
};

use qubit_executor::{
    CancelResult,
    TaskExecutionError,
    TaskStatus,
    TryGet,
    task::spi::TrackedTaskHandle,
};
use qubit_tokio_executor::service::{
    ExecutorService,
    TokioBlockingExecutorService,
};

#[tokio::test]
async fn test_tokio_blocking_executor_service_alias_runs_blocking_callable() {
    let service = TokioBlockingExecutorService::new();

    let handle = service
        .submit_callable(|| Ok::<usize, io::Error>(42))
        .expect("service should accept callable");

    assert_eq!(
        handle.await.expect("callable should complete successfully"),
        42
    );
    service.shutdown();
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_blocking_task_handle_get_returns_completed_value() {
    let service = TokioBlockingExecutorService::new();

    let handle = service
        .submit_tracked_callable(|| Ok::<usize, io::Error>(42))
        .expect("service should accept tracked callable");

    assert_eq!(handle.get().expect("tracked task should finish"), 42);
    service.shutdown();
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_blocking_task_handle_try_get_reports_pending_then_ready() {
    let service = TokioBlockingExecutorService::new();
    let (release_tx, release_rx) = mpsc::channel();

    let handle = service
        .submit_tracked_callable(move || {
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
            Ok::<usize, io::Error>(7)
        })
        .expect("service should accept tracked callable");

    let mut handle = match handle.try_get() {
        TryGet::Pending(handle) => handle,
        TryGet::Ready(_) => panic!("blocked task result should not be ready"),
    };
    release_tx
        .send(())
        .expect("blocking task should receive release signal");

    loop {
        match handle.try_get() {
            TryGet::Ready(result) => {
                assert_eq!(result.expect("tracked task should finish"), 7);
                break;
            }
            TryGet::Pending(pending_handle) => {
                handle = pending_handle;
                tokio::task::yield_now().await;
            }
        }
    }

    service.shutdown();
    service.wait_termination();
}

#[test]
fn test_tokio_blocking_task_handle_status_and_tracked_trait_cancel() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let service = TokioBlockingExecutorService::new();
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
            .submit_tracked_callable(|| Ok::<usize, io::Error>(13))
            .expect("service should accept queued tracked callable");

        assert_eq!(handle.status(), TaskStatus::Pending);
        assert!(!handle.is_done());
        assert_eq!(TrackedTaskHandle::status(&handle), TaskStatus::Pending);
        assert_eq!(TrackedTaskHandle::cancel(&handle), CancelResult::Cancelled);
        assert_eq!(handle.status(), TaskStatus::Cancelled);
        assert!(handle.is_done());
        assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));

        service.shutdown();
        service.wait_termination();
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");
    });
}
