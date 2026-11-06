// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::CancelResult;
use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;
use qubit_tokio_executor::TokioIoExecutorService;

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Releases a blocking test task even while unwinding an assertion.
struct ReleaseOnDrop(Option<mpsc::Sender<()>>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

#[tokio::test]
async fn dropping_blocking_handle_does_not_release_capacity_or_cancel_work() {
    let capacity = NonZeroUsize::new(1).expect("capacity should be nonzero");
    let service =
        TokioExecutorService::with_task_capacity(tokio::runtime::Handle::current(), capacity);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let ran_tx = started_tx.clone();

    let handle = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            ran_tx
                .send(())
                .expect("test should observe task completion");
            Ok::<(), io::Error>(())
        })
        .expect("first task should be accepted");
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking closure should start");
    drop(handle);

    assert!(matches!(
        service.submit(|| Ok::<(), io::Error>(())),
        Err(SubmissionError::Saturated)
    ));
    assert_eq!(service.stats().running, 1);

    release
        .0
        .take()
        .expect("release sender should exist")
        .send(())
        .expect("task should be released");
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("task should finish after release");
    let mut changes = service.capacity_changes();
    tokio::time::timeout(OBSERVE_TIMEOUT, async {
        loop {
            if service.stats().running == 0 {
                break;
            }
            changes
                .changed()
                .await
                .expect("service should publish a change");
        }
    })
    .await
    .expect("finished task should release its capacity");
    service
        .submit(|| Ok::<(), io::Error>(()))
        .expect("capacity should be available again");
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn dropping_io_handle_detaches_future_until_it_completes() {
    let capacity = NonZeroUsize::new(1).expect("capacity should be nonzero");
    let service =
        TokioIoExecutorService::with_task_capacity(tokio::runtime::Handle::current(), capacity);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_by_task = std::sync::Arc::clone(&completed);

    let handle = service
        .spawn(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
            completed_by_task.store(true, std::sync::atomic::Ordering::Release);
            Ok::<(), io::Error>(())
        })
        .expect("first future should be accepted");
    tokio::time::timeout(OBSERVE_TIMEOUT, started_rx)
        .await
        .expect("future should start")
        .expect("started signal should arrive");
    drop(handle);

    assert_eq!(service.stats().accepted_unfinished, 1);
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));
    release_tx
        .send(())
        .expect("detached future should be released");
    tokio::time::timeout(OBSERVE_TIMEOUT, async {
        while service.stats().accepted_unfinished != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("future completion should release capacity");
    assert!(completed.load(std::sync::atomic::Ordering::Acquire));
    let next = service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("capacity should be available again");
    next.await.expect("second future should finish");
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn timing_out_while_waiting_preserves_io_handle_for_explicit_cancel() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let handle = service
        .spawn(async move {
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("pending future should be accepted");
    tokio::time::timeout(OBSERVE_TIMEOUT, started_rx)
        .await
        .expect("future should start")
        .expect("started signal should arrive");

    let mut handle = handle;
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut handle)
            .await
            .is_err()
    );
    assert!(
        !handle.is_done(),
        "timeout should drop only the wait, not the task"
    );
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert_eq!(handle.cancel(), CancelResult::Cancelled);
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    service.shutdown();
    service.await_termination().await;
    assert_eq!(service.stats().accepted_unfinished, 0);
}

#[tokio::test]
async fn io_service_stop_cancels_accepted_pending_future() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let handle = service
        .spawn(async move {
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("pending future should be accepted");
    tokio::time::timeout(OBSERVE_TIMEOUT, started_rx)
        .await
        .expect("future should start")
        .expect("started signal should arrive");

    let report = service.stop();
    assert_eq!(report.cancelled, 1);
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    service.await_termination().await;
    assert!(service.is_terminated());
    assert_eq!(service.stats().accepted_unfinished, 0);
}

#[tokio::test]
async fn stopping_blocking_service_does_not_cancel_started_closure() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let handle = service
        .submit_tracked_callable(move || {
            started_tx.send(()).expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            Ok::<usize, io::Error>(42)
        })
        .expect("service should accept task");
    started_rx
        .recv_timeout(OBSERVE_TIMEOUT)
        .expect("blocking closure should start");

    let report = service.stop();
    assert_eq!(report.running, 1);
    assert_eq!(report.cancelled, 0);
    assert_eq!(handle.cancel(), CancelResult::AlreadyRunning);
    assert!(!service.wait_termination_timeout(Duration::from_millis(1)));

    release
        .0
        .take()
        .expect("release sender should exist")
        .send(())
        .expect("running closure should be released");
    assert_eq!(handle.await.expect("started closure should finish"), 42);
    assert!(service.wait_termination_timeout(OBSERVE_TIMEOUT));
    assert!(service.is_terminated());
}
