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

use qubit_executor::TaskExecutionError;
use qubit_executor::TaskHandle;
use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioExecutorService;

struct ReleaseOnDrop(Option<mpsc::Sender<()>>);

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

fn ok_unit_task() -> Result<(), io::Error> {
    Ok(())
}

fn ok_usize_task() -> Result<usize, io::Error> {
    Ok(42)
}

#[tokio::test]
async fn test_tokio_executor_service_submit_acceptance_is_not_task_success() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());

    service
        .submit(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("service should accept the shared runnable");

    let handle = service
        .submit_tracked(|| Err::<(), _>(io::Error::other("task failed")))
        .expect("service should accept the runnable");

    let err = handle
        .await
        .expect_err("accepted runnable should report task failure through handle");
    assert!(matches!(err, TaskExecutionError::Failed(_)));
}

#[tokio::test]
async fn test_tokio_executor_service_submit_callable_returns_value() {
    let service = TokioExecutorService::new(tokio::runtime::Handle::current());

    let handle = service
        .submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>)
        .expect("service should accept the callable");

    assert_eq!(handle.await.expect("callable should complete successfully"), 42,);
}

#[tokio::test]
async fn test_tokio_executor_service_capacity_bounds_and_reuses_completed_slots() {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first: TaskHandle<(), io::Error> = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should receive start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first task should start");

    assert!(matches!(
        service.submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>),
        Err(SubmissionError::Saturated)
    ));

    release_tx.send(()).expect("first task should be released");
    tokio::task::spawn_blocking(move || first.get())
        .await
        .expect("result waiter should join")
        .expect("first task should finish");
    let replacement: TaskHandle<usize, io::Error> = service
        .submit_callable(ok_usize_task as fn() -> Result<usize, io::Error>)
        .expect("completed task should release its capacity slot");
    assert_eq!(
        tokio::task::spawn_blocking(move || replacement.get())
            .await
            .expect("result waiter should join")
            .expect("replacement task should complete"),
        42
    );
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_executor_service_shutdown_precedes_capacity_error() {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should receive start signal");
            release_rx.recv().expect("task should receive release signal");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first task should start");
    assert!(matches!(
        service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
        Err(SubmissionError::Saturated)
    ));

    service.shutdown();
    assert!(matches!(
        service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
        Err(SubmissionError::Shutdown)
    ));
    release_tx.send(()).expect("first task should be released");
    tokio::task::spawn_blocking(move || first.get())
        .await
        .expect("result waiter should join")
        .expect("first task should finish");
    service.await_termination().await;
}

