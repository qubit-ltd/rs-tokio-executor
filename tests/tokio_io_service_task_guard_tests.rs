// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Tests for Tokio I/O executor service task guard behavior.

use qubit_tokio_executor::TokioIoExecutorService;
use tokio::runtime::Handle;

#[tokio::test]
async fn test_tokio_io_service_task_guard_notifies_termination_when_future_finishes() {
    let service = TokioIoExecutorService::new(Handle::current());
    let handle = service
        .spawn(async { Ok::<_, &'static str>("ok") })
        .expect("service should accept the task");

    service.shutdown();
    assert_eq!("ok", handle.await.unwrap());
    assert!(service.is_terminated());
}
