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
use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutorService;

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
}

fn ok_usize_task() -> Result<usize, io::Error> {
    Ok(42)
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
