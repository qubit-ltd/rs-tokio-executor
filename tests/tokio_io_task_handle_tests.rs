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

use qubit_executor::{
    CancelResult,
    TaskExecutionError,
};
use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_task_handle_cancel_requests_abort() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    assert_eq!(CancelResult::Cancelled, handle.cancel());
    tokio::task::yield_now().await;
    assert!(handle.is_done());
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    service.shutdown();
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_task_handle_cancel_reports_already_finished() {
    let service = TokioIoExecutorService::new();

    let handle = service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept task");
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !handle.is_done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task should finish promptly");

    assert_eq!(CancelResult::AlreadyFinished, handle.cancel());
    handle
        .await
        .expect("finished task should still report success");
    service.shutdown();
    assert!(service.is_terminated());
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
    assert!(service.is_terminated());
}
