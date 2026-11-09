// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::io;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
}

struct ReleaseOnDrop(Option<Sender<()>>);

impl ReleaseOnDrop {
    fn release(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.release();
    }
}

#[tokio::test]
async fn test_tokio_executor_service_shutdown_rejects_new_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    service.shutdown();

    let result = service.submit(ok_unit_task as fn() -> Result<(), io::Error>);

    assert!(matches!(result, Err(SubmissionError::Shutdown)));
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_wait_termination_waits_for_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));

    let handle = service
        .submit_tracked(move || {
            started_tx.send(()).expect("test should receive task start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("task should start");
    service.shutdown();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let waiter_service = service.clone();
    let waiter = std::thread::spawn(move || {
        entered_tx.send(()).expect("test should receive waiter entry signal");
        assert!(waiter_service.wait_termination_timeout(Duration::from_secs(5)));
        completed_tx.send(()).expect("test should receive waiter completion");
    });
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("waiter should enter before the completion check");
    assert!(matches!(
        completed_rx.recv_timeout(Duration::from_millis(25)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    release.release();
    completed_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("waiter should finish after task release");
    waiter.join().expect("waiter thread should finish");

    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_await_termination_waits_for_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));

    let handle = service
        .submit_tracked(move || {
            started_tx.send(()).expect("test should receive task start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("task should start");
    service.shutdown();
    let termination = service.await_termination();
    tokio::pin!(termination);
    let first_poll = std::future::poll_fn(|context| std::task::Poll::Ready(termination.as_mut().poll(context))).await;
    assert!(first_poll.is_pending());

    release.release();
    tokio::time::timeout(Duration::from_secs(5), termination)
        .await
        .expect("termination should be observed within 5 seconds after releasing the task");

    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_wait_termination_timeout_handles_max_duration() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    service.shutdown();
    assert!(service.wait_termination_timeout(Duration::from_secs(5)));

    assert!(service.wait_termination_timeout(Duration::MAX));
}

#[tokio::test]
async fn test_tokio_executor_service_wait_termination_timeout_reports_pending() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));

    let handle = service
        .submit_tracked(move || {
            started_tx.send(()).expect("test should receive task start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("task should start");
    service.shutdown();
    assert!(!service.wait_termination_timeout(Duration::from_millis(1)));
    release.release();
    assert!(service.wait_termination_timeout(Duration::from_secs(5)));
    handle.await.expect("task should complete successfully");
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_stop_reports_no_cancelled_running_blocking_task() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));

    let handle = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should receive blocking start signal");
            release_rx.recv().expect("blocking task should receive release signal");
            Ok::<usize, io::Error>(42)
        })
        .expect("service should accept task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocking task should start");
    let report = service.stop();
    let waiter_service = service.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        entered_tx.send(()).expect("observe waiter entry");
        assert!(waiter_service.wait_termination_timeout(Duration::from_secs(5)));
        completed_tx.send(()).expect("observe waiter completion");
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("waiter enters");
    assert!(matches!(
        completed_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.release();
    completed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("waiter completes after task release");
    waiter.join().expect("waiter thread should finish");

    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
    assert_eq!(handle.await.expect("running blocking task should finish"), 42);
}

#[test]
fn test_tokio_executor_service_stop_cancels_queued_detached_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::new(tokio::runtime::Handle::current());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).expect("test should receive blocking start signal");
            release_rx.recv().expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        service
            .submit(ok_unit_task as fn() -> Result<(), io::Error>)
            .expect("service should accept queued detached task");
        let report = service.stop();
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");

        assert_eq!(report.queued, 1);
        assert_eq!(report.running, 0);
        assert_eq!(report.cancelled, 1);
        assert!(service.is_not_running());
    });
}

#[test]
fn test_tokio_executor_service_stop_cancels_queued_tracked_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::new(tokio::runtime::Handle::current());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).expect("test should receive blocking start signal");
            release_rx.recv().expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        let handle = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("service should accept queued task");
        let report = service.stop();
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");
        assert!(service.wait_termination_timeout(Duration::from_secs(5)));

        assert_eq!(report.queued, 1);
        assert_eq!(report.running, 0);
        assert_eq!(report.cancelled, 1);
        assert!(service.is_not_running());
        assert!(service.is_terminated());
        assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    });
}

#[tokio::test]
async fn test_tokio_executor_service_stop_ignores_completed_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());

    service
        .submit_tracked(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept task")
        .await
        .expect("task should complete successfully");
    service.shutdown();
    assert!(service.wait_termination_timeout(Duration::from_secs(5)));

    let report = service.stop();

    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_executor_service_completion_keeps_waiting_for_remaining_tasks() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));

    let blocked_handle = service
        .submit_tracked(move || {
            started_tx.send(()).expect("test should receive blocking start signal");
            release_rx.recv().expect("blocking task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept blocked task");

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("blocking task should start");
    service
        .submit_tracked(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept quick task")
        .await
        .expect("quick task should complete successfully");

    service.shutdown();
    let waiter_service = service.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        entered_tx.send(()).expect("observe waiter entry");
        assert!(waiter_service.wait_termination_timeout(Duration::from_secs(5)));
        completed_tx.send(()).expect("observe waiter completion");
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("waiter enters");
    assert!(matches!(
        completed_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.release();
    completed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("waiter completes after task release");
    waiter.join().expect("waiter thread should finish");
    blocked_handle.await.expect("blocked task should complete successfully");
    assert!(service.is_terminated());
}
