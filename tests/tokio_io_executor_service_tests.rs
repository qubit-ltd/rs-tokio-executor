// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::pending;
use std::io::Error;
use std::num::NonZeroUsize;
use std::time::Duration;

use qubit_executor::SubmissionError;
use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_tokio_executor::TokioIoExecutorService;
use tokio::runtime::Handle;
use tokio::runtime::Runtime;
use tokio::time::timeout;

#[tokio::test]
async fn test_tokio_io_executor_service_spawns_future_and_rejects_after_shutdown() {
    let service = TokioIoExecutorService::new(Handle::current());
    let handle = service
        .spawn(async { Ok::<_, &'static str>(5usize) })
        .expect("io service should accept future");

    assert_eq!(5, handle.await.expect("future should complete successfully"));

    service.shutdown();
    assert!(matches!(
        service.spawn(async { Ok::<_, &'static str>(()) }),
        Err(SubmissionError::Shutdown),
    ));
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_lifecycle_accessors() {
    let service = TokioIoExecutorService::new(Handle::current());

    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Running);
    assert!(service.is_running());
    assert!(!service.is_shutting_down());
    assert!(!service.is_stopping());
    assert!(!service.is_not_running());
    assert!(!service.is_terminated());

    service.shutdown();

    assert!(service.is_not_running());
    assert!(service.is_terminated());
    assert!(!service.is_running());
}

#[test]
fn test_tokio_io_executor_service_spawn_uses_explicit_runtime_handle() {
    let runtime = Runtime::new().expect("runtime should build");
    let service = TokioIoExecutorService::new(runtime.handle().clone());
    let handle = service
        .spawn(async { Ok::<usize, Error>(42) })
        .expect("submission should succeed");
    assert_eq!(runtime.block_on(handle).expect("task should succeed"), 42);
    service.shutdown();
}

#[tokio::test]
async fn test_tokio_io_executor_service_stats_and_capacity_changes() {
    let capacity = NonZeroUsize::new(1).expect("capacity is nonzero");
    let service = TokioIoExecutorService::with_task_capacity(Handle::current(), capacity);
    let handle = service
        .spawn(async {
            pending::<()>().await;
            Ok::<(), Error>(())
        })
        .expect("one pending future should be accepted");
    let mut changes = service.capacity_changes();

    let stats = service.stats();
    assert_eq!(stats.task_capacity, 1);
    assert_eq!(stats.accepted_unfinished, 1);
    assert!(matches!(
        service.spawn(async { Ok::<(), Error>(()) }),
        Err(SubmissionError::Saturated)
    ));

    let report = service.stop();
    assert_eq!(report.cancelled, 1);
    timeout(Duration::from_secs(1), changes.changed())
        .await
        .expect("stop should publish a capacity change")
        .expect("capacity notification sender should remain open");
    let result = timeout(Duration::from_secs(5), handle)
        .await
        .expect("stopped IO task should report cancellation within five seconds");
    assert!(matches!(result, Err(TaskExecutionError::Cancelled)));
    timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("IO service should terminate within five seconds after cancellation");
    assert_eq!(service.stats().accepted_unfinished, 0);
}
