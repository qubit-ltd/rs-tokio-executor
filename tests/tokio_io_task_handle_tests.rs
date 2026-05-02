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
