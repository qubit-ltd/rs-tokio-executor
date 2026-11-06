// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::future::pending;
use std::io;
use std::num::NonZeroUsize;

use qubit_executor::TaskExecutionError;
use qubit_executor::service::SubmissionError;
use qubit_tokio_executor::TokioIoExecutorService;

#[tokio::test]
async fn test_tokio_io_executor_service_spawn_acceptance_is_not_task_success() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("service should accept async runnable")
        .await
        .expect("async runnable should complete successfully");

    let handle = service
        .spawn(async { Err::<(), _>(io::Error::other("task failed")) })
        .expect("service should accept async task");

    let err = handle
        .await
        .expect_err("accepted async task should report failure through handle");
    assert!(matches!(err, TaskExecutionError::Failed(_)));
}

#[tokio::test]
async fn test_tokio_io_executor_service_spawn_returns_value() {
    let service = TokioIoExecutorService::new(tokio::runtime::Handle::current());

    let handle = service
        .spawn(async { Ok::<usize, io::Error>(42) })
        .expect("service should accept async callable");

    assert_eq!(handle.await.expect("async callable should complete successfully"), 42,);
}

#[tokio::test]
async fn test_tokio_io_executor_service_capacity_bounds_and_reuses_completed_slots() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let first = service
        .spawn(async move {
            started_tx.send(()).expect("test should receive start signal");
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first task");
    started_rx.await.expect("first task should start");

    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));

    assert_eq!(first.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(first.await, Err(TaskExecutionError::Cancelled)));
    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("completed future should release its capacity slot")
        .await
        .expect("replacement future should complete");
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_io_executor_service_shutdown_precedes_capacity_error() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let first = service
        .spawn(async {
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the first future");
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));

    service.shutdown();
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Shutdown)
    ));
    assert_eq!(first.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(first.await, Err(TaskExecutionError::Cancelled)));
    service.await_termination().await;
}

#[tokio::test]
async fn test_tokio_io_executor_service_capacity_reuses_slot_after_pre_poll_abort() {
    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let cancelled = service
        .spawn(async {
            pending::<()>().await;
            Ok::<(), io::Error>(())
        })
        .expect("service should accept the future");
    assert_eq!(cancelled.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(cancelled.await, Err(TaskExecutionError::Cancelled)));

    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("pre-poll cancellation should release its capacity slot")
        .await
        .expect("replacement future should complete");
    service.shutdown();
    service.await_termination().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_admission_reserves_only_one_slot() {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let barrier = Arc::new(Barrier::new(3));
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let (results_tx, results_rx) = std::sync::mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..2 {
        let service = service.clone();
        let barrier = Arc::clone(&barrier);
        let results_tx = results_tx.clone();
        let receiver = release_rx.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            let result = service.spawn(async move {
                let mut receiver = receiver;
                while !*receiver.borrow() {
                    receiver.changed().await.expect("test should release accepted task");
                }
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
    assert_eq!(service.stats().accepted_unfinished, 1);
    release_tx.send(true).expect("accepted task should be released");
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
    use std::time::Duration;

    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let barrier = Arc::new(Barrier::new(3));
    let (submit_tx, submit_rx) = std::sync::mpsc::channel();
    let submit_service = service.clone();
    let submit_barrier = Arc::clone(&barrier);
    let submitter = thread::spawn(move || {
        submit_barrier.wait();
        submit_tx
            .send(submit_service.spawn(async { Ok::<usize, io::Error>(42) }))
            .expect("test should collect submission");
    });
    let (close_done_tx, close_done_rx) = std::sync::mpsc::channel();
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
        service.spawn(async { Ok::<(), io::Error>(()) }),
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
    assert_eq!(service.stats().accepted_unfinished, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_capacity_change_before_wait_is_observed() {
    use std::time::Duration;

    let service = TokioIoExecutorService::with_task_capacity(
        tokio::runtime::Handle::current(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let mut changes = service.capacity_changes();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let first = service
        .spawn(async move {
            started_tx.send(()).expect("test should observe task start");
            release_rx.await.expect("test should release task");
            Ok::<(), io::Error>(())
        })
        .expect("service should accept first task");
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("task should start in time")
        .expect("start signal should arrive");
    assert!(matches!(
        service.spawn(async { Ok::<(), io::Error>(()) }),
        Err(SubmissionError::Saturated)
    ));
    release_tx.send(()).expect("task should be released");
    tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("first task should finish in time")
        .expect("first task should succeed");
    tokio::time::timeout(Duration::from_secs(5), changes.changed())
        .await
        .expect("capacity notification should arrive")
        .expect("service remains alive");
    service
        .spawn(async { Ok::<(), io::Error>(()) })
        .expect("released capacity should admit retry")
        .await
        .expect("retry should succeed");
    service.shutdown();
    tokio::time::timeout(Duration::from_secs(5), service.await_termination())
        .await
        .expect("service should terminate in time");
}
