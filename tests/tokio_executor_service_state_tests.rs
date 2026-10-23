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

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocking task should start");
    service.shutdown();
    assert!(
        service.is_not_running(),
        "service should stop accepting tasks after shutdown"
    );
    assert!(
        !service.is_terminated(),
        "service should wait for its active task to finish"
    );
    release_tx
        .send(())
        .expect("blocking task should receive its release signal");
    handle.await.expect("blocking task should complete successfully");
    service.wait_termination();
    assert!(
        service.is_terminated(),
        "service should terminate after its active task finishes"
    );
}
