// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::service::ExecutorService;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_tokio_executor::TokioExecutorService;

#[tokio::test]
async fn test_tokio_executor_service_state_tracks_shutdown_and_active_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handle = service
        .submit_tracked(move || {
            started_tx.send(()).expect("test should receive task start signal");
            release_rx.recv().expect("test should release the blocking task");
            Ok::<_, io::Error>(())
        })
        .expect("service should accept blocking task");

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Running);
    service.shutdown();
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::ShuttingDown);
    assert!(service.is_not_running());
    assert!(!service.is_terminated());
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("task result should arrive within five seconds after release")
        .expect("released blocking task should succeed");
    assert!(
        service.wait_termination_timeout(Duration::from_secs(5)),
        "service should terminate within five seconds after task completion"
    );
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Terminated);
    assert!(service.is_terminated());
}
