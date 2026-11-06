// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use qubit_executor::service::ExecutorService;
use qubit_executor::service::SubmissionError;
use qubit_function::Callable;
use qubit_function::Runnable;
use qubit_tokio_executor::TokioExecutorService;
use qubit_tokio_executor::TokioIoExecutorService;

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

#[test]
fn test_shutdown_rejections_return_original_tasks_for_all_submission_apis() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let blocking = TokioExecutorService::new(runtime.handle().clone());
    blocking.shutdown();

    let runnable_calls = Arc::new(AtomicUsize::new(0));
    let runnable_calls_in_task = Arc::clone(&runnable_calls);
    let runnable = move || {
        runnable_calls_in_task.fetch_add(1, Ordering::SeqCst);
        Ok::<(), io::Error>(())
    };
    let rejected = blocking
        .try_submit::<_, io::Error>(runnable)
        .expect_err("shutdown should reject runnable");
    assert!(matches!(rejected.reason(), SubmissionError::Shutdown));
    let (_, mut runnable) = rejected.into_parts();
    runnable.run().expect("recovered runnable should execute");
    assert_eq!(runnable_calls.load(Ordering::SeqCst), 1);

    let callable_calls = Arc::new(AtomicUsize::new(0));
    let callable_calls_in_task = Arc::clone(&callable_calls);
    let callable = move || {
        callable_calls_in_task.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(42)
    };
    let rejected = match blocking.try_submit_callable::<_, usize, io::Error>(callable) {
        Err(rejected) => rejected,
        Ok(_) => panic!("shutdown should reject callable"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Shutdown));
    let (_, mut callable) = rejected.into_parts();
    assert_eq!(
        Callable::call(&mut callable).expect("recovered callable should execute"),
        42
    );
    assert_eq!(callable_calls.load(Ordering::SeqCst), 1);

    let tracked_callable = || Ok::<usize, io::Error>(7);
    let rejected = match blocking.try_submit_tracked_callable::<_, usize, io::Error>(tracked_callable) {
        Err(rejected) => rejected,
        Ok(_) => panic!("shutdown should reject tracked callable"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Shutdown));
    let (_, mut tracked_callable) = rejected.into_parts();
    assert_eq!(
        Callable::call(&mut tracked_callable).expect("recovered callable should execute"),
        7
    );

    let io_service = TokioIoExecutorService::new(runtime.handle().clone());
    io_service.shutdown();
    let future_calls = Arc::new(AtomicUsize::new(0));
    let calls_in_future = Arc::clone(&future_calls);
    let future = async move {
        let _pin = std::marker::PhantomPinned;
        calls_in_future.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(42)
    };
    let rejected = match io_service.try_spawn(future) {
        Err(rejected) => rejected,
        Ok(_) => panic!("shutdown should reject future"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Shutdown));
    let (_, future) = rejected.into_parts();
    runtime.block_on(future).expect("recovered future should run");
    assert_eq!(future_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_rejected_task_debug_does_not_require_or_format_task() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::new(runtime.handle().clone());
    service.shutdown();
    let task = String::from("private task details");
    let rejected = match service
        .try_submit_callable::<_, bool, io::Error>(move || Ok::<bool, io::Error>(task == "private task details"))
    {
        Err(rejected) => rejected,
        Ok(_) => panic!("shutdown should reject callable"),
    };
    let debug = format!("{rejected:?}");
    assert!(debug.contains("Shutdown"));
    assert!(!debug.contains("private task details"));
    assert_eq!(rejected.to_string(), SubmissionError::Shutdown.to_string());
    let error: &dyn std::error::Error = &rejected;
    assert!(error.source().is_none());
    let (_, mut task) = rejected.into_parts();
    assert!(Callable::call(&mut task).expect("recovered closure should run"));
}

