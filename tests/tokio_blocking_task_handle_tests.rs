// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Tests for Tokio blocking task handle future behavior.

use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::CancelResult;
use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorService;
use qubit_tokio_executor::TokioBlockingExecutorService;
use qubit_tokio_executor::TokioExecutorService;

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

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

#[test]
fn test_cancellation_handle_cancels_queued_task_after_into_future_and_releases_capacity() {
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
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (release_blocker_tx, release_blocker_rx) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker start should be observed");
            release_blocker_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("blocker should be released within ten seconds");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("blocking worker should start");

        let ran = Arc::new(AtomicBool::new(false));
        let ran_by_task = Arc::clone(&ran);
        let task = service
            .submit_tracked_callable(move || {
                ran_by_task.store(true, Ordering::SeqCst);
                Ok::<usize, io::Error>(7)
            })
            .expect("service should accept queued task");
        let cancellation = task.cancellation_handle();
        let cancellation_clone = cancellation.clone();
        let result_future = task.into_future();

        assert_eq!(cancellation.cancel(), CancelResult::Cancelled);
        assert_eq!(cancellation_clone.cancel(), CancelResult::AlreadyFinished);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), result_future)
                .await
                .expect("cancelled result should arrive"),
            Err(TaskExecutionError::Cancelled)
        ));
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(service.stats().queued, 0);

        let replacement = service
            .submit_tracked_callable(|| Ok::<usize, io::Error>(42))
            .expect("cancellation should eventually release capacity");
        release_blocker_tx.send(()).expect("worker should be released");
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

#[tokio::test]
async fn test_cancellation_handle_does_not_interrupt_running_blocking_task() {
    let service = TokioBlockingExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let (release_tx, release_rx) = mpsc::channel();
    let started_tx_for_task = Arc::clone(&started_tx);
    let task = service
        .submit_tracked_callable(move || {
            started_tx_for_task
                .lock()
                .expect("start signal lock should not poison")
                .take()
                .expect("start signal should be available")
                .send(())
                .expect("start should be observed");
            release_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("running task should be released within ten seconds");
            Ok::<usize, io::Error>(9)
        })
        .expect("service should accept task");
    let cancellation = task.cancellation_handle();
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .expect("task should start")
        .expect("start signal should be sent");

    assert_eq!(cancellation.cancel(), CancelResult::AlreadyRunning);
    release_tx.send(()).expect("running task should be released");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("running task should complete")
            .expect("running task should succeed"),
        9
    );
    service.shutdown();
    service.wait_termination();
}

#[test]
fn test_dropping_cancellation_handle_does_not_cancel_queued_task() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime.block_on(async {
        let service = TokioBlockingExecutorService::new(runtime.handle().clone());
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker start should be observed");
            release_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("blocker should be released within ten seconds");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("blocking worker should start");

        let task = service
            .submit_tracked_callable(|| Ok::<usize, io::Error>(11))
            .expect("service should accept queued task");
        drop(task.cancellation_handle());
        release_tx.send(()).expect("blocker should be released");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .expect("queued task should finish")
                .expect("queued task should succeed"),
            11
        );
        service.shutdown();
        service.wait_termination();
        blocker.await.expect("blocker should finish");
    });
}

#[test]
fn test_cancelled_queued_closure_is_eventually_destroyed() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime.block_on(async {
        let service = TokioBlockingExecutorService::new(runtime.handle().clone());
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker start should be observed");
            release_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("blocker should be released within ten seconds");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("blocking worker should start");

        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let drop_signal = std::sync::Mutex::new(Some(DropSignal(Some(dropped_tx))));
        let task = service
            .submit_tracked_callable(move || {
                let _drop_signal = drop_signal.lock().expect("drop signal lock should not poison").take();
                Ok::<(), io::Error>(())
            })
            .expect("service should accept queued task");
        let cancellation = task.cancellation_handle();
        let result_future = task.into_future();
        assert_eq!(cancellation.cancel(), CancelResult::Cancelled);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), result_future)
                .await
                .expect("cancelled result should arrive"),
            Err(TaskExecutionError::Cancelled)
        ));
        release_tx.send(()).expect("blocker should be released");
        tokio::time::timeout(Duration::from_secs(2), dropped_rx)
            .await
            .expect("cancelled closure should eventually be destroyed")
            .expect("drop notification should be sent");
        service.shutdown();
        service.wait_termination();
        blocker.await.expect("blocker should finish");
    });
}
