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