#[test]
fn test_saturated_callable_can_be_recovered_and_submitted_once() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime should build");
    let service = TokioExecutorService::with_task_capacity(
        runtime.handle().clone(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let blocker = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should observe blocker start");
            release_rx.recv().expect("test should release blocker");
            Ok::<(), io::Error>(())
        })
        .expect("blocker should be accepted");
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("blocking callable should start");

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_in_callable = Arc::clone(&calls);
    let callable = move || {
        calls_in_callable.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(42)
    };
    let rejected = match service.try_submit_callable::<_, usize, io::Error>(callable) {
        Err(rejected) => rejected,
        Ok(_) => panic!("capacity should reject the second callable"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Saturated));
    let (_, callable) = rejected.into_parts();
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut changes = service.capacity_changes();
    release.release();
    assert_eq!(blocker.get().expect("blocker should finish"), ());
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), changes.changed())
            .await
            .expect("capacity should change after blocker completes")
            .expect("capacity watcher should remain open");
    });
    let result = service
        .try_submit_callable::<_, usize, io::Error>(callable)
        .expect("recovered callable should be accepted");
    assert_eq!(result.get().expect("recovered callable should finish"), 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    service.shutdown();
    assert!(service.wait_termination_timeout(Duration::from_secs(2)));
}

#[test]
fn test_saturated_non_unpin_future_can_be_recovered_and_submitted_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::with_task_capacity(
        runtime.handle().clone(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let blocker = service
        .spawn(std::future::pending::<Result<(), io::Error>>())
        .expect("first future should be accepted");
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_in_future = Arc::clone(&calls);
    let future = async move {
        let _pin = std::marker::PhantomPinned;
        calls_in_future.fetch_add(1, Ordering::SeqCst);
        Ok::<usize, io::Error>(42)
    };
    let rejected = match service.try_spawn(future) {
        Err(rejected) => rejected,
        Ok(_) => panic!("capacity should reject future"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Saturated));
    let (_, future) = rejected.into_parts();
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    runtime.block_on(async {
        let _ = blocker.cancel();
        let _ = blocker.await;
    });
    let result = service.try_spawn(future).expect("recovered future should be accepted");
    assert_eq!(runtime.block_on(result).expect("future should finish"), 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    service.shutdown();
    runtime.block_on(service.await_termination());
}

#[test]
fn test_shutdown_reason_precedes_saturation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioIoExecutorService::with_task_capacity(
        runtime.handle().clone(),
        NonZeroUsize::new(1).expect("capacity should be nonzero"),
    );
    let blocker = service
        .spawn(std::future::pending::<Result<(), io::Error>>())
        .expect("first future should be accepted");
    service.shutdown();
    let rejected = match service.try_spawn(async { Ok::<usize, io::Error>(42) }) {
        Err(rejected) => rejected,
        Ok(_) => panic!("shutdown should reject even when capacity is full"),
    };
    assert!(matches!(rejected.reason(), SubmissionError::Shutdown));
    let (_, future) = rejected.into_parts();
    assert_eq!(runtime.block_on(future).expect("recovered future should run"), 42);
    runtime.block_on(async {
        let _ = blocker.cancel();
        let _ = blocker.await;
        service.await_termination().await;
    });
}

#[test]
fn test_tracked_callable_try_submission_can_cancel_queued_work() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime should build");
    let service = TokioExecutorService::with_task_capacity(
        runtime.handle().clone(),
        NonZeroUsize::new(2).expect("capacity should be nonzero"),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop(Some(release_tx));
    let blocker = service
        .submit_callable(move || {
            started_tx.send(()).expect("test should observe blocker start");
            release_rx.recv().expect("test should release blocker");
            Ok::<(), io::Error>(())
        })
        .expect("blocker should be accepted");
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("blocking callable should start");

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_in_callable = Arc::clone(&calls);
    let tracked = service
        .try_submit_tracked_callable::<_, usize, io::Error>(move || {
            calls_in_callable.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        })
        .expect("tracked callable should be accepted");
    assert_eq!(tracked.cancel(), qubit_executor::CancelResult::Cancelled);
    assert!(matches!(
        tracked.get(),
        Err(qubit_executor::TaskExecutionError::Cancelled)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    release.release();
    assert_eq!(blocker.get().expect("blocker should finish"), ());
    service.shutdown();
    assert!(service.wait_termination_timeout(Duration::from_secs(2)));
}
