// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Tests for the generic Tokio blocking cancellation endpoint.

use std::future::IntoFuture;
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::CancelResult;
use qubit_executor::CancellableTaskHandle;
use qubit_executor::TaskExecutionError;
use qubit_executor::TaskResult;
use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioExecutorService;

async fn cancel_after_timeout<H>(task: H)
where
    H: CancellableTaskHandle<usize, io::Error> + IntoFuture<Output = TaskResult<usize, io::Error>>,
{
    let cancellation = CancellableTaskHandle::cancellation_handle(&task);
    let mut result = Box::pin(task.into_future());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), result.as_mut())
            .await
            .is_err()
    );
    assert_eq!(cancellation.cancel(), CancelResult::Cancelled);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), result)
            .await
            .expect("cancelled result should arrive"),
        Err(TaskExecutionError::Cancelled)
    ));
}

#[test]
fn test_generic_cancellation_releases_queued_blocking_task_capacity() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async {
        let service = TokioExecutorService::with_task_capacity(
            runtime.handle().clone(),
            NonZeroUsize::new(1).expect("capacity should be nonzero"),
        );
        let (blocker_started_tx, blocker_started_rx) = tokio::sync::oneshot::channel();
        let (release_blocker_tx, release_blocker_rx) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker start should be observed");
            release_blocker_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("blocker should be released within ten seconds");
        });
        tokio::time::timeout(Duration::from_secs(2), blocker_started_rx)
            .await
            .expect("blocking worker should start")
            .expect("start signal should be sent");

        let ran = Arc::new(AtomicBool::new(false));
        let ran_by_task = Arc::clone(&ran);
        let task = service
            .submit_tracked_callable(move || {
                ran_by_task.store(true, Ordering::SeqCst);
                Ok::<usize, io::Error>(7)
            })
            .expect("service should accept queued task");

        cancel_after_timeout(task).await;

        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(service.stats().queued, 0);
        let replacement = service
            .submit_tracked_callable(|| Ok::<usize, io::Error>(42))
            .expect("cancellation should release service capacity");

        release_blocker_tx.send(()).expect("blocking worker should be released");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), replacement)
                .await
                .expect("replacement task should finish")
                .expect("replacement task should succeed"),
            42
        );
        tokio::time::timeout(Duration::from_secs(2), blocker)
            .await
            .expect("blocker should finish")
            .expect("blocker task should succeed");
        service.shutdown();
        service.wait_termination();
    });
}
