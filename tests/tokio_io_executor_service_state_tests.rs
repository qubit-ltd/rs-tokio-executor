// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::time::Duration;

use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_executor_service_state_tracks_abort_and_termination() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .spawn(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<_, &'static str>(())
        })
        .unwrap();

    let report = service.stop();

    assert!(report.running >= 1, "stopping should report the running task");
    assert!(report.cancelled >= 1, "stopping should report the cancelled task");
    assert!(handle.await.is_err(), "the aborted task should fail to join");
    assert!(service.is_not_running(), "the stopped service should not run");
    assert!(service.is_terminated(), "the stopped service should terminate");
}
