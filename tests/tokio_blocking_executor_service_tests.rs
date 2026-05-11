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

use std::io;

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
