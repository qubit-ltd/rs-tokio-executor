// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;

use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioBlockingExecutorService;

#[tokio::test]
async fn test_tokio_blocking_task_handle_into_future_returns_result() {
    let service = TokioBlockingExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .submit_tracked_callable(|| Ok::<usize, io::Error>(42))
        .expect("service should accept tracked callable");

    assert_eq!(handle.await.expect("task should finish"), 42);
    service.shutdown();
    service.wait_termination();
}
