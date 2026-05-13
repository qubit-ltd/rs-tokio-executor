/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
use std::{
    io,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
        mpsc,
    },
    time::Duration,
};

use qubit_executor::service::ExecutorService;
use qubit_executor::{
    CancelResult,
    TaskExecutionError,
};
use qubit_tokio_executor::TokioExecutorService;

#[test]
fn test_tokio_task_handle_cancel_aborts_queued_blocking_task() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime should be created");

    runtime.block_on(async {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx
                .send(())
                .expect("test should receive blocking start signal");
            release_rx
                .recv()
                .expect("blocking task should receive release signal");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking slot should be occupied");

        let service = TokioExecutorService::new();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_for_task = Arc::clone(&ran);
        let handle = service
            .submit_tracked(move || {
                ran_for_task.store(true, Ordering::Release);
                Ok::<(), io::Error>(())
            })
            .expect("service should accept queued task");

        assert_eq!(CancelResult::Cancelled, handle.cancel());
        assert!(matches!(handle.await, Err(TaskExecutionError::Cancelled)));
        service.shutdown();

        let (terminated_tx, terminated_rx) = mpsc::channel();
        let waiter_service = service.clone();
        let waiter = std::thread::spawn(move || {
            waiter_service.wait_termination();
            terminated_tx
                .send(())
                .expect("test should receive termination signal");
        });
        if terminated_rx.recv_timeout(Duration::from_millis(100)).is_err() {
            release_tx
                .send(())
                .expect("blocking task should receive release signal");
            blocker.await.expect("blocking slot task should finish");
            service.wait_termination();
            waiter.join().expect("termination waiter should finish");
            panic!("cancelled queued task should let the service terminate before the blocking slot is released");
        }

        waiter.join().expect("termination waiter should finish");
        assert!(!ran.load(Ordering::Acquire));
        assert!(service.is_terminated());
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");
    });
}

#[tokio::test]
async fn test_tokio_task_handle_reports_panicked_task() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit_tracked(|| -> Result<(), io::Error> { panic!("tokio service panic") })
        .expect("service should accept panicking task");

    assert!(matches!(handle.await, Err(TaskExecutionError::Panicked)));
    service.shutdown();
    service.wait_termination();
}

#[tokio::test]
async fn test_tokio_task_handle_panicked_is_not_cancelled() {
    let service = TokioExecutorService::new();

    let handle = service
        .submit_tracked(|| -> Result<(), io::Error> { panic!("tokio service panic") })
        .expect("service should accept panicking task");

    let error = handle
        .await
        .expect_err("panicked task should return execution error");
    assert!(error.is_panicked());
    assert!(!error.is_cancelled());
    service.shutdown();
    service.wait_termination();
}
