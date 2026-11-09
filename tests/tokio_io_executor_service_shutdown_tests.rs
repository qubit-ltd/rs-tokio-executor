// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::mpsc;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use qubit_executor::SubmissionError;
use qubit_executor::TaskExecutionError;
use qubit_executor::service::ExecutorServiceLifecycle;
use qubit_tokio_executor::TokioIoExecutorService;

struct DropProbe {
    polled: mpsc::Sender<()>,
    entered: mpsc::Sender<()>,
    drop_gate: Arc<TestGate>,
}

impl Future for DropProbe {
    type Output = Result<(), io::Error>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _ = self.polled.send(());
        Poll::Pending
    }
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        let _ = self.entered.send(());
        // The test thread releases this after observing service accounting.
        self.drop_gate.wait();
    }
}

struct TestGate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl TestGate {
    fn new() -> Self {
        Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn wait(&self) {
        let open = self.open.lock().expect("gate mutex should not be poisoned");
        let (open, result) = self
            .changed
            .wait_timeout_while(open, Duration::from_secs(5), |open| !*open)
            .expect("gate mutex should not be poisoned");
        assert!(*open || !result.timed_out(), "test gate should be released");
    }

    fn release(&self) {
        *self.open.lock().expect("gate mutex should not be poisoned") = true;
        self.changed.notify_all();
    }
}

struct DropGateRelease(Arc<TestGate>);

impl DropGateRelease {
    fn release(&mut self) {
        self.0.release();
    }
}

impl Drop for DropGateRelease {
    fn drop(&mut self) {
        self.release();
    }
}

fn drop_probe(gate: Arc<TestGate>) -> (DropProbe, mpsc::Receiver<()>, mpsc::Receiver<()>) {
    let (polled_tx, polled_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    (
        DropProbe {
            polled: polled_tx,
            entered: entered_tx,
            drop_gate: gate,
        },
        polled_rx,
        entered_rx,
    )
}

fn receive_signal(receiver: mpsc::Receiver<()>, message: &'static str) {
    receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("{message}"));
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_rejects_new_tasks() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    service.shutdown();

    let result = service.spawn(async { Ok::<(), io::Error>(()) });

    assert!(matches!(result, Err(SubmissionError::Shutdown)));
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_waits_for_task_handles() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    let handle = service
        .spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    service.shutdown();
    handle.await.expect("task should complete successfully");
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_await_termination_waits_for_task_completion() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = service
        .spawn(async move {
            release_rx.await.expect("release signal should arrive");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");
    service.shutdown();

    let waiting_service = service.clone();
    let waiter = tokio::spawn(async move {
        waiting_service.await_termination().await;
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    release_tx.send(()).expect("waiting task should receive release signal");
    handle.await.expect("task should complete successfully");
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("termination waiter should be notified")
        .expect("termination waiter should not panic");
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_aborts_running_task_handle() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();

    let handle = service
        .spawn(async move {
            started_tx.send(()).expect("start should be observed");
            std::future::pending::<Result<(), io::Error>>().await
        })
        .expect("service should accept task");

    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("task should start")
        .expect("start signal should arrive");
    assert_eq!(service.stats().accepted_unfinished, 1);

    let stop_service = service.clone();
    let (report_tx, report_rx) = mpsc::channel();
    let stop_thread = std::thread::spawn(move || {
        let report = stop_service.stop();
        report_tx.send(report).expect("test thread should receive stop report");
    });
    let report = report_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("stop should return a report");
    stop_thread.join().expect("stop thread should not panic");

    assert_eq!(report.running, 1);
    assert_eq!(report.cancelled, 1);
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("cancelled task should resolve");
    assert!(matches!(result, Err(TaskExecutionError::Cancelled)));
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate");
    assert_eq!(service.stats().accepted_unfinished, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_ignores_completed_tasks() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept task")
        .await
        .expect("task should complete successfully");

    let report = service.stop();

    assert_eq!(report.running, 0);
    assert_eq!(report.cancelled, 0);
    assert!(service.is_not_running());
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_stop_sets_stopping_while_task_runs() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = service
        .spawn(async move {
            started_tx.send(()).expect("start should be observed");
            release_rx.await.expect("release signal should arrive");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept future");
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("task should start")
        .expect("start should be signalled");

    let report = service.stop();

    assert_eq!(report.running, 1);
    assert_eq!(report.cancelled, 1);
    assert!(service.is_stopping() || service.is_terminated());
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("cancelled task should resolve");
    assert!(matches!(result, Err(TaskExecutionError::Cancelled)));
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate");
    assert_eq!(service.stats().accepted_unfinished, 0);
    assert!(service.is_terminated());
    drop(release_tx);
}

#[tokio::test]
async fn test_tokio_io_executor_service_completion_keeps_waiting_for_remaining_tasks() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();

    let pending_handle = service
        .spawn(async move {
            release_rx.await.expect("pending task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept pending task");

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept quick task")
        .await
        .expect("quick task should complete successfully");

    service.shutdown();

    assert!(!service.is_terminated());
    release_tx.send(()).expect("pending task should receive release signal");
    pending_handle.await.expect("pending task should complete successfully");
    assert!(service.is_terminated());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tokio_io_executor_service_stop_drops_pending_future_before_termination() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let gate = Arc::new(TestGate::new());
    let mut release = DropGateRelease(Arc::clone(&gate));
    let (probe, polled, entered) = drop_probe(gate);
    let handle = service.spawn(probe).expect("service should accept probe");

    receive_signal(polled, "probe should be polled before cancellation");
    service.shutdown();
    assert!(service.is_shutting_down());
    let report = service.stop();
    assert_eq!(report.running, 1);
    assert_eq!(report.cancelled, 1);
    receive_signal(entered, "probe destructor should enter after cancellation");

    // The destructor is known to be blocked here, so this is an ordering
    // observation rather than a timing-based guess.
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert!(!service.is_terminated());
    assert!(service.is_stopping());

    release.release();
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    assert_eq!(service.stats().accepted_unfinished, 0);
    assert!(service.is_terminated());
}

#[tokio::test]
async fn test_tokio_io_executor_service_guarded_future_preserves_panic_result() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let handle = service
        .spawn(async {
            panic!("probe panic should be reported by the task handle");
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        })
        .expect("service should accept task");

    assert!(matches!(handle.await, Err(TaskExecutionError::Panicked)));
    service.shutdown();
    assert_eq!(service.stats().accepted_unfinished, 0);
    assert!(service.is_terminated());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tokio_io_executor_service_cancel_drops_unpolled_future_before_termination() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());
    let blocker_gate = Arc::new(TestGate::new());
    let mut release_blocker = DropGateRelease(Arc::clone(&blocker_gate));
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let mut blockers = Vec::new();
    for _ in 0..2 {
        let started = blocker_started_tx.clone();
        let worker_gate = Arc::clone(&blocker_gate);
        blockers.push(tokio::spawn(async move {
            let _ = started.send(());
            worker_gate.wait();
        }));
    }
    drop(blocker_started_tx);
    for _ in 0..2 {
        blocker_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("both blockers should occupy the runtime workers");
    }

    // Both workers are blocked, so the submitted task cannot be polled before
    // service.stop() requests cancellation.
    let drop_gate = Arc::new(TestGate::new());
    let mut release_drop = DropGateRelease(Arc::clone(&drop_gate));
    let (probe, polled, entered) = drop_probe(drop_gate);
    let handle = service.spawn(probe).expect("service should accept probe");
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Running);
    let report = service.stop();
    assert_eq!(report.running, 1);
    assert_eq!(report.cancelled, 1);
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Stopping);
    service.shutdown();
    assert_eq!(service.lifecycle(), ExecutorServiceLifecycle::Stopping);

    release_blocker.release();
    receive_signal(entered, "unpolled probe destructor should enter");
    assert_eq!(service.stats().accepted_unfinished, 1);
    assert!(!service.is_terminated());
    assert!(service.is_stopping());

    release_drop.release();
    assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
    assert!(polled.try_recv().is_err(), "cancelled probe must remain unpolled");
    for blocker in blockers {
        blocker.await.expect("worker blocker should finish");
    }
    assert_eq!(service.stats().accepted_unfinished, 0);
    assert!(service.is_terminated());
}
