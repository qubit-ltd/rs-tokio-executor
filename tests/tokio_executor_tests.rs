/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
//! Tests for [`TokioExecutor`](qubit_tokio_executor::TokioExecutor).

use std::{
    io,
    sync::mpsc,
    time::Duration,
};

use qubit_executor::{
    CancelResult,
    TaskExecutionError,
};
use qubit_tokio_executor::{
    Executor,
    TokioExecutor,
};

#[tokio::test]
async fn test_tokio_executor_execute_returns_future_result() {
    let executor = TokioExecutor;

    executor
        .execute(|| Ok::<(), io::Error>(()))
        .expect("tokio executor should accept runnable")
        .await
        .expect("tokio executor should run runnable successfully");
}

#[tokio::test]
async fn test_tokio_executor_call_returns_future_value() {
    let executor = TokioExecutor;

    let value = executor
        .call(|| Ok::<usize, io::Error>(42))
        .expect("tokio executor should accept callable")
        .await
        .expect("tokio executor should return callable value");

    assert_eq!(value, 42);
}

#[tokio::test]
async fn test_tokio_execution_is_finished_reports_completion() {
    let executor = TokioExecutor;

    let execution = executor
        .call(|| {
            std::thread::sleep(Duration::from_millis(25));
            Ok::<usize, io::Error>(42)
        })
        .expect("tokio executor should accept callable");

    assert!(!execution.is_done());
    assert_eq!(
        execution.await.expect("tokio execution should complete"),
        42,
    );
}

#[test]
fn test_tokio_execution_cancel_queued_blocking_task_reports_cancelled() {
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

        let executor = TokioExecutor;
        let execution = executor
            .call(|| Ok::<(), io::Error>(()))
            .expect("tokio executor should accept callable");

        assert_eq!(execution.cancel(), CancelResult::Cancelled);
        release_tx
            .send(())
            .expect("blocking task should receive release signal");
        blocker.await.expect("blocking slot task should finish");

        let result = tokio::time::timeout(Duration::from_secs(1), execution)
            .await
            .expect("cancelled execution should complete promptly")
            .expect_err("cancelled execution should report cancellation");
        assert!(matches!(result, TaskExecutionError::Cancelled));
    });
}

#[tokio::test]
async fn test_tokio_execution_reports_task_panic() {
    let executor = TokioExecutor;

    let result = executor
        .call(|| -> Result<(), io::Error> { panic!("tokio executor panic") })
        .expect("tokio executor should accept callable")
        .await
        .expect_err("panic should be converted into task execution error");
    assert!(matches!(result, TaskExecutionError::Panicked));
}