#[test]
fn test_tokio_executor_service_capacity_reuses_slot_after_queued_cancel() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should be created");

    runtime.block_on(async {
        let service = TokioExecutorService::with_task_capacity(
            tokio::runtime::Handle::current(),
            NonZeroUsize::new(1).expect("capacity should be nonzero"),
        );
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (blocker_release_tx, blocker_release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx.send(()).expect("blocker should start");
            blocker_release_rx.recv().expect("blocker should be released");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking worker should be occupied");

        let cancelled = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("service should accept queued task");
        assert!(matches!(
            service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
            Err(SubmissionError::Saturated)
        ));
        assert_eq!(cancelled.cancel(), qubit_executor::CancelResult::Cancelled);
        assert!(matches!(cancelled.await, Err(TaskExecutionError::Cancelled)));

        let replacement = service
            .submit_tracked_callable(|| Ok::<(), io::Error>(()))
            .expect("queued cancellation should release its capacity slot");
        assert_eq!(replacement.cancel(), qubit_executor::CancelResult::Cancelled);
        assert!(matches!(replacement.await, Err(TaskExecutionError::Cancelled)));

        blocker_release_tx.send(()).expect("blocker should be released");
        blocker.await.expect("blocker should complete");
        service.shutdown();
        service.await_termination().await;
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_admission_reserves_only_one_slot() {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let barrier = Arc::new(Barrier::new(3));
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let (results_tx, results_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..2 {
        let service = service.clone();
        let barrier = Arc::clone(&barrier);
        let results_tx = results_tx.clone();
        let release_rx = std::sync::Arc::clone(&release_rx);
        workers.push(thread::spawn(move || {
            barrier.wait();
            let result = service.submit_callable(move || {
                release_rx
                    .lock()
                    .expect("release lock")
                    .take()
                    .expect("accepted task takes release signal")
                    .recv()
                    .expect("test must release accepted task");
                Ok::<(), io::Error>(())
            });
            results_tx.send(result).expect("test should collect result");
        }));
    }
    drop(results_tx);
    barrier.wait();
    let first = results_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first submit should finish");
    let second = results_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("second submit should finish");
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(SubmissionError::Saturated)))
            .count(),
        1
    );
    assert_eq!((service.stats().queued + service.stats().running), 1);
    release.release();
    for worker in workers {
        worker.join().expect("submitter should exit");
    }
    for handle in outcomes.into_iter().flatten() {
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("accepted task should finish in time")
            .expect("accepted task should succeed");
    }
    service.shutdown();
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate in time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_submission_races_with_shutdown() {
    run_submission_race_with_close(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_submission_races_with_stop() {
    run_submission_race_with_close(true).await;
}

async fn run_submission_race_with_close(stop: bool) {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let barrier = Arc::new(Barrier::new(3));
    let (submit_tx, submit_rx) = mpsc::channel();
    let submit_service = service.clone();
    let submit_barrier = Arc::clone(&barrier);
    let submitter = thread::spawn(move || {
        submit_barrier.wait();
        submit_tx
            .send(submit_service.submit_callable(|| Ok::<usize, io::Error>(42)))
            .expect("test should collect submission");
    });
    let (close_done_tx, close_done_rx) = mpsc::channel();
    let close_service = service.clone();
    let close_barrier = Arc::clone(&barrier);
    let closer = thread::spawn(move || {
        close_barrier.wait();
        if stop {
            let _ = close_service.stop();
        } else {
            close_service.shutdown();
        }
        close_done_tx.send(()).expect("test should observe close completion");
    });
    barrier.wait();
    let first = submit_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("submission should finish");
    close_done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("shutdown or stop should finish in time");
    submitter.join().expect("submitter should exit");
    closer.join().expect("closer should exit");
    assert!(matches!(first, Ok(_) | Err(SubmissionError::Shutdown)));
    assert!(matches!(
        service.submit_callable(|| Ok::<(), io::Error>(())),
        Err(SubmissionError::Shutdown)
    ));
    if let Ok(handle) = first {
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("accepted task should finish in time");
        assert!(result.is_ok() || (stop && matches!(result, Err(TaskExecutionError::Cancelled))));
    }
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate in time");
    assert_eq!((service.stats().queued + service.stats().running), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_capacity_change_before_wait_is_observed() {
    let service = TokioExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let mut changes = service.capacity_changes();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let started_tx = std::sync::Mutex::new(Some(started_tx));
    let first = service
        .submit_callable(move || {
            started_tx
                .lock()
                .expect("start lock")
                .take()
                .expect("start signal once")
                .send(())
                .expect("test should observe task start");
            release_rx.recv().expect("test should release task");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept first task");
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("task should start in time")
        .expect("start signal should arrive");
    assert!(matches!(
        service.submit_callable(ok_unit_task as fn() -> Result<(), io::Error>),
        Err(SubmissionError::Saturated)
    ));
    release.release();
    tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("first task should finish in time")
        .expect("first task should succeed");
    tokio::time::timeout(Duration::from_secs(5), changes.changed())
        .await
        .expect("capacity notification should arrive")
        .expect("service remains alive");
    service
        .submit_callable(ok_unit_task as fn() -> Result<(), io::Error>)
        .expect("released capacity should admit retry")
        .await
        .expect("retry should succeed");
    service.shutdown();
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate in time");
}
